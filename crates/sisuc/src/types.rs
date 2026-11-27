//! Interned types. A `Type` stands for a type's structure, as a `Symbol` stands for a name's
//! text, so that copying, comparing and hashing a type costs what an integer does, and a type
//! such as `[i64?]` allocates nothing once interned.

use rustc_hash::{FxHashMap, FxHashSet};
use std::fmt;
use std::sync::{LazyLock, Mutex, MutexGuard, OnceLock, PoisonError};

use crate::symbol::Symbol;
use crate::tir::{ClassId, EnumId, InterfaceId, StructId};

/// The free stack that a walk over the depth of a type keeps. rustc's `ensure_sufficient_stack`
/// uses the same value.
const STACK_RED_ZONE: usize = 100 * 1024;

/// The size of each new piece of stack. rustc's `ensure_sufficient_stack` uses the same value.
const STACK_SEGMENT: usize = 1024 * 1024;

/// Runs `f` on a new piece of stack when the stack that is left is less than `STACK_RED_ZONE`. A
/// type can nest deeper than the call stack allows, and walks over its depth call themselves once
/// for each level.
pub(crate) fn ensure_stack<R>(f: impl FnOnce() -> R) -> R {
    stacker::maybe_grow(STACK_RED_ZONE, STACK_SEGMENT, f)
}

/// A type, interned: two types are equal exactly when their structures are. `kind` reads the
/// structure.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Type(u32);

const _: () = assert!(size_of::<Type>() == 4, "a type should be 4 bytes");

/// A type's structure, one level deep: a nested type is a `Type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum TypeKind {
    I64,
    U8,
    F64,
    Bool,
    Char,
    String,
    Unit,
    Never,
    /// A class, with the type arguments of a generic one (`TypeArgs::NONE` when it takes none).
    Class(ClassId, TypeArgs),
    /// A struct, with the type arguments of a generic one (`TypeArgs::NONE` when it takes none).
    Struct(StructId, TypeArgs),
    /// An enum, with the type arguments of a generic one (`TypeArgs::NONE` when it takes none).
    Enum(EnumId, TypeArgs),
    /// A type parameter: its name and its index in its declaration's list.
    Param(Symbol, u32),
    /// `[T]`.
    Array(Type),
    /// Holds `I64`, `U8`, `F64`, `Bool`, `Char`, `String`, `Class`, `Struct`, `Enum`, `Param`,
    /// `Assoc`, `Any`, `Array` or `Infer`.
    Optional(Type),
    /// An associated type of a base: `S.Item`. It is the type at the index of the associated type
    /// among the interface's, through the interface with its arguments (without `Self`). The base
    /// is a type parameter, another associated type, or (until `conformance::normalize` replaces
    /// it) a class or an enum.
    Assoc(Type, InterfaceId, TypeArgs, u32),
    /// `any I`: a value of any type that conforms to the interface `I`, with the arguments of a
    /// generic interface (`TypeArgs::NONE` when it takes none, and without `Self`).
    Any(InterfaceId, TypeArgs),
    /// An inference variable of the table of the function being checked (`check::infer`). It never
    /// leaves the checker: `write_back` puts its type in its place.
    Infer(InferId),
}

/// An inference variable's place in the table of one top-level function. Ids start at 0 for each
/// table, so a type that holds one means something only in the table that made it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct InferId(pub(crate) u32);

/// The index of the hole, the type parameter that stands for a part a context has not fixed yet
/// (see `check::generic`). No declaration has a parameter at this index.
pub(crate) const HOLE_INDEX: u32 = u32::MAX;

/// The index of the rejected hole, the type parameter that stands for a part of a context whose
/// bound failed (see `check::generic`). It is a hole too: `Type::holds_hole` is true for it. No
/// declaration has a parameter at this index.
pub(crate) const REJECTED_INDEX: u32 = u32::MAX - 1;

/// The index of a join's mark: the callee's type parameter that a call's join decides, at its
/// place in a parameter's type (see `check::join::part_at`). A type that the call decided before
/// can hold a type parameter of the caller with the same name, never one at this index. No
/// declaration has a parameter at this index.
pub(crate) const MARK_INDEX: u32 = u32::MAX - 2;

/// What a type names, anywhere inside it. Each interned type and list holds its own, so a read
/// does not walk the type.
#[derive(Clone, Copy, Debug, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is a separate fact that a different query asks of a type"
)]
struct Names {
    /// A type parameter.
    param: bool,
    /// `HOLE_INDEX`.
    open: bool,
    /// `REJECTED_INDEX`.
    rejected: bool,
    /// An associated type.
    assoc: bool,
    /// An `any` type.
    any: bool,
    /// An inference variable.
    infer: bool,
    /// The least and the largest id of the inference variables, when `infer` is true.
    infer_lo: u32,
    infer_hi: u32,
    /// An optional, other than under an associated type.
    optional: bool,
    /// How many levels of types nest in it: 0 for a type with no part. A list holds the most of
    /// its elements'.
    depth: u32,
}

impl Names {
    fn or(self, other: Names) -> Names {
        let (infer_lo, infer_hi) = match (self.infer, other.infer) {
            (true, true) => (
                self.infer_lo.min(other.infer_lo),
                self.infer_hi.max(other.infer_hi),
            ),
            (true, false) => (self.infer_lo, self.infer_hi),
            (false, _) => (other.infer_lo, other.infer_hi),
        };
        Names {
            param: self.param || other.param,
            open: self.open || other.open,
            rejected: self.rejected || other.rejected,
            assoc: self.assoc || other.assoc,
            any: self.any || other.any,
            infer: self.infer || other.infer,
            infer_lo,
            infer_hi,
            optional: self.optional || other.optional,
            depth: self.depth.max(other.depth),
        }
    }
}

impl Names {
    /// What a type that holds a type with these names names: one level deeper.
    fn deeper(self) -> Names {
        Names {
            depth: self.depth + 1,
            ..self
        }
    }
}

impl TypeKind {
    /// What a type of this kind names, from what its children say.
    // `check::cycle::Graph::parts` and `Type::anys` look into the same kinds: a kind that holds a
    // type goes in all three.
    fn names(self) -> Names {
        #[cfg(test)]
        NAMED.set(NAMED.get() + 1);
        match self {
            TypeKind::Infer(id) => Names {
                infer: true,
                infer_lo: id.0,
                infer_hi: id.0,
                ..Names::default()
            },
            TypeKind::Param(_, i) => Names {
                param: true,
                open: i == HOLE_INDEX,
                rejected: i == REJECTED_INDEX,
                ..Names::default()
            },
            TypeKind::Array(ty) => ty.names().deeper(),
            TypeKind::Optional(ty) => Names {
                optional: true,
                ..ty.names().deeper()
            },
            TypeKind::Assoc(base, _, args, _) => Names {
                assoc: true,
                optional: false,
                ..base.names().or(args.names()).deeper()
            },
            TypeKind::Any(_, args) => Names {
                any: true,
                ..if args == TypeArgs::NONE {
                    Names::default()
                } else {
                    args.names().deeper()
                }
            },
            TypeKind::Class(_, args) | TypeKind::Struct(_, args) | TypeKind::Enum(_, args)
                if args == TypeArgs::NONE =>
            {
                Names::default()
            }
            TypeKind::Class(_, args) | TypeKind::Struct(_, args) | TypeKind::Enum(_, args) => {
                args.names().deeper()
            }
            TypeKind::I64
            | TypeKind::U8
            | TypeKind::F64
            | TypeKind::Bool
            | TypeKind::Char
            | TypeKind::String
            | TypeKind::Unit
            | TypeKind::Never => Names::default(),
        }
    }
}

/// A list of type arguments, interned: two lists are equal exactly when their elements are.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TypeArgs(u32);

#[derive(Default)]
struct ArgsInterner {
    ids: FxHashMap<&'static [Type], TypeArgs>,
    /// Each list, with what its elements name, at its handle.
    lists: Vec<(&'static [Type], Names)>,
}

/// Every list interned in this process, with the empty list at index 0. Each slice is leaked so
/// that `get` can lend it for the rest of the process, as `Symbol::intern` leaks each name.
static ARGS: LazyLock<Mutex<ArgsInterner>> = LazyLock::new(|| {
    Mutex::new(ArgsInterner {
        ids: FxHashMap::from_iter([(&[][..], TypeArgs(0))]),
        lists: vec![(&[], Names::default())],
    })
});

fn args() -> MutexGuard<'static, ArgsInterner> {
    // `intern` changes the tables only after its last fallible step, so a poisoned lock still
    // guards consistent tables.
    ARGS.lock().unwrap_or_else(PoisonError::into_inner)
}

