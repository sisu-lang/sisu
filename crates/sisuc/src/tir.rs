//! The typed tree: the checker's output, from which mir is lowered. Its S-expression printer backs
//! `--emit tir` and the checker tests.

use std::fmt;

use rustc_hash::FxHashMap;

use crate::ast::angled;
use crate::graph::components;
use crate::source::Span;

pub(crate) use crate::ast::{Align, CompareOp, UnaryOp};
use crate::types::{Head, write_args, write_method, write_type};
pub(crate) use crate::types::{Type, TypeArgs, TypeKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct LocalId(pub(crate) usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct FuncId(pub(crate) usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ClassId(pub(crate) usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct EnumId(pub(crate) usize);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct InterfaceId(pub(crate) usize);

/// The prelude's four language interfaces, which the compiler maps operators, `print` and
/// interpolation to by name. Only the checker sets them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct LangInterfaces {
    pub(crate) display: InterfaceId,
    pub(crate) eq: InterfaceId,
    pub(crate) ord: InterfaceId,
    pub(crate) any_object: InterfaceId,
}

/// `==` and `!=` are `ExprKind::Equal`, comparisons are `ExprKind::Compare`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BinaryOp {
    Add,
    /// `+` on `i64`s that the checker emits only where it cannot overflow: a loop counter's
    /// step. No overflow check; codegen emits `add nsw`.
    AddUnchecked,
    Sub,
    Mul,
    Div,
    Rem,
    And,
    Or,
}

/// A method of `f64` or `i64`. `Min`, `Max` and `Clamp` are also the methods of `i64`: the
/// operand type picks the lowering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MathOp {
    Abs,
    Sqrt,
    Floor,
    Ceil,
    Trunc,
    Round,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Exp,
    Ln,
    Log10,
    Log2,
    Pow,
    Hypot,
    Min,
    Max,
    Clamp,
    IsNan,
    IsFinite,
    IsInfinite,
}

impl MathOp {
    /// The method's name, which the printer also uses.
    pub(crate) fn name(self) -> &'static str {
        match self {
            MathOp::Abs => "abs",
            MathOp::Sqrt => "sqrt",
            MathOp::Floor => "floor",
            MathOp::Ceil => "ceil",
            MathOp::Trunc => "trunc",
            MathOp::Round => "round",
            MathOp::Sin => "sin",
            MathOp::Cos => "cos",
            MathOp::Tan => "tan",
            MathOp::Asin => "asin",
            MathOp::Acos => "acos",
            MathOp::Atan => "atan",
            MathOp::Atan2 => "atan2",
            MathOp::Exp => "exp",
            MathOp::Ln => "ln",
            MathOp::Log10 => "log10",
            MathOp::Log2 => "log2",
            MathOp::Pow => "pow",
            MathOp::Hypot => "hypot",
            MathOp::Min => "min",
            MathOp::Max => "max",
            MathOp::Clamp => "clamp",
            MathOp::IsNan => "is_nan",
            MathOp::IsFinite => "is_finite",
            MathOp::IsInfinite => "is_infinite",
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct Program {
    pub(crate) classes: Vec<Class>,
    pub(crate) enums: Vec<Enum>,
    pub(crate) functions: Vec<Function>,
    /// Whether the checker recorded a type argument: a written one, or one a call inferred, or
    /// whether the user's file reaches a function, a class or an enum of the prelude: a call, a
    /// construction, a variant path or a type name. A program with none, and with no generic item
    /// of the user's, has nothing for `mono` to instantiate. Only the checker sets it.
    pub(crate) instantiates: bool,
    /// Where the prelude's items start in each list; the user's come first. Only the checker
    /// sets it.
    pub(crate) prelude: ItemCounts,
    /// Indexed by `InterfaceId`. Only the checker sets it.
    pub(crate) interfaces: Vec<Interface>,
    /// Every conformance of a class or an enum to an interface. Only the checker sets it.
    pub(crate) conformances: Conformances,
    /// Where each choice that writes an associated type or a type parameter is written: `mono`
    /// reports a walk that it cannot end there. Only the checker sets it.
    pub(crate) choice_ats: FxHashMap<ChoicePlace, Span>,
    /// Where the language interfaces are among `interfaces`. Only the checker sets it.
    pub(crate) lang: LangInterfaces,
}

/// An interface, as the typed tree names it.
#[derive(Clone, Debug)]
pub(crate) struct Interface {
    pub(crate) name: String,
    /// The names of its requirements, in the order of `InterfaceCall::method`.
    pub(crate) methods: Vec<String>,
    /// The names of its associated types, in the order of `TypeKind::Assoc`'s index.
    pub(crate) assoc: Vec<String>,
}

/// The place of a choice of an associated type: the interface, the index of the conformance among
/// those of the interface, and the index of the associated type among the interface's.
pub(crate) type ChoicePlace = (InterfaceId, usize, usize);

/// The conformance of the class or enum `target` to an interface with the arguments `args`
/// (without `Self`). `type_params` is how many type parameters of an `extend` block the target
/// binds: they are the `Param`s of `target` and `args`. `methods` hold, in the order of the
/// interface's requirements, the method that meets each; the list is complete only in a program
/// that checks without an error. `assoc` holds, in the order of the interface's associated types,
/// the type that the conformance chooses for each, over the same type parameters as `target`;
/// `None` where it chooses none, which is an error. `bounds` holds the bounds of each of the
/// block's type parameters, in order; it is empty when no parameter has one.
#[derive(Clone, Debug)]
pub(crate) struct Conformance {
    pub(crate) type_params: usize,
    pub(crate) target: Type,
    pub(crate) args: TypeArgs,
    pub(crate) methods: Vec<FuncId>,
    pub(crate) assoc: Vec<Option<Type>>,
    pub(crate) bounds: Vec<Vec<ParamBound>>,
}

/// A bound of a type parameter of a conformance block: the interface and its arguments (without
/// `Self`), over the same type parameters as the block's target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ParamBound {
    pub(crate) interface: InterfaceId,
    pub(crate) args: TypeArgs,
}

/// The conformances of a program, by interface. A target without parameters is indexed by its
/// exact type and the interface's arguments, and a target with parameters by its class or enum,
/// so that a lookup reads the exact entry and then only the generic targets of the receiver's
/// class or enum. A target
/// without parameters is indexed by its class or enum too, for a new target with parameters to
/// meet.
#[derive(Clone, Debug, Default)]
pub(crate) struct Conformances {
    list: Vec<Vec<Conformance>>,
    exact: FxHashMap<(InterfaceId, Type, TypeArgs), Vec<usize>>,
    exact_heads: FxHashMap<(InterfaceId, Head), Vec<usize>>,
    generic: FxHashMap<(InterfaceId, Head), Vec<usize>>,
}

impl Conformances {
    /// Adds `conformance` to those of `interface`, and gives its index among them. Its target is
    /// a class or an enum type.
    pub(crate) fn add(&mut self, interface: InterfaceId, conformance: Conformance) -> usize {
        if self.list.len() <= interface.0 {
            self.list.resize_with(interface.0 + 1, Vec::new);
        }
        let list = &mut self.list[interface.0];
        let index = list.len();
        let head = head_of(conformance.target)
            .expect("the target of a conformance should be a class or an enum type");
        if conformance.type_params == 0 {
            self.exact
                .entry((interface, conformance.target, conformance.args))
                .or_default()
                .push(index);
            self.exact_heads
                .entry((interface, head))
                .or_default()
                .push(index);
        } else {
            self.generic
                .entry((interface, head))
                .or_default()
                .push(index);
        }
        list.push(conformance);
        index
    }

    /// The conformance `index` of `interface`.
    pub(crate) fn get(&self, interface: InterfaceId, index: usize) -> &Conformance {
        &self.list[interface.0][index]
    }

    /// The conformance `index` of `interface`, to fill its methods.
    pub(crate) fn get_mut(&mut self, interface: InterfaceId, index: usize) -> &mut Conformance {
        &mut self.list[interface.0][index]
    }

    /// Every conformance, with its interface.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (InterfaceId, &Conformance)> {
        (self.list.iter().enumerate())
            .flat_map(|(i, list)| list.iter().map(move |c| (InterfaceId(i), c)))
    }

    /// The conformances of `interface` with the arguments `args` whose target is `ty`, which has
    /// no parameters.
    pub(crate) fn exact(&self, interface: InterfaceId, ty: Type, args: TypeArgs) -> &[usize] {
        (self.exact.get(&(interface, ty, args))).map_or(&[], Vec::as_slice)
    }

    /// The conformances of `interface` whose target has no parameters and names `head`.
    pub(crate) fn exact_of(&self, interface: InterfaceId, head: Head) -> &[usize] {
        (self.exact_heads.get(&(interface, head))).map_or(&[], Vec::as_slice)
    }

    /// The conformances of `interface` whose target has parameters and names `head`.
    pub(crate) fn generic(&self, interface: InterfaceId, head: Head) -> &[usize] {
        (self.generic.get(&(interface, head))).map_or(&[], Vec::as_slice)
    }
}

/// The class or enum that `ty` is, if it is one.
pub(crate) fn head_of(ty: Type) -> Option<Head> {
    match ty.kind() {
        TypeKind::Class(id, _) => Some(Head::Class(id)),
        TypeKind::Enum(id, _) => Some(Head::Enum(id)),
        _ => None,
    }
}

/// How many classes, enums and functions (methods too) come first in their lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ItemCounts {
    pub(crate) classes: usize,
    pub(crate) enums: usize,
    pub(crate) funcs: usize,
}

#[derive(Debug)]
pub(crate) struct Class {
    pub(crate) name: String,
    /// The names of its type parameters, in order; empty when it is not generic.
    pub(crate) type_params: Vec<String>,
    pub(crate) fields: Vec<Field>,
}

/// An enum: a tag and its variant's payload, held inline.
#[derive(Debug)]
pub(crate) struct Enum {
    pub(crate) name: String,
    /// Where the declaration names the enum. An instance of a generic enum has the span of its
    /// declaration.
    pub(crate) span: Span,
    /// Whether it is an instance of a generic enum, which `mono` makes.
    pub(crate) instance: bool,
    /// The names of its type parameters, in order; empty when it is not generic.
    pub(crate) type_params: Vec<String>,
    pub(crate) variants: Vec<Variant>,
    /// Whether a value has counted parts: its component has a box, or a field is of a counted
    /// type.
    pub(crate) counted: bool,
    /// The enum's strongly connected component of the holds-inline graph, when that component
    /// has a boxed field.
    pub(crate) cycle: Option<usize>,
}

#[derive(Clone, Debug)]
pub(crate) struct Variant {
    pub(crate) name: String,
    pub(crate) fields: Vec<Payload>,
}

/// A payload field. A `boxed` one is stored behind a counted pointer, since its enum, or the
/// enum of its optional, holds the declaring enum inline (`layout_enums`).
#[derive(Clone, Debug)]
pub(crate) struct Payload {
    pub(crate) ty: Type,
    /// The label the declaration gives the field (`w` in `Rect(w: i64, h: i64)`), which a
    /// printed value writes before the field.
    pub(crate) label: Option<String>,
    pub(crate) boxed: bool,
    pub(crate) rank: Rank,
}

/// Which pass of `layout_enums` may box a payload field, from its declared type; kept through
/// `mono`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rank {
    /// It names no type parameter: `Two<Expr>` in `Expr`.
    Plain,
    /// It names a type parameter, and its own enum applied to its own parameters where
    /// `check::enums::inline_enums` looks (`check::enums::names_itself`): `Tree<T>` in `Tree`,
    /// `Wrap2<G<T>>` in `G`.
    Own,
    /// It names a type parameter, and otherwise, where `inline_enums` looks and with an argument
    /// that carries a parameter, an enum declaration on a cycle of such names with its own:
    /// `Wrap3<C<T>>` in `A` when `C<T>` holds `A<T>`.
    Open,
    /// It names a type parameter, and otherwise an enum declaration on a cycle of such names
    /// with its own, where `inline_enums` looks, only with arguments that carry none:
    /// `P2<T, Wrap2<i64>>` in `Wrap2`.
    Late,
    /// Any other field that holds an associated type: `T.Item`. Its choice may name the enum
    /// that holds it (`Item = E<W<T>>`), a cycle that no declared name shows.
    Assoc,
    /// Any other field that names a type parameter, never boxed: `Two<T>` in `Wrap2`, or `T`.
    Never,
}

