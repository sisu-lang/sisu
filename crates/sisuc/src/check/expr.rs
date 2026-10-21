//! Expressions, checked bottom-up and lowered to `tir`.

use std::ops::Range;

use super::{Binding, Checked, Checker, Poisoned, UNKNOWN, payload, type_of, wrap};
use crate::ast::{self, ExprKind, UnaryOp};
use crate::diagnostic::{
    Definition, Diagnostic, DiagnosticKind, Literal, MismatchSource, suggestion,
};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, Type, TypeKind};

impl Checker<'_> {
    /// `expected` is the type the context wants, `None` when it wants none. It only types a
    /// `None`, a `[]` and an integer literal, and flows into `if` and `if let` branches, a
    /// block's value, array elements, arithmetic and unary `-`; it never wraps.
    pub(super) fn expr(&mut self, e: &ast::Expr, expected: Option<Type>) -> Checked {
        // An assignment target's chain runs only through field bases, method receivers and
        // indexed bases; any other expression inside it is an ordinary read.
        let outer = self.body.in_target;
        self.body.in_target &= matches!(
            e.kind,
            ExprKind::Field { .. } | ExprKind::MethodCall { .. } | ExprKind::Index { .. }
        );
        let checked = self.expr_kind(e, expected);
        self.body.in_target = outer;
        checked
    }

    /// The integer literal `value` at `at`, whose token is `literal`, typed by `expected`. An
    /// `f64` must hold it exactly.
    fn integer(&mut self, at: Span, value: i64, literal: Span, expected: Option<Type>) -> Checked {
        let ty = literal_type(expected);
        if ty == Type::F64 {
            // Through `i128`: 2^63, the nearest `f64` to `i64::MAX`, saturates back to it.
            #[expect(
                clippy::cast_precision_loss,
                clippy::cast_possible_truncation,
                reason = "the round trip is the check that the `f64` is exact"
            )]
            let (x, exact) = (value as f64, (value as f64) as i128 == i128::from(value));
            if !exact {
                self.report(DiagnosticKind::NotExactlyF64 {
                    text: self.source.text[literal.range()].to_string(),
                    nearest: format!("{x:?}"),
                    at,
                });
                return Err(Poisoned::typed(ty));
            }
            return Ok(tir::Expr {
                kind: tir::ExprKind::Float(x),
                ty,
                span: at,
            });
        }
        if ty == Type::U8 && !(0..=i64::from(u8::MAX)).contains(&value) {
            // Only `-9223372036854775808` parses to a negative literal.
            self.diagnostics.push(if value < 0 {
                DiagnosticKind::NegatedU8 { at }.into()
            } else {
                DiagnosticKind::IntegerDoesNotFit {
                    text: self.source.text[literal.range()].to_string(),
                    at,
                }
                .into()
            });
            return Err(Poisoned::typed(ty));
        }
        Ok(tir::Expr {
            kind: tir::ExprKind::Int(value),
            ty,
            span: at,
        })
    }

    /// `expr` for each kind of expression.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per node kind, so the dispatch reads in one place"
    )]
    fn expr_kind(&mut self, e: &ast::Expr, expected: Option<Type>) -> Checked {
        let node = |kind, ty| {
            Ok(tir::Expr {
                kind,
                ty,
                span: e.span,
            })
        };
        match &e.kind {
            ExprKind::Int { value, literal } => self.integer(e.span, *value, *literal, expected),
            ExprKind::Float(x) => node(tir::ExprKind::Float(x.0), Type::F64),
            ExprKind::Bool(b) => node(tir::ExprKind::Bool(*b), Type::BOOL),
            ExprKind::Char(c) => node(tir::ExprKind::Char(*c), Type::CHAR),
            ExprKind::Str(s) => node(tir::ExprKind::Str(s.clone()), Type::STRING),
            ExprKind::Interpolate(parts) => self.interpolate(e.span, parts),
            ExprKind::ArrayLit(elements) => self.array_lit(e.span, elements, expected),
            ExprKind::ArrayFill { value, len } => self.array_fill(e.span, value, len, expected),
            ExprKind::Index { base, index } => self.index(e.span, base, index),
            ExprKind::NoneLit => self.none(e.span, expected),
            ExprKind::Name(name) => self.name(e.span, *name),
            ExprKind::Call {
                callee,
                types,
                angles,
                args,
            } => {
                if let Some(value) = self.named_args(callee, types, *angles, true) {
                    self.stray(args);
                    return Err(value);
                }
                self.call(e.span, callee, args)
            }
            ExprKind::Applied {
                name,
                types,
                angles,
            } => match self.named_args(name, types, *angles, false) {
                Some(value) => Err(value),
                None => self.name(name.span, name.name),
            },
            ExprKind::Todo { keyword, args } => self.todo(e.span, *keyword, args),
            ExprKind::SelfValue => {
                let Some(ty) = self.body.self_type else {
                    self.diagnostics
                        .push(DiagnosticKind::SelfOutsideMethod { at: e.span }.into());
                    return Err(Poisoned::error());
                };
                node(tir::ExprKind::Local(tir::LocalId(0)), ty)
            }
            ExprKind::Field { base, name, safe } => {
                if !safe && let Some(checked) = self.type_path(e.span, base, name, None) {
                    return checked;
                }
                self.member(e.span, *safe, base, name, |c, checked| {
                    c.field(e.span, checked, base, name)
                })
            }
            ExprKind::MethodCall {
                receiver,
                method,
                types,
                angles,
                args,
                parens,
                safe,
            } => self.method_expr(
                e.span,
                *safe,
                receiver,
                (method, types, *angles),
                args,
                *parens,
            ),
            ExprKind::Coalesce { lhs, rhs } => self.coalesce(e.span, lhs, rhs),
            ExprKind::Is { lhs, rhs } => self.is(e.span, lhs, rhs),
            ExprKind::Unary { op, operand } => {
                let (want, (operand, mismatched)) = match op {
                    UnaryOp::Neg => self.negated(e.span, operand, expected)?,
                    UnaryOp::Not => (Type::BOOL, self.operand(operand, Some(Type::BOOL))),
                };
                typed(
                    e.span,
                    want,
                    mismatched,
                    operand.ok().map(|operand| match operand.kind {
                        // `-1.5` is one literal, as it reads.
                        tir::ExprKind::Float(x) => tir::ExprKind::Float(-x),
                        _ => tir::ExprKind::Unary {
                            op: *op,
                            operand: Box::new(operand),
                        },
                    }),
                )
            }
            ExprKind::Binary {
                op,
                op_span,
                lhs,
                rhs,
            } => self.binary(e.span, (*op, *op_span), lhs, rhs, expected, false),
            ExprKind::Compare { operands, ops } => self.compare(e.span, operands, ops),
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => self.if_expr(e.span, cond, then_block, else_block.as_ref(), expected),
            ExprKind::IfLet {
                pattern,
                value,
                then_block,
                else_block,
            } => self.if_let(
                e.span,
                pattern,
                value,
                then_block,
                else_block.as_ref(),
                expected,
            ),
            ExprKind::Match {
                keyword,
                scrutinee,
                arms,
                close,
            } => self.match_expr(e.span, *keyword, *close, scrutinee, arms, expected),
            ExprKind::Loop {
                label,
                keyword,
                body,
            } => self.loop_expr(e.span, (*keyword, label.as_ref()), body, expected),
            ExprKind::Block(block) => self.block(block, expected).map(|block| {
                let ty = block.ty;
                tir::Expr {
                    kind: tir::ExprKind::Block(block),
                    ty,
                    span: e.span,
                }
            }),
        }
    }

    /// A read of `name`: a local, a constant, or an unknown name, which is reported.
    fn name(&mut self, span: Span, name: Symbol) -> Checked {
        match self.lookup(name) {
            Some(Binding {
                local,
                ty: Some(ty),
                ..
            }) => Ok(tir::Expr {
                kind: tir::ExprKind::Local(*local),
                ty: *ty,
                span,
            }),
            // `Error`: its diagnostic was recorded where it was bound.
            Some(_) => Err(Poisoned::error()),
            None => self.constant_use(name, span).unwrap_or_else(|| {
                let d = self
                    .bare_variant(name, span)
                    .unwrap_or_else(|| self.not_found(name, span));
                self.diagnostics.push(d);
                Err(Poisoned::error())
            }),
        }
    }

    /// An expression whose value is used: not `unit` or `never`.
    pub(super) fn value(&mut self, e: &ast::Expr, expected: Option<Type>) -> Checked {
        let checked = self.expr(e, expected);
        let never = match type_of(&checked) {
            Some(Type::UNIT) => false,
            Some(Type::NEVER) => true,
            _ => return checked,
        };
        self.diagnostics
            .push(DiagnosticKind::NoValue { at: e.span, never }.into());
        Err(Poisoned::error())
    }

    /// A value of type `want`; `None` is `Error`, which every type matches. A `T` where `want`
    /// is a `T?` is wrapped.
    pub(super) fn expect(&mut self, e: &ast::Expr, want: Option<Type>) -> Checked {
        self.operand(e, want).0
    }

    /// `expect`, and whether it reported a mismatch. A mismatch is the failure of the rule
    /// that wants `want`, so that rule's expression is `Error`.
    pub(super) fn operand(&mut self, e: &ast::Expr, want: Option<Type>) -> (Checked, bool) {
        let checked = self.value(e, Some(want.unwrap_or(UNKNOWN)));
        self.settled(e, checked, want)
    }

    /// `operand` for `e`, already checked against `want`'s expectation.
    pub(super) fn settled(
        &mut self,
        e: &ast::Expr,
        checked: Checked,
        want: Option<Type>,
    ) -> (Checked, bool) {
        // `want` was rejected inside `e`, which `against` cannot tell from an `Error` operand.
        if matches!(checked, Err(Poisoned { mismatch: true, .. })) {
            return (checked, true);
        }
        self.against(e, checked, want, None)
    }

    /// `operand` for `e`, already checked; `source` is where `want` came from, for a mismatch.
    pub(super) fn against(
        &mut self,
        e: &ast::Expr,
        checked: Checked,
        want: Option<Type>,
        source: Option<MismatchSource>,
    ) -> (Checked, bool) {
        if let (Some(want), Some(got)) = (want, type_of(&checked))
            && want != got
        {
            if let TypeKind::Optional(payload) = want.kind()
                && payload == got
            {
                let wrapped = match checked {
                    Ok(e) => Ok(wrap(e, want)),
                    Err(_) => Err(Poisoned::typed(want)),
                };
                return (wrapped, false);
            }
            let d = self.mismatch_from(e.span, Some(e), want, got, source);
            self.diagnostics.push(d);
            return (Err(Poisoned::error()), true);
        }
        (checked, false)
    }

    /// A `None`, of the optional type `expected`.
    fn none(&mut self, span: Span, expected: Option<Type>) -> Checked {
        let (kind, mismatch) = match expected {
            Some(ty) if matches!(ty.kind(), TypeKind::Optional(_)) => {
                return Ok(tir::Expr {
                    kind: tir::ExprKind::None,
                    ty,
                    span,
                });
            }
            Some(Type::NEVER) => return Err(Poisoned::error()),
            Some(ty) => (
                DiagnosticKind::Mismatch {
                    want: vec![self.type_name(ty)],
                    got: "None".to_string(),
                    at: span,
                    source: self.expected_from(span),
                    fix: None,
                    conversion: false,
                },
                true,
            ),
            None => (
                DiagnosticKind::CannotInfer {
                    literal: Literal::None,
                    at: span,
                },
                false,
            ),
        };
        self.diagnostics.push(kind.into());
        Err(Poisoned { ty: None, mismatch })
    }

    /// Whether a comment starts in one of the byte ranges `deleted`.
    pub(super) fn comment_in(&self, deleted: &[Range<usize>]) -> bool {
        self.source
            .comments
            .iter()
            .any(|c| deleted.iter().any(|d| d.contains(c)))
    }
}

