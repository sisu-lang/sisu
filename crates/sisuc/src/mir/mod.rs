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
//! The ownership pass (`ownership.rs`) decides them by liveness (`liveness.rs`): a `Use` moves
//! the local when nothing reads it later and copies it otherwise, and a borrow site reads its
//! root in place unless that is the root's last use, when the root moves into a temporary that
//! is released after the statement. A `match` binding read in its arm's test or guard is
//! borrowed, so it is copied or read in place whatever liveness says.
//!
//! The hoist (`hoist.rs`) then moves the `make_unique` of an array that a loop writes, and never
//! shares or replaces, to the loop's pre-header (ADR 0021).
//!
//! Elaboration (`elaborate.rs`) then decides each scope release by the set of states that the
//! certifier's dataflow says reaches it: it removes one where the local is empty on every path,
//! makes one where it is owned on every path a plain `release`, and keeps every other one as
//! `release?`.

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
mod print;

pub(crate) use elaborate::flagged;
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
    pub(crate) ret: Type,
    /// The parameters, then `tir`'s locals with `tir`'s ids, then the temporaries.
    pub(crate) locals: Vec<Local>,
    /// `blocks[0]` is the entry.
    pub(crate) blocks: Vec<Block>,
    /// In pre-order: an outer loop before the loops inside it.
    pub(crate) loops: Vec<Loop>,
    /// Each `match` binding, and the scrutinee local it borrows from.
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
/// `guard` marks a borrow site whose root is a `match` binding inside its arm's test or guard,
/// where the binding is borrowed: it always reads in place.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Place {
    pub(crate) local: LocalId,
    pub(crate) projections: Vec<Projection>,
    pub(crate) borrow: bool,
    pub(crate) guard: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Projection {
    Field(usize),
    Payload {
        variant: usize,
        field: usize,
    },
    /// An element; reading it checks the index against the length, and panics at `span`.
    Index {
        index: Operand,
        span: Span,
    },
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
    /// A `Use` of a `match` binding inside its arm's test or guard, where the binding is
    /// borrowed: the ownership pass makes it a copy, whatever liveness says.
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
    /// A `match` binding's read of a part of its scrutinee, in place.
    Borrow(Place),
    /// The payload of an optional that holds one.
    Unwrap(Operand),
    Call {
        func: FuncId,
        args: Vec<Operand>,
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
    /// A `match` binding's borrow ends.
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

/// The mir of `program`, with its copies, moves and releases placed, and each loop's
/// `make_unique` of the arrays it writes hoisted to its pre-header, and each scope release
/// elaborated. A debug build certifies each function and panics on a violation.
pub(crate) fn build(program: &tir::Program) -> Program {
    let mut mir = lower(program);
    for function in &mut mir.functions {
        ownership::place(function);
        hoist::hoist(function);
        elaborate::elaborate(function);
        if cfg!(debug_assertions) {
            certified(function);
        }
    }
    mir
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