#[derive(Debug)]
pub(crate) struct Field {
    pub(crate) name: String,
    pub(crate) ty: Type,
    pub(crate) mutable: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Function {
    pub(crate) name: String,
    /// The type a method is declared on (`self`'s); `None` for a function.
    pub(crate) owner: Option<Type>,
    /// The interface, with its arguments, of the `extend` block that declares a method; `None`
    /// for every other function.
    pub(crate) conformance: Option<(InterfaceId, TypeArgs)>,
    /// A method's bare name, which `owner` and `conformance` write the rest of its name around
    /// (`write_method`). `None` for a function, and for an instance that `mono` named: its `name`
    /// is then the whole symbol.
    pub(crate) method: Option<String>,
    /// The names of the type parameters in scope: a method's owner's, then its own.
    pub(crate) type_params: Vec<String>,
    /// How many of `type_params` belong to the class or enum of a method.
    pub(crate) owner_params: usize,
    /// Whether the prelude writes the function. A prelude function takes its caller's location
    /// as two hidden parameters, so its run-time panics name the caller's line.
    pub(crate) prelude: bool,
    pub(crate) params: Vec<LocalId>,
    pub(crate) ret: Type,
    pub(crate) locals: Vec<Local>,
    pub(crate) body: Block,
}

#[derive(Clone, Debug)]
pub(crate) struct Local {
    /// `None` for a local the checker made up.
    pub(crate) name: Option<String>,
    pub(crate) ty: Type,
    pub(crate) mutable: bool,
}

/// `value` is the last statement when it was an expression statement in `ast`.
#[derive(Clone, Debug)]
pub(crate) struct Block {
    pub(crate) stmts: Vec<Stmt>,
    pub(crate) value: Option<Box<Expr>>,
    pub(crate) ty: Type,
}

/// `Append` is `place += value` on a `string` field or element: its place is never a `Local`.
#[derive(Clone, Debug)]
pub(crate) enum Stmt {
    Let { local: LocalId, init: Expr },
    Assign { place: Place, value: Expr },
    Append { place: Place, value: Expr },
    Expr(Expr),
}

#[derive(Clone, Debug)]
pub(crate) enum Place {
    Local(LocalId),
    /// Field `index` of the class `base` has.
    Field {
        base: Expr,
        index: usize,
    },
    /// Element `index` of the array in the place `base`.
    Index {
        base: Box<Place>,
        index: Expr,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct Expr {
    pub(crate) kind: ExprKind,
    pub(crate) ty: Type,
    pub(crate) span: Span,
}

/// The stream a `print` writes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stream {
    Stdout,
    Stderr,
}

impl Stream {
    /// The runtime's symbol prefix for the stream's print functions.
    pub(crate) fn symbol_prefix(self) -> &'static str {
        match self {
            Stream::Stdout => "sisu_print",
            Stream::Stderr => "sisu_eprint",
        }
    }

    /// The stream's place in an array that holds one entry for each stream.
    pub(crate) fn index(self) -> usize {
        match self {
            Stream::Stdout => 0,
            Stream::Stderr => 1,
        }
    }

    /// The prelude function that writes to the stream.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Stream::Stdout => "print",
            Stream::Stderr => "eprint",
        }
    }
}

/// How an interpolation writes its part `part`: padded, and with `precision` decimals for an
/// `f64`, or laid out to fit 80 columns when `pretty`. A pretty part has no pad and no precision.
/// Its type makes a group (`conformance::makes_group`), or is a type parameter that may.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Format {
    pub(crate) part: usize,
    pub(crate) pad: Option<Pad>,
    pub(crate) precision: Option<u8>,
    pub(crate) pretty: bool,
}

/// Padding to `width` code points, the text aligned by `align`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Pad {
    pub(crate) align: Align,
    pub(crate) width: u16,
    /// The spec has no alignment and the part's type is a type parameter or an associated type,
    /// so `align` is only a stand-in: `mono` sets the default of the type argument, a number on
    /// the right and any other value on the left, and clears the flag.
    pub(crate) by_type: bool,
}

impl Format {
    /// Whether the part is written plain: no pad, no precision and no layout. The checker and
    /// `mono` keep no such row, so a part has a row only if its spec changes the text.
    pub(crate) fn is_plain(&self) -> bool {
        self.pad.is_none() && self.precision.is_none() && !self.pretty
    }
}

