//! Expressions, checked bottom-up and lowered to `tir`.

use std::ops::Range;

use super::generic::{Site, mismatch_poison};
use super::names::{self, Resolved};
use super::reports::Reported;
use super::{
    Checked, Checker, Expectation, Origin, Poisoned, SelfType, Uninferred, payload, type_of, wrap,
};
use crate::ast::{self, ExprKind, UnaryOp};
use crate::diagnostic::{
    Code, Definition, Diagnostic, DiagnosticKind, Expected, Literal, RangePlace, suggestion,
};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, Type, TypeKind};

/// A jump that failed where its value is read is `Error`, not a `never`: its own diagnostic is
/// the one report of the mistake, and a `never` would also fail every place that needs a value
/// with E0043. A jump statement keeps the `never`, so the code after it is still unreachable.
fn failed_jump(checked: Checked) -> Checked {
    checked.map_err(|poisoned| Poisoned {
        ty: None,
        jumped: false,
        ..poisoned
    })
}

/// A `never` that only a failed jump gives is `Error` where a value is read: the rule of
/// `failed_jump`, for a value that the jump ends. The jump has its own report, and a `never` would
/// also fail the place that needs a value with E0043, E0138 or E0061.
pub(super) fn read_jump(checked: Checked) -> Checked {
    checked.map_err(|poisoned| {
        if poisoned.jumped {
            Poisoned {
                ty: None,
                jumped: false,
                ..poisoned
            }
        } else {
            poisoned
        }
    })
}

