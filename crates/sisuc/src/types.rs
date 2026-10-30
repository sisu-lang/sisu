//! Interned types. A `Type` stands for a type's structure, as a `Symbol` stands for a name's
//! text, so that copying, comparing and hashing a type costs what an integer does, and a type
//! such as `[i64?]` allocates nothing once interned.

use rustc_hash::FxHashMap;
use std::fmt;
use std::sync::{LazyLock, Mutex, MutexGuard, OnceLock, PoisonError};

use crate::symbol::Symbol;
use crate::tir::{ClassId, EnumId, InterfaceId};

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
    /// An enum, with the type arguments of a generic one (`TypeArgs::NONE` when it takes none).
    Enum(EnumId, TypeArgs),
    /// A type parameter: its name and its index in its declaration's list.
    Param(Symbol, u32),
    /// `[T]`.
    Array(Type),
    /// Holds `I64`, `U8`, `F64`, `Bool`, `Char`, `String`, `Class`, `Enum`, `Param`, `Assoc` or
    /// `Array`.
    Optional(Type),
    /// An associated type of a base: `S.Item`. It is the type at the index of the associated type
    /// among the interface's, through the interface with its arguments (without `Self`). The base
    /// is a type parameter, another associated type, or (until `conformance::normalize` replaces
    /// it) a class or an enum.
    Assoc(Type, InterfaceId, TypeArgs, u32),
}

/// The index of the hole, the type parameter that stands for a part a context has not fixed yet
/// (see `check::generic`). No declaration has a parameter at this index.
pub(crate) const HOLE_INDEX: u32 = u32::MAX;

/// The index of the rejected hole, the type parameter that stands for a part of a context whose
/// bound failed (see `check::generic`). It is a hole too: `Type::holds_hole` is true for it. No
/// declaration has a parameter at this index.
pub(crate) const REJECTED_INDEX: u32 = u32::MAX - 1;

/// Which holes a type names, anywhere inside it: one flag for each would make `Names` hold too
/// many bools.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Holes {
    #[default]
    None,
    /// `HOLE_INDEX` only.
    Open,
    /// `REJECTED_INDEX` only.
    Rejected,
    /// Both.
    Both,
}

impl Holes {
    fn or(self, other: Holes) -> Holes {
        match (self, other) {
            (Holes::None, holes) | (holes, Holes::None) => holes,
            (a, b) if a == b => a,
            _ => Holes::Both,
        }
    }
}

/// What a type names, anywhere inside it. Each interned type and list holds its own, so a read
/// does not walk the type.
#[derive(Clone, Copy, Debug, Default)]
struct Names {
    /// A type parameter.
    param: bool,
    /// Which holes.
    holes: Holes,
    /// An associated type.
    assoc: bool,
}

impl Names {
    fn or(self, other: Names) -> Names {
        Names {
            param: self.param || other.param,
            holes: self.holes.or(other.holes),
            assoc: self.assoc || other.assoc,
        }
    }
}