/// The format as a spec: `>8.2`, or `#`.
impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.pretty {
            f.write_str("#")?;
        }
        if let Some(Pad { align, width, .. }) = self.pad {
            write!(f, "{}{width}", align.char())?;
        }
        if let Some(precision) = self.precision {
            write!(f, ".{precision}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(crate) enum ExprKind {
    Int(i64),
    Float(f64),
    Bool(bool),
    /// A `char` literal.
    Char(char),
    /// A string literal, decoded.
    Str(String),
    None,
    Local(LocalId),
    Call {
        func: FuncId,
        /// The callee's type arguments, a method's receiver's first; `NONE` when it is not
        /// generic.
        types: TypeArgs,
        args: Vec<Expr>,
    },
    /// A call of the requirement `method` of `interface` on a receiver whose type is a type
    /// parameter: `iargs` are `Self` and then the interface's own arguments, `types` the
    /// requirement's own, and `args` hold the receiver first. `mono` replaces it with a `Call` of
    /// the conformance's method.
    InterfaceCall {
        interface: InterfaceId,
        iargs: TypeArgs,
        method: u32,
        types: TypeArgs,
        args: Vec<Expr>,
    },
    New {
        class: ClassId,
        args: Vec<Expr>,
    },
    /// The variant `variant` of the enum `id`, holding `args` in declaration order.
    Variant {
        id: EnumId,
        variant: usize,
        args: Vec<Expr>,
    },
    /// Field `index` of the class `base` has.
    Field {
        base: Box<Expr>,
        index: usize,
    },
    /// A `T` where a `T?` is expected.
    Wrap(Box<Expr>),
    /// `u8(e)`, `i64(e)` or `char(e)`: `e` converted to the node's own type, a `u8?` or a
    /// `char?` for a conversion that can fail.
    Convert(Box<Expr>),
    /// `print(operand)` or `eprint(operand)`.
    Print {
        stream: Stream,
        operand: Box<Expr>,
    },
    /// `dbg(operand)`, of the operand's type. It writes `at`'s position, `text` (the operand's
    /// source as written) and the operand's value to standard error, and yields the value. `at`
    /// is the operand's span.
    Dbg {
        at: Span,
        text: String,
        operand: Box<Expr>,
    },
    /// `panic(message)`, of type `never`; its span starts where the panic reports.
    Panic(Box<Expr>),
    /// `read_line()`, of type `string?`.
    ReadLine,
    /// `[a, b]`.
    ArrayLit(Vec<Expr>),
    /// `[value; len]`.
    ArrayFill {
        value: Box<Expr>,
        len: Box<Expr>,
    },
    /// `base[index]`.
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
    },
    /// `base.len()`.
    Len(Box<Expr>),
    /// `base.chars()`: the code points of the string `base`, as a new `[char]`.
    Chars(Box<Expr>),
    /// `base.bytes()`: the UTF-8 bytes of the string `base`, as a new `[u8]`.
    Bytes(Box<Expr>),
    /// `base.words()`: the runs of non-white-space characters of the string `base`, as a new
    /// `[string]`.
    Words(Box<Expr>),
    /// `string(base)`: a new string that encodes the `[char]` `base`.
    FromChars(Box<Expr>),
    /// `operands[0].op(operands[1..])`: a method of `f64` or `i64`, with the receiver first.
    Math {
        op: MathOp,
        operands: Vec<Expr>,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// A string literal with interpolations: its text pieces as `Str`, and its expressions, in
    /// order, and the format of each part that has a spec, in part order.
    Interpolate {
        parts: Vec<Expr>,
        formats: Vec<Format>,
    },
    /// `lhs + rhs` on `string`s.
    Concat {
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Compare {
        operands: Vec<Expr>,
        ops: Vec<CompareOp>,
    },
    Equal {
        negated: bool,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// `lhs is rhs`.
    Is {
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    If {
        cond: Box<Expr>,
        then_block: Block,
        else_block: Option<Block>,
    },
    /// `if let bind = scrutinee { then } else { else }`; `bind` is in scope in `then_block`.
    IfSome {
        bind: LocalId,
        scrutinee: Box<Expr>,
        then_block: Block,
        else_block: Option<Block>,
    },
    /// `match scrutinee { arms }`: the scrutinee is evaluated once into `temp`, a local of its
    /// type that owns it, then the first arm whose pattern matches runs.
    Match {
        scrutinee: Box<Expr>,
        temp: LocalId,
        arms: Vec<Arm>,
    },
    Loop(Block),
    /// Leaves the loop `depth` loops out from the innermost one around it, giving it `value`.
    Break {
        depth: usize,
        value: Option<Box<Expr>>,
    },
    /// Starts the next pass of the loop `depth` loops out from the innermost one.
    Continue {
        depth: usize,
    },
    Return(Option<Box<Expr>>),
    Block(Block),
}

/// One arm of a `Match`. `bindings` holds each local its pattern binds, once, which every
/// alternative of an `Or` binds alike. `guard`, a `bool`, runs after the pattern matched.
#[derive(Clone, Debug)]
pub(crate) struct Arm {
    pub(crate) pattern: Pattern,
    pub(crate) bindings: Vec<LocalId>,
    pub(crate) guard: Option<Expr>,
    pub(crate) body: Block,
    /// False when earlier arms take every value the pattern matches (W0006).
    pub(crate) reachable: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Pattern {
    pub(crate) kind: PatternKind,
    pub(crate) span: Span,
}

#[derive(Clone, Debug)]
pub(crate) enum PatternKind {
    /// `_`.
    Wild,
    /// A name: binds the value at its position to the local.
    Bind(LocalId),
    /// The variant `variant` of the enum at its position, each field against its payload.
    Variant {
        variant: usize,
        fields: Vec<Pattern>,
    },
    /// The `bool` literal.
    Bool(bool),
    /// An `i64`, `u8` or `char` from `lo` through `hi`, a `char` as its code point; a literal
    /// is the range of one value.
    Range { lo: i64, hi: i64 },
    /// The `string` literal.
    Str(String),
    /// Any of the alternatives, tried in order; none of them is an `Or`.
    Or(Vec<Pattern>),
    /// A value of the optional at its position, the pattern against the value inside.
    Some(Box<Pattern>),
    /// The `None` of the optional at its position.
    None,
}

impl Type {
    /// Whether a value of this type is an owned reference, or an enum with counted parts,
    /// which codegen counts. `enums` are the program's.
    pub(crate) fn is_counted(self, enums: &[Enum]) -> bool {
        match self.kind() {
            TypeKind::String | TypeKind::Class(..) | TypeKind::Array(_) => true,
            TypeKind::Enum(id, _) => enums[id.0].counted,
            TypeKind::Optional(payload) => payload.is_counted(enums),
            _ => false,
        }
    }

    /// Whether a value of this type may be counted: it is, or a type argument may make it so.
    pub(crate) fn may_be_counted(self, enums: &[Enum]) -> bool {
        self.is_counted(enums) || self.has_param()
    }

    /// The enum a payload field of this type holds inline unless boxed: `F`'s for `F` or `F?`.
    /// These are the edges of the holds-inline graph.
    pub(crate) fn inline_enum(self) -> Option<EnumId> {
        match self.kind() {
            TypeKind::Enum(id, _) => Some(id),
            TypeKind::Optional(payload) => payload.inline_enum(),
            _ => None,
        }
    }
}

/// Lays out `enums`, whose payload types are final: boxes each payload field that closes a
/// cycle of the holds-inline graph, and sets each enum's `cycle` and `counted`. The box goes on
/// the declaring enum's own field rather than one inside a generic type. There is a pass per
/// `Rank`: `Plain`, `Own`, `Open`, `Late`, then `Assoc`. Each pass boxes its fields that are on
/// a cycle of what the passes before it left unboxed. A cycle that `Plain` leaves holds an
/// `Own`, `Open` or `Late` field, or an `Assoc` field. Take a cycle with no associated type.
/// A generic field of one enum on it names the declaration of the next, so the names close a
/// cycle of declarations. The choice of an associated type is a third source of enum names. It
/// can name the enum that holds it: `Item = E<W<T>>` for `A(T.Item)` in `E`. Only a field that
/// holds an associated type reads a choice. So a cycle that the earlier passes leave holds an
/// `Assoc` field. The last pass boxes each `Assoc` field that stays on a cycle. A `Never` field
/// is never boxed. The ranks read names only, so a type argument an enum never holds inline
/// (unused, or only in an array or a class) still counts: with `enum Two<T, U> { Both(T, T) }`,
/// `Wrap2<T>.W: Two<T, Wrap2<T>>` is `Own`, and `G<i64>` in `enum G<T> { P(Wrap2<G<T>>) }`
/// boxes `Wrap2<G<i64>>.W` beside `G<i64>.P`, which alone would break the cycle. And a `Late`
/// declaring field that names its cycle only through closed names can lose its box to an
/// `Open` field inside the generic type: with `enum Wrap8<T> { W(P2<T, Y8<T>>) N }` and
/// `enum Y8<T> { V(P2<T, Wrap8<i64>>) N }`, `G<T>.P: P2<T, Wrap8<G<i64>>>` is `Late`, so the
/// `Open` pass boxes `Wrap8<G<i64>>.W` and leaves `G<i64>.P` inline. And a ring of two
/// instances, `E3<P3<i64, bool>>` and `E3<P3<bool, i64>>` with `Item = E3<P3<B, A>>`, gets a
/// box on each, where one box breaks the ring. Each of these places a box where it is not
/// needed, and never leaves a cycle unboxed. Every member of a component with a box is on that
/// cycle, and counted, though a member may hold its links inline.
pub(crate) fn layout_enums(enums: &mut [Enum]) {
    // The holds-inline graph over the fields `keep` picks.
    let held = |enums: &[Enum], keep: fn(&Payload) -> bool| -> Vec<Vec<usize>> {
        (enums.iter())
            .map(|e| {
                let fields = e.variants.iter().flat_map(|v| &v.fields);
                (fields.filter(|p| keep(p)))
                    .filter_map(|p| p.ty.inline_enum())
                    .map(|id| id.0)
                    .collect()
            })
            .collect()
    };
    let component = components(&held(enums, |_| true));
    for (e, info) in enums.iter_mut().enumerate() {
        for p in info.variants.iter_mut().flat_map(|v| &mut v.fields) {
            p.boxed = p.rank == Rank::Plain
                && p.ty
                    .inline_enum()
                    .is_some_and(|f| component[f.0] == component[e]);
        }
    }
    // A program with only `Plain` and `Never` fields skips the later passes' graphs.
    let mut fields = enums
        .iter()
        .flat_map(|e| &e.variants)
        .flat_map(|v| &v.fields);
    if fields.any(|p| matches!(p.rank, Rank::Own | Rank::Open | Rank::Late | Rank::Assoc)) {
        for rank in [Rank::Own, Rank::Open, Rank::Late, Rank::Assoc] {
            let remaining = components(&held(enums, |p| !p.boxed));
            for (e, info) in enums.iter_mut().enumerate() {
                for p in info.variants.iter_mut().flat_map(|v| &mut v.fields) {
                    let on_cycle =
                        p.ty.inline_enum()
                            .is_some_and(|f| remaining[f.0] == remaining[e]);
                    p.boxed |= p.rank == rank && on_cycle;
                }
            }
        }
    }
    let mut has_box = vec![false; enums.len()];
    for (e, info) in enums.iter().enumerate() {
        has_box[component[e]] |= info
            .variants
            .iter()
            .flat_map(|v| &v.fields)
            .any(|p| p.boxed);
    }
    // A component is numbered after every component it holds, and holds its own members only
    // when it has a box, so counting in component order sees each other held enum counted.
    let mut order: Vec<usize> = (0..enums.len()).collect();
    order.sort_by_key(|&e| component[e]);
    for e in order {
        let boxed = has_box[component[e]];
        enums[e].cycle = boxed.then_some(component[e]);
        let counted = boxed
            || (enums[e].variants.iter().flat_map(|v| &v.fields))
                .any(|p| p.ty.may_be_counted(enums));
        enums[e].counted = counted;
    }
}

impl Expr {
    /// Whether a `Return`, `Break` or `Continue` sits anywhere inside, a nested loop included.
    pub(crate) fn exits(&self) -> bool {
        match &self.kind {
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Char(_)
            | ExprKind::Str(_)
            | ExprKind::None
            | ExprKind::ReadLine
            | ExprKind::Local(_) => false,
            ExprKind::Break { .. } | ExprKind::Continue { .. } | ExprKind::Return(_) => true,
            ExprKind::Call { args, .. }
            | ExprKind::InterfaceCall { args, .. }
            | ExprKind::New { args, .. }
            | ExprKind::Variant { args, .. } => args.iter().any(Expr::exits),
            ExprKind::Compare { operands, .. }
            | ExprKind::Math { operands, .. }
            | ExprKind::ArrayLit(operands)
            | ExprKind::Interpolate {
                parts: operands, ..
            } => operands.iter().any(Expr::exits),
            ExprKind::Field { base: operand, .. }
            | ExprKind::Len(operand)
            | ExprKind::Chars(operand)
            | ExprKind::Bytes(operand)
            | ExprKind::Words(operand)
            | ExprKind::FromChars(operand)
            | ExprKind::Wrap(operand)
            | ExprKind::Convert(operand)
            | ExprKind::Print { operand, .. }
            | ExprKind::Dbg { operand, .. }
            // A panic ends the process, so nothing before it needs binding for a release.
            | ExprKind::Panic(operand)
            | ExprKind::Unary { operand, .. } => operand.exits(),
            ExprKind::Binary { lhs, rhs, .. }
            | ExprKind::Concat { lhs, rhs }
            | ExprKind::Equal { lhs, rhs, .. }
            | ExprKind::Is { lhs, rhs }
            | ExprKind::ArrayFill {
                value: lhs,
                len: rhs,
            }
            | ExprKind::Index {
                base: lhs,
                index: rhs,
            } => lhs.exits() || rhs.exits(),
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                cond.exits() || then_block.exits() || else_block.as_ref().is_some_and(Block::exits)
            }
            ExprKind::IfSome {
                scrutinee,
                then_block,
                else_block,
                ..
            } => {
                scrutinee.exits()
                    || then_block.exits()
                    || else_block.as_ref().is_some_and(Block::exits)
            }
            ExprKind::Match {
                scrutinee, arms, ..
            } => {
                scrutinee.exits()
                    || arms
                        .iter()
                        .any(|arm| arm.guard.as_ref().is_some_and(Expr::exits) || arm.body.exits())
            }
            ExprKind::Loop(block) | ExprKind::Block(block) => block.exits(),
        }
    }
}

impl Expr {
    /// Calls `f` on each expression directly inside this one, through its blocks and places.
    pub(crate) fn each_child_mut(&mut self, f: &mut impl FnMut(&mut Expr)) {
        match &mut self.kind {
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Char(_)
            | ExprKind::Str(_)
            | ExprKind::None
            | ExprKind::ReadLine
            | ExprKind::Local(_)
            | ExprKind::Continue { .. }
            | ExprKind::Break { value: None, .. }
            | ExprKind::Return(None) => {}
            ExprKind::Call { args, .. }
            | ExprKind::InterfaceCall { args, .. }
            | ExprKind::New { args, .. }
            | ExprKind::Variant { args, .. }
            | ExprKind::Compare { operands: args, .. }
            | ExprKind::Math { operands: args, .. }
            | ExprKind::ArrayLit(args)
            | ExprKind::Interpolate { parts: args, .. } => args.iter_mut().for_each(f),
            ExprKind::Field { base: operand, .. }
            | ExprKind::Len(operand)
            | ExprKind::Chars(operand)
            | ExprKind::Bytes(operand)
            | ExprKind::Words(operand)
            | ExprKind::FromChars(operand)
            | ExprKind::Wrap(operand)
            | ExprKind::Convert(operand)
            | ExprKind::Print { operand, .. }
            | ExprKind::Dbg { operand, .. }
            | ExprKind::Panic(operand)
            | ExprKind::Unary { operand, .. }
            | ExprKind::Break {
                value: Some(operand),
                ..
            }
            | ExprKind::Return(Some(operand)) => f(operand),
            ExprKind::Binary { lhs, rhs, .. }
            | ExprKind::Concat { lhs, rhs }
            | ExprKind::Equal { lhs, rhs, .. }
            | ExprKind::Is { lhs, rhs }
            | ExprKind::ArrayFill {
                value: lhs,
                len: rhs,
            }
            | ExprKind::Index {
                base: lhs,
                index: rhs,
            } => {
                f(lhs);
                f(rhs);
            }
            ExprKind::If {
                cond: scrutinee,
                then_block,
                else_block,
            }
            | ExprKind::IfSome {
                scrutinee,
                then_block,
                else_block,
                ..
            } => {
                f(scrutinee);
                then_block.each_expr_mut(f);
                if let Some(block) = else_block {
                    block.each_expr_mut(f);
                }
            }
            ExprKind::Match {
                scrutinee, arms, ..
            } => {
                f(scrutinee);
                for arm in arms {
                    if let Some(guard) = &mut arm.guard {
                        f(guard);
                    }
                    arm.body.each_expr_mut(f);
                }
            }
            ExprKind::Loop(block) | ExprKind::Block(block) => block.each_expr_mut(f),
        }
    }
}

impl Block {
    /// Calls `f` on each expression directly in the block's statements, places and value.
    pub(crate) fn each_expr_mut(&mut self, f: &mut impl FnMut(&mut Expr)) {
        for stmt in &mut self.stmts {
            match stmt {
                Stmt::Let { init: e, .. } | Stmt::Expr(e) => f(e),
                Stmt::Assign { place, value } | Stmt::Append { place, value } => {
                    place.each_expr_mut(f);
                    f(value);
                }
            }
        }
        if let Some(value) = &mut self.value {
            f(value);
        }
    }
}

impl Place {
    fn each_expr_mut(&mut self, f: &mut impl FnMut(&mut Expr)) {
        match self {
            Place::Local(_) => {}
            Place::Field { base, .. } => f(base),
            Place::Index { base, index } => {
                base.each_expr_mut(f);
                f(index);
            }
        }
    }
}

impl Block {
    fn exits(&self) -> bool {
        self.stmts.iter().any(Stmt::exits) || self.value.as_ref().is_some_and(|v| v.exits())
    }
}

impl Stmt {
    fn exits(&self) -> bool {
        match self {
            Stmt::Let { init: e, .. } | Stmt::Expr(e) => e.exits(),
            Stmt::Assign { place, value } | Stmt::Append { place, value } => {
                place.exits() || value.exits()
            }
        }
    }
}

impl Place {
    fn exits(&self) -> bool {
        match self {
            Place::Local(_) => false,
            Place::Field { base, .. } => base.exits(),
            Place::Index { base, index } => base.exits() || index.exits(),
        }
    }
}

impl Expr {
    /// `self` and every expression inside it, outermost first.
    fn exprs<'a>(&'a self, out: &mut Vec<&'a Expr>) {
        use ExprKind as K;
        out.push(self);
        match &self.kind {
            K::Int(_)
            | K::Float(_)
            | K::Bool(_)
            | K::Char(_)
            | K::Str(_)
            | K::None
            | K::Local(_)
            | K::ReadLine
            | K::Break { value: None, .. }
            | K::Continue { .. }
            | K::Return(None) => {}
            K::Call { args, .. }
            | K::InterfaceCall { args, .. }
            | K::New { args, .. }
            | K::Variant { args, .. }
            | K::ArrayLit(args)
            | K::Math { operands: args, .. }
            | K::Interpolate { parts: args, .. }
            | K::Compare { operands: args, .. } => args.iter().for_each(|a| a.exprs(out)),
            K::Field { base: e, .. }
            | K::Wrap(e)
            | K::Convert(e)
            | K::Print { operand: e, .. }
            | K::Dbg { operand: e, .. }
            | K::Panic(e)
            | K::Len(e)
            | K::Chars(e)
            | K::Bytes(e)
            | K::Words(e)
            | K::FromChars(e)
            | K::Unary { operand: e, .. }
            | K::Break { value: Some(e), .. }
            | K::Return(Some(e)) => e.exprs(out),
            K::ArrayFill { value: a, len: b }
            | K::Index { base: a, index: b }
            | K::Binary { lhs: a, rhs: b, .. }
            | K::Concat { lhs: a, rhs: b }
            | K::Equal { lhs: a, rhs: b, .. }
            | K::Is { lhs: a, rhs: b } => {
                a.exprs(out);
                b.exprs(out);
            }
            K::If {
                cond: first,
                then_block,
                else_block,
            }
            | K::IfSome {
                scrutinee: first,
                then_block,
                else_block,
                ..
            } => {
                first.exprs(out);
                then_block.exprs(out);
                if let Some(b) = else_block {
                    b.exprs(out);
                }
            }
            K::Match {
                scrutinee, arms, ..
            } => {
                scrutinee.exprs(out);
                for arm in arms {
                    arm.guard.iter().for_each(|g| g.exprs(out));
                    arm.body.exprs(out);
                }
            }
            K::Loop(block) | K::Block(block) => block.exprs(out),
        }
    }
}

impl Block {
    /// Every expression in the block, each before those inside it, in order.
    pub(crate) fn exprs<'a>(&'a self, out: &mut Vec<&'a Expr>) {
        for stmt in &self.stmts {
            match stmt {
                Stmt::Let { init: e, .. } | Stmt::Expr(e) => e.exprs(out),
                Stmt::Assign { place, value } | Stmt::Append { place, value } => {
                    place.exprs(out);
                    value.exprs(out);
                }
            }
        }
        self.value.iter().for_each(|v| v.exprs(out));
    }
}

impl Place {
    fn exprs<'a>(&'a self, out: &mut Vec<&'a Expr>) {
        match self {
            Place::Local(_) => {}
            Place::Field { base, .. } => base.exprs(out),
            Place::Index { base, index } => {
                base.exprs(out);
                index.exprs(out);
            }
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.kind() {
            TypeKind::I64 => "i64",
            TypeKind::U8 => "u8",
            TypeKind::F64 => "f64",
            TypeKind::Bool => "bool",
            TypeKind::Char => "char",
            TypeKind::String => "string",
            TypeKind::Unit => "unit",
            TypeKind::Never => "never",
            // A class's name lives in the program: `Program::type_name` prints it.
            TypeKind::Param(name, _) => return write!(f, "{name}"),
            TypeKind::Assoc(base, _, _, k) => return write!(f, "{base}.assoc#{k}"),
            TypeKind::Class(id, _) => return write!(f, "class#{}", id.0),
            TypeKind::Enum(id, _) => return write!(f, "enum#{}", id.0),
            TypeKind::Array(element) => return write!(f, "[{element}]"),
            TypeKind::Optional(payload) => return write!(f, "{payload}?"),
        })
    }
}

impl fmt::Display for BinaryOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BinaryOp::Add => "+",
            BinaryOp::AddUnchecked => "+unchecked",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::Rem => "%",
            BinaryOp::And => "&&",
            BinaryOp::Or => "||",
        })
    }
}

