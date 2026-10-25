//! The syntax tree, and its S-expression printer used by `--emit ast` and the parser tests.

use std::fmt;

use crate::lexer::Float;
use crate::source::Span;
use crate::symbol::Symbol;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Program {
    pub(crate) items: Vec<Item>,
    /// The named items that hold a syntax error, which the checker treats as declared.
    pub(crate) broken: Vec<Broken>,
    /// Where each comment starts, so that a fix does not delete one as if it were safe.
    pub(crate) comments: Vec<usize>,
    /// Where each string literal starts, at its opening quote, so that a fix can tell one from
    /// a literal in parentheses, whose span starts at the `(`.
    pub(crate) quotes: Vec<usize>,
    /// Where each line break is, so that a fix does not wrap one in a string literal.
    pub(crate) breaks: Vec<usize>,
}

/// A function, class, enum, constant, interface or `extend` block that holds a syntax error, so
/// only its name survives. An `extend` block is named by its target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Broken {
    pub(crate) name: Ident,
    pub(crate) kind: BrokenKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrokenKind {
    Function,
    Class,
    Enum,
    Constant,
    Interface,
    Extend,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Item {
    Function(Function),
    Class(Class),
    Enum(Enum),
    Const(Const),
    Interface(Interface),
    Extend(Extend),
}

/// A type parameter and its bounds: `T`, or `T: Shape & Same`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TypeParam {
    pub(crate) name: Ident,
    /// The interfaces after the `:`, as types; empty when none is written.
    pub(crate) bounds: Vec<TypeExpr>,
}

/// `type Item = i64`, the choice of an associated type in a class, an enum or an `extend` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AssocChoice {
    pub(crate) name: Ident,
    pub(crate) ty: TypeExpr,
}

/// `type Item` or `type Item: Same & Named`, an associated type of an interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AssocType {
    pub(crate) name: Ident,
    pub(crate) bounds: Vec<TypeExpr>,
}

/// `interface Name<T>: Inherited { type Item; fn name(self) -> T }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Interface {
    pub(crate) name: Ident,
    /// `<T, U>` after the name; empty when the interface is not generic.
    pub(crate) type_params: Vec<TypeParam>,
    /// The interfaces after the `:`, as types.
    pub(crate) inherits: Vec<TypeExpr>,
    pub(crate) assoc: Vec<AssocType>,
    pub(crate) methods: Vec<Requirement>,
    /// The names of the requirements that hold a syntax error; the interface itself is intact.
    pub(crate) broken_methods: Vec<Ident>,
    /// `interface` through `}`.
    pub(crate) span: Span,
}

/// `extend<T> Target: Interface { type Item = i64; fn name(self) {} }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Extend {
    /// `<T, U>` after `extend`; empty when there are none.
    pub(crate) type_params: Vec<TypeParam>,
    /// The type the block extends, as written; the checker says whether it can be.
    pub(crate) target: TypeExpr,
    /// The type after the `:`, as written.
    pub(crate) interface: Option<TypeExpr>,
    pub(crate) assoc: Vec<AssocChoice>,
    pub(crate) methods: Vec<Function>,
    /// The names of the methods that hold a syntax error; the block itself is intact.
    pub(crate) broken_methods: Vec<Ident>,
    /// `extend` through `}`.
    pub(crate) span: Span,
}

/// `const name: ty = value`, at the top level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Const {
    pub(crate) name: Ident,
    pub(crate) ty: Option<TypeExpr>,
    pub(crate) value: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Class {
    pub(crate) name: Ident,
    /// `<T, U>` after the name; empty when the class is not generic.
    pub(crate) type_params: Vec<TypeParam>,
    /// The interfaces after the `:`, as types.
    pub(crate) conforms: Vec<TypeExpr>,
    pub(crate) assoc: Vec<AssocChoice>,
    pub(crate) members: Vec<Member>,
    /// The names of the methods that hold a syntax error; the class itself is intact.
    pub(crate) broken_methods: Vec<Ident>,
}

/// `enum Name { variants, then methods }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Enum {
    pub(crate) name: Ident,
    /// `<T, U>` after the name; empty when the enum is not generic.
    pub(crate) type_params: Vec<TypeParam>,
    /// The interfaces after the `:`, as types.
    pub(crate) conforms: Vec<TypeExpr>,
    pub(crate) assoc: Vec<AssocChoice>,
    pub(crate) variants: Vec<Variant>,
    pub(crate) methods: Vec<Function>,
    /// The names of the methods that hold a syntax error; the enum itself is intact.
    pub(crate) broken_methods: Vec<Ident>,
}

