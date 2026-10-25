//! What the generator knows at a point of the program: the names in scope, their types, the
//! sizes their values can reach, and the values it knows.

use std::borrow::Cow;
use std::rc::Rc;

use super::DEPTHS;
use super::generic::{GFunc, Generic};

/// The generic that is the prelude's `Result<T, U>`, which no `Scope::generics` entry holds and
/// no declaration writes.
pub(crate) const RESULT: usize = usize::MAX;

/// How a program writes the generic `generic`: `G3`, or `Result`.
pub(crate) fn generic_name(generic: usize) -> String {
    if generic == RESULT {
        "Result".to_string()
    } else {
        format!("G{generic}")
    }
}

/// An instance of a generic class or enum: `Scope::generics[generic]` applied to `args`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Applied {
    pub(crate) generic: usize,
    pub(crate) args: Vec<Ty>,
}

/// A type the generator writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Ty {
    I64,
    U8,
    F64,
    Bool,
    Char,
    String,
    Unit,
    Array(Box<Ty>),
    Optional(Box<Ty>),
    /// The class `Scope::classes[id]`, each of whose objects counts `size` (decision 5).
    Class {
        id: usize,
        size: u64,
        applied: Option<Rc<Applied>>,
    },
    /// The enum `Scope::enums[id]`, whose first variant's smallest value counts `least`.
    Enum {
        id: usize,
        least: u64,
        applied: Option<Rc<Applied>>,
    },
}

/// The type as Sisu writes it.
impl std::fmt::Display for Ty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Ty::I64 => f.write_str("i64"),
            Ty::U8 => f.write_str("u8"),
            Ty::F64 => f.write_str("f64"),
            Ty::Bool => f.write_str("bool"),
            Ty::Char => f.write_str("char"),
            Ty::String => f.write_str("string"),
            Ty::Unit => f.write_str("unit"),
            Ty::Array(element) => write!(f, "[{element}]"),
            Ty::Optional(payload) => write!(f, "{payload}?"),
            Ty::Class { id, applied, .. } => f.write_str(&written("K", *id, applied.as_deref())),
            Ty::Enum { id, applied, .. } => f.write_str(&written("E", *id, applied.as_deref())),
        }
    }
}

/// `K3` or `E3` for a declared type, `G1<i64, [string]>` for an instance of a generic one.
fn written(prefix: &str, id: usize, applied: Option<&Applied>) -> String {
    let Some(Applied { generic, args }) = applied else {
        return format!("{prefix}{id}");
    };
    let args: Vec<String> = args.iter().map(ToString::to_string).collect();
    format!("{}<{}>", generic_name(*generic), args.join(", "))
}

impl Ty {
    /// The declared class `K<id>`.
    pub(crate) const fn class(id: usize, size: u64) -> Ty {
        Ty::Class {
            id,
            size,
            applied: None,
        }
    }

    /// The declared enum `E<id>`.
    pub(crate) const fn enumeration(id: usize, least: u64) -> Ty {
        Ty::Enum {
            id,
            least,
            applied: None,
        }
    }

    /// The name without type arguments, which a pattern and an inferred construction write:
    /// `G<n>` for an instance of a generic class or enum.
    pub(crate) fn bare(&self) -> String {
        match self {
            Ty::Class {
                applied: Some(a), ..
            }
            | Ty::Enum {
                applied: Some(a), ..
            } => generic_name(a.generic),
            _ => self.to_string(),
        }
    }

    /// The types that have values: every type but `unit`.
    pub(crate) const VALUES: [Ty; 6] = [Ty::I64, Ty::U8, Ty::F64, Ty::Bool, Ty::Char, Ty::String];

    /// Whether a value of it written with no expected type may take another type: an integer
    /// literal is an `i64`, and `None` and `[]` cannot be inferred.
    pub(crate) fn inferred(&self) -> bool {
        matches!(self, Ty::U8 | Ty::Array(_) | Ty::Optional(_))
    }

    /// The size of its smallest value written with no expected type: an array holds an element.
    /// An optional of an optional has no such value, so its size is `u64::MAX`, which no `max`
    /// holds: an `if` with `None` in a branch takes one `?` only.
    pub(crate) fn least(&self) -> u64 {
        match self {
            Ty::Array(element) => element.least().saturating_add(1),
            Ty::Optional(payload) if matches!(**payload, Ty::Optional(_)) => u64::MAX,
            Ty::Optional(payload) => payload.least(),
            Ty::Class { .. } | Ty::Enum { .. } => self.floor(),
            Ty::I64 | Ty::U8 | Ty::F64 | Ty::Bool | Ty::Char | Ty::String | Ty::Unit => 0,
        }
    }

    /// The most a part of this type holds of a value that holds at most `whole`: all of it, but
    /// a class object, which holds its own size.
    pub(crate) fn within(&self, whole: u64) -> u64 {
        match self {
            Ty::Class { size, .. } => *size,
            _ => whole,
        }
    }

