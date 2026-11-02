//! What the generator knows at a point of the program: the names in scope, their types, the
//! sizes their values can reach, and the values it knows.

use std::cell::OnceCell;
use std::rc::Rc;

use super::DEPTHS;
use super::generic::{GFunc, Generic};
use crate::check::built_in_type;

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
    pub(crate) const VALUES: &[Ty] = &[Ty::I64, Ty::U8, Ty::F64, Ty::Bool, Ty::Char, Ty::String];

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
    /// most `size`, with `precision` decimals for an `f64`: `-1.2345678901234568e-300` is the
    /// longest `f64` without one, and `1e308` with one has 309 digits before the point. `nodes` is
    /// `Scope::nodes`, which an enum value needs. An array, an optional, a class and an enum take
    /// their bound from `Text`.
    pub(crate) fn printed(&self, size: u64, precision: Option<u64>, nodes: Nodes) -> u64 {
        match self {
            Ty::I64 => 20,
            Ty::U8 => 3,
            Ty::F64 => precision.map_or(24, |p| 311 + p),
            Ty::Bool => 5,
            Ty::Char => 1,
            Ty::String => size,
            Ty::Array(_) | Ty::Optional(_) | Ty::Class { .. } | Ty::Enum { .. } => {
                Text::of(self, nodes).flat(size)
            }
            Ty::Unit => unreachable!("`unit` has no text"),
        }
    }

    /// The most chars that `{x:#}` writes for a value of this type that holds at most `size`: the
    /// text of `printed`, and the line breaks and the indent of a layout that breaks every group.
    /// `#` changes nothing for a scalar, a `string` or a class.
    pub(crate) fn printed_pretty(&self, size: u64, nodes: Nodes) -> u64 {
        match self {
            Ty::Array(_) | Ty::Optional(_) | Ty::Enum { .. } => Text::of(self, nodes).pretty(size),
            _ => self.printed(size, None, nodes),
        }
    }
}

/// The columns that a broken group indents its children by: `INDENT` in
/// `crates/runtime/src/show.rs`.
const INDENT: u64 = 4;

/// The chars of `None`.
const NONE: u64 = 4;

/// The chars of `text`.
fn chars(text: &str) -> u64 {
    u64::try_from(text.chars().count()).expect("a name should have fewer than 2^64 chars")
}

/// The most that one unit of the size of an enum value writes, over the variants of the enums of
/// the program (`Scope::note`).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Nodes {
    /// The chars it writes.
    pub(crate) text: u64,
    /// For `#`: the lines it writes, which are the groups and their children.
    pub(crate) lines: u64,
}

/// The most text that a value writes, inside a group or at the top, as the line
/// `per_unit * size + fixed`, where `size` is the value's size (decision 5).
///
/// A `string` inside a group is quoted, and each char takes the longest escape: `\u{2028}`, which
/// is 8 chars. A control char takes `\u{9F}` at most. A `char` is quoted too: `'\u{2028}'` is
/// 10 chars. A class writes its name. For an instance of a generic class that is the type with
/// its type arguments, as `Ty`'s `Display` writes it. An optional writes its payload, or `None`.
///
/// An array writes `[`, its elements with `, ` between them, and `]`. Each element adds 1 to the
/// array's size, and its own size. An element of `e` units writes at most `per_unit * e + fixed`
/// chars, so the text of `n` elements is at most `n * (fixed + 2) + per_unit * (size - n)`: under
/// `max(per_unit, fixed + 2) * size` as `n` is at most `size`. An empty array writes the 2 chars of
/// `[]`, which is the `fixed` of an array.
///
/// An enum value writes the name of its variant, and in parentheses its fields, with a label and
/// `: ` for each field that has one. A node adds 1 to the size, and `Nodes::text` is the most
/// chars that each unit of the size of an enum value writes.
#[derive(Clone, Copy, Debug)]
struct Text {
    /// The chars that each unit of size adds.
    per_unit: u64,
    /// The chars that no size changes.
    fixed: u64,
    /// For `#`: the lines that each unit of size adds, and the lines that no size changes. A line
    /// is a group or a child of a group, so a group of `n` children is `n + 1` lines. An array has
    /// 1 line that no size changes, and at most `max(lines, line_fixed + 1)` lines for each unit
    /// of size, because `n` is at most the size, as for the chars.
    lines: u64,
    line_fixed: u64,
    /// For `#`: how many groups nest at most, or `None` when it is as many as the size: an enum
    /// can nest as deep as that.
    depth: Option<u64>,
}

impl Text {
    const fn scalar(fixed: u64) -> Text {
        Text {
            per_unit: 0,
            fixed,
            lines: 0,
            line_fixed: 0,
            depth: Some(0),
        }
    }