/// A variant and its payload, if it has parentheses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Variant {
    pub(crate) name: Ident,
    pub(crate) payload: Option<Vec<PayloadField>>,
}

/// One field of a variant's payload, with its label if it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PayloadField {
    pub(crate) label: Option<Ident>,
    pub(crate) ty: TypeExpr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Member {
    Field(FieldDecl),
    Method(Function),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FieldDecl {
    pub(crate) mutable: bool,
    pub(crate) name: Ident,
    pub(crate) ty: TypeExpr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Function {
    pub(crate) name: Ident,
    /// `<T, U>` after the name; empty when the function is not generic.
    pub(crate) type_params: Vec<TypeParam>,
    /// `<` through `>` of `type_params`; empty, after the name, when there are none.
    pub(crate) angles: Span,
    /// The `self` token of a method's first parameter.
    pub(crate) self_param: Option<Span>,
    /// The `mut self` of a method of an `extend` block, which the checker reports once it knows
    /// whether the target is a class or an enum. A declaration's parser reports it at once.
    pub(crate) mut_self: Option<MutSelf>,
    pub(crate) params: Vec<Param>,
    pub(crate) ret: Option<TypeExpr>,
    pub(crate) body: Block,
    /// `fn` through `}`.
    pub(crate) span: Span,
}

/// A `mut` in front of a method's `self`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MutSelf {
    pub(crate) keyword: Span,
    pub(crate) receiver: Span,
    /// The `: Type` after `self`, if written. Removing the `mut` is a safe fix only without it.
    pub(crate) annotation: Option<Span>,
}

/// A method of an interface: a signature with no body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Requirement {
    pub(crate) name: Ident,
    /// `<T, U>` after the name; empty when the requirement is not generic.
    pub(crate) type_params: Vec<TypeParam>,
    /// `<` through `>` of `type_params`; empty, after the name, when there are none.
    pub(crate) angles: Span,
    pub(crate) self_param: Span,
    pub(crate) params: Vec<Param>,
    pub(crate) ret: Option<TypeExpr>,
    /// `fn` through the return type.
    pub(crate) span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ident {
    pub(crate) name: Symbol,
    pub(crate) span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Param {
    pub(crate) name: Ident,
    pub(crate) ty: TypeExpr,
}

/// A type as written, and how many `?` follow it: each is one optional layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TypeExpr {
    pub(crate) kind: TypeKind,
    pub(crate) optional: u16,
    /// The name, or `[` through `]`, then through the last `?`, if any.
    pub(crate) span: Span,
    /// The type without its `?`: `span` when `optional` is 0. A line break may lie before a `?`.
    pub(crate) bare: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TypeKind {
    Name(Symbol),
    /// `Name<A, B>`, boxed as rare, so a plain name's `TypeExpr` stays small.
    Applied(Box<Applied>),
    /// `[T]`; the element type may itself be optional or an array.
    Array(Box<TypeExpr>),
    /// `Self`.
    SelfType,
    /// `Base.Name`, an associated type, as in `S.Item`; `Base` may be another one.
    Member {
        base: Box<TypeExpr>,
        name: Ident,
    },
}

/// `Name<A, B>`; at least one argument. `angles` is `<` through `>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Applied {
    pub(crate) name: Symbol,
    pub(crate) args: Vec<TypeExpr>,
    pub(crate) angles: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Block {
    pub(crate) stmts: Vec<Stmt>,
    /// `{` through `}`.
    pub(crate) span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stmt {
    pub(crate) kind: StmtKind,
    pub(crate) span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StmtKind {
    Let {
        /// The `mut` keyword of a `let mut`.
        mutable: Option<Span>,
        name: Ident,
        ty: Option<TypeExpr>,
        init: Expr,
    },
    /// `while cond { body }`; `keyword` is the `while` token's span. Each loop's `label` is its
    /// loop label.
    While {
        label: Option<Ident>,
        keyword: Span,
        cond: Expr,
        body: Block,
    },
    /// `while let pattern = value { body }`.
    WhileLet {
        label: Option<Ident>,
        keyword: Span,
        pattern: Pattern,
        value: Expr,
        body: Block,
    },
    /// `for name in source { body }`.
    For {
        label: Option<Ident>,
        keyword: Span,
        name: Ident,
        source: ForSource,
        body: Block,
    },
    Return(Option<Expr>),
    /// `break`, or `break value`, which only a `loop` takes.
    Break {
        label: Option<Ident>,
        value: Option<Expr>,
    },
    Continue {
        label: Option<Ident>,
    },
    /// `target` is a `Name` or a `Field`.
    Assign {
        target: Expr,
        value: Expr,
    },
    /// `target op= value`; the checker lowers a name to `target = target op value`, and a
    /// field `a.f` to `Block { let t = a; Assign { t.f, t.f op value } }`.
    /// `op_span` is the `op=` token's.
    CompoundAssign {
        op: BinaryOp,
        op_span: Span,
        target: Expr,
        value: Expr,
    },
    Expr(Expr),
}

/// What a `for` walks; a range belongs to the `for` and is not an expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ForSource {
    Array(Expr),
    /// `start..end`, or `start..=end` when `inclusive`.
    Range {
        start: Expr,
        end: Expr,
        inclusive: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Expr {
    pub(crate) kind: ExprKind,
    pub(crate) span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExprKind {
    /// An integer literal: its value, and the span of its token, whose text as written a
    /// diagnostic quotes. The expression's span can be wider: a parenthesized literal's starts at
    /// `(`, and `-9223372036854775808`'s at the `-`.
    Int {
        value: i64,
        literal: Span,
    },
    /// A float literal.
    Float(Float),
    Bool(bool),
    /// A `char` literal, decoded.
    Char(char),
    /// A string literal, or a text piece of an interpolation, decoded.
    Str(String),
    /// A string literal with interpolations: its text pieces as `Str` parts, without empty ones,
    /// and its expressions, in order.
    Interpolate(Vec<Part>),
    Name(Symbol),
    /// `types` are the type arguments of `f<i64>(x)`; empty when none is written. `angles` is
    /// `<` through `>`, or empty after the callee.
    Call {
        callee: Ident,
        types: Vec<TypeExpr>,
        angles: Span,
        args: Vec<Arg>,
    },
    /// `Tree<i64>` before a `.`, as in `Tree<i64>.Leaf`; `angles` is `<` through `>`.
    Applied {
        name: Ident,
        types: Vec<TypeExpr>,
        angles: Span,
    },
    /// `todo` or `todo(args)`; bare `todo` has no args. `keyword` is the `todo` token's span,
    /// which parentheses around the expression do not widen.
    Todo {
        keyword: Span,
        args: Vec<Arg>,
    },
    SelfValue,
    NoneLit,
    /// `safe` is `base?.name`.
    Field {
        base: Box<Expr>,
        name: Ident,
        safe: bool,
    },
    /// `safe` is `receiver?.method(args)`; `parens` is the span of `(args)`; `types` are the
    /// type arguments of `receiver.method<i64>(args)`, empty when none is written, and `angles`
    /// is `<` through `>`, or empty after the method.
    MethodCall {
        receiver: Box<Expr>,
        method: Ident,
        types: Vec<TypeExpr>,
        angles: Span,
        args: Vec<Arg>,
        parens: Span,
        safe: bool,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    /// `op_span` is the operator token's.
    Binary {
        op: BinaryOp,
        op_span: Span,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// A chain `a < b <= c`; `operands.len() == ops.len() + 1`.
    Compare {
        operands: Vec<Expr>,
        ops: Vec<CompareOp>,
    },
    /// `lhs ?? rhs`.
    Coalesce {
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
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
    /// `lhs is rhs`.
    Is {
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// `else if` is an `else_block` holding one `Expr` statement with the inner `if`.
    If {
        cond: Box<Expr>,
        then_block: Block,
        else_block: Option<Block>,
    },
    /// `if let pattern = value { then } else { else }`; `else if` and `else if let` fold into
    /// `else_block` as for `If`.
    IfLet {
        pattern: Pattern,
        value: Box<Expr>,
        then_block: Block,
        else_block: Option<Block>,
    },
    /// `match scrutinee { arms }`. `keyword` is the `match` token's span, which parentheses
    /// around the expression do not widen, and `close` its `}`'s.
    Match {
        keyword: Span,
        scrutinee: Box<Expr>,
        arms: Vec<Arm>,
        close: Span,
    },
    /// `loop { body }`; `keyword` is the `loop` token's span.
    Loop {
        label: Option<Ident>,
        keyword: Span,
        body: Block,
    },
    /// `{ ... }` where an expression belongs: its value is its last expression.
    Block(Block),
}

/// One line of a `match`: `pattern ["if" guard] => body`. The guard covers every alternative of
/// the pattern. The parser wraps a body that is a statement (an assignment, `return`, `break` or
/// `continue`) in a block holding it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Arm {
    pub(crate) pattern: Pattern,
    pub(crate) guard: Option<Expr>,
    pub(crate) body: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pattern {
    pub(crate) kind: PatternKind,
    pub(crate) span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PatternKind {
    /// `_`.
    Wild,
    /// A bare name: a binding, or a variant of the enum expected there; the checker decides.
    Name(Ident),
    /// `None`.
    None,
    /// `Num(p, ...)`, `Expr.Num(p, ...)` or `Expr.Idle`. `parens` is the span of `(` through
    /// `)`, so that `Idle()`, which has `fields: []`, differs from `Expr.Idle`, which has none.
    Variant {
        qualifier: Option<Ident>,
        name: Ident,
        fields: Vec<Pattern>,
        parens: Option<Span>,
    },
    /// `0`, `-1`, `'a'`, `"yes"`, `true`.
    Literal(Literal),
    /// `start..end`, or `start..=end` when `inclusive`.
    Range {
        start: Bound,
        end: Bound,
        inclusive: bool,
    },
    /// `p | q | ...`: at least two alternatives, none of them an `Or`.
    Or(Vec<Pattern>),
}

/// A literal in a pattern, or at the end of a range pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Literal {
    /// `value` is signed, so `-5` holds `-5`. `text` is the span of the digits as written, which
    /// a diagnostic quotes, and `negative` the span of the `-` before them, when there is one:
    /// `-0` is negative with value `0`.
    Int {
        value: i64,
        text: Span,
        negative: Option<Span>,
    },
    Char(char),
    Str(String),
    Bool(bool),
}

/// One end of a range pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Bound {
    Literal(Literal),
    /// A bare name, which the checker reports: no range takes a binding.
    Name(Ident),
}

/// A part of an interpolation: a text piece, or an expression with the format spec it may have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Part {
    pub(crate) value: Expr,
    pub(crate) spec: Option<FormatSpec>,
}

/// A format spec, `[<^>][width][.precision]`, and its span, from its `:`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FormatSpec {
    pub(crate) align: Option<Align>,
    pub(crate) width: Option<u16>,
    pub(crate) precision: Option<u8>,
    pub(crate) span: Span,
}

/// A format spec's alignment: `<`, `>` or `^`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Align {
    Left,
    Right,
    Center,
}

impl Align {
    /// The character that writes it.
    pub(crate) fn char(self) -> char {
        match self {
            Align::Left => '<',
            Align::Right => '>',
            Align::Center => '^',
        }
    }
}

impl fmt::Display for Part {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some(spec) = self.spec else {
            return self.value.fmt(f);
        };
        write!(f, "(fmt {} \"", self.value)?;
        if let Some(align) = spec.align {
            write!(f, "{}", align.char())?;
        }
        if let Some(width) = spec.width {
            write!(f, "{width}")?;
        }
        if let Some(precision) = spec.precision {
            write!(f, ".{precision}")?;
        }
        f.write_str("\")")
    }
}

/// A call argument; the checker decides where a label belongs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Arg {
    pub(crate) label: Option<Ident>,
    pub(crate) value: Expr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    And,
    Or,
    Eq,
    Ne,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompareOp {
    Lt,
    Le,
    Gt,
    Ge,
}

impl CompareOp {
    /// `<` and `<=` run one way, `>` and `>=` the other.
    pub(crate) fn is_less(self) -> bool {
        matches!(self, CompareOp::Lt | CompareOp::Le)
    }
}

impl fmt::Display for UnaryOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            UnaryOp::Neg => "-",
            UnaryOp::Not => "!",
        })
    }
}

impl fmt::Display for BinaryOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::Rem => "%",
            BinaryOp::And => "&&",
            BinaryOp::Or => "||",
            BinaryOp::Eq => "==",
            BinaryOp::Ne => "!=",
        })
    }
}