impl TypeArgs {
    /// No arguments: the arguments of every non-generic class and enum.
    pub(crate) const NONE: TypeArgs = TypeArgs(0);

    pub(crate) fn intern(list: &[Type]) -> TypeArgs {
        let mut args = args();
        if let Some(&interned) = args.ids.get(list) {
            return interned;
        }
        let interned = TypeArgs(
            u32::try_from(args.lists.len())
                .expect("a process should intern fewer than 2^32 argument lists"),
        );
        let list: &'static [Type] = Box::leak(list.into());
        let names = list
            .iter()
            .fold(Names::default(), |names, ty| names.or(ty.names()));
        args.lists.push((list, names));
        args.ids.insert(list, interned);
        interned
    }

    pub(crate) fn get(self) -> &'static [Type] {
        args().lists[self.0 as usize].0
    }

    fn names(self) -> Names {
        args().lists[self.0 as usize].1
    }

    /// Whether an element holds an associated type, anywhere inside it.
    pub(crate) fn has_assoc(self) -> bool {
        self.names().assoc
    }

    /// Whether an element holds an inference variable, anywhere inside it.
    pub(crate) fn holds_infer(self) -> bool {
        self.names().infer
    }

    /// The list with `Type::subst` applied to each element. A list without a parameter comes
    /// back as it is.
    pub(crate) fn subst(self, with: &[Type]) -> TypeArgs {
        self.subst_in(with, &mut FxHashMap::<Type, Type>::default())
    }

    /// `subst`, sharing `memo` with the walk that called it.
    fn subst_in(self, with: &[Type], memo: &mut impl Memo) -> TypeArgs {
        if !self.names().param {
            return self;
        }
        let list: Vec<Type> = self
            .get()
            .iter()
            .map(|ty| ty.subst_in(with, memo))
            .collect();
        TypeArgs::intern(&list)
    }
}

/// As the slice, so a failing test names the types, not a table index.
impl fmt::Debug for TypeArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.get(), f)
    }
}

/// The bytes of type text that `write_type` writes before it cuts the text.
pub(crate) const NAME_LIMIT: usize = 256;

/// A class, an enum, an interface or an associated type (an interface and the index of the
/// associated type among its), which a type's text names by its head.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Head {
    Class(ClassId),
    Struct(StructId),
    Enum(EnumId),
    Interface(InterfaceId),
    Assoc(InterfaceId, u32),
}

#[cfg(test)]
thread_local! {
    /// The types `Type::subst` has substituted on this thread: those that name a parameter.
    static SUBSTITUTED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The kinds `TypeKind::names` has looked at on this thread, one for each type interned.
    static NAMED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The entries `Type::entry` has read on this thread: each read of a kind or a flag.
    pub(crate) static READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The most steps `write_steps` has held at once on this thread.
    static HELD: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The most types `Type::subst` may substitute on this thread: the one that passes it panics,
    /// so a test of a substitution that would take too many steps fails at once.
    static SUBSTITUTED_BUDGET: std::cell::Cell<usize> = const { std::cell::Cell::new(usize::MAX) };
    /// The steps `write_steps` has popped or scanned on this thread.
    pub(crate) static WRITE_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many types `Type::subst` has substituted on this thread: those that name a parameter.
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) fn substituted() -> usize {
    SUBSTITUTED.get()
}

/// Sets the count of `substituted` to 0, and the budget after which a substitution panics.
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) fn start_substituting(budget: usize) {
    SUBSTITUTED.set(0);
    SUBSTITUTED_BUDGET.set(budget);
}

/// What a substitution remembers, so that each distinct type is substituted once.
trait Memo {
    fn get(&self, ty: Type) -> Option<Type>;
    fn put(&mut self, ty: Type, done: Type);
    /// Whether the walk must stop. What it gives from then on is not kept, nor is it a type.
    fn over(&self) -> bool {
        false
    }
}

impl Memo for FxHashMap<Type, Type> {
    fn get(&self, ty: Type) -> Option<Type> {
        FxHashMap::get(self, &ty).copied()
    }

    fn put(&mut self, ty: Type, done: Type) {
        self.insert(ty, done);
    }
}

/// What a substitution that many calls share remembers: for each type and list of arguments, the
/// type it gives.
pub(crate) type SharedMemo = FxHashMap<(Type, TypeArgs), Type>;

/// A `SharedMemo` for the arguments `with`, which stops the walk once the memo holds `limit`
/// entries.
struct Shared<'m> {
    with: TypeArgs,
    memo: &'m mut SharedMemo,
    limit: usize,
}

impl Memo for Shared<'_> {
    fn get(&self, ty: Type) -> Option<Type> {
        self.memo.get(&(ty, self.with)).copied()
    }

    fn put(&mut self, ty: Type, done: Type) {
        self.memo.insert((ty, self.with), done);
    }

    fn over(&self) -> bool {
        self.memo.len() > self.limit
    }
}

/// What `write_steps` writes next: a type, the text that closes one, the arguments of a list
/// after its first, each after a `, `, or the `.` and name of an associated type. A list waits as
/// one slice, so a wide list costs one step.
enum Step<'a> {
    Type(Type),
    Close(&'static str),
    Rest(&'a [Type]),
    Assoc(InterfaceId, u32),
}

/// Writes `ty` to `out` as a program writes it, each class and enum named by `head`. Returns
/// whether the text was cut. When `out` and the closers it owes reach `NAME_LIMIT` bytes, `...`
/// and the owed closers and associated type names end it. Each level entered before the cut adds
/// at most one closer or one name.
pub(crate) fn write_type<'n>(out: &mut String, ty: Type, head: impl Fn(Head) -> &'n str) -> bool {
    write_type_with(out, ty, head, |_| None)
}

/// Writes `ty` to `out`, reusing any `(text, cut)` pair that `named` supplies.
fn write_type_with<'n>(
    out: &mut String,
    ty: Type,
    head: impl Fn(Head) -> &'n str,
    named: impl Fn(Type) -> Option<(&'n str, bool)>,
) -> bool {
    write_steps(out, vec![Step::Type(ty)], 0, head, named)
}

/// Writes the name of a method to `out`: the type `owner` it is declared on, the interface of its
/// conformance block with the interface's arguments, then `method`. Every symbol and every
/// `--emit tir` name of a method is written here, from the parts, so that no code splits a
/// rendered name: a written owner may be cut with `...` before its `.`. Returns whether the text
/// was cut.
pub(crate) fn write_method<'n>(
    out: &mut String,
    owner: Type,
    interface: Option<(&str, TypeArgs)>,
    method: &str,
    head: impl Fn(Head) -> &'n str + Copy,
) -> bool {
    write_method_with(out, owner, interface, method, head, |_| None)
}

/// Writes a method name to `out`, reusing any `(text, cut)` pair that `named` supplies.
pub(crate) fn write_method_with<'n>(
    out: &mut String,
    owner: Type,
    interface: Option<(&str, TypeArgs)>,
    method: &str,
    head: impl Fn(Head) -> &'n str + Copy,
    named: impl Fn(Type) -> Option<(&'n str, bool)> + Copy,
) -> bool {
    let mut cut = write_type_with(out, owner, head, named);
    if let Some((name, args)) = interface {
        out.push('.');
        out.push_str(name);
        cut |= write_args_with(out, args.get(), head, named);
    }
    out.push('.');
    out.push_str(method);
    cut
}

/// Writes `<a, b>` for `args` to `out`, and nothing for none, as `write_type` writes each type.
/// Returns whether the text was cut.
pub(crate) fn write_args<'n>(
    out: &mut String,
    args: &[Type],
    head: impl Fn(Head) -> &'n str,
) -> bool {
    write_args_with(out, args, head, |_| None)
}

/// Writes `<a, b>` for `args` to `out`, reusing any `(text, cut)` pair that `named` supplies.
/// Returns whether the text was cut.
pub(crate) fn write_args_with<'n>(
    out: &mut String,
    args: &[Type],
    head: impl Fn(Head) -> &'n str,
    named: impl Fn(Type) -> Option<(&'n str, bool)>,
) -> bool {
    let mut steps = Vec::new();
    let owed = push_args(out, &mut steps, args);
    write_steps(out, steps, owed, head, named)
}