    /// The text of a value of `ty`.
    fn of(ty: &Ty, nodes: Nodes) -> Text {
        match ty {
            Ty::I64 => Text::scalar(20),
            Ty::U8 => Text::scalar(3),
            Ty::F64 => Text::scalar(24),
            Ty::Bool => Text::scalar(5),
            Ty::Char => Text::scalar(10),
            Ty::String => Text {
                per_unit: 8,
                fixed: 2,
                ..Text::scalar(0)
            },
            Ty::Optional(payload) => {
                let text = Text::of(payload, nodes);
                Text {
                    fixed: text.fixed.max(NONE),
                    ..text
                }
            }
            Ty::Array(element) => {
                let text = Text::of(element, nodes);
                Text {
                    per_unit: text.per_unit.max(text.fixed + 2),
                    fixed: 2,
                    lines: text.lines.max(text.line_fixed + 1),
                    line_fixed: 1,
                    depth: text.depth.map(|depth| depth + 1),
                }
            }
            Ty::Class { .. } => Text::scalar(chars(&ty.to_string())),
            Ty::Enum { .. } => Text {
                per_unit: nodes.text,
                lines: nodes.lines,
                depth: None,
                ..Text::scalar(0)
            },
            Ty::Unit => unreachable!("`unit` has no text"),
        }
    }

    /// The most chars of a value of this size on one line.
    fn flat(self, size: u64) -> u64 {
        self.per_unit * size + self.fixed
    }

    /// The most chars of a value of this size when every group breaks. A group at depth `t` has
    /// the indent `INDENT * (t - 1)`. Each of its `n` children has a line break and `INDENT * t`
    /// spaces, and its closing bracket has a line break and `INDENT * (t - 1)` spaces. A `,` and
    /// a line break replace each `, ` between children. So the group adds `n * INDENT * t +
    /// INDENT * (t - 1) + 2` chars at most, which is under `INDENT * t * (n + 1)`. The groups hold
    /// `lines * size + line_fixed` lines between them, at depth `depth` at most. No value nests
    /// deeper than its size, because each group on a path holds at least one unit, and an empty
    /// group holds no line.
    fn pretty(self, size: u64) -> u64 {
        let depth = self.depth.map_or(size, |depth| depth.min(size));
        self.flat(size) + INDENT * depth * (self.lines * size + self.line_fixed)
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
    /// For a mutable array, or optional array: the size that `push` may still add, summed over
    /// every run of each one. It is part of `size`, and of the size an element can reach.
    pub(crate) grow: u64,
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
            grow: 0,
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

    /// Takes the growth of a `push` that adds at most `added` size, `times` times.
    pub(crate) fn push(&mut self, added: u64, times: u64) {
        self.grow -= added * times;
    }
}

/// What a generic alias `type Al<n><T> = ...` makes of its parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Wrap {
    /// `[T]`.
    Array,
    /// `T?`.
    Optional,
    /// `[[T]]`.
    Nested,
}

impl Wrap {
    /// The body of the alias, over `T`.
    pub(crate) fn body(self) -> &'static str {
        match self {
            Wrap::Array => "[T]",
            Wrap::Optional => "T?",
            Wrap::Nested => "[[T]]",
        }
    }

    /// The type argument `T` of `Al<T>` that makes `ty`, if any does.
    pub(crate) fn argument(self, ty: &Ty) -> Option<&Ty> {
        match (self, ty) {
            (Wrap::Array, Ty::Array(element)) | (Wrap::Optional, Ty::Optional(element)) => {
                Some(element)
            }
            (Wrap::Nested, Ty::Array(inner)) => match &**inner {
                Ty::Array(element) => Some(element),
                _ => None,
            },
            _ => None,
        }
    }
}

/// A top-level type alias `Al<n>`: another name for a type, or, with a parameter, for a type over
/// it.
pub(crate) struct Alias {
    pub(crate) name: String,
    /// The type it names, if it takes no parameter.
    pub(crate) plain: Option<Ty>,
    /// What it makes of its parameter, if it takes one.
    pub(crate) wrap: Option<Wrap>,
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

/// Every this many `let` locals, one is named `echo`.
const ECHO_EVERY: usize = 5;

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
    /// The type aliases declared so far, which a type may be written as.
    pub(crate) aliases: Vec<Alias>,
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
    /// How many `let` locals `local` has named.
    lets: usize,
    /// `Generic::result()`, built on first use.
    result: OnceCell<Generic>,
    /// What one unit of the size of an enum value writes, over every variant noted so far
    /// (`Scope::note`).
    pub(crate) nodes: Nodes,
}

impl Scope {
    /// The generic `generic`, which may be `RESULT`.
    pub(crate) fn generic(&self, generic: usize) -> &Generic {
        if generic == RESULT {
            self.result.get_or_init(Generic::result)
        } else {
            &self.generics[generic]
        }
    }

    /// A name that no other `fresh` call makes, and that names no built-in type: `prefix` and a
    /// number. See [`fresh_name`].
    pub(crate) fn fresh(&mut self, prefix: &str) -> String {
        fresh_name(&mut self.names, prefix)
    }

