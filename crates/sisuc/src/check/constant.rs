//! Top-level constants: the gate on what a value may hold, and folding it as run time would.
//! Each use of a constant lowers to its literal, so `tir` and codegen never see a constant. An
//! `f64` folds with Rust's `f64`, whose operators give the IEEE 754 results that LLVM's do, so
//! folding never fails on one.

use rustc_hash::FxHashMap;

use super::{Body, Checked, Checker, Declared, Poisoned, built_in_type, type_of};
use crate::ast::{self, ExprKind};
use crate::diagnostic::{Diagnostic, DiagnosticKind, Panic, TypedLiteral};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, BinaryOp, CompareOp, Type, UnaryOp};

/// A folded constant's value; the variant fixes the type.
#[derive(Clone, PartialEq)]
enum Constant {
    Int(i64),
    U8(u8),
    /// Compared by the derived `PartialEq`, which is IEEE 754's `==`, as at run time.
    F64(f64),
    Bool(bool),
    Char(char),
    Str(String),
}

impl Constant {
    /// The integer `n` as a value of the type `ty`, or `None` when it does not fit: a `u8`
    /// holds `0..=255` and a `char` a Unicode scalar value.
    fn of(n: i64, ty: Type) -> Option<Constant> {
        match ty {
            Type::U8 => u8::try_from(n).ok().map(Constant::U8),
            Type::CHAR => u32::try_from(n)
                .ok()
                .and_then(char::from_u32)
                .map(Constant::Char),
            _ => Some(Constant::Int(n)),
        }
    }

    fn ty(&self) -> Type {
        match self {
            Constant::Int(_) => Type::I64,
            Constant::U8(_) => Type::U8,
            Constant::F64(_) => Type::F64,
            Constant::Bool(_) => Type::BOOL,
            Constant::Char(_) => Type::CHAR,
            Constant::Str(_) => Type::STRING,
        }
    }

    /// The literal node that a use at `span` lowers to.
    fn lower(&self, span: Span) -> tir::Expr {
        let kind = match self {
            Constant::Int(n) => tir::ExprKind::Int(*n),
            Constant::U8(n) => tir::ExprKind::Int(i64::from(*n)),
            Constant::F64(x) => tir::ExprKind::Float(*x),
            Constant::Bool(b) => tir::ExprKind::Bool(*b),
            Constant::Char(c) => tir::ExprKind::Char(*c),
            Constant::Str(s) => tir::ExprKind::Str(s.clone()),
        };
        tir::Expr {
            kind,
            ty: self.ty(),
            span,
        }
    }

    /// The value of an `i64` or a `u8`, or the code point of a `char`, which comparisons and
    /// `i64(c)` use.
    fn int(&self) -> i64 {
        match self {
            Constant::Int(n) => *n,
            Constant::U8(n) => i64::from(*n),
            Constant::Char(c) => i64::from(u32::from(*c)),
            Constant::F64(_) | Constant::Bool(_) | Constant::Str(_) => {
                unreachable!("the checker should give an integer operator integers")
            }
        }
    }

    fn float(&self) -> f64 {
        match self {
            Constant::F64(x) => *x,
            Constant::Int(_)
            | Constant::U8(_)
            | Constant::Bool(_)
            | Constant::Char(_)
            | Constant::Str(_) => {
                unreachable!("the checker should give an `f64` operator `f64`s")
            }
        }
    }

    fn bool(&self) -> bool {
        match self {
            Constant::Bool(b) => *b,
            Constant::Int(_)
            | Constant::U8(_)
            | Constant::F64(_)
            | Constant::Char(_)
            | Constant::Str(_) => {
                unreachable!("the checker should give a `bool` operator `bool`s")
            }
        }
    }

    fn str(self) -> String {
        match self {
            Constant::Str(s) => s,
            Constant::Int(_)
            | Constant::U8(_)
            | Constant::F64(_)
            | Constant::Bool(_)
            | Constant::Char(_) => {
                unreachable!("the checker should give `+` over `string`s `string`s")
            }
        }
    }
}

pub(super) struct ConstInfo {
    /// The name's span, for "first defined here" and "declared with `const` here".
    pub(super) span: Span,
    /// Source order among the constants.
    order: usize,
    /// The `: T` if given, else the folded value's type; `None` while unknown.
    pub(super) ty: Option<Type>,
    /// `None` until folded, and for good once folding fails.
    value: Option<Constant>,
}

impl ConstInfo {
    /// The constant named at `span`, at `order` among the constants, not yet folded.
    pub(super) fn new(span: Span, order: usize) -> Self {
        Self {
            span,
            order,
            ty: None,
            value: None,
        }
    }
}