/// What printing a node needs: callee names come from the program, local names from the function.
struct Printer<'a> {
    program: &'a Program,
    function: &'a Function,
}

impl Printer<'_> {
    fn local(&self, f: &mut fmt::Formatter<'_>, id: LocalId) -> fmt::Result {
        match &self.function.locals[id.0].name {
            Some(name) => write!(f, "{name}#{}", id.0),
            None => write!(f, "#{}", id.0),
        }
    }

    fn block(&self, f: &mut fmt::Formatter<'_>, block: &Block) -> fmt::Result {
        f.write_str("(block")?;
        for stmt in &block.stmts {
            f.write_str(" ")?;
            self.stmt(f, stmt)?;
        }
        if let Some(value) = &block.value {
            f.write_str(" ")?;
            self.expr(f, value)?;
        }
        f.write_str(")")
    }

    fn stmt(&self, f: &mut fmt::Formatter<'_>, stmt: &Stmt) -> fmt::Result {
        match stmt {
            Stmt::Let { local, init } => {
                let keyword = if self.function.locals[local.0].mutable {
                    "let mut"
                } else {
                    "let"
                };
                write!(f, "({keyword} ")?;
                self.local(f, *local)?;
                f.write_str(" ")?;
                self.expr(f, init)?;
                f.write_str(")")
            }
            Stmt::Assign { place, value } => self.write(f, "=", place, value),
            Stmt::Append { place, value } => self.write(f, "append", place, value),
            Stmt::Expr(expr) => self.expr(f, expr),
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per node kind, so the printed form reads in one place"
    )]
    fn expr(&self, f: &mut fmt::Formatter<'_>, expr: &Expr) -> fmt::Result {
        match &expr.kind {
            ExprKind::Int(n) if expr.ty == Type::U8 => write!(f, "{n}u8"),
            ExprKind::Int(n) => write!(f, "{n}"),
            // Rust's shortest text, which spells NaN `NaN`; Sisu spells it `nan`.
            ExprKind::Float(x) if x.is_nan() => f.write_str("nan"),
            ExprKind::Float(x) => write!(f, "{x:?}"),
            ExprKind::Bool(b) => write!(f, "{b}"),
            ExprKind::Char(c) => write!(f, "{c:?}"),
            ExprKind::Str(s) => write!(f, "{s:?}"),
            ExprKind::None => f.write_str("None"),
            ExprKind::Local(id) => self.local(f, *id),
            ExprKind::Call { func, types, args } => {
                let callee = &self.program.functions[func.0];
                let name = (callee
                    .conformance
                    .and_then(|_| self.program.method_name(callee)))
                .map_or_else(|| callee.name.clone(), |(name, _)| name);
                let types = self.program.args_name(types.get());
                self.list(f, format_args!("call {name}{types}"), args)
            }
            ExprKind::InterfaceCall {
                interface,
                iargs,
                method,
                types,
                args,
            } => {
                let info = &self.program.interfaces[interface.0];
                let name = format!(
                    "{}{}.{}{}",
                    info.name,
                    self.program.args_name(&iargs.get()[1..]),
                    info.methods[*method as usize],
                    self.program.args_name(types.get()),
                );
                self.list(f, format_args!("call {name}"), args)
            }
            ExprKind::New { class, args } => {
                let name = &self.program.classes[class.0].name;
                self.list(f, format_args!("new {name}"), args)
            }
            ExprKind::Variant { id, variant, args } => {
                let e = &self.program.enums[id.0];
                let name = &e.variants[*variant].name;
                self.list(f, format_args!("variant {}.{name}", e.name), args)
            }
            ExprKind::Field { base, index } => self.field(f, base, *index),
            ExprKind::Wrap(operand) => self.unary(f, "wrap", operand),
            ExprKind::Convert(operand) => self.unary(f, &expr.ty.to_string(), operand),
            ExprKind::Print { stream, operand } => self.unary(f, stream.name(), operand),
            ExprKind::Dbg { text, operand, .. } => self.unary(f, &format!("dbg {text:?}"), operand),
            ExprKind::Panic(message) => self.unary(f, "panic", message),
            ExprKind::ReadLine => f.write_str("(read_line)"),
            ExprKind::ArrayLit(elements) => self.list(f, format_args!("array"), elements),
            ExprKind::ArrayFill { value, len } => self.binary(f, "array-fill", value, len),
            ExprKind::Index { base, index } => self.binary(f, "index", base, index),
            ExprKind::Len(base) => self.unary(f, "len", base),
            ExprKind::Chars(base) => self.unary(f, "chars", base),
            ExprKind::Bytes(base) => self.unary(f, "bytes", base),
            ExprKind::Words(base) => self.unary(f, "words", base),
            ExprKind::FromChars(base) => self.unary(f, "string", base),
            ExprKind::Math { op, operands } => {
                self.list(f, format_args!("math {}", op.name()), operands)
            }
            ExprKind::Unary { op, operand } => self.unary(f, &op.to_string(), operand),
            ExprKind::Binary { op, lhs, rhs } => self.binary(f, &op.to_string(), lhs, rhs),
            ExprKind::Interpolate { parts, formats } => self.interpolate(f, parts, formats),
            ExprKind::Concat { lhs, rhs } => self.binary(f, "concat", lhs, rhs),
            ExprKind::Compare { operands, ops } => {
                // `(< a b <= c)`: the first operator leads, the rest sit between operands.
                write!(f, "({} ", ops[0])?;
                self.expr(f, &operands[0])?;
                f.write_str(" ")?;
                self.expr(f, &operands[1])?;
                for (op, operand) in ops[1..].iter().zip(&operands[2..]) {
                    write!(f, " {op} ")?;
                    self.expr(f, operand)?;
                }
                f.write_str(")")
            }
            ExprKind::Equal { negated, lhs, rhs } => {
                self.binary(f, if *negated { "!=" } else { "==" }, lhs, rhs)
            }
            ExprKind::Is { lhs, rhs } => self.binary(f, "is", lhs, rhs),
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                f.write_str("(if ")?;
                self.expr(f, cond)?;
                self.branches(f, then_block, else_block.as_ref())
            }
            ExprKind::IfSome {
                bind,
                scrutinee,
                then_block,
                else_block,
            } => {
                f.write_str("(if-some ")?;
                self.local(f, *bind)?;
                f.write_str(" ")?;
                self.expr(f, scrutinee)?;
                self.branches(f, then_block, else_block.as_ref())
            }
            ExprKind::Match {
                scrutinee, arms, ..
            } => {
                f.write_str("(match ")?;
                self.expr(f, scrutinee)?;
                for arm in arms {
                    f.write_str(" (arm ")?;
                    self.pattern(f, &arm.pattern, scrutinee.ty)?;
                    if let Some(guard) = &arm.guard {
                        f.write_str(" ")?;
                        self.unary(f, "if", guard)?;
                    }
                    f.write_str(" ")?;
                    self.block(f, &arm.body)?;
                    f.write_str(")")?;
                }
                f.write_str(")")
            }
            ExprKind::Loop(body) => {
                f.write_str("(loop ")?;
                self.block(f, body)?;
                f.write_str(")")
            }
            ExprKind::Break { depth, value } => {
                f.write_str("(break")?;
                jump_depth(f, *depth)?;
                if let Some(value) = value {
                    f.write_str(" ")?;
                    self.expr(f, value)?;
                }
                f.write_str(")")
            }
            ExprKind::Continue { depth } => {
                f.write_str("(continue")?;
                jump_depth(f, *depth)?;
                f.write_str(")")
            }
            ExprKind::Return(None) => f.write_str("(return)"),
            ExprKind::Return(Some(value)) => self.unary(f, "return", value),
            ExprKind::Block(block) => self.block(f, block),
        }
    }

    /// `_`, a local, a payload-free variant's name, `(Name field ...)`, a literal, `lo..=hi`,
    /// `(| alternative ...)`, `(Some pattern)` or `None`, at a position of type `ty`.
    fn pattern(&self, f: &mut fmt::Formatter<'_>, pattern: &Pattern, ty: Type) -> fmt::Result {
        match &pattern.kind {
            PatternKind::Wild => f.write_str("_"),
            PatternKind::Bind(local) => self.local(f, *local),
            PatternKind::Variant { variant, fields } => {
                let TypeKind::Enum(id, _) = ty.kind() else {
                    panic!(
                        "the checker should only emit a variant pattern at an enum position, not `{ty}`"
                    )
                };
                let variant = &self.program.enums[id.0].variants[*variant];
                if fields.is_empty() {
                    return f.write_str(&variant.name);
                }
                write!(f, "({}", variant.name)?;
                for (field, payload) in fields.iter().zip(&variant.fields) {
                    f.write_str(" ")?;
                    self.pattern(f, field, payload.ty)?;
                }
                f.write_str(")")
            }
            PatternKind::Bool(value) => write!(f, "{value}"),
            PatternKind::Range { lo, hi } => {
                let value = |f: &mut fmt::Formatter<'_>, v: i64| match ty {
                    Type::CHAR => {
                        let c = u32::try_from(v).ok().and_then(char::from_u32);
                        write!(f, "{:?}", c.unwrap_or(char::REPLACEMENT_CHARACTER))
                    }
                    _ => write!(f, "{v}"),
                };
                value(f, *lo)?;
                if lo != hi {
                    f.write_str("..=")?;
                    value(f, *hi)?;
                }
                Ok(())
            }
            PatternKind::Str(text) => write!(f, "{text:?}"),
            PatternKind::Or(alternatives) => {
                f.write_str("(|")?;
                for alternative in alternatives {
                    f.write_str(" ")?;
                    self.pattern(f, alternative, ty)?;
                }
                f.write_str(")")
            }
            PatternKind::Some(inner) => {
                let TypeKind::Optional(payload) = ty.kind() else {
                    panic!(
                        "the checker should only emit a `Some` pattern at an optional position, not `{ty}`"
                    )
                };
                f.write_str("(Some ")?;
                self.pattern(f, inner, payload)?;
                f.write_str(")")
            }
            PatternKind::None => f.write_str("None"),
        }
    }

    /// The blocks of an `if` or `if-some`, each after a space, and its closing `)`.
    fn branches(
        &self,
        f: &mut fmt::Formatter<'_>,
        then_block: &Block,
        else_block: Option<&Block>,
    ) -> fmt::Result {
        for block in std::iter::once(then_block).chain(else_block) {
            f.write_str(" ")?;
            self.block(f, block)?;
        }
        f.write_str(")")
    }

    /// `(head place value)`.
    fn write(
        &self,
        f: &mut fmt::Formatter<'_>,
        head: &str,
        place: &Place,
        value: &Expr,
    ) -> fmt::Result {
        write!(f, "({head} ")?;
        self.place(f, place)?;
        f.write_str(" ")?;
        self.expr(f, value)?;
        f.write_str(")")
    }

    /// A local, `(. base name)` or `(index place i)`.
    fn place(&self, f: &mut fmt::Formatter<'_>, place: &Place) -> fmt::Result {
        match place {
            Place::Local(local) => self.local(f, *local),
            Place::Field { base, index } => self.field(f, base, *index),
            Place::Index { base, index } => {
                f.write_str("(index ")?;
                self.place(f, base)?;
                f.write_str(" ")?;
                self.expr(f, index)?;
                f.write_str(")")
            }
        }
    }

    /// `(. base name)`.
    fn field(&self, f: &mut fmt::Formatter<'_>, base: &Expr, index: usize) -> fmt::Result {
        let TypeKind::Class(class, _) = base.ty.kind() else {
            panic!(
                "the checker should only emit a field access on a class-typed base, not `{}`",
                base.ty
            )
        };
        f.write_str("(. ")?;
        self.expr(f, base)?;
        write!(f, " {})", self.program.classes[class.0].fields[index].name)
    }

    fn unary(&self, f: &mut fmt::Formatter<'_>, head: &str, operand: &Expr) -> fmt::Result {
        write!(f, "({head} ")?;
        self.expr(f, operand)?;
        f.write_str(")")
    }

    fn binary(
        &self,
        f: &mut fmt::Formatter<'_>,
        head: &str,
        lhs: &Expr,
        rhs: &Expr,
    ) -> fmt::Result {
        write!(f, "({head} ")?;
        self.expr(f, lhs)?;
        f.write_str(" ")?;
        self.expr(f, rhs)?;
        f.write_str(")")
    }

    /// `(head e1 e2 ...)`.
    fn list(
        &self,
        f: &mut fmt::Formatter<'_>,
        head: fmt::Arguments<'_>,
        exprs: &[Expr],
    ) -> fmt::Result {
        write!(f, "({head}")?;
        self.each(f, exprs)?;
        f.write_str(")")
    }

    /// `(interp ...)`, each part with a format as `(fmt <part> "<spec>")`.
    fn interpolate(
        &self,
        f: &mut fmt::Formatter<'_>,
        parts: &[Expr],
        formats: &[Format],
    ) -> fmt::Result {
        f.write_str("(interp")?;
        // The formats are in part order, so one pass over them meets each in turn.
        let mut formats = formats.iter().peekable();
        for (i, part) in parts.iter().enumerate() {
            f.write_str(" ")?;
            match formats.next_if(|format| format.part == i) {
                Some(format) => {
                    f.write_str("(fmt ")?;
                    self.expr(f, part)?;
                    write!(f, " \"{format}\")")?;
                }
                None => self.expr(f, part)?,
            }
        }
        f.write_str(")")
    }

    /// Writes each expression after a space.
    fn each(&self, f: &mut fmt::Formatter<'_>, exprs: &[Expr]) -> fmt::Result {
        exprs.iter().try_for_each(|e| {
            f.write_str(" ")?;
            self.expr(f, e)
        })
    }
}