impl fmt::Display for CompareOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CompareOp::Lt => "<",
            CompareOp::Le => "<=",
            CompareOp::Gt => ">",
            CompareOp::Ge => ">=",
        })
    }
}

impl fmt::Display for TypeExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            TypeKind::Name(name) => f.write_str(name.as_str())?,
            TypeKind::Applied(applied) => write!(f, "{}{}", applied.name, angled(&applied.args))?,
            TypeKind::Array(element) => write!(f, "[{element}]")?,
            TypeKind::SelfType => f.write_str("Self")?,
            TypeKind::Member { base, name } => write!(f, "{base}.{}", name.name)?,
        }
        for _ in 0..self.optional {
            f.write_str("?")?;
        }
        Ok(())
    }
}

/// Writes each item after a space.
fn spaced<T: fmt::Display>(f: &mut fmt::Formatter<'_>, items: &[T]) -> fmt::Result {
    items.iter().try_for_each(|item| write!(f, " {item}"))
}

/// `<a, b>` to write after a name, or nothing for an empty list.
pub(crate) struct Angled<'a, T>(&'a [T]);

impl<T: fmt::Display> fmt::Display for Angled<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some((first, rest)) = self.0.split_first() else {
            return Ok(());
        };
        write!(f, "<{first}")?;
        for item in rest {
            write!(f, ", {item}")?;
        }
        f.write_str(">")
    }
}