/// The constant whose value is being checked: `value` is that whole value, `name_end` the end of
/// its name, where `: T` goes, and `declared` its declared type, if it has one.
pub(super) struct ConstantSite {
    value: Span,
    name_end: usize,
    declared: Option<Type>,
}

impl ConstantSite {
    /// The literal that replaces the conversion `to(arg)` at `call`: only when the call is the
    /// whole value, any declared type is `to`, and `arg` converts to a value that a literal shows
    /// with no escape. An `f64` converts when it is a whole number in `i64`'s range.
    pub(super) fn literal(&self, call: Span, to: Type, arg: &tir::Expr) -> Option<TypedLiteral> {
        if call != self.value || self.declared.is_some_and(|ty| ty != to) {
            return None;
        }
        let n = match evaluate(arg, &mut FxHashMap::default()).ok()? {
            Constant::F64(x) => whole(x)?,
            value => value.int(),
        };
        let literal = match Constant::of(n, to)? {
            Constant::Char(c) if c.is_ascii_graphic() && !matches!(c, '\'' | '\\') => {
                format!("'{c}'")
            }
            Constant::U8(b) => b.to_string(),
            Constant::Int(n) => n.to_string(),
            _ => return None,
        };
        Some(TypedLiteral {
            call,
            literal,
            annotate: self
                .declared
                .is_none()
                .then(|| (self.name_end, to.to_string())),
        })
    }
}

impl Checker<'_> {
    /// Folds every constant in source order.
    pub(super) fn fold_constants(&mut self, program: &ast::Program) {
        let constants = program.items.iter().filter_map(|item| match item {
            ast::Item::Const(c) => Some(c),
            ast::Item::Function(_) | ast::Item::Class(_) | ast::Item::Enum(_) => None,
        });
        for (order, c) in constants.enumerate() {
            let (ty, value) = self.fold(c, order);
            // A duplicate is folded for its own errors only.
            if let Some(Declared::Known(info)) = self.constants.get_mut(&c.name.name)
                && info.span == c.name.span
            {
                info.ty = ty;
                info.value = value;
            }
        }
    }

    /// The type and value of `c`, the constant at `order`; each is `None` when not known. The
    /// first step that fails reports its errors, and the rest do not run.
    fn fold(&mut self, c: &ast::Const, order: usize) -> (Option<Type>, Option<Constant>) {
        let declared = match &c.ty {
            None => None,
            Some(ty) => match self.resolve(ty) {
                None => return (None, None),
                Some(
                    resolved @ (Type::I64
                    | Type::U8
                    | Type::F64
                    | Type::BOOL
                    | Type::CHAR
                    | Type::STRING),
                ) => Some(resolved),
                Some(_) => {
                    self.report(DiagnosticKind::ConstantType { at: ty.span });
                    return (None, None);
                }
            },
        };
        if !self.gate(&c.value, &c.name, order) {
            return (declared, None);
        }
        // The locals a `string` chain binds belong to no function, so the body is dropped.
        let site = ConstantSite {
            value: c.value.span,
            name_end: c.name.span.end(),
            declared,
        };
        let (checked, _) = self.with_body(
            Body {
                in_constant: Some(site),
                ..Body::default()
            },
            |this| match declared {
                Some(ty) => this.expect(&c.value, Some(ty)),
                None => this.value(&c.value, None),
            },
        );
        let ty = declared.or_else(|| type_of(&checked));
        let Ok(checked) = checked else {
            return (ty, None);
        };
        match evaluate(&checked, &mut FxHashMap::default()) {
            Ok(value) => (ty, Some(value)),
            Err(d) => {
                self.diagnostics.push(d);
                (ty, None)
            }
        }
    }

    /// Whether `e`, in the value of the constant `user` at `order`, holds only literals,
    /// operators, conversions and names. Reports every part that does not, without entering it,
    /// and every use of `user` itself or of a constant below it.
    fn gate(&mut self, e: &ast::Expr, user: &ast::Ident, order: usize) -> bool {
        // Every part is gated, so that each one that is not known is reported.
        let all = |c: &mut Self, parts: &mut dyn Iterator<Item = &ast::Expr>| {
            parts.filter(|part| !c.gate(part, user, order)).count() == 0
        };
        match &e.kind {
            ExprKind::Int { .. }
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Char(_)
            | ExprKind::Str(_) => true,
            ExprKind::Unary { operand, .. } => self.gate(operand, user, order),
            ExprKind::Binary { lhs, rhs, .. } => all(self, &mut [&**lhs, &**rhs].into_iter()),
            ExprKind::Compare { operands, .. } => all(self, &mut operands.iter()),
            ExprKind::Call { callee, args }
                if matches!(callee.name.as_str(), "i64" | "u8" | "char" | "f64") =>
            {
                all(self, &mut args.iter().map(|arg| &arg.value))
            }
            // `f64.pi`; `f64.PI` and `i64.max` pass too, and the checker reports them.
            ExprKind::Field { base, .. } if matches!(base.kind, ExprKind::Name(name) if built_in_type(name.as_str()).is_some()) => {
                true
            }
            ExprKind::Name(name) => {
                let declared = if *name == user.name {
                    user.span
                } else {
                    match self.constants.get(name) {
                        Some(Declared::Known(info)) if info.order > order => info.span,
                        _ => return true,
                    }
                };
                self.report(DiagnosticKind::ConstantOrder {
                    name: name.to_string(),
                    user: user.name.to_string(),
                    at: e.span,
                    declared,
                    itself: *name == user.name,
                });
                false
            }
            _ => {
                self.report(DiagnosticKind::ConstantNotKnown { at: e.span });
                false
            }
        }
    }

    /// A use of `name` at `span` if it names a constant: its literal, or, if folding failed,
    /// `Poisoned` of its `ty`, with no diagnostic.
    pub(super) fn constant_use(&self, name: Symbol, span: Span) -> Option<Checked> {
        Some(match self.constants.get(&name)? {
            Declared::Known(ConstInfo {
                value: Some(value), ..
            }) => Ok(value.lower(span)),
            Declared::Known(info) => Err(Poisoned::of(info.ty)),
            Declared::Broken => Err(Poisoned::error()),
        })
    }
}