impl Program {
    /// A type as `--emit tir` prints it: a class or an enum by its name.
    pub(crate) fn type_name(&self, ty: Type) -> String {
        let mut name = String::new();
        write_type(&mut name, ty, |head| self.head(head));
        name
    }

    /// `<a, b>` for `args`, each named as `type_name` names it; nothing for none.
    pub(crate) fn args_name(&self, args: &[Type]) -> String {
        let mut name = String::new();
        write_args(&mut name, args, |head| self.head(head));
        name
    }

    /// The name of `function`, a method, as `--emit tir` prints it, written from the type it is
    /// declared on, the interface of its conformance block and its bare name: the interface
    /// stands between the type and the method, so two conformances may each hold a `name`. Its
    /// own type arguments follow. `None` for a function, and for an instance that `mono` named:
    /// `Function::name` is then the whole name. Before `mono`, `Function::name` keeps the text
    /// without the interface.
    pub(crate) fn method_name(&self, function: &Function) -> Option<(String, bool)> {
        let method = function.method.as_deref()?;
        let owner = (function.owner).expect("a method should have the type it is declared on");
        let interface = function
            .conformance
            .map(|(interface, args)| (self.interfaces[interface.0].name.as_str(), args));
        let mut name = String::new();
        let cut = write_method(&mut name, owner, interface, method, |head| self.head(head));
        Some((name, cut))
    }

    fn head(&self, head: Head) -> &str {
        match head {
            Head::Class(id) => &self.classes[id.0].name,
            Head::Enum(id) => &self.enums[id.0].name,
            Head::Assoc(interface, k) => &self.interfaces[interface.0].assoc[k as usize],
        }
    }
}

impl fmt::Display for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Classes first, then enums, then functions, one per line.
        let mut separator = "";
        for class in &self.classes {
            write!(
                f,
                "{separator}(class {}{}",
                class.name,
                angled(&class.type_params)
            )?;
            for field in &class.fields {
                let keyword = if field.mutable { "mut " } else { "" };
                let ty = self.type_name(field.ty);
                write!(f, " ({keyword}{} {ty})", field.name)?;
            }
            f.write_str(")")?;
            separator = "\n";
        }
        for e in &self.enums {
            write!(f, "{separator}(enum {}{}", e.name, angled(&e.type_params))?;
            for variant in &e.variants {
                if variant.fields.is_empty() {
                    write!(f, " {}", variant.name)?;
                    continue;
                }
                write!(f, " ({}", variant.name)?;
                for field in &variant.fields {
                    let ty = self.type_name(field.ty);
                    if field.boxed {
                        write!(f, " (box {ty})")?;
                    } else {
                        write!(f, " {ty}")?;
                    }
                }
                f.write_str(")")?;
            }
            f.write_str(")")?;
            separator = "\n";
        }
        for function in &self.functions {
            f.write_str(separator)?;
            separator = "\n";
            let printer = Printer {
                program: self,
                function,
            };
            let own = &function.type_params[function.owner_params..];
            let name = (self.method_name(function))
                .map_or_else(|| function.name.clone(), |(name, _)| name);
            write!(f, "(fn {name}{} (", angled(own))?;
            for (j, param) in function.params.iter().enumerate() {
                if j > 0 {
                    f.write_str(" ")?;
                }
                f.write_str("(")?;
                printer.local(f, *param)?;
                write!(f, " {})", self.type_name(function.locals[param.0].ty))?;
            }
            write!(f, ") {} ", self.type_name(function.ret))?;
            printer.block(f, &function.body)?;
            f.write_str(")")?;
        }
        Ok(())
    }
}