pub(crate) fn angled<T>(items: &[T]) -> Angled<'_, T> {
    Angled(items)
}

/// `: A & B` after a name, or nothing for an empty list.
struct Bounds<'a>(&'a [TypeExpr]);

impl fmt::Display for Bounds<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some((first, rest)) = self.0.split_first() else {
            return Ok(());
        };
        write!(f, ": {first}")?;
        rest.iter().try_for_each(|bound| write!(f, " & {bound}"))
    }
}

/// ` (: A B)` after a name, or nothing for an empty list: the interfaces a declaration lists.
struct Listed<'a>(&'a [TypeExpr]);

impl fmt::Display for Listed<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return Ok(());
        }
        f.write_str(" (:")?;
        spaced(f, self.0)?;
        f.write_str(")")
    }
}

impl fmt::Display for TypeParam {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.name.name, Bounds(&self.bounds))
    }
}

impl fmt::Display for AssocChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(type {} {})", self.name.name, self.ty)
    }
}

impl fmt::Display for AssocType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(type {}{})", self.name.name, Bounds(&self.bounds))
    }
}

impl fmt::Display for Requirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "(fn {}{} (self",
            self.name.name,
            angled(&self.type_params)
        )?;
        for p in &self.params {
            write!(f, " ({} {})", p.name.name, p.ty)?;
        }
        let ret = self
            .ret
            .as_ref()
            .map_or("unit".to_string(), ToString::to_string);
        write!(f, ") {ret})")
    }
}