    /// Whether a fill of it would share one object among its elements (E0067): a class or
    /// a class optional.
    pub(crate) fn shared(&self) -> bool {
        let class = |ty: &Ty| matches!(ty, Ty::Class { .. });
        class(self) || matches!(self, Ty::Optional(payload) if class(payload))
    }

    /// The size of its smallest value written with an expected type: `""`, `[]` and `None`
    /// hold none, but a class object holds its fields and an enum value its first variant.
    pub(crate) fn floor(&self) -> u64 {
        match self {
            Ty::Class { size, .. } => *size,
            Ty::Enum { least, .. } => *least,
            Ty::I64
            | Ty::U8
            | Ty::F64
            | Ty::Bool
            | Ty::Char
            | Ty::String
            | Ty::Unit
            | Ty::Array(_)
            | Ty::Optional(_) => 0,
        }
    }

    /// Whether its values have a size: a `string`, an array, a class object, an enum value, or
    /// an optional of one.
    pub(crate) fn sized(&self) -> bool {
        match self {
            Ty::String | Ty::Array(_) | Ty::Class { .. } | Ty::Enum { .. } => true,
            Ty::Optional(payload) => payload.sized(),
            Ty::I64 | Ty::U8 | Ty::F64 | Ty::Bool | Ty::Char | Ty::Unit => false,
        }
    }

    /// The most chars `print` or an interpolation writes for a value of this type that holds at
    /// most `size` chars, with `precision` decimals for an `f64`: `-1.2345678901234568e-300` is
    /// the longest `f64` without one, and `1e308` with one has 309 digits before the point.
    pub(crate) fn printed(&self, size: u64, precision: Option<u64>) -> u64 {
        match self {
            Ty::I64 => 20,
            Ty::U8 => 3,
            Ty::F64 => precision.map_or(24, |p| 311 + p),
            Ty::Bool => 5,
            Ty::Char => 1,
            Ty::String => size,
            Ty::Unit | Ty::Array(_) | Ty::Optional(_) | Ty::Class { .. } | Ty::Enum { .. } => {
                unreachable!("only a value of `Ty::VALUES` is printed")
            }
        }
    }
}

/// A local or a parameter.
pub(crate) struct Var {
    pub(crate) name: String,
    pub(crate) ty: Ty,
    /// A `let mut` that no loop counts with: the body may assign to it.
    pub(crate) assignable: bool,
    /// A `let mut`, whose value a statement between a read of it and a `let` that read moved
    /// to may change (decision 2).
    pub(crate) mutable: bool,
    /// For a `string`: the most chars its first value or a plain assignment may hold. For a
    /// mutable array: the most size each element may take that way.
    pub(crate) base: u64,
    /// The most size its value can ever reach (decision 5): for a `string`, its base and its
    /// room at declaration.
    pub(crate) size: u64,
    /// For a `string`, or a mutable array of them: the chars that `+=` may still add, summed
    /// over every run of each one.
    pub(crate) room: u64,
    /// How many loops are around its declaration.
    pub(crate) loops: usize,
    /// For an array: how many elements it holds at least, which an index stays under.
    pub(crate) len: u64,
    /// For an array: the most size an element can reach.
    pub(crate) elem: u64,
}

impl Var {
    /// A local or a parameter, with `base` and `room` as a `string` has them, 0 otherwise.
    pub(crate) fn new(
        name: String,
        ty: Ty,
        assignable: bool,
        base: u64,
        room: u64,
        loops: usize,
    ) -> Var {
        Var {
            name,
            ty,
            assignable,
            mutable: assignable,
            base,
            size: base + room,
            room,
            loops,
            len: 0,
            elem: base + room,
        }
    }

    /// The most chars one run of a `+=` may add, when it runs `times` times.
    pub(crate) fn append_max(&self, times: u64) -> u64 {
        self.room / times.max(1)
    }

    /// Takes the room of a `+=` that adds at most `added` chars, `times` times.
    pub(crate) fn append(&mut self, added: u64, times: u64) {
        self.room -= added * times;
    }
}

/// A top-level `const`.
pub(crate) struct Const {
    pub(crate) name: String,
    pub(crate) ty: Ty,
    /// The value of an `i64` or a `u8`, or the code point of a `char`.
    pub(crate) value: i64,
    /// For a `string`: its length.
    pub(crate) size: u64,
}

/// A generated function or method: `depth: i64` first, then its parameters.
pub(crate) struct Func {
    pub(crate) name: String,
    /// For a method, the class whose method it is.
    pub(crate) owner: Option<Ty>,
    /// Each parameter's type, and for a `string` the most chars an argument may hold.
    pub(crate) params: Vec<(Ty, u64)>,
    pub(crate) ret: Ty,
    /// For a `string` result: the most chars it holds.
    pub(crate) ret_size: u64,
    /// What a call costs at each `depth` it can pass (decision 5); set once the body is written.
    pub(crate) cost: [u64; DEPTHS],
}