/// The operator and the type it takes, for all but `==` and `!=`.
pub(super) fn operator(op: ast::BinaryOp) -> Option<(tir::BinaryOp, Type)> {
    Some(match op {
        ast::BinaryOp::Eq | ast::BinaryOp::Ne => return None,
        ast::BinaryOp::Add => (tir::BinaryOp::Add, Type::I64),
        ast::BinaryOp::Sub => (tir::BinaryOp::Sub, Type::I64),
        ast::BinaryOp::Mul => (tir::BinaryOp::Mul, Type::I64),
        ast::BinaryOp::Div => (tir::BinaryOp::Div, Type::I64),
        ast::BinaryOp::Rem => (tir::BinaryOp::Rem, Type::I64),
        ast::BinaryOp::And => (tir::BinaryOp::And, Type::BOOL),
        ast::BinaryOp::Or => (tir::BinaryOp::Or, Type::BOOL),
    })
}

/// An integer literal, or `+ - * / %` and unary `-` over such operands only (spec rule 2).
pub(super) fn int_literal(e: &ast::Expr) -> bool {
    match &e.kind {
        ExprKind::Int { .. } => true,
        ExprKind::Unary {
            op: UnaryOp::Neg,
            operand,
        } => int_literal(operand),
        ExprKind::Binary { op, lhs, rhs, .. } => {
            operator(*op).is_some_and(|(_, ty)| ty == Type::I64)
                && int_literal(lhs)
                && int_literal(rhs)
        }
        _ => false,
    }
}