impl Checker<'_> {
    /// `expected` is the type the context wants, `Expectation::None` when it wants none. It only
    /// types a `None`, a `[]` and an integer literal. It flows into `if` and `if let` branches, a
    /// block's value, array elements, arithmetic and unary `-`; it never wraps.
    pub(super) fn expr(&mut self, e: &ast::Expr, expected: Expectation) -> Checked {
        #[cfg(test)]
        super::EXPR_CHECKS.set(super::EXPR_CHECKS.get() + 1);
        // The chain that an index ends runs only through field bases, method receivers and
        // indexed bases; any other expression inside it is an ordinary read.
        let outer = self.body.indexed;
        self.body.indexed |=
            !self.body.mut_values.is_empty() && self.body.mut_values.contains(&e.span);
        self.body.indexed &= matches!(
            e.kind,
            ExprKind::Field { .. } | ExprKind::MethodCall { .. } | ExprKind::Index { .. }
        );
        // The chain of a write runs only through field bases and indexed bases.
        let chain = self.body.write_chain;
        if !matches!(e.kind, ExprKind::Field { .. } | ExprKind::Index { .. }) {
            self.body.write_chain = None;
        }
        let checked = self.expr_kind(e, expected);
        self.body.indexed = outer;
        self.body.write_chain = chain;
        checked
    }

    /// Whether `e` is an integer literal, or `+ - * / %` and unary `-` over such operands only
    /// (spec rule 2). The answer for each operator is kept in `body.int_literals`, so the nested
    /// operators of a chain walk their operands once, not once for each level.
    pub(super) fn int_literal(&mut self, e: &ast::Expr) -> bool {
        let (op, lhs, rhs) = match &e.kind {
            ExprKind::Int { .. } => return true,
            ExprKind::Unary {
                op: UnaryOp::Neg,
                operand,
            } => (None, &**operand, None),
            ExprKind::Binary { op, lhs, rhs, .. } => (Some(*op), &**lhs, Some(&**rhs)),
            _ => return false,
        };
        if let Some(&known) = self.body.int_literals.get(&e.span) {
            return known;
        }
        #[cfg(test)]
        super::INT_LITERAL_STEPS.set(super::INT_LITERAL_STEPS.get() + 1);
        let found = op.is_none_or(|op| operator(op).is_some_and(|(_, ty)| ty == Type::I64))
            && self.int_literal(lhs)
            && rhs.is_none_or(|rhs| self.int_literal(rhs));
        self.body.int_literals.insert(e.span, found);
        found
    }

    /// The integer literal `value` at `at`, whose token is `literal`, typed by `expected`. An
    /// `f64` must hold it exactly.
    fn integer(&mut self, at: Span, value: i64, literal: Span, expected: Expectation) -> Checked {
        let ty = literal_type(expected.ty());
        if ty == Type::F64 {
            // Through `i128`: 2^63, the nearest `f64` to `i64::MAX`, saturates back to it.
            #[expect(
                clippy::cast_precision_loss,
                clippy::cast_possible_truncation,
                reason = "the round trip is the check that the `f64` is exact"
            )]
            let (x, exact) = (value as f64, (value as f64) as i128 == i128::from(value));
            if !exact {
                let reported = self.error(DiagnosticKind::NotExactlyF64 {
                    text: self.source.text(literal).to_string(),
                    nearest: format!("{x:?}"),
                    at,
                });
                return Err(Poisoned {
                    ty: Some(ty),
                    ..mismatch_poison(reported)
                });
            }
            return Ok(tir::Expr::new(tir::ExprKind::Float(x), ty, at));
        }
        if ty == Type::U8 && !(0..=i64::from(u8::MAX)).contains(&value) {
            // Only `-9223372036854775808` parses to a negative literal.
            let reported = self.diagnostics.error(if value < 0 {
                DiagnosticKind::NegatedU8 { at }
            } else {
                DiagnosticKind::IntegerDoesNotFit {
                    text: self.source.text(literal).to_string(),
                    at,
                }
            });
            return Err(Poisoned {
                ty: Some(ty),
                ..mismatch_poison(reported)
            });
        }
        Ok(tir::Expr::new(tir::ExprKind::Int(value), ty, at))
    }

    /// `expr` for each kind of expression.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per node kind, so the dispatch reads in one place"
    )]
    fn expr_kind(&mut self, e: &ast::Expr, expected: Expectation) -> Checked {
        let node = |kind, ty| Ok(tir::Expr::new(kind, ty, e.span));
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
            ExprKind::Range(range) => {
                let d = Self::range_outside(range, RangePlace::Elsewhere);
                Err(Poisoned::untyped(self.diagnostics.error(d)))
            }
            ExprKind::NoneLit => self.none(e.span, expected),
            ExprKind::Name(name) => self.name(e.span, *name, expected),
            ExprKind::Call {
                callee,
                types,
                angles,
                args,
            } => {
                self.stamp_marked(args);
                if let Some(value) = self.named_args(callee, types, *angles, true) {
                    self.stray(args);
                    return Err(value);
                }
                let site = Site {
                    name: callee,
                    written: types,
                    angles: *angles,
                    expected,
                };
                let checked = self.call(e.span, site, args);
                self.misread_unless_checked(e.span, types, &checked);
                checked
            }
            ExprKind::Applied {
                name,
                types,
                angles,
            } => {
                if let Some(value) = self.named_args(name, types, *angles, false) {
                    return Err(value);
                }
                let reported = self.diagnostics.mark();
                let checked = self.name(name.span, name.name, Expectation::None);
                // The type arguments follow the name, so `Holder.W<i64>` followed by `.` does not
                // parse: the fix of a bare variant would break the syntax.
                self.diagnostics
                    .strip_fixes_since(reported, Code::BareVariant);
                checked
            }
            ExprKind::Todo { keyword, args } => self.todo(e.span, *keyword, args),
            ExprKind::SelfValue => match self.body.self_type {
                SelfType::Known(ty) => node(tir::ExprKind::Local(tir::LocalId(0)), ty),
                SelfType::Failed => {
                    Err(Poisoned::untyped(self.proof(e.span))) // proof: declaration
                }
                SelfType::Absent => {
                    let d = DiagnosticKind::SelfOutsideMethod { at: e.span };
                    Err(Poisoned::untyped(self.diagnostics.error(d)))
                }
            },
            ExprKind::Field { base, name, safe } => {
                if !safe && let Some(checked) = self.type_path(e.span, base, name, None, expected) {
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
            } => {
                self.stamp_marked(args);
                let site = Site {
                    name: method,
                    written: types,
                    angles: *angles,
                    expected,
                };
                // `report_unbound` reads it: the result of a safe call is optional.
                let outer = self.body.safe_call;
                if *safe {
                    self.body.safe_call = Some(e.span);
                }
                let checked = self.method_expr(e.span, *safe, receiver, site, args, *parens);
                self.body.safe_call = outer;
                self.misread_unless_checked(e.span, types, &checked);
                checked
            }
            ExprKind::Coalesce { lhs, rhs } => self.coalesce(e.span, (lhs, rhs), expected),
            ExprKind::Is { lhs, rhs } => self.is(e.span, lhs, rhs),
            ExprKind::Unary { op, operand } => {
                let (want, (operand, mismatched)) = match op {
                    UnaryOp::Neg => self.negated(e.span, operand, expected)?,
                    UnaryOp::Not => (
                        Type::BOOL,
                        self.operand(operand, Expectation::Required(Type::BOOL, Origin::None)),
                    ),
                };
                // `-` of a value that kept its E0050 keeps it, as an operator does.
                if let Err(failed) = &operand
                    && *op == UnaryOp::Neg
                {
                    if failed.kept {
                        return Err(Poisoned::kept_error(failed.reported()));
                    }
                    // The operand of `-` decides its type, so a failed operand gives no type.
                    if failed.ty.is_none() {
                        return Err(Poisoned::untyped(failed.reported()));
                    }
                }
                typed(
                    e.span,
                    want,
                    mismatched,
                    operand
                        .map_err(|failed| failed.reported())
                        .map(|operand| match operand.kind {
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
                ..
            } => self.loop_expr(e.span, (*keyword, label.as_ref()), body, expected),
            ExprKind::Return { .. } | ExprKind::Break { .. } | ExprKind::Continue { .. } => {
                failed_jump(self.jump_expr(e))
            }
            ExprKind::Block(block) => self.block(block, expected).map(|block| {
                let ty = block.ty;
                tir::Expr::new(tir::ExprKind::Block(block), ty, e.span)
            }),
        }
    }

    /// The `return`, `break` or `continue` `e`, of type `never` even when it fails.
    pub(super) fn jump_expr(&mut self, e: &ast::Expr) -> Checked {
        match &e.kind {
            ExprKind::Return { value, .. } => self.return_expr(e.span, value.as_deref()),
            ExprKind::Break { label, value, .. } => {
                self.jump(e.span, "break", label.as_ref(), value.as_deref())
            }
            ExprKind::Continue { label } => self.jump(e.span, "continue", label.as_ref(), None),
            _ => unreachable!("`jump_expr` should be called on a jump"),
        }
    }

    /// A read of `name`: a local, a constant, or an unknown name, which is reported. The `expected`
    /// type of a local with no type may give the type that the help of its E0050 writes.
    fn name(&mut self, span: Span, name: Symbol, expected: Expectation) -> Checked {
        let quiet = self.body.quiet == Some(span);
        match self.resolve_name(name, names::Site::value(quiet)) {
            Resolved::Local { local, ty, .. } => {
                if let Some(ty) = ty {
                    return Ok(tir::Expr::new(tir::ExprKind::Local(local), ty, span));
                }
                // `Error`: its diagnostic was recorded where it was bound.
                self.inferred_by_expected(local, expected, span);
                Err(Poisoned::untyped(self.untyped_binding(span)))
            }
            Resolved::Constant(id) => self.constant_use(id, span),
            // The item's own error is reported.
            Resolved::Poisoned(_, reported) => {
                Err(Poisoned::untyped(self.poison_token(reported, span)))
            }
            Resolved::PreludeVariant => {
                // These variants of the prelude's enums are written without the enum, and
                // `Option` makes no enum for `bare_variant` to find.
                let reported = self.error(DiagnosticKind::VariantForm {
                    path: name.to_string(),
                    payload: true,
                    held: false,
                    at: span,
                });
                Err(Poisoned::untyped(reported))
            }
            _ => {
                let reported = match self.bare_variant(name, span) {
                    Some(d) => self.diagnostics.error(d),
                    None => self.not_found(name, span),
                };
                Err(Poisoned::untyped(reported))
            }
        }
    }

    /// Runs `f`, and tells whether it made a `break` or `continue` outside a loop, which has its
    /// own report. Nested blocks leave the flag set, so an enclosing check sees it too.
    pub(super) fn jumps<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> (R, bool) {
        let outer_jump = std::mem::take(&mut self.body.invalid_jump);
        let result = f(self);
        let invalid_jump = self.body.invalid_jump;
        self.body.invalid_jump |= outer_jump;
        (result, invalid_jump)
    }

    /// Where the value at `span` is reported: at the first part of a string literal that is
    /// `never` because of a part, else at `span`. A `unit` value has no such part.
    pub(super) fn never_at(&self, span: Span) -> Span {
        self.body
            .never_part
            .filter(|&(literal, _)| literal == span)
            .map_or(span, |(_, at)| at)
    }

    /// An expression whose value is used: not `unit` or `never`.
    pub(super) fn value(&mut self, e: &ast::Expr, expected: Expectation) -> Checked {
        let checked = read_jump(self.expr(e, expected));
        match type_of(&checked) {
            Some(ty @ (Type::UNIT | Type::NEVER)) => {
                let at = self.never_at(e.span);
                Err(Poisoned::untyped(self.no_value(at, ty)))
            }
            _ => checked,
        }
    }

    /// Reports a value of `ty`, `unit` or `never`, used at `at`.
    pub(super) fn no_value(&mut self, at: Span, ty: Type) -> Reported {
        self.error(DiagnosticKind::NoValue {
            at,
            never: ty == Type::NEVER,
        })
    }

    /// A value of type `want`; `Poisoned` is `Error`, which every type matches. A `T` where
    /// `want` is a `T?` is wrapped.
    pub(super) fn expect(&mut self, e: &ast::Expr, want: Expectation) -> Checked {
        self.operand(e, want).0
    }

    /// `expect`, and whether it reported a mismatch. A mismatch is the failure of the rule
    /// that wants `want`, so that rule's expression is `Error`.
    pub(super) fn operand(&mut self, e: &ast::Expr, want: Expectation) -> (Checked, bool) {
        let checked = self.value(e, want);
        self.settled(e, checked, want)
    }

    /// `operand` for `e`, already checked against `want`'s expectation.
    pub(super) fn settled(
        &mut self,
        e: &ast::Expr,
        checked: Checked,
        want: Expectation,
    ) -> (Checked, bool) {
        // `want` was rejected inside `e`, which `against` cannot tell from an `Error` operand.
        if matches!(checked, Err(Poisoned { mismatch: true, .. })) {
            return (checked, true);
        }
        self.against(e, checked, want)
    }

    /// `operand` for `e`, already checked; a mismatch names where `want` came from.
    pub(super) fn against(
        &mut self,
        e: &ast::Expr,
        checked: Checked,
        want: Expectation,
    ) -> (Checked, bool) {
        let (hint, want, origin) = (want, want.ty(), want.origin());
        if let (Some(want), Some(got)) = (want, type_of(&checked))
            && want != got
        {
            if let TypeKind::Optional(payload) = want.kind()
                && payload == got
            {
                let wrapped = match checked {
                    Ok(e) => Ok(wrap(e, want)),
                    Err(failed) => Err(Poisoned::typed(want, failed.reported())),
                };
                return (wrapped, false);
            }
            if let Ok(done) = self.any_conversion(e.span, checked, want, got, origin) {
                return done;
            }
            let d = self.mismatch(e.span, Some(e), hint.shown(got), got, origin);
            return (Err(Poisoned::untyped(self.diagnostics.error(d))), true);
        }
        // A generic call that cannot meet `want` is reported with the type it found, as a
        // concrete call is with its type.
        if let Some(want) = want
            && let Some(found) = self.misfit_type(&checked).filter(|&found| found != want)
        {
            let d = self.mismatch(e.span, Some(e), hint.shown(found), found, origin);
            let reported = self.diagnostics.error(d);
            self.settle_misfit(&checked);
            return (Err(Poisoned::untyped(reported)), true);
        }
        (checked, false)
    }

    /// A `None`, of the optional type `expected`.
    pub(super) fn none(&mut self, span: Span, expected: Expectation) -> Checked {
        let kind = match expected {
            Expectation::Poisoned => return Err(Poisoned::untyped(self.poisoned_context(span))),
            Expectation::Required(ty, _) | Expectation::Hint { want: ty, .. }
                if matches!(ty.kind(), TypeKind::Optional(_)) =>
            {
                return Ok(tir::Expr::new(tir::ExprKind::None, ty, span));
            }
            Expectation::Required(ty, _) | Expectation::Hint { want: ty, .. } => {
                DiagnosticKind::Mismatch {
                    want: Expected::Types(vec![self.want_name(span, ty)]),
                    got: "None".to_string(),
                    at: span,
                    origin: self.origin_fact(expected.origin()),
                    fix: None,
                    conversion: false,
                    layers: false,
                }
            }
            Expectation::None => {
                return Err(Poisoned::untyped(
                    self.report_uninferred(Literal::None, span),
                ));
            }
        };
        Err(mismatch_poison(self.diagnostics.error(kind)))
    }

    /// Reports E0050 for the `None` or `[]` at `span`. When it is the whole initializer of a
    /// `let`, the `let` finds the report (see `Checker::uninferred_of`).
    pub(super) fn report_uninferred(&mut self, literal: Literal, span: Span) -> Reported {
        let binding = self.let_binding(span);
        self.error(DiagnosticKind::CannotInfer {
            literal,
            binding,
            ty: None,
            at: span,
            regroup: None,
        })
    }

    /// The help of the E0050 of `local` writes `payload` (`payload?` for a `None`, `[payload]`
    /// for a `[]`). It needs an E0050 for the local and a concrete `payload`. `at` is where in
    /// the source the use takes effect. The use that comes first in the source gives the type,
    /// because the checker does not always check in source order. A use after the one that set
    /// the help leaves it. A new check of the same use that gives a type sets the help again.
    pub(super) fn inferred_by_use(&mut self, local: tir::LocalId, payload: Type, at: usize) {
        if payload.holds_hole() || matches!(payload, Type::NEVER | Type::UNIT) {
            return;
        }
        let Some(uninferred) = self.body.uninferred.get_mut(&local) else {
            return;
        };
        if uninferred.settled.is_some_and(|settled| at > settled) {
            return;
        }
        uninferred.settled = Some(at);
        let ty = match uninferred.literal {
            Literal::None => Type::optional(payload),
            Literal::EmptyArray => Type::array(payload),
        };
        let (index, span, literal, binding) = (
            uninferred.at,
            uninferred.span,
            uninferred.literal,
            uninferred.binding.clone(),
        );
        // No known path moves the E0050 or drops it before this use, and no test reaches this
        // check. It stays as a cheap guard: a place that no longer holds this E0050 is left as it
        // is.
        let reported = self
            .diagnostics
            .get(index)
            .is_some_and(|d| d.code() == Code::CannotInfer && d.span() == span);
        if reported {
            let help = self.type_name(ty);
            let d = DiagnosticKind::CannotInfer {
                literal,
                binding: Some(binding),
                ty: Some(help),
                at: span,
                regroup: None,
            };
            self.diagnostics.rewrite(index, d.into());
        }
    }

    /// A read of `local`, a local with no type, where the type `expected` is wanted: a `None`
    /// takes an optional, and a `[]` an array or an optional array.
    pub(super) fn inferred_by_expected(
        &mut self,
        local: tir::LocalId,
        expected: Expectation,
        span: Span,
    ) {
        let Some(uninferred) = self.body.uninferred.get(&local) else {
            return;
        };
        let payload = match (uninferred.literal, expected.ty().map(Type::kind)) {
            (Literal::None, Some(TypeKind::Optional(payload))) => payload,
            (Literal::EmptyArray, Some(TypeKind::Array(element))) => element,
            (Literal::EmptyArray, Some(TypeKind::Optional(inner))) => match inner.kind() {
                TypeKind::Array(element) => element,
                _ => return,
            },
            _ => return,
        };
        self.inferred_by_use(local, payload, span.start());
    }

    /// The write `target = value` that ends at `end`, where `target` is the bare name of a local
    /// with no type: the value gives it a type.
    pub(super) fn inferred_by_write(&mut self, target: &ast::Expr, value: &Checked, end: usize) {
        let (ExprKind::Name(name), Ok(value)) = (&target.kind, value) else {
            return;
        };
        if self.body.uninferred.is_empty() {
            return;
        }
        let Some(local) = self.lookup(*name).map(|b| b.local) else {
            return;
        };
        let Some(uninferred) = self.body.uninferred.get(&local) else {
            return;
        };
        let payload = match (uninferred.literal, value.ty.kind()) {
            (Literal::None, TypeKind::Optional(payload)) => payload,
            (Literal::None, _) => value.ty,
            (Literal::EmptyArray, TypeKind::Array(element)) => element,
            (Literal::EmptyArray, _) => return,
        };
        self.inferred_by_use(local, payload, end);
    }

    /// The receiver of a `push`: the local with no type that the `[]` of its `let` left, if the
    /// receiver is its bare name.
    pub(super) fn pushed_to_uninferred(&mut self, receiver: &ast::Expr) -> Option<tir::LocalId> {
        let ExprKind::Name(name) = &receiver.kind else {
            return None;
        };
        let local = self.lookup(*name)?.local;
        let uninferred = self.body.uninferred.get(&local)?;
        matches!(uninferred.literal, Literal::EmptyArray).then_some(local)
    }

    /// The E0050 of `init`, the initializer of a `let` just checked. `init` is a `None`, an
    /// `Option.None` or a `[]` that the checker reported as the last diagnostic. Call it before
    /// `let_init` is cleared.
    pub(super) fn uninferred_of(&self, init: &ast::Expr) -> Option<Uninferred> {
        // Not `bare_none_written`: `Opt.None` of an alias goes to `generic_call`, which reports
        // another E0050 that no use type replaces.
        let literal = match &init.kind {
            _ if bare_none(init) => Literal::None,
            ExprKind::ArrayLit(elements) if elements.is_empty() => Literal::EmptyArray,
            _ => return None,
        };
        let last = self.diagnostics.last()?;
        if last.code() != Code::CannotInfer || last.span() != init.span {
            return None;
        }
        Some(Uninferred {
            at: self.diagnostics.len() - 1,
            span: init.span,
            literal,
            binding: self.let_binding(init.span)?,
            settled: None,
        })
    }

    /// What the `let` whose whole initializer is the literal at `span` writes before its type,
    /// if any: its name, after `mut` when it has it.
    pub(super) fn let_binding(&self, span: Span) -> Option<String> {
        let (init, name, mutable) = self.body.let_init?;
        (init == span && name.as_str() != "_")
            .then(|| format!("{}{name}", if mutable { "mut " } else { "" }))
    }

    /// Whether a comment starts in one of the byte ranges `deleted`. The comment starts are in
    /// source order, so the first one at or after a range's start is the only one to test.
    pub(super) fn comment_in(&self, deleted: &[Range<usize>]) -> bool {
        let comments = self.source.comments;
        let read = |c: &usize| {
            #[cfg(test)]
            super::COMMENT_READS.set(super::COMMENT_READS.get() + 1);
            *c
        };
        deleted.iter().any(|d| {
            let first = comments.partition_point(|c| read(c) < d.start);
            comments.get(first).is_some_and(|c| read(c) < d.end)
        })
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

/// The type literals take beside a value of type `ty`: `u8` if `ty` is `u8` or `u8?`, `f64` if
/// it is `f64` or `f64?`, else `i64`.
pub(super) fn literal_type(ty: Option<Type>) -> Type {
    match ty.map(payload) {
        Some(Type::U8) => Type::U8,
        Some(Type::F64) => Type::F64,
        _ => Type::I64,
    }
}

/// Whether `e` is a bare `None` or an empty array literal, also under `dbg`, which passes its
/// expected type to its argument. Nothing in such a value gives its type, so it takes the type
/// of its siblings, or is reported if none is known.
pub(super) fn bare_hole(e: &ast::Expr) -> bool {
    if let Some(argument) = dbg_argument(e) {
        return bare_hole(argument);
    }
    bare_none(e) || matches!(&e.kind, ExprKind::ArrayLit(elements) if elements.is_empty())
}

/// The argument of `e`, if `e` is a call of `dbg` with one argument, labeled or not, and no
/// type argument. A label is an error (E0055), and the argument still takes the context, so
/// the label adds no second error. No user item can have the name `dbg` (see
/// `generic_callee`), so the name tells.
pub(super) fn dbg_argument(e: &ast::Expr) -> Option<&ast::Expr> {
    match &e.kind {
        ExprKind::Call {
            callee,
            types,
            args,
            ..
        } if types.is_empty() && callee.name.as_str() == "dbg" => match &args[..] {
            [arg] => Some(&arg.value),
            _ => None,
        },
        _ => None,
    }
}

/// Whether `e` is the empty optional, as `None`, `Option.None` or `Option<T>.None`.
pub(super) fn none_literal(e: &ast::Expr) -> bool {
    matches!(e.kind, ExprKind::NoneLit) || option_none(e).is_some()
}

/// Whether `e` is the empty optional without a type argument: `None` or `Option.None`.
pub(super) fn bare_none(e: &ast::Expr) -> bool {
    matches!(e.kind, ExprKind::NoneLit) || option_none(e) == Some(false)
}

/// Whether `e` is `Option.None` or `Option<T>.None`, and if so, whether it writes a type
/// argument.
fn option_none(e: &ast::Expr) -> Option<bool> {
    match &e.kind {
        ExprKind::Field {
            base,
            name,
            safe: false,
        } if name.name == Symbol::NONE => option_written(base),
        _ => None,
    }
}

/// Whether `e` is `Option` or `Option<T>`, and if so, whether it writes a type argument. No item
/// can take the name `Option`, so the name alone tells.
pub(super) fn option_written(e: &ast::Expr) -> Option<bool> {
    match &e.kind {
        ExprKind::Name(name) if *name == Symbol::OPTION => Some(false),
        ExprKind::Applied { name, .. } if name.name == Symbol::OPTION => Some(true),
        _ => None,
    }
}

/// A read of `local`, of type `ty`.
pub(super) fn local(local: tir::LocalId, ty: Type, span: Span) -> tir::Expr {
    tir::Expr::new(tir::ExprKind::Local(local), ty, span)
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
    tir::Expr::new(
        tir::ExprKind::IfSome {
            bind,
            scrutinee: Box::new(scrutinee),
            then_block,
            else_block,
        },
        ty,
        span,
    )
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
        add: None,
    }
    .into()
}
/// The node `kind` makes, of type `ty`. `kind` is the proof of a report when an operand failed;
/// the node keeps `ty`, which its rule fixes whatever its operands are, unless the rule itself
/// reported a `mismatched` operand: a rule that failed makes its expression `Error`.
pub(super) fn typed(
    span: Span,
    ty: Type,
    mismatched: bool,
    kind: Result<tir::ExprKind, Reported>,
) -> Checked {
    kind.map(|kind| tir::Expr::new(kind, ty, span))
        .map_err(|reported| Poisoned::new((!mismatched).then_some(ty), reported))
}

/// The value of `checked`, or the proof of the report that failed it: its own, else the one of a
/// label in its call, `labeled`.
pub(super) fn unlabeled<T>(
    checked: Result<T, Poisoned>,
    labeled: Option<Reported>,
) -> Result<T, Reported> {
    let value = checked.map_err(|failed| failed.reported())?;
    labeled.map_or(Ok(value), Err)
}