impl fmt::Display for Interface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "(interface {}{}{}",
            self.name.name,
            angled(&self.type_params),
            Listed(&self.inherits)
        )?;
        spaced(f, &self.assoc)?;
        spaced(f, &self.methods)?;
        f.write_str(")")
    }
}

impl fmt::Display for Extend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(extend{} {}", angled(&self.type_params), self.target)?;
        if let Some(interface) = &self.interface {
            write!(f, ": {interface}")?;
        }
        spaced(f, &self.assoc)?;
        spaced(f, &self.methods)?;
        f.write_str(")")
    }
}

impl fmt::Display for Ident {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name.as_str())
    }
}

impl fmt::Display for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, item) in self.items.iter().enumerate() {
            if i > 0 {
                f.write_str("\n")?;
            }
            match item {
                Item::Function(function) => write!(f, "{function}")?,
                Item::Class(class) => write!(f, "{class}")?,
                Item::Enum(e) => write!(f, "{e}")?,
                Item::Interface(interface) => write!(f, "{interface}")?,
                Item::Extend(extend) => write!(f, "{extend}")?,
                Item::Const(c) => {
                    write!(f, "(const {}", c.name.name)?;
                    if let Some(ty) = &c.ty {
                        write!(f, " {ty}")?;
                    }
                    write!(f, " {})", c.value)?;
                }
            }
        }
        Ok(())
    }
}

impl fmt::Display for Class {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "(class {}{}{}",
            self.name.name,
            angled(&self.type_params),
            Listed(&self.conforms)
        )?;
        spaced(f, &self.assoc)?;
        for member in &self.members {
            match member {
                Member::Field(field) => {
                    let keyword = if field.mutable { "mut " } else { "" };
                    write!(f, " ({keyword}{} {})", field.name.name, field.ty)?;
                }
                Member::Method(method) => write!(f, " {method}")?,
            }
        }
        f.write_str(")")
    }
}

impl fmt::Display for Enum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "(enum {}{}{}",
            self.name.name,
            angled(&self.type_params),
            Listed(&self.conforms)
        )?;
        spaced(f, &self.assoc)?;
        for variant in &self.variants {
            match &variant.payload {
                None => write!(f, " {}", variant.name.name)?,
                Some(fields) => {
                    write!(f, " ({}", variant.name.name)?;
                    for field in fields {
                        match &field.label {
                            Some(label) => write!(f, " ({} {})", label.name, field.ty)?,
                            None => write!(f, " {}", field.ty)?,
                        }
                    }
                    f.write_str(")")?;
                }
            }
        }
        for method in &self.methods {
            write!(f, " {method}")?;
        }
        f.write_str(")")
    }
}

impl fmt::Display for Arm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(arm {}", self.pattern)?;
        if let Some(guard) = &self.guard {
            write!(f, " (if {guard})")?;
        }
        write!(f, " {})", self.body)
    }
}