/// ` ^depth` for a jump to an outer loop; nothing for the innermost one.
fn jump_depth(f: &mut fmt::Formatter<'_>, depth: usize) -> fmt::Result {
    if depth > 0 {
        write!(f, " ^{depth}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbol::Symbol;

    fn expr(kind: ExprKind, ty: Type) -> Expr {
        Expr {
            kind,
            ty,
            span: Span::new(0, 0),
        }
    }

    /// `print(operand)`.
    fn print_of(operand: Expr) -> Expr {
        expr(
            ExprKind::Print {
                stream: Stream::Stdout,
                operand: Box::new(operand),
            },
            Type::UNIT,
        )
    }

    fn int(n: i64) -> Expr {
        expr(ExprKind::Int(n), Type::I64)
    }

    fn local(id: usize) -> Expr {
        expr(ExprKind::Local(LocalId(id)), Type::I64)
    }

    fn block(stmts: Vec<Stmt>) -> Block {
        Block {
            stmts,
            value: None,
            ty: Type::UNIT,
        }
    }

    #[test]
    fn jumps_should_print_their_depth_and_value() {
        let jump = |kind| Stmt::Expr(expr(kind, Type::NEVER));
        let inner = block(vec![
            jump(ExprKind::Break {
                depth: 1,
                value: Some(Box::new(int(1))),
            }),
            jump(ExprKind::Continue { depth: 1 }),
        ]);
        let outer = block(vec![
            Stmt::Expr(expr(ExprKind::Loop(inner), Type::NEVER)),
            jump(ExprKind::Break {
                depth: 0,
                value: Some(Box::new(int(2))),
            }),
        ]);
        let program = Program {
            classes: vec![],
            enums: vec![],
            functions: vec![Function {
                name: "f".into(),
                owner: None,
                conformance: None,
                method: None,
                type_params: vec![],
                owner_params: 0,
                prelude: false,
                params: vec![],
                ret: Type::I64,
                locals: vec![],
                body: Block {
                    stmts: vec![],
                    value: Some(Box::new(expr(ExprKind::Loop(outer), Type::I64))),
                    ty: Type::I64,
                },
            }],
            ..Program::default()
        };
        assert_eq!(
            program.to_string(),
            "(fn f () i64 (block (loop (block (loop (block (break ^1 1) (continue ^1))) (break 2)))))"
        );
    }

    /// `(if (!= x#1 2) (block (print (< 1 n#0 <= 9))))` and
    /// `(loop (block (if true (block (break)) (block))))`.
    fn if_and_loop() -> (Expr, Expr) {
        let x_ne_2 = expr(
            ExprKind::Equal {
                negated: true,
                lhs: Box::new(local(1)),
                rhs: Box::new(int(2)),
            },
            Type::BOOL,
        );
        let chain = expr(
            ExprKind::Compare {
                operands: vec![int(1), local(0), int(9)],
                ops: vec![CompareOp::Lt, CompareOp::Le],
            },
            Type::BOOL,
        );
        let print = print_of(chain);
        let if_stmt = expr(
            ExprKind::If {
                cond: Box::new(x_ne_2),
                then_block: block(vec![Stmt::Expr(print)]),
                else_block: None,
            },
            Type::UNIT,
        );
        let brk = expr(
            ExprKind::Break {
                depth: 0,
                value: None,
            },
            Type::NEVER,
        );
        let inner_if = expr(
            ExprKind::If {
                cond: Box::new(expr(ExprKind::Bool(true), Type::BOOL)),
                then_block: block(vec![Stmt::Expr(brk)]),
                else_block: Some(block(vec![])),
            },
            Type::UNIT,
        );
        let looped = expr(
            ExprKind::Loop(block(vec![Stmt::Expr(inner_if)])),
            Type::UNIT,
        );
        (if_stmt, looped)
    }

    #[test]
    fn every_node_should_print() {
        let locals = vec![
            Local {
                name: Some("n".into()),
                ty: Type::I64,
                mutable: false,
            },
            Local {
                name: Some("x".into()),
                ty: Type::I64,
                mutable: true,
            },
            Local {
                name: None,
                ty: Type::I64,
                mutable: false,
            },
        ];
        let neg3 = expr(
            ExprKind::Unary {
                op: UnaryOp::Neg,
                operand: Box::new(int(3)),
            },
            Type::I64,
        );
        let call = expr(
            ExprKind::Call {
                func: FuncId(0),
                types: TypeArgs::NONE,
                args: vec![neg3],
            },
            Type::I64,
        );
        let x_plus_1 = expr(
            ExprKind::Binary {
                op: BinaryOp::Add,
                lhs: Box::new(local(1)),
                rhs: Box::new(int(1)),
            },
            Type::I64,
        );
        let (if_stmt, looped) = if_and_loop();
        let ret = expr(ExprKind::Return(Some(Box::new(local(1)))), Type::NEVER);
        let program = Program {
            classes: vec![],
            enums: vec![],
            functions: vec![Function {
                name: "f".into(),
                owner: None,
                conformance: None,
                method: None,
                type_params: Vec::new(),
                owner_params: 0,
                prelude: false,
                params: vec![LocalId(0)],
                ret: Type::I64,
                locals,
                body: Block {
                    stmts: vec![
                        Stmt::Let {
                            local: LocalId(1),
                            init: local(0),
                        },
                        Stmt::Assign {
                            place: Place::Local(LocalId(1)),
                            value: x_plus_1,
                        },
                        Stmt::Let {
                            local: LocalId(2),
                            init: call,
                        },
                        Stmt::Expr(if_stmt),
                        Stmt::Expr(looped),
                        Stmt::Expr(ret),
                    ],
                    value: None,
                    ty: Type::NEVER,
                },
            }],
            ..Program::default()
        };
        assert_eq!(
            program.to_string(),
            "(fn f ((n#0 i64)) i64 (block (let mut x#1 n#0) (= x#1 (+ x#1 1)) (let #2 (call f (- 3))) (if (!= x#1 2) (block (print (< 1 n#0 <= 9)))) (loop (block (if true (block (break)) (block)))) (return x#1)))"
        );
    }

    /// `match s { Running(c) => c  Idle => 0  _ => 1 }` over `enum State { Idle  Running(i64) }`.
    #[test]
    fn tir_should_print_a_match_over_an_enum() {
        let state = Type::intern(TypeKind::Enum(EnumId(0), TypeArgs::NONE));
        let pattern = |kind| Pattern {
            kind,
            span: Span::new(0, 0),
        };
        let arm = |kind, bindings, value| Arm {
            pattern: pattern(kind),
            bindings,
            guard: None,
            body: Block {
                stmts: vec![],
                value: Some(Box::new(value)),
                ty: Type::I64,
            },
            reachable: true,
        };
        let arms = vec![
            arm(
                PatternKind::Variant {
                    variant: 1,
                    fields: vec![pattern(PatternKind::Bind(LocalId(1)))],
                },
                vec![LocalId(1)],
                local(1),
            ),
            arm(
                PatternKind::Variant {
                    variant: 0,
                    fields: vec![],
                },
                vec![],
                int(0),
            ),
            arm(PatternKind::Wild, vec![], int(1)),
        ];
        let matched = expr(
            ExprKind::Match {
                scrutinee: Box::new(expr(ExprKind::Local(LocalId(0)), state)),
                temp: LocalId(2),
                arms,
            },
            Type::I64,
        );
        let program = Program {
            classes: vec![],
            enums: vec![Enum {
                name: "State".into(),
                span: Span::new(0, 0),
                instance: false,
                type_params: Vec::new(),
                variants: vec![
                    Variant {
                        name: "Idle".into(),
                        fields: vec![],
                    },
                    Variant {
                        name: "Running".into(),
                        fields: vec![Payload {
                            ty: Type::I64,
                            label: None,
                            boxed: false,
                            rank: Rank::Plain,
                        }],
                    },
                ],
                counted: false,
                cycle: None,
            }],
            functions: vec![Function {
                name: "f".into(),
                owner: None,
                conformance: None,
                method: None,
                type_params: Vec::new(),
                owner_params: 0,
                prelude: false,
                params: vec![LocalId(0)],
                ret: Type::I64,
                locals: vec![
                    param("s", state),
                    param("c", Type::I64),
                    Local {
                        name: None,
                        ty: state,
                        mutable: false,
                    },
                ],
                body: Block {
                    stmts: vec![],
                    value: Some(Box::new(matched)),
                    ty: Type::I64,
                },
            }],
            ..Program::default()
        };
        assert_eq!(
            program.to_string(),
            "(enum State Idle (Running i64))\n\
             (fn f ((s#0 State)) i64 (block (match s#0 (arm (Running c#1) (block c#1)) (arm Idle (block 0)) (arm _ (block 1)))))"
        );
    }

    /// A writer that refuses the write number `at`, and takes every other one.
    struct FailsAt {
        at: usize,
        writes: usize,
    }

    impl fmt::Write for FailsAt {
        fn write_str(&mut self, _: &str) -> fmt::Result {
            self.writes += 1;
            if self.writes - 1 == self.at {
                return Err(fmt::Error);
            }
            Ok(())
        }
    }

    /// `match o { Some(_) => 1  None => 0 }` over an `i64?`: the patterns of an optional.
    #[test]
    fn print_should_give_the_error_of_a_write_that_fails_in_an_optional_pattern() {
        use fmt::Write as _;

        let optional = Type::intern(TypeKind::Optional(Type::I64));
        let arm = |kind, value| Arm {
            pattern: Pattern {
                kind,
                span: Span::new(0, 0),
            },
            bindings: vec![],
            guard: None,
            body: Block {
                stmts: vec![],
                value: Some(Box::new(int(value))),
                ty: Type::I64,
            },
            reachable: true,
        };
        let wild = Pattern {
            kind: PatternKind::Wild,
            span: Span::new(0, 0),
        };
        let matched = expr(
            ExprKind::Match {
                scrutinee: Box::new(expr(ExprKind::Local(LocalId(0)), optional)),
                temp: LocalId(1),
                arms: vec![
                    arm(PatternKind::Some(Box::new(wild)), 1),
                    arm(PatternKind::None, 0),
                ],
            },
            Type::I64,
        );
        let program = Program {
            functions: vec![Function {
                name: "f".into(),
                owner: None,
                conformance: None,
                method: None,
                type_params: Vec::new(),
                owner_params: 0,
                prelude: false,
                params: vec![LocalId(0)],
                ret: Type::I64,
                locals: vec![
                    param("o", optional),
                    Local {
                        name: None,
                        ty: optional,
                        mutable: false,
                    },
                ],
                body: Block {
                    stmts: vec![],
                    value: Some(Box::new(matched)),
                    ty: Type::I64,
                },
            }],
            ..Program::default()
        };
        assert_eq!(
            program.to_string(),
            "(fn f ((o#0 i64?)) i64 (block (match o#0 (arm (Some _) (block 1)) (arm None (block 0)))))"
        );
        // Whichever single write fails, the print gives that error and does not go on.
        let mut counter = FailsAt {
            at: usize::MAX,
            writes: 0,
        };
        write!(counter, "{program}").expect("a writer that never fails takes the print");
        for at in 0..counter.writes {
            let mut writer = FailsAt { at, writes: 0 };
            assert!(write!(writer, "{program}").is_err(), "write {at}");
        }
    }

    /// A method of a conformance block prints its interface between its type and its name, in its
    /// definition and in a call, and a method of the class itself prints none; a write that fails
    /// in any of them gives that error.
    #[test]
    fn print_should_name_a_conformance_method_with_its_interface() {
        use fmt::Write as _;

        let class = Type::intern(TypeKind::Class(ClassId(0), TypeArgs::NONE));
        let method = Function {
            name: "C.name".into(),
            owner: Some(class),
            conformance: Some((InterfaceId(0), TypeArgs::NONE)),
            method: Some("name".into()),
            type_params: Vec::new(),
            owner_params: 0,
            prelude: false,
            params: vec![LocalId(0)],
            ret: Type::I64,
            locals: vec![param("self", class)],
            body: Block {
                stmts: vec![],
                value: Some(Box::new(int(1))),
                ty: Type::I64,
            },
        };
        // A method of the class itself prints no interface.
        let own = Function {
            name: "C.get".into(),
            conformance: None,
            method: Some("get".into()),
            body: Block {
                stmts: vec![],
                value: Some(Box::new(int(2))),
                ty: Type::I64,
            },
            ..method.clone()
        };
        let caller = Function {
            name: "f".into(),
            owner: None,
            conformance: None,
            method: None,
            type_params: Vec::new(),
            owner_params: 0,
            prelude: false,
            params: vec![LocalId(0)],
            ret: Type::I64,
            locals: vec![param("c", class)],
            body: Block {
                stmts: vec![],
                value: Some(Box::new(expr(
                    ExprKind::Call {
                        func: FuncId(1),
                        types: TypeArgs::NONE,
                        args: vec![expr(ExprKind::Local(LocalId(0)), class)],
                    },
                    Type::I64,
                ))),
                ty: Type::I64,
            },
        };
        let program = Program {
            classes: vec![Class {
                name: "C".into(),
                type_params: Vec::new(),
                fields: Vec::new(),
            }],
            interfaces: vec![Interface {
                methods: Vec::new(),
                assoc: Vec::new(),
                name: "Label".into(),
            }],
            functions: vec![caller, method, own],
            ..Program::default()
        };
        assert_eq!(
            program.to_string(),
            "(class C)\n\
             (fn f ((c#0 C)) i64 (block (call C.Label.name c#0)))\n\
             (fn C.Label.name ((self#0 C)) i64 (block 1))\n\
             (fn C.get ((self#0 C)) i64 (block 2))"
        );
        // Whichever single write fails, the print gives that error and does not go on.
        let mut counter = FailsAt {
            at: usize::MAX,
            writes: 0,
        };
        write!(counter, "{program}").expect("a writer that never fails takes the print");
        for at in 0..counter.writes {
            let mut writer = FailsAt { at, writes: 0 };
            assert!(write!(writer, "{program}").is_err(), "write {at}");
        }
    }

    fn binary(op: BinaryOp, lhs: Expr, rhs: Expr) -> Expr {
        expr(
            ExprKind::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            Type::BOOL,
        )
    }

    fn param(name: &str, ty: Type) -> Local {
        Local {
            name: Some(name.into()),
            ty,
            mutable: false,
        }
    }

    #[test]
    fn functions_should_print_joined_by_newline() {
        // fn first() { return }
        // fn second(a: i64, ok: bool) -> bool { first(); ok && (a == 3 && a < 4) }
        let first = Function {
            name: "first".into(),
            owner: None,
            conformance: None,
            method: None,
            type_params: Vec::new(),
            owner_params: 0,
            prelude: false,
            params: vec![],
            ret: Type::UNIT,
            locals: vec![],
            body: block(vec![Stmt::Expr(expr(ExprKind::Return(None), Type::NEVER))]),
        };
        let call = expr(
            ExprKind::Call {
                func: FuncId(0),
                types: TypeArgs::intern(&[Type::I64]),
                args: vec![],
            },
            Type::UNIT,
        );
        let a_eq_3 = expr(
            ExprKind::Equal {
                negated: false,
                lhs: Box::new(local(0)),
                rhs: Box::new(int(3)),
            },
            Type::BOOL,
        );
        let a_lt_4 = expr(
            ExprKind::Compare {
                operands: vec![local(0), int(4)],
                ops: vec![CompareOp::Lt],
            },
            Type::BOOL,
        );
        let ok = expr(ExprKind::Local(LocalId(1)), Type::BOOL);
        let value = binary(BinaryOp::And, ok, binary(BinaryOp::And, a_eq_3, a_lt_4));
        let second = Function {
            name: "second".into(),
            owner: None,
            conformance: None,
            method: None,
            type_params: Vec::new(),
            owner_params: 0,
            prelude: false,
            params: vec![LocalId(0), LocalId(1)],
            ret: Type::BOOL,
            locals: vec![param("a", Type::I64), param("ok", Type::BOOL)],
            body: Block {
                stmts: vec![Stmt::Expr(call)],
                value: Some(Box::new(value)),
                ty: Type::BOOL,
            },
        };
        let program = Program {
            classes: vec![],
            enums: vec![],
            functions: vec![first, second],
            ..Program::default()
        };
        assert_eq!(
            program.to_string(),
            "(fn first () unit (block (return)))\n\
             (fn second ((a#0 i64) (ok#1 bool)) bool (block (call first<i64>) (&& ok#1 (&& (== a#0 3) (< a#0 4)))))"
        );
    }

    #[test]
    fn prints_every_binary_operator() {
        let ops = [
            (BinaryOp::Add, "+"),
            (BinaryOp::AddUnchecked, "+unchecked"),
            (BinaryOp::Sub, "-"),
            (BinaryOp::Mul, "*"),
            (BinaryOp::Div, "/"),
            (BinaryOp::Rem, "%"),
            (BinaryOp::And, "&&"),
            (BinaryOp::Or, "||"),
        ];
        for (op, symbol) in ops {
            assert_eq!(op.to_string(), symbol);
        }
    }

    #[test]
    fn float_literals_should_print() {
        let float = |x| expr(ExprKind::Float(x), Type::F64);
        let program = Program {
            classes: vec![],
            enums: vec![],
            functions: vec![Function {
                name: "f".into(),
                owner: None,
                conformance: None,
                method: None,
                type_params: Vec::new(),
                owner_params: 0,
                prelude: false,
                params: vec![],
                ret: Type::UNIT,
                locals: vec![],
                body: block(
                    [1.0, 1e16, f64::NAN]
                        .into_iter()
                        .map(|x| Stmt::Expr(print_of(float(x))))
                        .collect(),
                ),
            }],
            ..Program::default()
        };
        assert_eq!(
            program.to_string(),
            "(fn f () unit (block (print 1.0) (print 1e16) (print nan)))"
        );
    }

    #[test]
    fn char_literals_and_conversions_should_print() {
        // fn f(c: char) { print('ä'); print(i64(c)); print(char(228)) }
        let char_lit = expr(ExprKind::Char('ä'), Type::CHAR);
        let to_i64 = expr(
            ExprKind::Convert(Box::new(expr(ExprKind::Local(LocalId(0)), Type::CHAR))),
            Type::I64,
        );
        let to_char = expr(ExprKind::Convert(Box::new(int(228))), Type::CHAR);
        let print = |arg| Stmt::Expr(print_of(arg));
        let program = Program {
            classes: vec![],
            enums: vec![],
            functions: vec![Function {
                name: "f".into(),
                owner: None,
                conformance: None,
                method: None,
                type_params: Vec::new(),
                owner_params: 0,
                prelude: false,
                params: vec![LocalId(0)],
                ret: Type::UNIT,
                locals: vec![param("c", Type::CHAR)],
                body: block(vec![print(char_lit), print(to_i64), print(to_char)]),
            }],
            ..Program::default()
        };
        assert_eq!(
            program.to_string(),
            "(fn f ((c#0 char)) unit (block (print 'ä') (print (i64 c#0)) (print (char 228))))"
        );
    }

    #[test]
    fn eprint_should_write_to_standard_error() {
        // fn f() { eprint(1) }
        let eprint = expr(
            ExprKind::Print {
                stream: Stream::Stderr,
                operand: Box::new(int(1)),
            },
            Type::UNIT,
        );
        let program = Program {
            classes: vec![],
            enums: vec![],
            functions: vec![Function {
                name: "f".into(),
                owner: None,
                conformance: None,
                method: None,
                type_params: Vec::new(),
                owner_params: 0,
                prelude: false,
                params: vec![],
                ret: Type::UNIT,
                locals: vec![],
                body: block(vec![Stmt::Expr(eprint)]),
            }],
            ..Program::default()
        };
        assert_eq!(program.to_string(), "(fn f () unit (block (eprint 1)))");
    }

    #[test]
    fn read_line_should_print() {
        // fn f() { read_line() }
        let read_line = expr(ExprKind::ReadLine, Type::optional(Type::STRING));
        let program = Program {
            classes: vec![],
            enums: vec![],
            functions: vec![Function {
                name: "f".into(),
                owner: None,
                conformance: None,
                method: None,
                type_params: Vec::new(),
                owner_params: 0,
                prelude: false,
                params: vec![],
                ret: Type::UNIT,
                locals: vec![],
                body: block(vec![Stmt::Expr(read_line)]),
            }],
            ..Program::default()
        };
        assert_eq!(program.to_string(), "(fn f () unit (block (read_line)))");
    }

    #[test]
    fn words_should_print() {
        // fn f(s: string) { s.words() }
        let s = Box::new(expr(ExprKind::Local(LocalId(0)), Type::STRING));
        let words = expr(ExprKind::Words(s), Type::array(Type::STRING));
        let program = Program {
            classes: vec![],
            enums: vec![],
            functions: vec![Function {
                name: "f".into(),
                owner: None,
                conformance: None,
                method: None,
                type_params: Vec::new(),
                owner_params: 0,
                prelude: false,
                params: vec![LocalId(0)],
                ret: Type::UNIT,
                locals: vec![param("s", Type::STRING)],
                body: block(vec![Stmt::Expr(words)]),
            }],
            ..Program::default()
        };
        assert_eq!(
            program.to_string(),
            "(fn f ((s#0 string)) unit (block (words s#0)))"
        );
    }

    #[test]
    fn strings_should_print() {
        // fn f(s: string) { print("a\"\n" + s) }
        let lit = expr(ExprKind::Str("a\"\n".into()), Type::STRING);
        let s = expr(ExprKind::Local(LocalId(0)), Type::STRING);
        let concat = ExprKind::Concat {
            lhs: Box::new(lit),
            rhs: Box::new(s),
        };
        let print = print_of(expr(concat, Type::STRING));
        let program = Program {
            classes: vec![],
            enums: vec![],
            functions: vec![Function {
                name: "f".into(),
                owner: None,
                conformance: None,
                method: None,
                type_params: Vec::new(),
                owner_params: 0,
                prelude: false,
                params: vec![LocalId(0)],
                ret: Type::UNIT,
                locals: vec![param("s", Type::STRING)],
                body: block(vec![Stmt::Expr(print)]),
            }],
            ..Program::default()
        };
        assert_eq!(
            program.to_string(),
            r#"(fn f ((s#0 string)) unit (block (print (concat "a\"\n" s#0))))"#
        );
    }

    #[test]
    fn interpolation_should_print() {
        // fn f(s: string, n: i64) { print("a{s}{n}") }
        let parts = vec![
            expr(ExprKind::Str("a".into()), Type::STRING),
            expr(ExprKind::Local(LocalId(0)), Type::STRING),
            expr(ExprKind::Local(LocalId(1)), Type::I64),
        ];
        let interp = expr(
            ExprKind::Interpolate {
                parts,
                formats: vec![],
            },
            Type::STRING,
        );
        let print = print_of(interp);
        let program = Program {
            classes: vec![],
            enums: vec![],
            functions: vec![Function {
                name: "f".into(),
                owner: None,
                conformance: None,
                method: None,
                type_params: Vec::new(),
                owner_params: 0,
                prelude: false,
                params: vec![LocalId(0), LocalId(1)],
                ret: Type::UNIT,
                locals: vec![param("s", Type::STRING), param("n", Type::I64)],
                body: block(vec![Stmt::Expr(print)]),
            }],
            ..Program::default()
        };
        assert_eq!(
            program.to_string(),
            r#"(fn f ((s#0 string) (n#1 i64)) unit (block (print (interp "a" s#0 n#1))))"#
        );
    }

    #[test]
    fn interpolation_should_print_each_format_with_its_part() {
        // fn f(n: i64, x: f64) { print("a{n:^4}{x:.3}{n:<10.0}") }, as if the checker allowed
        // the last precision.
        let n = || expr(ExprKind::Local(LocalId(0)), Type::I64);
        let parts = vec![
            expr(ExprKind::Str("a".into()), Type::STRING),
            n(),
            expr(ExprKind::Local(LocalId(1)), Type::F64),
            n(),
        ];
        let pad = |align, width| {
            Some(Pad {
                align,
                width,
                by_type: false,
            })
        };
        let formats = vec![
            Format {
                part: 1,
                pad: pad(Align::Center, 4),
                precision: None,
                pretty: false,
            },
            Format {
                part: 2,
                pad: None,
                precision: Some(3),
                pretty: false,
            },
            Format {
                part: 3,
                pad: pad(Align::Left, 10),
                precision: Some(0),
                pretty: false,
            },
        ];
        let interp = expr(ExprKind::Interpolate { parts, formats }, Type::STRING);
        let print = print_of(interp);
        let program = Program {
            classes: vec![],
            enums: vec![],
            functions: vec![Function {
                name: "f".into(),
                owner: None,
                conformance: None,
                method: None,
                type_params: Vec::new(),
                owner_params: 0,
                prelude: false,
                params: vec![LocalId(0), LocalId(1)],
                ret: Type::UNIT,
                locals: vec![param("n", Type::I64), param("x", Type::F64)],
                body: block(vec![Stmt::Expr(print)]),
            }],
            ..Program::default()
        };
        assert_eq!(
            program.to_string(),
            r#"(fn f ((n#0 i64) (x#1 f64)) unit (block (print (interp "a" (fmt n#0 "^4") (fmt x#1 ".3") (fmt n#0 "<10.0")))))"#
        );
    }

    #[test]
    fn chars_bytes_and_string_from_chars_should_print() {
        // fn f(s: string) { let cs = s.chars(); print(s.bytes().len()); print(string(cs)) }
        let s = || Box::new(expr(ExprKind::Local(LocalId(0)), Type::STRING));
        let chars = || Box::new(expr(ExprKind::Local(LocalId(1)), Type::array(Type::CHAR)));
        let bytes = expr(ExprKind::Bytes(s()), Type::array(Type::U8));
        let print = |arg| Stmt::Expr(print_of(arg));
        let program = Program {
            classes: vec![],
            enums: vec![],
            functions: vec![Function {
                name: "f".into(),
                owner: None,
                conformance: None,
                method: None,
                type_params: Vec::new(),
                owner_params: 0,
                prelude: false,
                params: vec![LocalId(0)],
                ret: Type::UNIT,
                locals: vec![
                    param("s", Type::STRING),
                    param("cs", Type::array(Type::CHAR)),
                ],
                body: block(vec![
                    Stmt::Let {
                        local: LocalId(1),
                        init: expr(ExprKind::Chars(s()), Type::array(Type::CHAR)),
                    },
                    print(expr(ExprKind::Len(Box::new(bytes)), Type::I64)),
                    print(expr(ExprKind::FromChars(chars()), Type::STRING)),
                ]),
            }],
            ..Program::default()
        };
        assert_eq!(
            program.to_string(),
            "(fn f ((s#0 string)) unit (block (let cs#1 (chars s#0)) (print (len (bytes s#0))) (print (string cs#1))))"
        );
    }

    #[test]
    fn appends_should_print() {
        // fn f() { let mut names = ["a"]; names[0] += "x" }
        let strings = Type::array(Type::STRING);
        let names = expr(
            ExprKind::ArrayLit(vec![expr(ExprKind::Str("a".into()), Type::STRING)]),
            strings,
        );
        let append = Stmt::Append {
            place: Place::Index {
                base: Box::new(Place::Local(LocalId(0))),
                index: int(0),
            },
            value: expr(ExprKind::Str("x".into()), Type::STRING),
        };
        let program = Program {
            classes: vec![],
            enums: vec![],
            functions: vec![Function {
                name: "f".into(),
                owner: None,
                conformance: None,
                method: None,
                type_params: Vec::new(),
                owner_params: 0,
                prelude: false,
                params: vec![],
                ret: Type::UNIT,
                locals: vec![Local {
                    name: Some("names".into()),
                    ty: strings,
                    mutable: true,
                }],
                body: block(vec![
                    Stmt::Let {
                        local: LocalId(0),
                        init: names,
                    },
                    append,
                ]),
            }],
            ..Program::default()
        };
        assert_eq!(
            program.to_string(),
            r#"(fn f () unit (block (let mut names#0 (array "a")) (append (index names#0 0) "x")))"#
        );
    }

    #[test]
    fn panic_should_print() {
        // fn f() { panic("x") }
        let message = expr(ExprKind::Str("x".into()), Type::STRING);
        let panic = expr(ExprKind::Panic(Box::new(message)), Type::NEVER);
        let program = Program {
            classes: vec![],
            enums: vec![],
            functions: vec![Function {
                name: "f".into(),
                owner: None,
                conformance: None,
                method: None,
                type_params: Vec::new(),
                owner_params: 0,
                prelude: false,
                params: vec![],
                ret: Type::UNIT,
                locals: vec![],
                body: block(vec![Stmt::Expr(panic)]),
            }],
            ..Program::default()
        };
        assert_eq!(program.to_string(), r#"(fn f () unit (block (panic "x")))"#);
    }

    fn block_expr(stmts: Vec<Stmt>, value: Option<Expr>) -> Expr {
        let block = Block {
            stmts,
            value: value.map(Box::new),
            ty: Type::I64,
        };
        expr(ExprKind::Block(block), Type::I64)
    }

    #[test]
    fn types_print_their_source_names() {
        let class = Type::intern(TypeKind::Class(ClassId(3), TypeArgs::NONE));
        let nested = Type::optional(Type::array(Type::array(Type::optional(Type::I64))));
        let param = |name, i| Type::intern(TypeKind::Param(Symbol::intern(name), i));
        for (ty, text) in [
            (Type::I64, "i64"),
            (Type::U8, "u8"),
            (Type::F64, "f64"),
            (Type::BOOL, "bool"),
            (Type::CHAR, "char"),
            (Type::STRING, "string"),
            (Type::UNIT, "unit"),
            (Type::NEVER, "never"),
            (class, "class#3"),
            (nested, "[[i64?]]?"),
            (param("T", 0), "T"),
        ] {
            assert_eq!(ty.to_string(), text);
        }
        // Kept so that a failing test's message names the type, not a table index.
        assert_eq!(format!("{:?}", Type::optional(Type::I64)), "Optional(I64)");
    }

    #[test]
    fn prints_an_enum_type_by_its_id() {
        let ty = Type::optional(Type::intern(TypeKind::Enum(EnumId(2), TypeArgs::NONE)));
        assert_eq!(ty.to_string(), "enum#2?");
    }

    #[test]
    fn type_names_should_write_the_arguments() {
        let t = Type::intern(TypeKind::Param(Symbol::intern("T"), 0));
        let program = Program {
            classes: vec![Class {
                name: "Stack".into(),
                type_params: Vec::new(),
                fields: vec![],
            }],
            enums: vec![Enum {
                name: "Tree".into(),
                span: Span::new(0, 0),
                instance: false,
                type_params: Vec::new(),
                variants: vec![],
                counted: false,
                cycle: None,
            }],
            functions: vec![],
            ..Program::default()
        };
        let args = TypeArgs::intern(&[Type::array(t)]);
        let pair = TypeArgs::intern(&[Type::I64, t]);
        let class = |args| Type::intern(TypeKind::Class(ClassId(0), args));
        let enumeration = |args| Type::intern(TypeKind::Enum(EnumId(0), args));
        for (ty, text) in [
            (class(TypeArgs::NONE), "Stack"),
            (class(args), "Stack<[T]>"),
            (enumeration(TypeArgs::NONE), "Tree"),
            (enumeration(pair), "Tree<i64, T>"),
            (Type::optional(class(args)), "Stack<[T]>?"),
            (t, "T"),
        ] {
            assert_eq!(program.type_name(ty), text);
        }
    }

    /// A type nested deeper than a 64 KiB stack allows recursing into is named all the same, cut
    /// at `NAME_LIMIT` bytes.
    #[test]
    fn deep_types_should_be_cut_in_tir_names() {
        let levels = 10_000;
        let name = std::thread::Builder::new()
            .stack_size(64 * 1024)
            .spawn(move || {
                let mut ty = Type::I64;
                for _ in 0..levels {
                    ty = Type::array(ty);
                }
                let program = Program {
                    classes: Vec::new(),
                    enums: Vec::new(),
                    functions: Vec::new(),
                    ..Program::default()
                };
                program.type_name(ty)
            })
            .expect("the thread should start")
            .join()
            .expect("naming should not overflow the stack");
        let limit = crate::types::NAME_LIMIT;
        assert_eq!(
            name,
            format!("{}...{}", "[".repeat(limit), "]".repeat(limit))
        );
    }

    #[test]
    fn exits_finds_a_jump_at_any_depth() {
        let (if_stmt, looped) = if_and_loop();
        // A `break` in a loop in a block's statement, and in a block's value.
        assert!(block_expr(vec![Stmt::Expr(looped)], None).exits());
        let (_, looped) = if_and_loop();
        assert!(block_expr(vec![], Some(looped)).exits());
        // A `continue` in a `let` initializer, and a `return` in an argument of a call.
        let cont = expr(ExprKind::Continue { depth: 0 }, Type::NEVER);
        let let_cont = Stmt::Let {
            local: LocalId(0),
            init: block_expr(vec![Stmt::Expr(cont)], None),
        };
        assert!(block_expr(vec![let_cont], Some(int(1))).exits());
        let ret = expr(ExprKind::Return(None), Type::NEVER);
        let call = ExprKind::Call {
            func: FuncId(0),
            types: TypeArgs::NONE,
            args: vec![int(1), ret],
        };
        assert!(expr(call, Type::UNIT).exits());
        // A `return` in an argument of a variant.
        let variant = ExprKind::Variant {
            id: EnumId(0),
            variant: 0,
            args: vec![int(1), expr(ExprKind::Return(None), Type::NEVER)],
        };
        assert!(
            expr(
                variant,
                Type::intern(TypeKind::Enum(EnumId(0), TypeArgs::NONE))
            )
            .exits()
        );
        // A `return` in an operand of `is`.
        let is = ExprKind::Is {
            lhs: Box::new(local(0)),
            rhs: Box::new(expr(ExprKind::Return(None), Type::NEVER)),
        };
        assert!(expr(is, Type::BOOL).exits());
        // A panic ends the process: only a `return` in its message exits.
        let panic = |message| expr(ExprKind::Panic(Box::new(message)), Type::NEVER);
        assert!(!panic(expr(ExprKind::Str("x".into()), Type::STRING)).exits());
        assert!(panic(expr(ExprKind::Return(None), Type::NEVER)).exits());
        // A `return` in the string that `words()` splits.
        let words = |base| expr(ExprKind::Words(Box::new(base)), Type::array(Type::STRING));
        assert!(words(expr(ExprKind::Return(None), Type::NEVER)).exits());
        assert!(!words(local(0)).exits());
        // An `if`, a comparison chain and a print with no jump in them.
        assert!(!if_stmt.exits());
        assert!(!block_expr(vec![Stmt::Expr(if_stmt)], Some(int(1))).exits());
    }

    /// `each_child_mut` reaches each expression directly inside every kind, in order: through
    /// its blocks, statements and places, and no further.
    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one row per node kind, so the table reads in one place"
    )]
    fn each_child_mut_should_reach_every_direct_child() {
        let n = |k| Box::new(int(k));
        let ints = |ks: [i64; 2]| ks.map(int).into();
        let valued = |k| Block {
            stmts: Vec::new(),
            value: Some(n(k)),
            ty: Type::I64,
        };
        let arm = |guard, k| Arm {
            pattern: Pattern {
                kind: PatternKind::Wild,
                span: Span::new(0, 0),
            },
            bindings: Vec::new(),
            guard,
            body: valued(k),
            reachable: true,
        };
        let place = Place::Index {
            base: Box::new(Place::Field {
                base: int(3),
                index: 0,
            }),
            index: int(4),
        };
        let body = Block {
            stmts: vec![
                Stmt::Let {
                    local: LocalId(0),
                    init: int(1),
                },
                Stmt::Expr(int(2)),
                Stmt::Assign {
                    place,
                    value: int(5),
                },
                Stmt::Append {
                    place: Place::Local(LocalId(0)),
                    value: int(6),
                },
            ],
            value: Some(n(7)),
            ty: Type::I64,
        };
        let cases = [
            (ExprKind::Return(None), 0),
            (
                ExprKind::Call {
                    func: FuncId(0),
                    types: TypeArgs::NONE,
                    args: ints([1, 2]),
                },
                2,
            ),
            (
                ExprKind::InterfaceCall {
                    interface: InterfaceId(0),
                    iargs: TypeArgs::NONE,
                    method: 0,
                    types: TypeArgs::NONE,
                    args: ints([1, 2]),
                },
                2,
            ),
            (
                ExprKind::New {
                    class: ClassId(0),
                    args: ints([1, 2]),
                },
                2,
            ),
            (
                ExprKind::Variant {
                    id: EnumId(0),
                    variant: 0,
                    args: ints([1, 2]),
                },
                2,
            ),
            (
                ExprKind::Compare {
                    operands: ints([1, 2]),
                    ops: vec![CompareOp::Lt],
                },
                2,
            ),
            (
                ExprKind::Math {
                    op: MathOp::Abs,
                    operands: ints([1, 2]),
                },
                2,
            ),
            (ExprKind::ArrayLit(ints([1, 2])), 2),
            (
                ExprKind::Interpolate {
                    parts: ints([1, 2]),
                    formats: Vec::new(),
                },
                2,
            ),
            (
                ExprKind::Field {
                    base: n(1),
                    index: 0,
                },
                1,
            ),
            (ExprKind::Len(n(1)), 1),
            (ExprKind::Chars(n(1)), 1),
            (ExprKind::Bytes(n(1)), 1),
            (ExprKind::Words(n(1)), 1),
            (ExprKind::FromChars(n(1)), 1),
            (ExprKind::Wrap(n(1)), 1),
            (ExprKind::Convert(n(1)), 1),
            (
                ExprKind::Print {
                    stream: Stream::Stdout,
                    operand: n(1),
                },
                1,
            ),
            (
                ExprKind::Dbg {
                    at: Span::new(0, 0),
                    text: "x".to_string(),
                    operand: n(1),
                },
                1,
            ),
            (ExprKind::Panic(n(1)), 1),
            (
                ExprKind::Unary {
                    op: UnaryOp::Neg,
                    operand: n(1),
                },
                1,
            ),
            (
                ExprKind::Break {
                    depth: 0,
                    value: Some(n(1)),
                },
                1,
            ),
            (ExprKind::Return(Some(n(1))), 1),
            (
                ExprKind::Binary {
                    op: BinaryOp::Add,
                    lhs: n(1),
                    rhs: n(2),
                },
                2,
            ),
            // `2` is a grandchild, under the `-`, which `f` would have to reach itself.
            (
                ExprKind::Binary {
                    op: BinaryOp::Add,
                    lhs: n(1),
                    rhs: Box::new(expr(
                        ExprKind::Unary {
                            op: UnaryOp::Neg,
                            operand: n(2),
                        },
                        Type::I64,
                    )),
                },
                1,
            ),
            (
                ExprKind::Concat {
                    lhs: n(1),
                    rhs: n(2),
                },
                2,
            ),
            (
                ExprKind::Equal {
                    negated: false,
                    lhs: n(1),
                    rhs: n(2),
                },
                2,
            ),
            (
                ExprKind::Is {
                    lhs: n(1),
                    rhs: n(2),
                },
                2,
            ),
            (
                ExprKind::ArrayFill {
                    value: n(1),
                    len: n(2),
                },
                2,
            ),
            (
                ExprKind::Index {
                    base: n(1),
                    index: n(2),
                },
                2,
            ),
            (
                ExprKind::If {
                    cond: n(1),
                    then_block: valued(2),
                    else_block: Some(valued(3)),
                },
                3,
            ),
            (
                ExprKind::IfSome {
                    bind: LocalId(0),
                    scrutinee: n(1),
                    then_block: valued(2),
                    else_block: Some(valued(3)),
                },
                3,
            ),
            (
                ExprKind::Match {
                    scrutinee: n(1),
                    temp: LocalId(0),
                    arms: vec![arm(Some(int(2)), 3), arm(None, 4)],
                },
                4,
            ),
            (ExprKind::Loop(body), 7),
            (ExprKind::Block(valued(1)), 1),
        ];
        for (kind, children) in cases {
            let mut e = expr(kind, Type::I64);
            let mut seen = Vec::new();
            e.each_child_mut(&mut |child| {
                if let ExprKind::Int(k) = child.kind {
                    seen.push(k);
                }
            });
            assert_eq!(seen, (1..=children).collect::<Vec<_>>(), "{:?}", e.kind);
        }
    }

    /// A type is written without its program, so an associated type shows its base and its place.
    #[test]
    fn display_should_write_an_associated_type_through_its_base() {
        let t = Type::intern(TypeKind::Param(Symbol::intern("T"), 0));
        let item = Type::intern(TypeKind::Assoc(t, InterfaceId(0), TypeArgs::NONE, 3));
        assert_eq!(item.to_string(), "T.assoc#3");
        assert_eq!(
            Type::array(Type::optional(item)).to_string(),
            "[T.assoc#3?]"
        );
    }
}
