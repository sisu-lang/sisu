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

use rustc_hash::FxHashMap;

use crate::source::Span;
use crate::tir::{self, BinaryOp, ClassId, CompareOp, EnumId, Format, FuncId, MathOp, Stream};
pub(crate) use crate::tir::{LocalId, Type, UnaryOp};

mod lower;
mod print;

pub(crate) use lower::lower;

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
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Place {
    pub(crate) local: LocalId,
    pub(crate) projections: Vec<Projection>,
    pub(crate) borrow: bool,
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

/// The mir of `program`, ready for codegen. For now it only lowers.
pub(crate) fn build(program: &tir::Program) -> Program {
    lower(program)
}

#[cfg(test)]
mod tests;