/// Runs a block of a `string` chain: its `let`s, then its comparison.
fn evaluate_block(
    block: &tir::Block,
    locals: &mut FxHashMap<tir::LocalId, Constant>,
) -> Result<Constant, Diagnostic> {
    for stmt in &block.stmts {
        let tir::Stmt::Let { local, init } = stmt else {
            unreachable!("a `string` chain's blocks should hold only `let`s")
        };
        let value = evaluate(init, locals)?;
        locals.insert(*local, value);
    }
    let value = block
        .value
        .as_ref()
        .expect("a `string` chain's block should end in its comparison");
    evaluate(value, locals)
}

/// Runs a checked initializer as run time would: operands in order, `&&` and `||` and a
/// comparison chain stopping where run time stops, and a panic as the error at its node.
/// `locals` holds the values of the locals that a `string` chain binds.
fn evaluate(
    e: &tir::Expr,
    locals: &mut FxHashMap<tir::LocalId, Constant>,
) -> Result<Constant, Diagnostic> {
    let panic =
        |panic| -> Diagnostic { DiagnosticKind::ConstantPanics { panic, at: e.span }.into() };
    // An integer result in the node's type; one that does not fit overflows.
    let int = |n: Option<i64>| {
        n.and_then(|n| Constant::of(n, e.ty))
            .ok_or_else(|| panic(Panic::Overflow))
    };
    Ok(match &e.kind {
        tir::ExprKind::Int(n) => {
            Constant::of(*n, e.ty).expect("the checker should fit a literal in its type")
        }
        tir::ExprKind::Float(x) => Constant::F64(*x),
        tir::ExprKind::Bool(b) => Constant::Bool(*b),
        tir::ExprKind::Char(c) => Constant::Char(*c),
        tir::ExprKind::Str(s) => Constant::Str(s.clone()),
        tir::ExprKind::Concat { lhs, rhs } => {
            Constant::Str(evaluate(lhs, locals)?.str() + &evaluate(rhs, locals)?.str())
        }
        tir::ExprKind::Block(block) => evaluate_block(block, locals)?,
        tir::ExprKind::Local(local) => locals
            .get(local)
            .cloned()
            .expect("a `string` chain should bind each local before it reads it"),
        tir::ExprKind::Unary { op, operand } => {
            let operand = evaluate(operand, locals)?;
            match op {
                UnaryOp::Not => Constant::Bool(!operand.bool()),
                UnaryOp::Neg if e.ty == Type::F64 => Constant::F64(-operand.float()),
                UnaryOp::Neg => int(operand.int().checked_neg())?,
            }
        }
        tir::ExprKind::Binary {
            op: BinaryOp::And,
            lhs,
            rhs,
        } => Constant::Bool(evaluate(lhs, locals)?.bool() && evaluate(rhs, locals)?.bool()),
        tir::ExprKind::Binary {
            op: BinaryOp::Or,
            lhs,
            rhs,
        } => Constant::Bool(evaluate(lhs, locals)?.bool() || evaluate(rhs, locals)?.bool()),
        tir::ExprKind::Binary { op, lhs, rhs } if e.ty == Type::F64 => {
            let l = evaluate(lhs, locals)?.float();
            Constant::F64(float_binary(*op, l, evaluate(rhs, locals)?.float()))
        }
        tir::ExprKind::Binary { op, lhs, rhs } => {
            let (l, r) = (evaluate(lhs, locals)?.int(), evaluate(rhs, locals)?.int());
            if matches!(op, BinaryOp::Div | BinaryOp::Rem) && r == 0 {
                return Err(panic(Panic::DivisionByZero));
            }
            // Each operand fits in an `i64`, so a `u8` result overflows only out of `u8`.
            int(match op {
                BinaryOp::Add => l.checked_add(r),
                BinaryOp::Sub => l.checked_sub(r),
                BinaryOp::Mul => l.checked_mul(r),
                BinaryOp::Div => l.checked_div(r),
                BinaryOp::Rem => l.checked_rem(r),
                BinaryOp::AddUnchecked => {
                    unreachable!(
                        "the checker should step only a for loop's counter, never a constant"
                    )
                }
                BinaryOp::And | BinaryOp::Or => {
                    unreachable!("`&&` and `||` should take the arms above")
                }
            })?
        }
        tir::ExprKind::Compare { operands, ops } => {
            let mut lhs = evaluate(&operands[0], locals)?;
            for (op, operand) in ops.iter().zip(&operands[1..]) {
                let rhs = evaluate(operand, locals)?;
                // A `string` orders by its bytes, as Rust's does, and an `f64` by IEEE 754: `nan`
                // is unordered, so every comparison with it is false.
                let order = match (&lhs, &rhs) {
                    (Constant::Str(l), Constant::Str(r)) => Some(l.cmp(r)),
                    (Constant::F64(l), Constant::F64(r)) => l.partial_cmp(r),
                    _ => Some(lhs.int().cmp(&rhs.int())),
                };
                let holds = order.is_some_and(|order| match op {
                    CompareOp::Lt => order.is_lt(),
                    CompareOp::Le => order.is_le(),
                    CompareOp::Gt => order.is_gt(),
                    CompareOp::Ge => order.is_ge(),
                });
                if !holds {
                    return Ok(Constant::Bool(false));
                }
                lhs = rhs;
            }
            Constant::Bool(true)
        }
        tir::ExprKind::Equal { negated, lhs, rhs } => {
            Constant::Bool((evaluate(lhs, locals)? == evaluate(rhs, locals)?) != *negated)
        }
        // A conversion that can fail is an error in a constant, so only `i64(x)` from a `u8` or
        // a `char`, and `f64(n)`, get here. `f64(n)` rounds to the nearest `f64`, as `sitofp`.
        tir::ExprKind::Convert(operand) if e.ty == Type::F64 => {
            Constant::F64(rounded(evaluate(operand, locals)?.int()))
        }
        tir::ExprKind::Convert(operand) => Constant::Int(evaluate(operand, locals)?.int()),
        _ => unreachable!("the gate should let only literals, operators and conversions through"),
    })
}