impl TypeKind {
    /// What a type of this kind names, from what its children say.
    // `check::cycle::Graph::parts` looks into the same kinds: a kind that holds a type goes in
    // both.
    fn names(self) -> Names {
        #[cfg(test)]
        NAMED.set(NAMED.get() + 1);
        match self {
            TypeKind::Param(_, i) => Names {
                param: true,
                holes: match i {
                    HOLE_INDEX => Holes::Open,
                    REJECTED_INDEX => Holes::Rejected,
                    _ => Holes::None,
                },
                assoc: false,
            },
            TypeKind::Array(ty) | TypeKind::Optional(ty) => ty.names(),
            TypeKind::Assoc(base, _, args, _) => Names {
                assoc: true,
                ..base.names().or(args.names())
            },
            TypeKind::Class(_, args) | TypeKind::Enum(_, args) => args.names(),
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

    /// The list with `Type::subst` applied to each element. A list without a parameter comes
    /// back as it is.
    pub(crate) fn subst(self, with: &[Type]) -> TypeArgs {
        self.subst_in(with, &mut FxHashMap::default())
    }

    /// `subst`, sharing `memo` with the walk that called it.
    fn subst_in(self, with: &[Type], memo: &mut FxHashMap<Type, Type>) -> TypeArgs {
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

/// A class, an enum or an associated type (an interface and the index of the associated type
/// among its), which a type's text names by its head.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Head {
    Class(ClassId),
    Enum(EnumId),
    Assoc(InterfaceId, u32),
}

#[cfg(test)]
thread_local! {
    /// The types `Type::subst` has substituted on this thread: those that name a parameter.
    static SUBSTITUTED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The kinds `TypeKind::names` has looked at on this thread, one for each type interned.
    static NAMED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The entries `Type::entry` has read on this thread: each read of a kind or a flag.
    static READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The most steps `write_steps` has held at once on this thread.
    static HELD: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
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
/// whether the text was cut: past `NAME_LIMIT` bytes, `...` and the closers and the associated
/// type names still owed end it. Each level entered before the cut adds at most one closer or one
/// name, so the text is never longer than the whole text.
pub(crate) fn write_type<'n>(out: &mut String, ty: Type, head: impl Fn(Head) -> &'n str) -> bool {
    write_steps(out, vec![Step::Type(ty)], head)
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
    let mut cut = write_type(out, owner, head);
    if let Some((name, args)) = interface {
        out.push('.');
        out.push_str(name);
        cut |= write_args(out, args.get(), head);
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
    let mut steps = Vec::new();
    push_args(out, &mut steps, args);
    write_steps(out, steps, head)
}

/// Writes `steps` to `out`, the last first. One string grows in place, and a stack stands in for
/// recursion: a type can nest deeper than the call stack allows. Once `out` holds `NAME_LIMIT`
/// bytes, no type or separator follows, so the work is linear in the text, not in the type.
fn write_steps<'n>(
    out: &mut String,
    mut steps: Vec<Step<'_>>,
    head: impl Fn(Head) -> &'n str,
) -> bool {
    while let Some(step) = steps.pop() {
        #[cfg(test)]
        HELD.set(HELD.get().max(steps.len() + 1));
        let ty = match step {
            Step::Close(text) => {
                out.push_str(text);
                continue;
            }
            Step::Rest([]) => continue,
            Step::Type(_) | Step::Rest(_) if out.len() >= NAME_LIMIT => {
                // The cut elides only what has not begun: a type or the rest of a list. A type
                // that has begun gets its end, each closer and each associated type name, so a
                // name that ends in `.Item` or `]` still names what it began to name.
                out.push_str("...");
                for step in steps.iter().rev() {
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
        let args = match ty.kind() {
            TypeKind::Class(id, args) => {
                out.push_str(head(Head::Class(id)));
                args
            }
            TypeKind::Enum(id, args) => {
                out.push_str(head(Head::Enum(id)));
                args
            }
            TypeKind::Array(element) => {
                out.push('[');
                steps.extend([Step::Close("]"), Step::Type(element)]);
                continue;
            }
            TypeKind::Optional(payload) => {
                steps.extend([Step::Close("?"), Step::Type(payload)]);
                continue;
            }
            TypeKind::Assoc(base, interface, _, k) => {
                steps.extend([Step::Assoc(interface, k), Step::Type(base)]);
                continue;
            }
            _ => {
                fmt::Write::write_fmt(out, format_args!("{ty}"))
                    .expect("writing to a `String` should not fail");
                continue;
            }
        };
        push_args(out, &mut steps, args.get());
    }
    false
}

/// Writes the `<` of a nonempty `args` to `out`, and pushes the rest of `<a, b>` onto `steps`
/// for `write_steps`, `a` on top.
fn push_args<'a>(out: &mut String, steps: &mut Vec<Step<'a>>, args: &'a [Type]) {
    let [first, rest @ ..] = args else {
        return;
    };
    out.push('<');
    steps.extend([Step::Close(">"), Step::Rest(rest), Step::Type(*first)]);
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
        self.names().holes != Holes::None
    }

    /// Whether the type names `REJECTED_INDEX` and not `HOLE_INDEX`, anywhere inside it.
    pub(crate) fn holds_rejected_hole_only(self) -> bool {
        self.names().holes == Holes::Rejected
    }

    /// Whether the type holds an associated type, anywhere inside it.
    pub(crate) fn has_assoc(self) -> bool {
        self.names().assoc
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
        self.subst_in(args, &mut FxHashMap::default())
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
            self.subst_walk(args, &mut FxHashMap::default()),
            names.assoc,
        )
    }

    /// `subst`, with `memo` holding each type that the walk has already substituted.
    fn subst_in(self, args: &[Type], memo: &mut FxHashMap<Type, Type>) -> Type {
        if !self.has_param() {
            return self;
        }
        self.subst_walk(args, memo)
    }

    /// `subst_in` for a type that names a parameter.
    fn subst_walk(self, args: &[Type], memo: &mut FxHashMap<Type, Type>) -> Type {
        if let Some(&done) = memo.get(&self) {
            return done;
        }
        #[cfg(test)]
        SUBSTITUTED.set(SUBSTITUTED.get() + 1);
        let done = match self.kind() {
            TypeKind::Param(_, i) => args[i as usize],
            TypeKind::Array(element) => Type::array(element.subst_in(args, memo)),
            TypeKind::Optional(payload) => Type::optional(payload.subst_in(args, memo)),
            TypeKind::Class(id, list) => {
                Type::intern(TypeKind::Class(id, list.subst_in(args, memo)))
            }
            TypeKind::Enum(id, list) => Type::intern(TypeKind::Enum(id, list.subst_in(args, memo))),
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
            | TypeKind::Never => {
                unreachable!("only a kind with a child can name a parameter")
            }
        };
        memo.insert(self, done);
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
                Head::Enum(_) => unreachable!("the type holds no enum"),
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
            doubled(param, 20),
            doubled(Type::I64, 20),
        );
        assert!(NAMED.get() <= 4 * 21, "{} kinds looked at", NAMED.get());
        let reads = |query: fn(Type) -> bool, ty: Type| {
            READS.set(0);
            let answer = query(ty);
            (answer, READS.get())
        };
        for (ty, hole, rejected_only, param) in [
            (holed, true, false, true),
            (rejected, true, true, true),
            (mixed, true, false, true),
            (named, false, false, true),
            (plain, false, false, false),
        ] {
            assert_eq!(reads(Type::holds_hole, ty), (hole, 1));
            assert_eq!(
                reads(Type::holds_rejected_hole_only, ty),
                (rejected_only, 1)
            );
            assert_eq!(reads(Type::has_param, ty), (param, 1));
        }
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
}
