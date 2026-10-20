//! mir: the mid-level IR between `tir` and LLVM IR (ADR 0025). A function is a list of typed
//! locals and a list of basic blocks; each block is a list of statements and one terminator.
//! mir is not SSA: a local is a mutable slot, and a value that `tir` gives an `if` or a block
//! is assigned to a destination local on each path to the join.
//!
//! Lowering (`lower.rs`) evaluates operands in source order and writes the count work that
//! the source implies as statements: a `copy` of every string literal, a `release` of every
//! temporary that a borrowing statement read, a scope release (`release?`) of each source local
//! on every edge that leaves its scope, and `check`, `make_unique` and `take` for each write
//! through a place. Two forms are left for a later pass to decide: `Use`, a consuming read of a
//! whole counted local, and a borrow site, a place whose root a statement reads in place.
//!
//! Inference (`params.rs`) then decides which counted parameters are borrowed: those their
//! function never consumes, across the whole program's calls (ADR 0029). Each function is lowered
//! under assumed flags, and lowered again only where a flag that decided the form of a `match` or
//! an `if let` came out owned. The rewrite then gives each function and each call its flags: a
//! call reads a borrowed argument in place, and the caller releases a temporary it passed there
//! after the call. A borrowed parameter has no scope release.
//!
//! The ownership pass (`ownership.rs`) decides them by liveness (`liveness.rs`): a `Use` moves
//! the local when nothing reads it later and copies it otherwise, and a borrow site reads its
//! root in place unless that is the root's last use, when the root moves into a temporary that
//! is released after the statement. A `match` binding read in its arm's test or guard is
//! borrowed, as is a binding of a `match` or an `if let` on a borrowed place through its arm, so
//! it is copied or read in place whatever liveness says.
//!
//! The hoist (`hoist.rs`) then moves the `make_unique` of an array that a loop writes, and never
//! shares or replaces, to the loop's pre-header (ADR 0021).
//!
//! Elaboration (`elaborate.rs`) then decides each scope release by the set of states that the
//! certifier's dataflow says reaches it: it removes one where the local is empty on every path,
//! makes one where it is owned on every path a plain `release`, and keeps every other one as
//! `release?`.
//!
//! Pair removal (`pairs.rs`) then turns a copy that is only read within its block, and released
//! there, into a borrow, and its release into `end`s: the retain and the release are gone. It
//! keeps a copy whose root may change while the copy is open, one that a call or a counted write
//! may free when it came through a field or an element, and one that is written when it is an
//! array or a string, which copy-on-write must still copy.

use rustc_hash::FxHashMap;

use crate::source::Span;
use crate::tir::{self, BinaryOp, ClassId, CompareOp, EnumId, Format, FuncId, MathOp, Stream};
pub(crate) use crate::tir::{LocalId, Type, UnaryOp};

mod certify;
mod elaborate;
mod hoist;
mod liveness;
mod lower;
mod ownership;
mod pairs;
mod params;
mod print;

pub(crate) use elaborate::flagged;
#[cfg(test)]
pub(crate) use lower::lower;
pub(crate) use print::print;

/// A program's functions, in `tir`'s order.
#[derive(Debug)]
pub(crate) struct Program {
    pub(crate) functions: Vec<Function>,
}

#[derive(Debug)]
pub(crate) struct Function {
    pub(crate) name: String,
    /// `locals[..params]` are the parameters.
    pub(crate) params: usize,
    /// Per parameter, whether it is borrowed: the caller keeps its reference, and the function
    /// reads the value in place and never consumes it. An uncounted parameter is never borrowed.
    pub(crate) borrowed: Vec<bool>,
    pub(crate) ret: Type,
    /// The parameters, then `tir`'s locals with `tir`'s ids, then the temporaries.
    pub(crate) locals: Vec<Local>,
    /// `blocks[0]` is the entry.
    pub(crate) blocks: Vec<Block>,
    /// In pre-order: an outer loop before the loops inside it.
    pub(crate) loops: Vec<Loop>,
    /// Each `match` binding, and the scrutinee local it borrows from; and each member of a pair
    /// that pair removal made a borrow, and the local its copy read.
    pub(crate) owners: FxHashMap<LocalId, LocalId>,
}