/// The type literals take beside a value of type `ty`: `u8` if `ty` is `u8` or `u8?`, `f64` if
/// it is `f64` or `f64?`, else `i64`.
pub(super) fn literal_type(ty: Option<Type>) -> Type {
    match ty.map(payload) {
        Some(Type::U8) => Type::U8,
        Some(Type::F64) => Type::F64,
        _ => Type::I64,
    }
}

/// Which side of an `if`, `if let` or `==` with no expected type is checked first, the lower
/// first: a concrete value is 0, an `int_literal` 1, a bare `None` or `[]` 2.
pub(super) fn rank(e: &ast::Expr) -> u8 {
    match &e.kind {
        ExprKind::NoneLit => 2,
        ExprKind::ArrayLit(elements) if elements.is_empty() => 2,
        _ if int_literal(e) => 1,
        _ => 0,
    }
}

/// Whether `e` is the bare literal `None`.
pub(super) fn none_literal(e: &ast::Expr) -> bool {
    matches!(e.kind, ExprKind::NoneLit)
}

/// A read of `local`, of type `ty`.
pub(super) fn local(local: tir::LocalId, ty: Type, span: Span) -> tir::Expr {
    tir::Expr {
        kind: tir::ExprKind::Local(local),
        ty,
        span,
    }
}