/// `l op r` on `f64`s, as LLVM's `fadd`, `fsub`, `fmul`, `fdiv` and `frem` give it.
fn float_binary(op: BinaryOp, l: f64, r: f64) -> f64 {
    match op {
        BinaryOp::Add => l + r,
        BinaryOp::Sub => l - r,
        BinaryOp::Mul => l * r,
        BinaryOp::Div => l / r,
        // The remainder has the sign of `l`, as LLVM's `frem` and C's `fmod`.
        BinaryOp::Rem => l % r,
        BinaryOp::AddUnchecked => unreachable!("the checker should step only `i64` counters"),
        BinaryOp::And | BinaryOp::Or => {
            unreachable!("`&&` and `||` should take the `bool` arms of `evaluate`")
        }
    }
}

/// `f64(n)`: the `f64` nearest `n`, as `sitofp` gives it.
fn rounded(n: i64) -> f64 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "`f64(n)` rounds to the nearest `f64`, as the language defines it"
    )]
    let x = n as f64;
    x
}

/// `x` as an `i64` when it is a whole number in `i64`'s range, as `i64(x)` gives at run time.
fn whole(x: f64) -> Option<i64> {
    // -2^63 and 2^63, exact as `f64`s; `nan` fails both comparisons.
    let bound = 9_223_372_036_854_775_808.0;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "`x` is whole and in range, so the cast is exact"
    )]
    (x >= -bound && x < bound && x.fract() == 0.0).then_some(x as i64)
}