/// Writes `steps` to `out`, the last first. One string grows in place, and a stack stands in for
/// recursion: a type can nest deeper than the call stack allows. When `named` returns
/// `Some((head, true))`, it writes `head<...>` and marks the output cut. When `named` returns
/// `Some((text, false))` and `out.len() + owed + text.len() <= NAME_LIMIT`, it pastes `text`
/// without descending. Once `out.len() + owed >= NAME_LIMIT`, no type or separator follows, so
/// the work is linear in `NAME_LIMIT` and in the depth of associated types, which are never cut.
#[expect(
    clippy::too_many_lines,
    reason = "one stack-machine loop handles every step and type kind"
)]
fn write_steps<'n>(
    out: &mut String,
    mut steps: Vec<Step<'_>>,
    mut owed: usize,
    head: impl Fn(Head) -> &'n str,
    named: impl Fn(Type) -> Option<(&'n str, bool)>,
) -> bool {
    let mut cut = false;
    while let Some(step) = steps.pop() {
        #[cfg(test)]
        HELD.set(HELD.get().max(steps.len() + 1));
        #[cfg(test)]
        WRITE_STEPS.set(WRITE_STEPS.get() + 1);
        let ty = match step {
            Step::Close(text) => {
                owed -= text.len();
                out.push_str(text);
                continue;
            }
            Step::Rest([]) => continue,
            Step::Type(_) | Step::Rest(_) if out.len() + owed >= NAME_LIMIT => {
                // The cut elides only what has not begun: a type or the rest of a list. A type
                // that has begun gets its end, each closer and each associated type name, so a
                // name that ends in `.Item` or `]` still names what it began to name.
                out.push_str("...");
                for step in steps.iter().rev() {
                    #[cfg(test)]
                    WRITE_STEPS.set(WRITE_STEPS.get() + 1);
                    match step {
                        Step::Close(text) => out.push_str(text),
                        Step::Assoc(interface, k) => {
                            out.push('.');
                            out.push_str(head(Head::Assoc(*interface, *k)));
                        }
                        Step::Type(_) | Step::Rest(_) => {}
                    }
                }
                return true;
            }
            Step::Assoc(interface, k) => {
                out.push('.');
                out.push_str(head(Head::Assoc(interface, k)));
                continue;
            }
            Step::Rest([ty, rest @ ..]) => {
                out.push_str(", ");
                steps.extend([Step::Rest(rest), Step::Type(*ty)]);
                continue;
            }
            Step::Type(ty) => ty,
        };
        if let Some((text, is_cut)) = named(ty) {
            if is_cut {
                out.push_str(text);
                out.push_str("<...>");
                cut = true;
                continue;
            } else if out.len() + owed + text.len() <= NAME_LIMIT {
                out.push_str(text);
                continue;
            }
        }
        let args = match ty.kind() {
            TypeKind::Class(id, args) => {
                out.push_str(head(Head::Class(id)));
                args
            }
            TypeKind::Struct(id, args) => {
                out.push_str(head(Head::Struct(id)));
                args
            }
            TypeKind::Enum(id, args) => {
                out.push_str(head(Head::Enum(id)));
                args
            }
            TypeKind::Any(id, args) => {
                out.push_str("any ");
                out.push_str(head(Head::Interface(id)));
                args
            }
            TypeKind::Array(element) => {
                out.push('[');
                owed += 1;
                steps.extend([Step::Close("]"), Step::Type(element)]);
                continue;
            }
            TypeKind::Optional(payload) => {
                // `any I?` is not a type: the interface value is grouped.
                if matches!(payload.kind(), TypeKind::Any(..)) {
                    out.push('(');
                    owed += 2;
                    steps.extend([Step::Close(")?"), Step::Type(payload)]);
                } else {
                    owed += 1;
                    steps.extend([Step::Close("?"), Step::Type(payload)]);
                }
                continue;
            }
            TypeKind::Assoc(base, interface, _, k) => {
                steps.extend([Step::Assoc(interface, k), Step::Type(base)]);
                continue;
            }
            // A variable that is still open is a hole, as an unbound parameter is.
            TypeKind::Infer(_) => {
                out.push('_');
                continue;
            }
            _ => {
                fmt::Write::write_fmt(out, format_args!("{ty}"))
                    .expect("writing to a `String` should not fail");
                continue;
            }
        };
        owed += push_args(out, &mut steps, args.get());
    }
    debug_assert_eq!(
        owed, 0,
        "all owed closers should be popped before returning"
    );
    cut
}

/// Writes `<` for a nonempty `args` to `out`, and pushes `<a, b>` onto `steps` with `a` on top.
/// Returns the byte count of any closer pushed.
fn push_args<'a>(out: &mut String, steps: &mut Vec<Step<'a>>, args: &'a [Type]) -> usize {
    let [first, rest @ ..] = args else {
        return 0;
    };
    out.push('<');
    steps.extend([Step::Close(">"), Step::Rest(rest), Step::Type(*first)]);
    1
}

/// The kinds without a payload, at the indices of `Type`'s constants.
const SEEDS: [TypeKind; 8] = [
    TypeKind::I64,
    TypeKind::U8,
    TypeKind::F64,
    TypeKind::Bool,
    TypeKind::Char,
    TypeKind::String,
    TypeKind::Unit,
    TypeKind::Never,
];

struct Interner {
    ids: FxHashMap<TypeKind, Type>,
    /// How many types are interned: the index of the next handle.
    len: usize,
}

/// Every type interned in this process. A kind holds only ids and handles, never a program's
/// names or fields, so one table serves every program the process checks.
static TYPES: LazyLock<Mutex<Interner>> = LazyLock::new(|| {
    Mutex::new(Interner {
        ids: SEEDS
            .into_iter()
            .zip(0..)
            .map(|(kind, i)| (kind, Type(i)))
            .collect(),
        len: SEEDS.len(),
    })
});

fn types() -> MutexGuard<'static, Interner> {
    // `intern` changes the table only after its last fallible step, so a poisoned lock still
    // guards a consistent table.
    TYPES.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The slots in the first segment of `KINDS`; segment `s` holds `FIRST << s`.
const FIRST: usize = 64;

/// Segments for every `u32` handle: together they hold `FIRST * (2^SEGMENTS - 1)` kinds.
const SEGMENTS: usize = 27;

const _: () = assert!(
    FIRST * ((1 << SEGMENTS) - 1) > u32::MAX as usize,
    "the segments should hold a kind for every u32 handle"
);

/// A type's kind, and what it names.
type Entry = (TypeKind, Names);

/// The kind of each interned handle past `SEEDS`, at its index, with what it names. `intern` sets
/// a slot under the lock before it hands out the handle, and segments never move, so `kind` reads
/// without the lock: `Type::kind` is the hottest read of the checker and codegen.
static KINDS: [OnceLock<Box<[OnceLock<Entry>]>>; SEGMENTS] = [const { OnceLock::new() }; SEGMENTS];

/// The segment of `KINDS` that holds handle index `i`, and `i`'s slot in it.
fn slot(i: usize) -> (usize, usize) {
    let segment = (i / FIRST + 1).ilog2() as usize;
    (segment, i - FIRST * ((1 << segment) - 1))
}

impl Type {
    pub(crate) const I64: Type = Type(0);
    pub(crate) const U8: Type = Type(1);
    pub(crate) const F64: Type = Type(2);
    pub(crate) const BOOL: Type = Type(3);
    pub(crate) const CHAR: Type = Type(4);
    pub(crate) const STRING: Type = Type(5);
    pub(crate) const UNIT: Type = Type(6);
    pub(crate) const NEVER: Type = Type(7);

    pub(crate) fn intern(kind: TypeKind) -> Type {
        let mut types = types();
        if let Some(&ty) = types.ids.get(&kind) {
            return ty;
        }
        let ty =
            Type(u32::try_from(types.len).expect("a process should intern fewer than 2^32 types"));
        let (segment, at) = slot(types.len);
        let slots =
            KINDS[segment].get_or_init(|| (0..FIRST << segment).map(|_| OnceLock::new()).collect());
        slots[at]
            .set((kind, kind.names()))
            .expect("only `intern` should set a slot, under the lock, at the next index");
        types.len += 1;
        types.ids.insert(kind, ty);
        ty
    }

    /// The structure, read without the lock.
    pub(crate) fn kind(self) -> TypeKind {
        self.entry().0
    }