#[derive(Debug)]
pub(crate) struct Local {
    /// `None` for a temporary, and for a local the checker made up.
    pub(crate) name: Option<String>,
    pub(crate) ty: Type,
    /// Whether the local holds a reference that codegen counts: `ty.is_counted`.
    pub(crate) counted: bool,
    /// The block that was current when lowering created the local, which dominates every
    /// block that mentions it. A source local that lowering never reaches keeps the entry block.
    pub(crate) scope: BlockId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct BlockId(pub(crate) usize);

#[derive(Debug)]
pub(crate) struct Block {
    pub(crate) stmts: Vec<Statement>,
    pub(crate) term: Terminator,
}

/// A loop: the block that enters it, its head, and every block inside it, the head included.
#[derive(Debug)]
pub(crate) struct Loop {
    pub(crate) pre_header: BlockId,
    pub(crate) header: BlockId,
    pub(crate) blocks: Vec<BlockId>,
}

/// A local and a path into it: a class field, a payload field under a known variant, an
/// element. `borrow` marks a borrow site: a statement reads the counted root in place, and the
/// ownership pass decides whether the root stays put or is moved into a temporary first.
/// `guard` marks a borrow site whose root is a borrowed binding: a `match` binding inside its
/// arm's test or guard, or a binding of a `match` or an `if let` on a borrowed place inside its
/// arm. It always reads in place, as a borrow site rooted at a borrowed parameter does without
/// the mark.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Place {
    pub(crate) local: LocalId,
    pub(crate) projections: Vec<Projection>,
    pub(crate) borrow: bool,
    pub(crate) guard: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Projection {
    /// A class field; `mutable` for a `mut` field.
    Field {
        index: usize,
        mutable: bool,
    },
    Payload {
        variant: usize,
        field: usize,
    },
    /// An element; reading it checks the index against the length, and panics at `span`.
    Index {
        index: Operand,
        span: Span,
    },
    /// The payload of an optional that holds one; only ever the last projection.
    Some,
}

impl Projection {
    /// Whether nothing a call or a write does can replace or free what the projection reaches
    /// while its root lives: a field that is not `mut` cannot be replaced nor written through
    /// (E0001), and a payload or a present optional's value is fixed with its container.
    pub(crate) const fn stable(&self) -> bool {
        match self {
            Projection::Field { mutable, .. } => !*mutable,
            Projection::Payload { .. } | Projection::Some => true,
            Projection::Index { .. } => false,
        }
    }
}

/// A constant operand, with the type an integer or a `None` needs.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Const {
    Int(i64, Type),
    Float(f64),
    Bool(bool),
    Char(char),
    None(Type),
    /// A string pattern's literal, which a test compares with in place.
    Str(String),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Operand {
    Const(Const),
    /// A read that takes no reference: an uncounted value, or a counted root read in place.
    Read(Place),
    /// A consuming read of an owned local, which is empty after it.
    Move(LocalId),
    /// A consuming read of a whole counted local, which the ownership pass turns into a move
    /// or a copy.
    Use(LocalId),
    /// A `Use` of a borrowed binding (see `Place::guard`): the ownership pass makes it a copy,
    /// whatever liveness says, as it makes a `Use` of a borrowed parameter without the mark.
    Guarded(LocalId),
}

/// The right side of an assignment. The count work inside one is the emitter's: the rows of
/// the spec's table say which operands each consumes and which it borrows.
#[derive(Debug)]
pub(crate) enum Rvalue {
    Use(Operand),
    /// Adds a reference to the block the place holds.
    Copy(Place),
    /// A string literal's global, with a reference added.
    CopyStr(String),
    /// A read of a place in place, with no count: a `match` binding's, or a copy that pair
    /// removal turned into one.
    Borrow(Place),
    /// The payload of an optional that holds one.
    Unwrap(Operand),
    /// `borrowed` holds one flag per argument, the callee's `Function::borrowed`: a borrowed
    /// argument is read in place, and the caller keeps its reference.
    Call {
        func: FuncId,
        args: Vec<Operand>,
        borrowed: Vec<bool>,
    },
    New {
        class: ClassId,
        args: Vec<Operand>,
    },
    Variant {
        id: EnumId,
        variant: usize,
        args: Vec<Operand>,
    },
    ArrayLit(Vec<Operand>),
    /// `[value; len]`; a negative or oversized length panics at `span`.
    ArrayFill {
        value: Operand,
        len: Operand,
        span: Span,
    },
    Wrap(Operand),
    Concat {
        lhs: Operand,
        rhs: Operand,
    },
    Interpolate {
        parts: Vec<Operand>,
        formats: Vec<Format>,
    },
    ReadLine,
    Chars(Operand),
    Bytes(Operand),
    Words(Operand),
    FromChars(Operand),
    Len(Operand),
    /// `s.words().len()`, which counts the words without building them.
    LenWords(Operand),
    /// An overflow panics at `span`.
    Sum {
        base: Operand,
        span: Span,
    },
    Contains {
        base: Operand,
        value: Operand,
    },
    /// The operand converted to the destination's type.
    Convert(Operand),
    Math {
        op: MathOp,
        operands: Vec<Operand>,
    },
    /// An overflow panics at `span`.
    Unary {
        op: UnaryOp,
        operand: Operand,
        span: Span,
    },
    /// An overflow or a division by zero panics at `span`.
    Binary {
        op: BinaryOp,
        lhs: Operand,
        rhs: Operand,
        span: Span,
    },
    /// One step of a comparison chain.
    Compare {
        op: CompareOp,
        lhs: Operand,
        rhs: Operand,
    },
    Equal {
        negated: bool,
        lhs: Operand,
        rhs: Operand,
    },
    Is {
        lhs: Operand,
        rhs: Operand,
    },
    Print {
        stream: Stream,
        operand: Operand,
    },
}

#[derive(Debug)]
pub(crate) enum Statement {
    /// `dest = value`; `None` for a `unit` result.
    Assign {
        dest: Option<LocalId>,
        value: Rvalue,
    },
    /// Moves the place's reference into `dest`; the place owns nothing until its store.
    Take {
        dest: LocalId,
        place: Place,
    },
    Store {
        place: Place,
        value: Operand,
    },
    Release(LocalId),
    /// Releases the local if it still owns a reference.
    ScopeRelease(LocalId),
    /// Copies the array or string the place holds when it is shared (ADR 0021).
    MakeUnique(Place),
    /// The bounds check of the place's last index.
    Check(Place),
    /// A borrow ends.
    End(LocalId),
}

#[derive(Debug)]
pub(crate) enum Terminator {
    Goto(BlockId),
    Branch {
        cond: Operand,
        then: BlockId,
        else_: BlockId,
    },
    /// Branches on whether the optional local holds a value.
    IfPresent {
        optional: LocalId,
        some: BlockId,
        none: BlockId,
    },
    SwitchTag {
        place: Place,
        arms: Vec<(usize, BlockId)>,
        otherwise: Option<BlockId>,
    },
    SwitchValue {
        operand: Operand,
        arms: Vec<(Const, BlockId)>,
        otherwise: Option<BlockId>,
    },
    Return(Option<Operand>),
    /// Ends the program; the message is consumed.
    Panic {
        message: Operand,
        span: Span,
    },
    Unreachable,
}

impl Terminator {
    /// The blocks this terminator can jump to.
    pub(crate) fn successors(&self) -> impl Iterator<Item = BlockId> + '_ {
        let (fixed, tags, values, otherwise): ([Option<BlockId>; 2], &[_], &[_], _) = match self {
            Terminator::Goto(to) => ([Some(*to), None], &[], &[], None),
            Terminator::Branch { then, else_, .. } => ([Some(*then), Some(*else_)], &[], &[], None),
            Terminator::IfPresent { some, none, .. } => {
                ([Some(*some), Some(*none)], &[], &[], None)
            }
            Terminator::SwitchTag {
                arms, otherwise, ..
            } => ([None, None], arms.as_slice(), &[], *otherwise),
            Terminator::SwitchValue {
                arms, otherwise, ..
            } => ([None, None], &[], arms.as_slice(), *otherwise),
            Terminator::Return(_) | Terminator::Panic { .. } | Terminator::Unreachable => {
                ([None, None], &[], &[], None)
            }
        };
        fixed
            .into_iter()
            .flatten()
            .chain(tags.iter().map(|(_, to)| *to))
            .chain(values.iter().map(|(_, to)| *to))
            .chain(otherwise)
    }
}