    /// The name of a `let` local. Every `ECHO_EVERY`th local is `echo` when no `echo` is in
    /// scope. The word was reserved before `dbg` and is an ordinary name now. The name still takes
    /// its number from `fresh`, so the names after it do not move. A count decides, not a draw, so
    /// no seed's data is read differently.
    pub(crate) fn local(&mut self) -> String {
        let name = self.fresh("v");
        self.lets += 1;
        if self.lets.is_multiple_of(ECHO_EVERY) && self.vars.iter().all(|var| var.name != "echo") {
            return "echo".to_string();
        }
        name
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

    /// Takes `variant` of an enum into `nodes`. A node of an enum value counts 1, and so does
    /// each unit of its fields' sizes: a value of size `n` is one node and fields of `n - 1`
    /// units. A field of `e` units writes at most `per_unit * e + fixed` chars, and `lines * e +
    /// line_fixed` lines. So the variant writes at most `written + per_unit * (n - 1)` chars.
    /// `written` is its name, the parentheses, `, ` between fields, the label and `: ` of each
    /// field that has one, and the `fixed` of each field. That is at most `text * n` when `text`
    /// is at least the `written` and the `per_unit` of every field.
    ///
    /// A field that is an enum is at most `text` for each unit, so `text` covers it, and its
    /// `per_unit` is 0 here. The lines are the same, with the node's group and one child for each
    /// field. A field has at most 2 lines for each unit, and a node with such a field has 3 at
    /// least, so the lines for each unit need no term for the fields. The `fixed` of a field never
    /// depends on `nodes`.
    pub(crate) fn note(&mut self, variant: &Variant) {
        let fields = u64::try_from(variant.fields.len()).expect("a variant should hold few fields");
        let mut text =
            chars(&variant.name) + variant.labels.iter().map(|l| chars(l) + 2).sum::<u64>();
        // The parentheses and `, ` between the fields, and for `#` the group and its children.
        let mut lines = 0;
        if fields > 0 {
            text += 2 * fields;
            lines = 1 + fields;
        }
        let mut per_unit = 0;
        for field in &variant.fields {
            let field = Text::of(field, Nodes::default());
            text += field.fixed;
            lines += field.line_fixed;
            per_unit = per_unit.max(field.per_unit);
        }
        self.nodes.text = self.nodes.text.max(text).max(per_unit);
        self.nodes.lines = self.nodes.lines.max(lines);
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

/// `prefix` and the next number of `counter`. A number that makes the name of a built-in type,
/// such as `f64`, is skipped. `Scope::fresh` and `Writer::fresh` both make their names here.
pub(super) fn fresh_name(counter: &mut usize, prefix: &str) -> String {
    loop {
        *counter += 1;
        let name = format!("{prefix}{counter}");
        if built_in_type(&name).is_none() {
            return name;
        }
    }
}

#[cfg(test)]
mod tests {
    use rustc_hash::FxHashSet;

    use super::{Scope, Ty, Variant, fresh_name};
    use crate::check::built_in_type;

    #[test]
    fn note_should_take_the_most_text_and_lines_of_any_variant() {
        let variant = |name: &str, fields: Vec<Ty>, labels: &[&str]| Variant {
            name: name.to_string(),
            fields,
            labels: labels.iter().map(ToString::to_string).collect(),
            recursive: false,
            inferable: false,
        };
        let mut scope = Scope::default();
        // `Z` writes 1 char and no group.
        scope.note(&variant("Z", Vec::new(), &[]));
        assert_eq!((scope.nodes.text, scope.nodes.lines), (1, 0));
        // `Wide(first: i64, second: string, third: i64)` writes 4, 2 for each of 3 fields, 22 for
        // the labels and their `: `, and 42 for the fields. It has a group and 3 children.
        let labels = ["first", "second", "third"];
        let fields = vec![Ty::I64, Ty::String, Ty::I64];
        scope.note(&variant("Wide", fields, &labels));
        assert_eq!((scope.nodes.text, scope.nodes.lines), (74, 4));
        // `S([i64])` writes 22 chars for each unit of the array, which is more than its 9, and its
        // group, its child and the array's group are 3 lines: under the last variant's 4.
        scope.note(&variant("S", vec![Ty::Array(Box::new(Ty::I64))], &[]));
        assert_eq!((scope.nodes.text, scope.nodes.lines), (74, 4));
        let mut other = Scope::default();
        other.note(&variant("S", vec![Ty::Array(Box::new(Ty::I64))], &[]));
        assert_eq!((other.nodes.text, other.nodes.lines), (22, 3));
    }

    #[test]
    fn fresh_name_should_be_unique_and_name_no_built_in_type() {
        // `f`, `i` and `u` reach `f64`, `i64` and `u8`. The largest number any of them needs is
        // 64, so 640 names per prefix is 10x the smallest size that shows the skip.
        for prefix in ["f", "i", "u"] {
            let mut counter = 0;
            let names: Vec<String> = (0..640).map(|_| fresh_name(&mut counter, prefix)).collect();
            for name in &names {
                assert!(built_in_type(name).is_none(), "{name} is a built-in type");
            }
            let unique: FxHashSet<&String> = names.iter().collect();
            assert_eq!(unique.len(), names.len(), "prefix {prefix}");
        }
    }
}