    /// Whether the type names a type parameter, anywhere inside it.
    pub(crate) fn has_param(self) -> bool {
        self.names().param
    }

    /// Whether the type names a hole (`HOLE_INDEX` or `REJECTED_INDEX`), anywhere inside it.
    pub(crate) fn holds_hole(self) -> bool {
        let names = self.names();
        names.open || names.rejected
    }

    /// Whether the type holds an associated type, anywhere inside it.
    pub(crate) fn has_assoc(self) -> bool {
        self.names().assoc
    }

    /// Whether the type holds an inference variable, anywhere inside it. A cache that outlives a
    /// function's table must not keep a key for which this is true: ids start at 0 in each table.
    pub(crate) fn holds_infer(self) -> bool {
        self.names().infer
    }

    /// The least and the largest id of the inference variables in the type, or `None` when it
    /// holds none.
    pub(crate) fn infer_range(self) -> Option<(u32, u32)> {
        let names = self.names();
        names.infer.then_some((names.infer_lo, names.infer_hi))
    }

    /// Whether the type is an optional, or holds one, other than under an associated type.
    pub(crate) fn holds_optional(self) -> bool {
        self.names().optional
    }

    /// Whether the type holds an `any` type, anywhere inside it.
    pub(crate) fn holds_any(self) -> bool {
        self.names().any
    }

    /// Each interface of an `any` type inside the type, also inside the arguments of another, once
    /// (`any I<i64>` and `any I<bool>` give `I` once). The walk reads each distinct type once,
    /// however many paths lead to it, and enters none that holds no `any` type.
    pub(crate) fn anys(self) -> Vec<InterfaceId> {
        if !self.holds_any() {
            return Vec::new();
        }
        let (mut found, mut seen, mut stack) = (Vec::new(), FxHashSet::default(), vec![self]);
        while let Some(ty) = stack.pop() {
            let (kind, names) = ty.entry();
            if !names.any || !seen.insert(ty) {
                continue;
            }
            match kind {
                TypeKind::Array(inner) | TypeKind::Optional(inner) => stack.push(inner),
                TypeKind::Assoc(base, _, args, _) => {
                    stack.push(base);
                    stack.extend(args.get());
                }
                TypeKind::Class(_, args) | TypeKind::Struct(_, args) | TypeKind::Enum(_, args) => {
                    stack.extend(args.get());
                }
                TypeKind::Any(interface, args) => {
                    if !found.contains(&interface) {
                        found.push(interface);
                    }
                    stack.extend(args.get());
                }
                TypeKind::I64
                | TypeKind::U8
                | TypeKind::F64
                | TypeKind::Bool
                | TypeKind::Char
                | TypeKind::String
                | TypeKind::Unit
                | TypeKind::Never
                | TypeKind::Infer(_)
                | TypeKind::Param(..) => {}
            }
        }
        found
    }

    fn names(self) -> Names {
        self.entry().1
    }

    fn entry(self) -> Entry {
        #[cfg(test)]
        READS.set(READS.get() + 1);
        let i = self.0 as usize;
        // A constant is read before anything is interned, so its kind is not in `KINDS`.
        if let Some(&kind) = SEEDS.get(i) {
            return (kind, Names::default());
        }
        let (segment, at) = slot(i);
        *KINDS[segment]
            .get()
            .and_then(|slots| slots[at].get())
            .expect("`intern` should set a handle's slot before it returns the handle")
    }

    /// The type with each `Param(_, i)` replaced by `args[i]`. A type without a parameter comes
    /// back as it is. An optional is rebuilt with `Type::optional`, so it never flattens. Each
    /// distinct type inside this one is substituted once, however many paths lead to it.
    pub(crate) fn subst(self, args: &[Type]) -> Type {
        self.subst_in(args, &mut FxHashMap::<Type, Type>::default())
    }

    /// `subst` for the list `with`, through `memo`, which the caller keeps from one call to the
    /// next: a type is substituted once for each list, whatever calls ask. `None` when the memo
    /// holds more than `limit` entries before the walk is done.
    pub(crate) fn subst_shared(
        self,
        with: TypeArgs,
        memo: &mut SharedMemo,
        limit: usize,
    ) -> Option<Type> {
        if !self.has_param() {
            return Some(self);
        }
        let mut shared = Shared { with, memo, limit };
        let done = self.subst_walk(with.get(), &mut shared);
        (!shared.over()).then_some(done)
    }

    /// How many levels of types nest in the type: 0 for one with no part, such as `i64` and
    /// `Node`, and one more than its deepest part for another. A read of one entry.
    pub(crate) fn depth(self) -> u32 {
        self.names().depth
    }

    /// `subst`, and whether `self` holds an associated type, read with the one look at what the
    /// type names that `subst` takes. A caller that replaces the associated types of the result
    /// asks this, and not `has_assoc` of the result, which is a second look.
    pub(crate) fn subst_noting_assoc(self, args: &[Type]) -> (Type, bool) {
        let names = self.names();
        if !names.param {
            return (self, names.assoc);
        }
        (
            self.subst_walk(args, &mut FxHashMap::<Type, Type>::default()),
            names.assoc,
        )
    }

    /// `subst`, with `memo` holding each type that the walk has already substituted.
    fn subst_in(self, args: &[Type], memo: &mut impl Memo) -> Type {
        if !self.has_param() {
            return self;
        }
        self.subst_walk(args, memo)
    }

    /// `subst_in` for a type that names a parameter.
    fn subst_walk(self, args: &[Type], memo: &mut impl Memo) -> Type {
        if memo.over() {
            return self;
        }
        if let Some(done) = memo.get(self) {
            return done;
        }
        #[cfg(test)]
        {
            SUBSTITUTED.set(SUBSTITUTED.get() + 1);
            assert!(
                SUBSTITUTED.get() <= SUBSTITUTED_BUDGET.get(),
                "a substitution should stay within its budget of {} types",
                SUBSTITUTED_BUDGET.get()
            );
        }
        let done = ensure_stack(|| match self.kind() {
            TypeKind::Param(_, i) => args[i as usize],
            TypeKind::Array(element) => Type::array(element.subst_in(args, memo)),
            TypeKind::Optional(payload) => Type::optional(payload.subst_in(args, memo)),
            TypeKind::Class(id, list) => {
                Type::intern(TypeKind::Class(id, list.subst_in(args, memo)))
            }
            TypeKind::Struct(id, list) => {
                Type::intern(TypeKind::Struct(id, list.subst_in(args, memo)))
            }
            TypeKind::Enum(id, list) => Type::intern(TypeKind::Enum(id, list.subst_in(args, memo))),
            TypeKind::Any(id, list) => Type::intern(TypeKind::Any(id, list.subst_in(args, memo))),
            TypeKind::Assoc(base, interface, list, k) => Type::intern(TypeKind::Assoc(
                base.subst_in(args, memo),
                interface,
                list.subst_in(args, memo),
                k,
            )),
            TypeKind::I64
            | TypeKind::U8
            | TypeKind::F64
            | TypeKind::Bool
            | TypeKind::Char
            | TypeKind::String
            | TypeKind::Unit
            | TypeKind::Never
            | TypeKind::Infer(_) => {
                unreachable!("only a kind with a child can name a parameter")
            }
        });
        if !memo.over() {
            memo.put(self, done);
        }
        done
    }

    /// `[element]`.
    pub(crate) fn array(element: Type) -> Type {
        Type::intern(TypeKind::Array(element))
    }

    /// `payload?`.
    pub(crate) fn optional(payload: Type) -> Type {
        Type::intern(TypeKind::Optional(payload))
    }
}