impl fmt::Display for Literal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Literal::Int {
                value, negative, ..
            } => {
                let sign = if negative.is_some() { "-" } else { "" };
                write!(f, "{sign}{}", value.unsigned_abs())
            }
            Literal::Char(c) => write!(f, "{c:?}"),
            Literal::Str(s) => write!(f, "{s:?}"),
            Literal::Bool(b) => write!(f, "{b}"),
        }
    }
}

impl fmt::Display for Bound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Bound::Literal(literal) => literal.fmt(f),
            Bound::Name(name) => f.write_str(name.name.as_str()),
        }
    }
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            PatternKind::Wild => f.write_str("_"),
            PatternKind::Name(name) => f.write_str(name.name.as_str()),
            PatternKind::None => f.write_str("None"),
            PatternKind::Variant {
                qualifier,
                name,
                fields,
                parens,
            } => {
                let path = match qualifier {
                    Some(qualifier) => format!("{}.{}", qualifier.name, name.name),
                    None => name.name.to_string(),
                };
                // Without parentheses, a qualified variant has no payload to show.
                if parens.is_none() {
                    return f.write_str(&path);
                }
                write!(f, "({path}")?;
                if fields.is_empty() {
                    f.write_str(" ()")?;
                }
                spaced(f, fields)?;
                f.write_str(")")
            }
            PatternKind::Literal(literal) => literal.fmt(f),
            PatternKind::Range {
                start,
                end,
                inclusive,
            } => {
                let op = if *inclusive { "..=" } else { ".." };
                write!(f, "({op} {start} {end})")
            }
            PatternKind::Or(alternatives) => {
                f.write_str("(|")?;
                spaced(f, alternatives)?;
                f.write_str(")")
            }
        }
    }
}

impl fmt::Display for Arg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.label {
            Some(label) => write!(f, "(: {} {})", label.name, self.value),
            None => write!(f, "{}", self.value),
        }
    }
}

impl fmt::Display for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(fn {}{} (", self.name.name, angled(&self.type_params))?;
        let params = self
            .params
            .iter()
            .map(|p| format!("({} {})", p.name.name, p.ty));
        let all: Vec<String> = self
            .self_param
            .map(|_| "self".to_string())
            .into_iter()
            .chain(params)
            .collect();
        f.write_str(&all.join(" "))?;
        let ret = self
            .ret
            .as_ref()
            .map_or("unit".to_string(), ToString::to_string);
        write!(f, ") {ret} {})", self.body)
    }
}

impl fmt::Display for Block {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("(block")?;
        spaced(f, &self.stmts)?;
        f.write_str(")")
    }
}

impl fmt::Display for Stmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            StmtKind::Let {
                mutable,
                name,
                ty,
                init,
            } => {
                let keyword = if mutable.is_some() { "let mut" } else { "let" };
                write!(f, "({keyword} {}", name.name)?;
                if let Some(ty) = ty {
                    write!(f, " {ty}")?;
                }
                write!(f, " {init})")
            }
            StmtKind::While {
                label, cond, body, ..
            } => write!(f, "(while{} {cond} {body})", Label(label)),
            StmtKind::WhileLet {
                label,
                pattern,
                value,
                body,
                ..
            } => write!(f, "(while-let{} {pattern} {value} {body})", Label(label)),
            StmtKind::For {
                label,
                name,
                source,
                body,
                ..
            } => {
                write!(f, "(for{} {} ", Label(label), name.name)?;
                match source {
                    ForSource::Array(e) => write!(f, "{e} {body})"),
                    ForSource::Range {
                        start,
                        end,
                        inclusive,
                    } => {
                        let op = if *inclusive { "..=" } else { ".." };
                        write!(f, "({op} {start} {end}) {body})")
                    }
                }
            }
            StmtKind::Return(None) => f.write_str("(return)"),
            StmtKind::Return(Some(e)) => write!(f, "(return {e})"),
            StmtKind::Break { label, value: None } => write!(f, "(break{})", Label(label)),
            StmtKind::Break {
                label,
                value: Some(value),
            } => write!(f, "(break{} {value})", Label(label)),
            StmtKind::Continue { label } => write!(f, "(continue{})", Label(label)),
            StmtKind::Assign { target, value } => write!(f, "(= {target} {value})"),
            StmtKind::CompoundAssign {
                op, target, value, ..
            } => {
                write!(f, "({op}= {target} {value})")
            }
            StmtKind::Expr(e) => write!(f, "{e}"),
        }
    }
}

