//! Interned types. A `Type` stands for a type's structure, as a `Symbol` stands for a name's
//! text, so that copying, comparing and hashing a type costs what an integer does, and a type
//! such as `[i64?]` allocates nothing once interned.

use rustc_hash::FxHashMap;
use std::fmt;
use std::sync::{LazyLock, Mutex, MutexGuard, OnceLock, PoisonError};

use crate::symbol::Symbol;
use crate::tir::{ClassId, EnumId};

/// A type, interned: two types are equal exactly when their structures are. `kind` reads the
/// structure.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Type(u32);

const _: () = assert!(size_of::<Type>() == 4);

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
    /// Holds `I64`, `U8`, `F64`, `Bool`, `Char`, `String`, `Class`, `Enum`, `Param` or `Array`.
    Optional(Type),
}

impl TypeKind {
    /// Whether a type of this kind names a type parameter, from what its children say.
    fn has_param(self) -> bool {
        match self {
            TypeKind::Param(..) => true,
            TypeKind::Array(ty) | TypeKind::Optional(ty) => ty.has_param(),
            TypeKind::Class(_, args) | TypeKind::Enum(_, args) => args.has_param(),
            _ => false,
        }
    }
}

/// A list of type arguments, interned: two lists are equal exactly when their elements are.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TypeArgs(u32);

#[derive(Default)]
struct ArgsInterner {
    ids: FxHashMap<&'static [Type], TypeArgs>,
    /// Each list, with whether an element names a type parameter, at its handle.
    lists: Vec<(&'static [Type], bool)>,
}

/// Every list interned in this process, with the empty list at index 0. Each slice is leaked so
/// that `get` can lend it for the rest of the process, as `Symbol::intern` leaks each name.
static ARGS: LazyLock<Mutex<ArgsInterner>> = LazyLock::new(|| {
    Mutex::new(ArgsInterner {
        ids: FxHashMap::from_iter([(&[][..], TypeArgs(0))]),
        lists: vec![(&[], false)],
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
        args.lists
            .push((list, list.iter().any(|ty| ty.has_param())));
        args.ids.insert(list, interned);
        interned
    }

    pub(crate) fn get(self) -> &'static [Type] {
        args().lists[self.0 as usize].0
    }

    /// The arguments as a program writes them, `<i64, [T]>` with each type named by `name`;
    /// nothing for `NONE`.
    pub(crate) fn written(self, name: impl Fn(Type) -> String) -> String {
        if self == TypeArgs::NONE {
            return String::new();
        }
        let list: Vec<String> = self.get().iter().map(|&ty| name(ty)).collect();
        format!("<{}>", list.join(", "))
    }

    fn has_param(self) -> bool {
        args().lists[self.0 as usize].1
    }

    fn subst(self, with: &[Type]) -> TypeArgs {
        let list: Vec<Type> = self.get().iter().map(|ty| ty.subst(with)).collect();
        TypeArgs::intern(&list)
    }
}

/// As the slice, so a failing test names the types, not a table index.
impl fmt::Debug for TypeArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.get(), f)
    }
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

const _: () = assert!(FIRST * ((1 << SEGMENTS) - 1) > u32::MAX as usize);

/// A type's kind, and whether it names a type parameter.
type Entry = (TypeKind, bool);

/// The kind of each interned handle past `SEEDS`, at its index, with whether it names a type
/// parameter. `intern` sets a slot under the lock before it hands out the handle, and segments
/// never move, so `kind` reads without the lock: `Type::kind` is the hottest read of the checker
/// and codegen.
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
            .set((kind, kind.has_param()))
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
        self.entry().1
    }

    fn entry(self) -> Entry {
        let i = self.0 as usize;
        // A constant is read before anything is interned, so its kind is not in `KINDS`.
        if let Some(&kind) = SEEDS.get(i) {
            return (kind, false);
        }
        let (segment, at) = slot(i);
        *KINDS[segment]
            .get()
            .and_then(|slots| slots[at].get())
            .expect("`intern` should set a handle's slot before it returns the handle")
    }

    /// The type with each `Param(_, i)` replaced by `args[i]`. A type without a parameter comes
    /// back as it is. An optional is rebuilt with `Type::optional`, so it never flattens.
    pub(crate) fn subst(self, args: &[Type]) -> Type {
        if !self.has_param() {
            return self;
        }
        match self.kind() {
            TypeKind::Param(_, i) => args[i as usize],
            TypeKind::Array(element) => Type::array(element.subst(args)),
            TypeKind::Optional(payload) => Type::optional(payload.subst(args)),
            TypeKind::Class(id, list) => Type::intern(TypeKind::Class(id, list.subst(args))),
            TypeKind::Enum(id, list) => Type::intern(TypeKind::Enum(id, list.subst(args))),
            _ => unreachable!("only a kind with a child can name a parameter"),
        }
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
    fn type_args_are_interned_lists() {
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

    #[test]
    fn subst_replaces_each_param_by_index() {
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
}