/// The mir of `program`, with each parameter that its function does not consume borrowed, its
/// copies, moves and releases placed, and each loop's `make_unique` of the arrays it writes
/// hoisted to its pre-header, each scope release elaborated, and each retain/release pair within
/// a block removed. A debug build certifies each function and panics on a violation.
pub(crate) fn build(program: &tir::Program) -> Program {
    let assumed = params::assumed(program);
    let (mut functions, consulted): (Vec<_>, Vec<_>) = assumed
        .iter()
        .enumerate()
        .map(|(id, borrowed)| lower::function(program, FuncId(id), borrowed))
        .unzip();
    let main = program
        .functions
        .iter()
        .position(|f| f.name == "main")
        .expect("the checker should reject a program without a `main` function");
    let modes = params::infer(&functions, FuncId(main));
    for (id, consulted) in consulted.iter().enumerate() {
        if consulted
            .iter()
            .zip(&modes[id])
            .any(|(&consulted, &borrowed)| consulted && !borrowed)
        {
            functions[id] = lower::function(program, FuncId(id), &modes[id]).0;
        }
    }
    params::rewrite(&mut functions, &modes);
    if cfg!(debug_assertions) {
        modes_checked(&functions);
    }
    for function in &mut functions {
        ownership::place(function);
        hoist::hoist(function);
        elaborate::elaborate(function);
        pairs::remove_pairs(function);
        if cfg!(debug_assertions) {
            certified(function);
        }
    }
    Program { functions }
}

/// Panics on a call whose flags are not its callee's `borrowed`.
fn modes_checked(functions: &[Function]) {
    for function in functions {
        for stmt in function.blocks.iter().flat_map(|block| &block.stmts) {
            if let Statement::Assign {
                value: Rvalue::Call { func, borrowed, .. },
                ..
            } = stmt
            {
                let callee = &functions[func.0];
                assert_eq!(
                    borrowed, &callee.borrowed,
                    "fn {}'s call to {} should carry its parameters' modes",
                    function.name, callee.name
                );
            }
        }
    }
}

/// Panics on the first rule `function` breaks.
fn certified(function: &Function) {
    if let Err(violation) = certify::certify(function) {
        panic!(
            "mir certifier: fn {}, bb{}, {}: {}",
            function.name,
            violation.block.0,
            print::local_name(function, violation.local),
            violation.rule
        );
    }
}

#[cfg(test)]
pub(crate) mod tests;