/// A loop label as the tree prints it after its loop's keyword or its jump's: ` :name`.
struct Label<'a>(&'a Option<Ident>);

impl fmt::Display for Label<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(label) => write!(f, " :{}", label.name),
            None => Ok(()),
        }
    }
}

/// The access operator as the tree prints it.
pub(crate) fn dot(safe: bool) -> &'static str {
    if safe { "?." } else { "." }
}

impl fmt::Display for Expr {
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per node kind, so the printed form reads in one place"
    )]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ExprKind::Int { value, .. } => write!(f, "{value}"),
            ExprKind::Float(x) => write!(f, "{x}"),
            ExprKind::Bool(b) => write!(f, "{b}"),
            ExprKind::Char(c) => write!(f, "{c:?}"),
            ExprKind::Str(s) => write!(f, "{s:?}"),
            ExprKind::Interpolate(parts) => {
                f.write_str("(interp")?;
                spaced(f, parts)?;
                f.write_str(")")
            }
            ExprKind::Name(name) => f.write_str(name.as_str()),
            ExprKind::Call {
                callee,
                types,
                args,
                ..
            } => {
                write!(f, "(call {}{}", callee.name, angled(types))?;
                spaced(f, args)?;
                f.write_str(")")
            }
            ExprKind::Applied { name, types, .. } => write!(f, "{}{}", name.name, angled(types)),
            ExprKind::Todo { args, .. } => {
                f.write_str("(todo")?;
                spaced(f, args)?;
                f.write_str(")")
            }
            ExprKind::SelfValue => f.write_str("self"),
            ExprKind::NoneLit => f.write_str("None"),
            ExprKind::Field { base, name, safe } => {
                write!(f, "({} {base} {})", dot(*safe), name.name)
            }
            ExprKind::MethodCall {
                receiver,
                method,
                types,
                args,
                safe,
                ..
            } => {
                write!(
                    f,
                    "(call ({} {receiver} {}{})",
                    dot(*safe),
                    method.name,
                    angled(types)
                )?;
                spaced(f, args)?;
                f.write_str(")")
            }
            ExprKind::Unary { op, operand } => write!(f, "({op} {operand})"),
            ExprKind::Binary { op, lhs, rhs, .. } => write!(f, "({op} {lhs} {rhs})"),
            ExprKind::Coalesce { lhs, rhs } => write!(f, "(?? {lhs} {rhs})"),
            ExprKind::Is { lhs, rhs } => write!(f, "(is {lhs} {rhs})"),
            ExprKind::Compare { operands, ops } => {
                // `(< a b <= c)`: the first operator leads, the rest sit between operands.
                write!(f, "({} {} {}", ops[0], operands[0], operands[1])?;
                for (op, operand) in ops[1..].iter().zip(&operands[2..]) {
                    write!(f, " {op} {operand}")?;
                }
                f.write_str(")")
            }
            ExprKind::ArrayLit(elements) => {
                f.write_str("(array")?;
                spaced(f, elements)?;
                f.write_str(")")
            }
            ExprKind::ArrayFill { value, len } => write!(f, "(array-fill {value} {len})"),
            ExprKind::Index { base, index } => write!(f, "(index {base} {index})"),
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                write!(f, "(if {cond} {then_block}")?;
                if let Some(else_block) = else_block {
                    write!(f, " {else_block}")?;
                }
                f.write_str(")")
            }
            ExprKind::IfLet {
                pattern,
                value,
                then_block,
                else_block,
            } => {
                write!(f, "(if-let {pattern} {value} {then_block}")?;
                if let Some(else_block) = else_block {
                    write!(f, " {else_block}")?;
                }
                f.write_str(")")
            }
            ExprKind::Match {
                scrutinee, arms, ..
            } => {
                write!(f, "(match {scrutinee}")?;
                spaced(f, arms)?;
                f.write_str(")")
            }
            ExprKind::Loop { label, body, .. } => write!(f, "(loop{} {body})", Label(label)),
            ExprKind::Block(block) => write!(f, "{block}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp() -> Span {
        Span::new(0, 0)
    }

    fn ident(name: &str) -> Ident {
        Ident {
            name: Symbol::intern(name),
            span: sp(),
        }
    }

    fn ty(name: &str) -> TypeExpr {
        TypeExpr {
            kind: TypeKind::Name(Symbol::intern(name)),
            optional: 0,
            span: sp(),
            bare: sp(),
        }
    }

    fn expr(kind: ExprKind) -> Expr {
        Expr { kind, span: sp() }
    }

    fn name(n: &str) -> Expr {
        expr(ExprKind::Name(Symbol::intern(n)))
    }

    fn int(n: i64) -> Expr {
        expr(ExprKind::Int {
            value: n,
            literal: sp(),
        })
    }

    fn stmt(kind: StmtKind) -> Stmt {
        Stmt { kind, span: sp() }
    }

    fn block(stmts: Vec<Stmt>) -> Block {
        Block { stmts, span: sp() }
    }

    fn func(name: &str, params: Vec<Param>, ret: Option<TypeExpr>, body: Block) -> Function {
        Function {
            name: ident(name),
            type_params: Vec::new(),
            angles: sp(),
            self_param: None,
            mut_self: None,
            params,
            ret,
            body,
            span: sp(),
        }
    }

    #[test]
    fn display_formats() {
        let main = func(
            "main",
            vec![],
            None,
            block(vec![
                stmt(StmtKind::Let {
                    mutable: None,
                    name: ident("x"),
                    ty: None,
                    init: int(1),
                }),
                stmt(StmtKind::Expr(expr(ExprKind::Call {
                    callee: ident("print"),
                    types: Vec::new(),
                    angles: sp(),
                    args: vec![Arg {
                        label: None,
                        value: name("x"),
                    }],
                }))),
            ]),
        );
        assert_eq!(
            main.to_string(),
            "(fn main () unit (block (let x 1) (call print x)))"
        );
    }

    #[test]
    fn display_float() {
        let float = |x| expr(ExprKind::Float(Float(x)));
        assert_eq!(float(1.5).to_string(), "1.5");
        assert_eq!(float(1e16).to_string(), "1e16");
        assert_eq!(float(0.1).to_string(), "0.1");
    }

    #[test]
    fn display_every_node() {
        let cmp = expr(ExprKind::Compare {
            operands: vec![name("a"), name("b"), name("c")],
            ops: vec![CompareOp::Lt, CompareOp::Le],
        });
        let is_prime = func(
            "is_prime",
            vec![Param {
                name: ident("n"),
                ty: ty("i64"),
            }],
            Some(ty("bool")),
            block(vec![
                stmt(StmtKind::Let {
                    mutable: Some(sp()),
                    name: ident("d"),
                    ty: Some(ty("i64")),
                    init: expr(ExprKind::Unary {
                        op: UnaryOp::Neg,
                        operand: Box::new(int(2)),
                    }),
                }),
                stmt(StmtKind::Assign {
                    target: name("d"),
                    value: expr(ExprKind::Binary {
                        op: BinaryOp::Add,
                        op_span: sp(),
                        lhs: Box::new(name("d")),
                        rhs: Box::new(int(1)),
                    }),
                }),
                stmt(StmtKind::While {
                    label: None,
                    keyword: sp(),
                    cond: cmp.clone(),
                    body: block(vec![]),
                }),
                stmt(StmtKind::Expr(expr(ExprKind::If {
                    cond: Box::new(expr(ExprKind::Unary {
                        op: UnaryOp::Not,
                        operand: Box::new(expr(ExprKind::Bool(true))),
                    })),
                    then_block: block(vec![stmt(StmtKind::Return(None))]),
                    else_block: Some(block(vec![stmt(StmtKind::Return(Some(cmp)))])),
                }))),
            ]),
        );
        let second = func("g", vec![], None, block(vec![]));
        let shape = Enum {
            name: ident("Shape"),
            type_params: Vec::new(),
            conforms: Vec::new(),
            assoc: Vec::new(),
            variants: vec![
                Variant {
                    name: ident("Dot"),
                    payload: None,
                },
                Variant {
                    name: ident("Rect"),
                    payload: Some(vec![
                        PayloadField {
                            label: Some(ident("w")),
                            ty: ty("i64"),
                        },
                        PayloadField {
                            label: None,
                            ty: ty("i64"),
                        },
                    ]),
                },
            ],
            methods: vec![func("area", vec![], None, block(vec![]))],
            broken_methods: Vec::new(),
        };
        let program = Program {
            items: vec![
                Item::Function(is_prime),
                Item::Enum(shape),
                Item::Function(second),
            ],
            broken: Vec::new(),
            comments: Vec::new(),
            quotes: Vec::new(),
            breaks: Vec::new(),
        };
        assert_eq!(
            program.to_string(),
            "(fn is_prime ((n i64)) bool (block (let mut d i64 (- 2)) (= d (+ d 1)) \
             (while (< a b <= c) (block)) \
             (if (! true) (block (return)) (block (return (< a b <= c))))))\n\
             (enum Shape Dot (Rect (w i64) i64) (fn area () unit (block)))\n\
             (fn g () unit (block))"
        );
    }
}