/// A field of a class.
pub(crate) struct Field {
    pub(crate) name: String,
    pub(crate) ty: Ty,
    pub(crate) mutable: bool,
    /// The most size its value may take (decision 5).
    pub(crate) cap: u64,
}

/// A generated class: `K<id>`. Its fields' types are declared before it, so no class holds
/// itself.
pub(crate) struct Class {
    pub(crate) fields: Vec<Field>,
    /// The size of each of its objects: one for the object and each field's cap.
    pub(crate) size: u64,
    /// For an instance of a generic class, which one and with what.
    pub(crate) applied: Option<Rc<Applied>>,
    /// For an instance: whether its fields name every type parameter of the generic class, so a
    /// construction may leave out the type arguments.
    pub(crate) inferable: bool,
}

/// A variant of an enum.
pub(crate) struct Variant {
    pub(crate) name: String,
    pub(crate) fields: Vec<Ty>,
    /// Each field's label, or none at all.
    pub(crate) labels: Vec<String>,
    /// Whether a value of it holds a value of the enum, or of one that holds the enum again. A
    /// value built finite ends in a variant that is not. The first variant is not.
    pub(crate) recursive: bool,
    /// For a variant of an instance: whether its fields name every type parameter of the
    /// generic enum, so a construction may leave the type arguments out.
    pub(crate) inferable: bool,
}

/// A generated enum: `E<id>`. Its first variant holds only types declared before it; a later
/// one may hold the enum itself.
pub(crate) struct Enum {
    pub(crate) variants: Vec<Variant>,
    /// The size of its first variant's smallest value.
    pub(crate) least: u64,
    /// Whether a value of it may hold a class object.
    pub(crate) has_class: bool,
    /// For an instance of a generic enum, which one and with what.
    pub(crate) applied: Option<Rc<Applied>>,
}

#[derive(Default)]
pub(crate) struct Scope {
    pub(crate) consts: Vec<Const>,
    pub(crate) funcs: Vec<Func>,
    pub(crate) classes: Vec<Class>,
    pub(crate) enums: Vec<Enum>,
    /// The types of the classes and enums declared so far.
    pub(crate) types: Vec<Ty>,
    /// The instances of generic classes and enums that the program names in its types.
    pub(crate) generic_types: Vec<Ty>,
    /// The generic classes and enums, each written once.
    pub(crate) generics: Vec<Generic>,
    /// The generic functions.
    pub(crate) gfuncs: Vec<GFunc>,
    /// The instances made so far: a generic, its type arguments, and the type that results.
    pub(crate) instances: Vec<(Rc<Applied>, Ty)>,
    /// The locals in scope, innermost last.
    pub(crate) vars: Vec<Var>,
    /// Where each open block's locals start in `vars`.
    frames: Vec<usize>,
    /// The function being written, an index into `funcs`; `None` in `main` and in constants.
    pub(crate) func: Option<usize>,
    /// How many branches, loops and right operands of `&&`, `||` and `??` are around the point
    /// being written: code there may not run (decision 2).
    pub(crate) conditional: usize,
    /// The last number a name took.
    names: usize,
}

impl Scope {
    /// The generic `generic`, which may be `RESULT`.
    pub(crate) fn generic(&self, generic: usize) -> Cow<'_, Generic> {
        if generic == RESULT {
            Cow::Owned(Generic::result())
        } else {
            Cow::Borrowed(&self.generics[generic])
        }
    }

    /// A name that no other name of the program has: `prefix` and a number.
    pub(crate) fn fresh(&mut self, prefix: &str) -> String {
        self.names += 1;
        format!("{prefix}{}", self.names)
    }

    /// Whether a value of `ty` may hold a class object, which no field or element assignment
    /// may store (decision 4).
    pub(crate) fn contains_class(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Class { .. } => true,
            Ty::Enum { id, .. } => self.enums[*id].has_class,
            Ty::Array(inner) | Ty::Optional(inner) => self.contains_class(inner),
            Ty::I64 | Ty::U8 | Ty::F64 | Ty::Bool | Ty::Char | Ty::String | Ty::Unit => false,
        }
    }

    /// The type of the class `Scope::classes[id]`.
    pub(crate) fn class_ty(&self, id: usize) -> Ty {
        let class = &self.classes[id];
        Ty::Class {
            id,
            size: class.size,
            applied: class.applied.clone(),
        }
    }

    /// The type of the enum `Scope::enums[id]`.
    pub(crate) fn enum_ty(&self, id: usize) -> Ty {
        let e = &self.enums[id];
        Ty::Enum {
            id,
            least: e.least,
            applied: e.applied.clone(),
        }
    }

    pub(crate) fn open(&mut self) {
        self.frames.push(self.vars.len());
    }

    pub(crate) fn close(&mut self) {
        let start = self
            .frames
            .pop()
            .expect("each closed block should have been opened");
        self.vars.truncate(start);
    }
}