/// An `IfSome` over `scrutinee` whose blocks hold only `then` and `other`, of `then`'s type.
pub(super) fn if_some(
    bind: tir::LocalId,
    scrutinee: tir::Expr,
    then: tir::Expr,
    other: Option<tir::Expr>,
    span: Span,
) -> tir::Expr {
    let ty = then.ty;
    let block = |value: Option<tir::Expr>| tir::Block {
        stmts: Vec::new(),
        value: value.map(Box::new),
        ty,
    };
    let (then_block, else_block) = (block(Some(then)), Some(block(other)));
    tir::Expr {
        kind: tir::ExprKind::IfSome {
            bind,
            scrutinee: Box::new(scrutinee),
            then_block,
            else_block,
        },
        ty,
        span,
    }
}

/// Whether a branch of type `ty` fits the expected `want`: as is, or as its payload.
pub(super) fn fits(want: Type, ty: Type) -> bool {
    want == ty || matches!(want.kind(), TypeKind::Optional(payload) if payload == ty)
}

/// A name, `self`, or a chain of `.` and `?.` fields over one, as written.
pub(super) fn path(e: &ast::Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Name(name) => Some(name.to_string()),
        ExprKind::SelfValue => Some("self".to_owned()),
        ExprKind::Field { base, name, safe } => {
            Some(format!("{}{}{}", path(base)?, ast::dot(*safe), name.name))
        }
        _ => None,
    }
}

/// Whether `expr` takes a `.method()` as written, without parentheses around it.
pub(super) fn postfix(expr: &ast::Expr) -> bool {
    matches!(
        expr.kind,
        ExprKind::Name(_)
            | ExprKind::SelfValue
            | ExprKind::Field { .. }
            | ExprKind::Call { .. }
            | ExprKind::MethodCall { .. }
            | ExprKind::Index { .. }
            | ExprKind::ArrayLit(_)
            | ExprKind::ArrayFill { .. }
    )
}

/// "`P` has no field `z`": `owner` is the type's name, and `members` are its members of `kind`.
pub(super) fn no_member<'a>(
    kind: Definition,
    owner: String,
    name: &ast::Ident,
    members: impl IntoIterator<Item = &'a str>,
) -> Diagnostic {
    DiagnosticKind::NoMember {
        owner,
        kind,
        name: name.name.to_string(),
        at: name.span,
        suggestion: suggestion(
            name.name.as_str(),
            members.into_iter().map(|member| (member, kind)),
        ),
    }
    .into()
}

/// The node `kind` makes, of type `ty`. `kind` is `None` when an operand failed; the node
/// keeps `ty`, which its rule fixes whatever its operands are, unless the rule itself
/// reported a `mismatched` operand: a rule that failed makes its expression `Error`.
pub(super) fn typed(
    span: Span,
    ty: Type,
    mismatched: bool,
    kind: Option<tir::ExprKind>,
) -> Checked {
    match kind {
        Some(kind) => Ok(tir::Expr { kind, ty, span }),
        None => Err(Poisoned::of((!mismatched).then_some(ty))),
    }
}