/// As the kind, so a failing test names the type, not a table index.
impl fmt::Debug for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.kind(), f)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn equal_structures_share_a_handle() {
        let a = Type::array(Type::optional(Type::I64));
        assert_eq!(a, Type::array(Type::optional(Type::I64)));
        assert_ne!(a, Type::optional(Type::array(Type::I64)));
        assert_ne!(Type::array(Type::I64), Type::array(Type::U8));
        assert_ne!(Type::optional(Type::I64), Type::optional(Type::U8));
        let class = |n| Type::intern(TypeKind::Class(ClassId(n), TypeArgs::NONE));
        assert_eq!(class(0), class(0));
        assert_ne!(class(0), class(1));
        assert_ne!(
            class(0),
            Type::intern(TypeKind::Enum(EnumId(0), TypeArgs::NONE))
        );
        assert_eq!(a.kind(), TypeKind::Array(Type::optional(Type::I64)));
    }

    #[test]
    fn type_args_should_be_interned_lists() {
        let pair = TypeArgs::intern(&[Type::I64, Type::STRING]);
        assert_eq!(pair, TypeArgs::intern(&[Type::I64, Type::STRING]));
        assert_ne!(pair, TypeArgs::intern(&[Type::STRING, Type::I64]));
        assert_eq!(TypeArgs::intern(&[]), TypeArgs::NONE);
        assert_eq!(pair.get(), &[Type::I64, Type::STRING]);
        let one = TypeArgs::intern(&[Type::I64]);
        assert_ne!(one, TypeArgs::NONE);
        assert_eq!(one.get(), &[Type::I64]);
        // Kept so that a failing test's message names the types, not a table index.
        assert_eq!(format!("{pair:?}"), "[I64, String]");
        let stack = |args| Type::intern(TypeKind::Class(ClassId(0), args));
        assert_ne!(stack(pair), stack(TypeArgs::NONE));
    }

    /// `t_0 = leaf` and `t_{i+1} = P<t_i, t_i>` over one class `P`: `depth + 1` interned types
    /// whose text, written whole, doubles at each level.
    fn doubled(leaf: Type, depth: usize) -> Type {
        (0..depth).fold(leaf, |t, _| {
            Type::intern(TypeKind::Class(ClassId(0), TypeArgs::intern(&[t, t])))
        })
    }

    /// The text of `ty`, each class named `P`, and whether it was cut.
    fn text(ty: Type) -> (String, bool) {
        let mut out = String::new();
        let cut = write_type(&mut out, ty, |_| "P");
        (out, cut)
    }

    fn count(text: &str, c: char) -> usize {
        text.chars().filter(|&x| x == c).count()
    }

    #[test]
    fn type_text_should_be_cut_at_the_limit() {
        let whole = "P<P<P<i64, i64>, P<i64, i64>>, P<P<i64, i64>, P<i64, i64>>>";
        assert_eq!(text(doubled(Type::I64, 3)), (whole.to_string(), false));
        // Written whole, `t_40` has 2^40 leaves.
        let (long, cut) = text(doubled(Type::I64, 40));
        assert!(cut, "{long}");
        assert!(long.len() <= NAME_LIMIT + 64, "{long}");
        assert!(long.contains("..."), "{long}");
        assert_eq!(count(&long, '<'), count(&long, '>'), "{long}");
        let (optional, cut) = text(Type::array(Type::optional(doubled(Type::I64, 40))));
        assert!(cut && optional.ends_with(">?]"), "{optional}");
        let nested = |depth| (0..depth).fold(Type::I64, |t, _| Type::array(t));
        let (deep, cut) = text(nested(300));
        assert!(cut, "{deep}");
        assert_eq!(count(&deep, '['), count(&deep, ']'), "{deep}");
        assert!(deep.len() <= 2 * NAME_LIMIT + 3, "{deep}");
        // 257 bytes, past the limit only by its closers.
        let (closed, cut) = text(nested(127));
        assert_eq!(closed.len(), 257, "{closed}");
        assert!(!cut && !closed.contains("..."), "{closed}");
    }

    /// A cut elides only what has not begun, so a name that is cut inside the type before an
    /// associated type's name keeps that name: the text names an associated type, and not a class.
    /// The names are all `P` here, except where a case says so.
    #[test]
    fn type_text_should_keep_the_associated_type_name_when_the_base_is_cut() {
        let item = |base| Type::intern(TypeKind::Assoc(base, InterfaceId(0), TypeArgs::NONE, 0));
        let (cut_inside, cut) = text(item(doubled(Type::I64, 40)));
        assert!(cut && cut_inside.contains("..."), "{cut_inside}");
        assert!(cut_inside.ends_with(">.P"), "{cut_inside}");
        assert_eq!(
            count(&cut_inside, '<'),
            count(&cut_inside, '>'),
            "{cut_inside}"
        );
        // The name after the closers of an array and an optional, outermost last.
        let (wrapped, _) = text(Type::array(item(item(Type::optional(doubled(
            Type::I64,
            40,
        ))))));
        assert!(wrapped.ends_with("?.P.P]"), "{wrapped}");
        // A chain of them is begun when its base is written, so it is written whole.
        let chain = (0..200).fold(item(Type::I64), |t, _| item(t));
        let (chain, cut) = text(chain);
        assert!(!cut && chain.rsplit('.').next() == Some("P"), "{chain}");
        // The base ends exactly at the limit, before its name, inside an array: the cut is at the
        // step of the name, which is not elided.
        let long = "x".repeat(NAME_LIMIT - 1);
        let named = |ty| {
            let mut out = String::new();
            let cut = write_type(&mut out, ty, |head| match head {
                Head::Class(_) => &long,
                Head::Assoc(_, k) => ["B", "C"][k as usize],
                Head::Enum(_) | Head::Interface(_) | Head::Struct(_) => {
                    unreachable!("the type holds no enum and no struct")
                }
            });
            (out, cut)
        };
        let base = Type::intern(TypeKind::Class(ClassId(0), TypeArgs::NONE));
        let assoc =
            |base, k| Type::intern(TypeKind::Assoc(base, InterfaceId(0), TypeArgs::NONE, k));
        let (text, cut) = named(Type::array(assoc(base, 0)));
        assert!(!cut && text == format!("[{long}.B]"), "{text}");
        // The name `.C` of the outer associated type stands before the first closer, and stays.
        let (text, cut) = named(Type::array(assoc(assoc(base, 0), 1)));
        assert!(!cut && text == format!("[{long}.B.C]"), "{text}");
    }

    /// A list of 100,000 arguments, cut after a few dozen of them.
    #[test]
    fn steps_held_should_stay_within_the_text_when_a_list_is_wide() {
        let wide = TypeArgs::intern(&vec![Type::I64; 100_000]);
        HELD.set(0);
        let (text, cut) = text(Type::intern(TypeKind::Class(ClassId(0), wide)));
        assert!(cut && text.len() <= NAME_LIMIT + 64, "{text}");
        assert!(HELD.get() <= text.len(), "{} steps for {text}", HELD.get());
    }

    #[test]
    fn seed_kinds_should_name_no_parameter() {
        for kind in SEEDS {
            assert!(!kind.names().param, "{kind:?}");
        }
    }

    /// `t_20` has 21 distinct types but 2^20 paths to its leaf.
    #[test]
    fn subst_should_visit_each_shared_type_once() {
        let param = Type::intern(TypeKind::Param(Symbol::intern("T"), 0));
        SUBSTITUTED.set(0);
        assert_eq!(
            doubled(param, 20).subst(&[Type::I64]),
            doubled(Type::I64, 20)
        );
        assert!(SUBSTITUTED.get() <= 2 * 21, "{} visits", SUBSTITUTED.get());
    }

    /// An optional is found inside an array and the arguments of a class, an enum and an `any`
    /// type, and nowhere else: not under an associated type, and not in a type parameter.
    #[test]
    fn holds_optional_should_find_an_optional_outside_an_associated_type() {
        let optional = Type::optional(Type::I64);
        let args = |ty| TypeArgs::intern(&[ty]);
        let param = Type::intern(TypeKind::Param(Symbol::intern("T"), 0));
        let assoc = |base, args| Type::intern(TypeKind::Assoc(base, InterfaceId(0), args, 0));
        let class = |ty| Type::intern(TypeKind::Class(ClassId(0), args(ty)));
        for (name, ty, want) in [
            ("i64", Type::I64, false),
            ("i64?", optional, true),
            ("[i64?]", Type::array(optional), true),
            ("[i64]", Type::array(Type::I64), false),
            ("class of i64?", class(optional), true),
            ("class of i64", class(Type::I64), false),
            (
                "enum of i64?",
                Type::intern(TypeKind::Enum(EnumId(0), args(optional))),
                true,
            ),
            ("param", param, false),
            ("assoc over i64?", assoc(optional, TypeArgs::NONE), false),
            (
                "assoc with i64? arguments",
                assoc(param, args(optional)),
                false,
            ),
            (
                "any of i64?",
                Type::intern(TypeKind::Any(InterfaceId(0), args(optional))),
                true,
            ),
        ] {
            assert_eq!(ty.holds_optional(), want, "{name}");
        }
    }

    /// `t_20` has 21 distinct types but 2^20 paths to its leaf. A walk of it reads many entries; a
    /// flag read reads one.
    #[test]
    fn flag_queries_should_read_one_entry_whatever_the_depth_of_a_type() {
        let hole = Type::intern(TypeKind::Param(Symbol::intern("_"), HOLE_INDEX));
        let rejected = Type::intern(TypeKind::Param(Symbol::intern("_"), REJECTED_INDEX));
        let param = Type::intern(TypeKind::Param(Symbol::intern("T"), 0));
        let mixed = Type::intern(TypeKind::Class(
            ClassId(0),
            TypeArgs::intern(&[hole, Type::array(rejected)]),
        ));
        NAMED.set(0);
        let (holed, rejected, named, plain) = (
            doubled(hole, 20),
            doubled(rejected, 20),
            doubled(Type::optional(param), 20),
            doubled(Type::I64, 20),
        );
        assert!(NAMED.get() <= 4 * 21, "{} kinds looked at", NAMED.get());
        let reads = |query: fn(Type) -> bool, ty: Type| {
            READS.set(0);
            let answer = query(ty);
            (answer, READS.get())
        };
        for (ty, hole, param, optional) in [
            (holed, true, true, false),
            (rejected, true, true, false),
            (mixed, true, true, false),
            (named, false, true, true),
            (plain, false, false, false),
        ] {
            assert_eq!(reads(Type::holds_hole, ty), (hole, 1));
            assert_eq!(reads(Type::has_param, ty), (param, 1));
            assert_eq!(reads(Type::holds_optional, ty), (optional, 1));
        }
        // A walk of `t_20` for its depth would read the entries of every part.
        for (ty, depth) in [
            (holed, 20),
            (rejected, 20),
            (named, 21),
            (plain, 20),
            (mixed, 2),
        ] {
            READS.set(0);
            assert_eq!((ty.depth(), READS.get()), (depth, 1));
        }
    }

    /// `t_n` has `n + 1` distinct types but 2^n paths to its `any` leaf. The reads for 20, 40 and
    /// 60 levels grow in a line, and a type with no `any` type costs one read.
    #[test]
    fn anys_should_read_each_shared_type_once() {
        let any = Type::intern(TypeKind::Any(InterfaceId(3), TypeArgs::NONE));
        let reads = |depth| {
            let ty = doubled(any, depth);
            READS.set(0);
            assert_eq!(ty.anys(), [InterfaceId(3)], "depth {depth}");
            READS.get()
        };
        crate::mir::tests::assert_near_linear(reads(20), reads(40), reads(60));
        let plain = doubled(Type::I64, 20);
        READS.set(0);
        assert_eq!(plain.anys(), []);
        assert_eq!(READS.get(), 1);
    }

    /// One interface in two `any` types of one type is found once.
    #[test]
    fn anys_should_give_an_interface_once() {
        let any = |args| Type::intern(TypeKind::Any(InterfaceId(7), TypeArgs::intern(&[args])));
        let pair = TypeArgs::intern(&[any(Type::I64), any(Type::BOOL)]);
        let class = Type::intern(TypeKind::Class(ClassId(0), pair));
        assert_eq!(class.anys(), [InterfaceId(7)]);
    }

    /// An `any` type is found inside an array, an optional, the arguments of a class, an enum and
    /// another `any` type, and an associated type, and nowhere else.
    #[test]
    fn anys_should_find_each_interface_inside_a_type() {
        let any = |i, args| Type::intern(TypeKind::Any(InterfaceId(i), args));
        let inner = any(1, TypeArgs::NONE);
        let outer = any(2, TypeArgs::intern(&[Type::optional(inner)]));
        let class = Type::intern(TypeKind::Class(ClassId(0), TypeArgs::intern(&[outer])));
        let assoc = Type::intern(TypeKind::Assoc(
            Type::intern(TypeKind::Enum(
                EnumId(0),
                TypeArgs::intern(&[any(4, TypeArgs::NONE)]),
            )),
            InterfaceId(5),
            TypeArgs::intern(&[any(6, TypeArgs::NONE)]),
            0,
        ));
        let mut found = Type::array(class).anys();
        found.sort_unstable_by_key(|i| i.0);
        assert_eq!(found, [InterfaceId(1), InterfaceId(2)]);
        assert!(Type::array(class).holds_any());
        let mut found = assoc.anys();
        found.sort_unstable_by_key(|i| i.0);
        assert_eq!(found, [InterfaceId(4), InterfaceId(6)]);
        assert!(assoc.holds_any());
        let plain = Type::intern(TypeKind::Class(ClassId(0), TypeArgs::intern(&[Type::I64])));
        assert!(!plain.holds_any() && plain.anys().is_empty());
    }

    /// A type with no part is 0 levels deep, and each array, optional, type argument list and
    /// associated type adds one over its deepest part.
    #[test]
    fn depth_should_count_the_levels_of_a_type() {
        let class =
            |args: &[Type]| Type::intern(TypeKind::Class(ClassId(0), TypeArgs::intern(args)));
        let param = Type::intern(TypeKind::Param(Symbol::intern("T"), 0));
        let item = |base| Type::intern(TypeKind::Assoc(base, InterfaceId(0), TypeArgs::NONE, 0));
        for (ty, depth) in [
            (Type::I64, 0),
            (param, 0),
            (class(&[]), 0),
            (Type::array(Type::I64), 1),
            (Type::optional(Type::array(Type::I64)), 2),
            (class(&[Type::I64]), 1),
            (class(&[Type::I64, Type::array(param)]), 2),
            (Type::array(class(&[class(&[])])), 2),
            (item(param), 1),
            (Type::array(item(item(param))), 3),
        ] {
            assert_eq!(ty.depth(), depth, "{ty:?}");
        }
    }

    /// The memo that the caller owns holds a type once for each list of arguments, so a second
    /// substitution of the same list visits nothing, and another list is substituted afresh. A
    /// walk that passes the limit gives `None` and keeps nothing it computed after the limit.
    #[test]
    fn shared_subst_should_substitute_once_for_each_list_of_arguments() {
        let param = Type::intern(TypeKind::Param(Symbol::intern("T"), 0));
        let ty = doubled(param, 5);
        let (ints, strings) = (
            TypeArgs::intern(&[Type::I64]),
            TypeArgs::intern(&[Type::STRING]),
        );
        let mut memo = SharedMemo::default();
        SUBSTITUTED.set(0);
        assert_eq!(
            ty.subst_shared(ints, &mut memo, usize::MAX),
            Some(doubled(Type::I64, 5))
        );
        let first = SUBSTITUTED.get();
        assert_eq!(first, 6, "one visit for each of the 6 distinct types");
        assert_eq!(
            ty.subst_shared(ints, &mut memo, usize::MAX),
            Some(doubled(Type::I64, 5))
        );
        assert_eq!(
            SUBSTITUTED.get(),
            first,
            "the second use should read the memo"
        );
        assert_eq!(
            ty.subst_shared(strings, &mut memo, usize::MAX),
            Some(doubled(Type::STRING, 5))
        );
        assert_eq!(SUBSTITUTED.get(), 2 * first);
        // 12 entries so far: a limit of 15 lets 3 more in, and the walk of 6 types passes it.
        let atoms = TypeArgs::intern(&[Type::BOOL]);
        assert_eq!(ty.subst_shared(atoms, &mut memo, 15), None);
        assert_eq!(
            memo.len(),
            16,
            "the walk stops at the first entry past the limit"
        );
        // Nothing that the cut walk computed after the limit is kept: it is no type.
        assert_eq!(
            ty.subst_shared(atoms, &mut memo, usize::MAX),
            Some(doubled(Type::BOOL, 5))
        );
    }

    #[test]
    fn a_shared_substitution_should_fit_a_limit_of_exactly_its_entries() {
        let param = Type::intern(TypeKind::Param(Symbol::intern("T"), 0));
        let ty = doubled(param, 5);
        let ints = TypeArgs::intern(&[Type::I64]);
        // The walk adds one entry for each of the 6 distinct types.
        assert_eq!(
            ty.subst_shared(ints, &mut SharedMemo::default(), 6),
            Some(doubled(Type::I64, 5))
        );
        assert_eq!(ty.subst_shared(ints, &mut SharedMemo::default(), 5), None);
    }

    #[test]
    fn subst_should_replace_each_param_by_index() {
        let t = Type::intern(TypeKind::Param(Symbol::intern("T"), 0));
        let u = Type::intern(TypeKind::Param(Symbol::intern("U"), 1));
        let class =
            |args: &[Type]| Type::intern(TypeKind::Class(ClassId(0), TypeArgs::intern(args)));
        let ty = Type::array(class(&[Type::optional(u), t]));
        let args = [Type::STRING, Type::array(Type::I64)];
        assert_eq!(
            ty.subst(&args),
            Type::array(class(&[
                Type::optional(Type::array(Type::I64)),
                Type::STRING
            ]))
        );
        assert!(ty.has_param());
        assert!(!ty.subst(&args).has_param());
        let tree = |args: &[Type]| Type::intern(TypeKind::Enum(EnumId(0), TypeArgs::intern(args)));
        assert_eq!(tree(&[u]).subst(&args), tree(&[Type::array(Type::I64)]));
        assert_eq!(Type::BOOL.subst(&args), Type::BOOL);
    }

    #[test]
    fn constants_are_their_kinds() {
        for (ty, kind) in [
            (Type::I64, TypeKind::I64),
            (Type::U8, TypeKind::U8),
            (Type::F64, TypeKind::F64),
            (Type::BOOL, TypeKind::Bool),
            (Type::CHAR, TypeKind::Char),
            (Type::STRING, TypeKind::String),
            (Type::UNIT, TypeKind::Unit),
            (Type::NEVER, TypeKind::Never),
        ] {
            assert_eq!(Type::intern(kind), ty, "{kind:?}");
            assert_eq!(ty.kind(), kind, "{kind:?}");
        }
    }

    #[test]
    fn slots_fill_each_segment_in_order() {
        assert_eq!(slot(0), (0, 0));
        assert_eq!(slot(FIRST - 1), (0, FIRST - 1));
        assert_eq!(slot(FIRST), (1, 0));
        assert_eq!(slot(3 * FIRST - 1), (1, 2 * FIRST - 1));
        assert_eq!(slot(3 * FIRST), (2, 0));
        let last = slot(u32::MAX as usize);
        assert!(last.0 < SEGMENTS && last.1 < FIRST << last.0, "{last:?}");
    }

    #[test]
    fn threads_share_handles() {
        // Thread k interns the 64 structures starting at k * 8, so a per-thread table, which
        // numbers them in its own order, gives each thread different handles.
        let build = |k: usize| {
            let mut handles = vec![None; 64];
            for i in 0..64 {
                let n = (k * 8 + i) % 64;
                handles[n] = Some(Type::array(Type::intern(TypeKind::Class(
                    ClassId(1000 + n),
                    TypeArgs::NONE,
                ))));
            }
            // Another thread may have interned a handle first, so this reads its kind unlocked.
            for (n, ty) in handles.iter().enumerate() {
                let class = Type::intern(TypeKind::Class(ClassId(1000 + n), TypeArgs::NONE));
                assert_eq!(ty.map(Type::kind), Some(TypeKind::Array(class)));
            }
            handles
        };
        let threads: Vec<_> = (1..8)
            .map(|k| std::thread::spawn(move || build(k)))
            .collect();
        let first = build(0);
        for t in threads {
            assert_eq!(
                t.join().expect("an interning thread should not panic"),
                first
            );
        }
    }

    /// An associated type names the parameters of its base and its arguments, holds an associated
    /// type, and is written `S.Item`, also inside the types that hold it.
    #[test]
    fn an_associated_type_should_substitute_and_write_as_a_member() {
        let param = Type::intern(TypeKind::Param(Symbol::intern("S"), 0));
        let item = |base, args| Type::intern(TypeKind::Assoc(base, InterfaceId(2), args, 1));
        let ty = item(param, TypeArgs::NONE);
        assert!(ty.has_param() && ty.has_assoc());
        assert!(!Type::I64.has_assoc() && !param.has_assoc());
        assert!(!Type::array(Type::optional(param)).has_assoc());
        assert!(Type::array(Type::optional(ty)).has_assoc());
        // The arguments of the interface count too.
        let with_arg = item(Type::I64, TypeArgs::intern(&[param]));
        assert!(with_arg.has_param() && with_arg.has_assoc());
        assert!(TypeArgs::intern(&[Type::I64, ty]).has_assoc());
        assert!(!TypeArgs::intern(&[Type::I64, param]).has_assoc());
        // Substituting the base keeps the associated type, which normalizing replaces later.
        let class = Type::intern(TypeKind::Class(ClassId(0), TypeArgs::NONE));
        let substituted = ty.subst(&[class]);
        assert_eq!(substituted, item(class, TypeArgs::NONE));
        assert!(!substituted.has_param() && substituted.has_assoc());
        assert_eq!(ty.subst_noting_assoc(&[class]), (substituted, true));
        assert_eq!(Type::I64.subst_noting_assoc(&[class]), (Type::I64, false));
        assert_eq!(param.subst_noting_assoc(&[class]), (class, false));
        // `S.Item`, and `S.Item.Item`, with the name of the associated type from the head.
        let named = |ty| {
            let mut out = String::new();
            let cut = write_type(&mut out, ty, |_| "Item");
            (out, cut)
        };
        assert_eq!(named(ty), ("S.Item".to_string(), false));
        assert_eq!(
            named(Type::array(Type::optional(item(ty, TypeArgs::NONE)))),
            ("[S.Item.Item?]".to_string(), false)
        );
        // A chain of them is begun when its base is written, and a type that has begun gets its
        // end: no link is elided, so the text is as long as the whole type (`S` and 201 `.Item`),
        // and no more, and it is not cut.
        let mut deep = ty;
        for _ in 0..200 {
            deep = item(deep, TypeArgs::NONE);
        }
        let (text, cut) = named(deep);
        assert!(!cut && text.starts_with("S.Item.Item"), "{text}");
        assert!(
            text.rsplit('.').next() == Some("Item") && text.len() == 1 + 201 * ".Item".len(),
            "{text}"
        );
    }

    /// A deeply nested type substitutes on a 64 KiB stack without stack overflow.
    #[test]
    fn subst_walk_should_follow_a_deep_type_on_a_small_stack() {
        const STACK: usize = 64 * 1024;
        // Without `ensure_stack` in `subst_walk`, 449 levels abort on this stack and 448 pass:
        // 4,500 levels give about ten times the abort depth.
        let levels = 4_500;
        let result = std::thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || {
                let param = Type::intern(TypeKind::Param(Symbol::intern("T"), 0));
                let mut ty = param;
                let mut want = Type::I64;
                for _ in 0..levels {
                    ty = Type::array(ty);
                    want = Type::array(want);
                }
                (ty.subst(&[Type::I64]), want)
            })
            .expect("the thread should start")
            .join()
            .expect("the walk should not panic");
        assert_eq!(result.0, result.1);
    }

    #[test]
    fn write_args_with_should_paste_fitting_whole_names_and_fall_through_on_overflow() {
        let leaf = Type::intern(TypeKind::Class(ClassId(0), TypeArgs::NONE));
        let wrapped = doubled(leaf, 4);
        let mut whole_wrapped = String::new();
        assert!(!write_type(&mut whole_wrapped, wrapped, |_| "P"));
        assert!(whole_wrapped.len() < NAME_LIMIT);

        // Exact boundary: `prefix` + `<` + `whole_wrapped` + `>` reaches `NAME_LIMIT` without
        // descending.
        let prefix = "X".repeat(NAME_LIMIT - 2 - whole_wrapped.len());
        let mut out = prefix.clone();
        WRITE_STEPS.set(0);
        let cut = write_args_with(
            &mut out,
            &[wrapped],
            |_| "P",
            |ty| (ty == wrapped).then_some((whole_wrapped.as_str(), false)),
        );
        assert!(!cut);
        assert_eq!(out, format!("{prefix}<{whole_wrapped}>"));
        assert_eq!(WRITE_STEPS.get(), 3);

        // One byte over `NAME_LIMIT`: falls through and walks `wrapped` just like `write_args`.
        let mut over = format!("Y{prefix}");
        let mut plain = over.clone();
        WRITE_STEPS.set(0);
        let cut_with = write_args_with(
            &mut over,
            &[wrapped],
            |_| "P",
            |ty| (ty == wrapped).then_some((whole_wrapped.as_str(), false)),
        );
        let steps_over = WRITE_STEPS.get();
        let cut_plain = write_args(&mut plain, &[wrapped], |_| "P");
        assert_eq!((cut_with, over), (cut_plain, plain));
        assert!(steps_over > 3, "{steps_over} steps");

        // Cut cached child: writes `head<...>` and returns `true`.
        let mut cut_out = String::from("C");
        WRITE_STEPS.set(0);
        let cut = write_args_with(
            &mut cut_out,
            &[wrapped],
            |_| "P",
            |ty| (ty == wrapped).then_some(("P", true)),
        );
        assert!(cut);
        assert_eq!(cut_out, "C<P<...>>");
        assert_eq!(WRITE_STEPS.get(), 3);
    }

    #[test]
    fn nested_optionals_should_cut_at_the_limit_with_bounded_steps() {
        let nested_opt = |depth| (0..depth).fold(Type::I64, |t, _| Type::optional(t));
        HELD.set(0);
        WRITE_STEPS.set(0);
        let (deep, cut) = text(nested_opt(10_000));
        assert!(cut, "{deep}");
        assert_eq!(deep, format!("...{}", "?".repeat(NAME_LIMIT)));
        assert!(HELD.get() <= NAME_LIMIT + 4, "{} held", HELD.get());
        assert!(
            WRITE_STEPS.get() <= 2 * NAME_LIMIT + 4,
            "{} steps",
            WRITE_STEPS.get()
        );

        let (uncut, cut) = text(nested_opt(NAME_LIMIT - 1));
        assert!(!cut, "{uncut}");
        assert_eq!(uncut, format!("i64{}", "?".repeat(NAME_LIMIT - 1)));
        let (at_limit, cut) = text(nested_opt(NAME_LIMIT));
        assert!(cut, "{at_limit}");
        assert_eq!(at_limit, format!("...{}", "?".repeat(NAME_LIMIT)));

        let class = |arg| Type::intern(TypeKind::Class(ClassId(0), TypeArgs::intern(&[arg])));
        let (uncut, cut) = text(class(nested_opt(NAME_LIMIT - 4)));
        assert!(!cut, "{uncut}");
        assert_eq!(uncut, format!("P<i64{}>", "?".repeat(NAME_LIMIT - 4)));
        let (at_limit, cut) = text(class(nested_opt(NAME_LIMIT - 3)));
        assert!(cut, "{at_limit}");
        assert_eq!(at_limit, format!("P<...{}>", "?".repeat(NAME_LIMIT - 3)));

        let args_text = |arg| {
            let mut out = String::from("f");
            let cut = write_args(&mut out, &[arg], |_| "P");
            (out, cut)
        };
        let (uncut, cut) = args_text(nested_opt(NAME_LIMIT - 4));
        assert!(!cut, "{uncut}");
        assert_eq!(uncut, format!("f<i64{}>", "?".repeat(NAME_LIMIT - 4)));
        let (at_limit, cut) = args_text(nested_opt(NAME_LIMIT - 3));
        assert!(cut, "{at_limit}");
        assert_eq!(at_limit, format!("f<...{}>", "?".repeat(NAME_LIMIT - 3)));

        let any_i = Type::intern(TypeKind::Any(InterfaceId(0), TypeArgs::NONE));
        let opt_any = Type::optional(any_i);
        let wrap_opt = |base, depth| (0..depth).fold(base, |t, _| Type::optional(t));
        let named_any = |ty| {
            let mut out = String::new();
            let cut = write_type(&mut out, ty, |_| "I");
            (out, cut)
        };
        let (uncut, cut) = named_any(wrap_opt(opt_any, NAME_LIMIT - 4));
        assert!(!cut, "{uncut}");
        assert_eq!(uncut, format!("(any I)?{}", "?".repeat(NAME_LIMIT - 4)));
        let (at_limit, cut) = named_any(wrap_opt(opt_any, NAME_LIMIT - 3));
        assert!(cut, "{at_limit}");
        assert_eq!(at_limit, format!("(...)?{}", "?".repeat(NAME_LIMIT - 3)));
    }

    #[test]
    fn popped_closers_should_not_count_toward_later_arguments() {
        let nested_opt = (0..130).fold(Type::I64, |t, _| Type::optional(t));
        let pair = Type::intern(TypeKind::Class(
            ClassId(0),
            TypeArgs::intern(&[nested_opt, Type::I64]),
        ));
        let (got, cut) = text(pair);
        assert!(!cut, "{got}");
        assert_eq!(got, format!("P<i64{}, i64>", "?".repeat(130)));
    }

    #[test]
    fn write_args_with_should_not_paste_past_limit_when_closers_are_owed() {
        let leaf = Type::intern(TypeKind::Class(ClassId(0), TypeArgs::NONE));
        let wrapped = doubled(leaf, 4);
        let mut whole_wrapped = String::new();
        assert!(!write_type(&mut whole_wrapped, wrapped, |_| "P"));
        assert!(2 + whole_wrapped.len() <= NAME_LIMIT);

        // `out` starts as `"C"` (`"C<"` is 2 bytes), and `owed` is `1 + k` (`>` and `k` `?`s).
        let k = NAME_LIMIT - 3 - whole_wrapped.len();
        assert!(3 + k < NAME_LIMIT);
        let fit_ty = (0..k).fold(wrapped, |t, _| Type::optional(t));
        let mut fitted = String::from("C");
        WRITE_STEPS.set(0);
        let cut_fit = write_args_with(
            &mut fitted,
            &[fit_ty],
            |_| "P",
            |ty| (ty == wrapped).then_some((whole_wrapped.as_str(), false)),
        );
        assert!(!cut_fit);
        assert_eq!(fitted, format!("C<{whole_wrapped}{}>", "?".repeat(k)));
        assert_eq!(fitted.len(), NAME_LIMIT);
        assert_eq!(WRITE_STEPS.get(), 2 * k + 3);

        let over_ty = Type::optional(fit_ty);
        let mut over = String::from("C");
        let mut plain = String::from("C");
        WRITE_STEPS.set(0);
        let cut_with = write_args_with(
            &mut over,
            &[over_ty],
            |_| "P",
            |ty| (ty == wrapped).then_some((whole_wrapped.as_str(), false)),
        );
        let over_steps = WRITE_STEPS.get();
        let cut_plain = write_args(&mut plain, &[over_ty], |_| "P");
        assert!(cut_with);
        assert_eq!((cut_with, over), (cut_plain, plain));
        assert!(over_steps > 2 * (k + 1) + 3, "{over_steps} steps");
    }

    #[test]
    fn without_optionals_should_read_one_entry_at_any_depth() {
        let param = Type::intern(TypeKind::Param(Symbol::intern("T"), 0));
        let bases = [
            ("i64", Type::I64),
            ("u8", Type::U8),
            ("f64", Type::F64),
            ("bool", Type::BOOL),
            ("char", Type::CHAR),
            ("string", Type::STRING),
            ("unit", Type::UNIT),
            ("never", Type::NEVER),
            (
                "class",
                Type::intern(TypeKind::Class(ClassId(0), TypeArgs::NONE)),
            ),
            (
                "struct",
                Type::intern(TypeKind::Struct(StructId(0), TypeArgs::NONE)),
            ),
            (
                "enum",
                Type::intern(TypeKind::Enum(EnumId(0), TypeArgs::NONE)),
            ),
            ("param", param),
            ("array", Type::array(Type::I64)),
            (
                "assoc",
                Type::intern(TypeKind::Assoc(param, InterfaceId(0), TypeArgs::NONE, 0)),
            ),
            (
                "any",
                Type::intern(TypeKind::Any(InterfaceId(0), TypeArgs::NONE)),
            ),
        ];
        for (name, base) in bases {
            for depth in [0, 1, 2, 1_000] {
                let ty = (0..depth).fold(base, |t, _| Type::optional(t));
                READS.set(0);
                assert_eq!(
                    (ty.without_optionals(), READS.get()),
                    (base, 1),
                    "{name} depth {depth}"
                );
            }
        }
    }

    #[test]
    fn anys_should_skip_optional_layers_in_constant_reads() {
        let any = Type::intern(TypeKind::Any(InterfaceId(3), TypeArgs::NONE));
        let deep = (0..1_000).fold(any, |t, _| Type::optional(t));
        READS.set(0);
        assert_eq!(deep.anys(), [InterfaceId(3)]);
        assert_eq!(READS.get(), 3, "(any I)?^1000");

        let wrapped = Type::array(deep);
        READS.set(0);
        assert_eq!(wrapped.anys(), [InterfaceId(3)]);
        assert_eq!(READS.get(), 4, "[(any I)?^1000]");
    }
}
