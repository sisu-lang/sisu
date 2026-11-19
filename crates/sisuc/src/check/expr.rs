//! Expressions, checked bottom-up and lowered to `tir`.

use std::ops::Range;

use super::generic::{Site, mismatch_poison};
use super::infer::{Kind, LiteralCheck};
use super::names::{self, Resolved};
use super::reports::Reported;
use super::stmt::optional_unit;
use super::{
    Checked, CheckedBlock, Checker, Expectation, Origin, Poisoned, SelfType, Uninferred, payload,
    type_of, wrap, wrap_block,
};
use crate::ast::{self, ExprKind, UnaryOp};
use crate::conformance::ensure_stack;
use crate::diagnostic::{
    Code, Definition, Diagnostic, DiagnosticKind, Expected, Literal, NoValueCause, RangePlace,
    suggestion,
};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, Type, TypeKind};
use crate::types::{HOLE_INDEX, REJECTED_INDEX};

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

/// For an `if` or `if let` whose chain of `else if` ends with no `else`: whether that last one
/// is an `if let`.
fn missing_else(mut e: &ast::Expr) -> Option<bool> {
    loop {
        let (if_let, else_block) = match &e.kind {
            ExprKind::If { else_block, .. } => (false, else_block),
            ExprKind::IfLet { else_block, .. } => (true, else_block),
            _ => return None,
        };
        let Some(block) = else_block else {
            return Some(if_let);
        };
        // `else if` is an `else` block that holds the inner `if`, and nothing else, with the span
        // of that `if`. A block in braces has its braces in its span: the `else` it belongs to
        // is there, so the outer `if` has an `else`.
        let [
            ast::Stmt {
                kind: ast::StmtKind::Expr(inner),
                ..
            },
        ] = block.stmts.as_slice()
        else {
            return None;
        };
        if block.span != inner.span {
            return None;
        }
        e = inner;
    }
}

/// The function that the call `value` calls, through the block and the `MutCall` that a call with
/// `mut` arguments becomes. `Checker::mut_call` (`check/mut_args.rs`) owns these forms: a change
/// there checks this function.
pub(super) fn callee(value: &tir::Expr) -> Option<tir::FuncId> {
    match &value.kind {
        tir::ExprKind::Call { func, .. } => Some(*func),
        tir::ExprKind::MutCall { call, .. } => callee(call),
        tir::ExprKind::Block(block) => block.value.as_deref().and_then(callee),
        _ => None,
    }
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
        // A member's literal is an `Int` variable: its fit waits for the type that the join gives.
        if matches!(expected, Expectation::Member) {
            let ty = self.int_variable(LiteralCheck {
                span: at,
                literal,
                value: value.unsigned_abs(),
                negated: (value < 0).then_some(at),
            });
            return Ok(tir::Expr::new(tir::ExprKind::Int(value), ty, at));
        }
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
                DiagnosticKind::NegatedU8 { at, convert: None }
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

    /// A new `Int` variable, made at the literal of `check`, whose fit waits for its type.
    pub(super) fn int_variable(&mut self, check: LiteralCheck) -> Type {
        let ty = self.table.fresh(Kind::Int, check.span);
        self.table.literal(ty, check);
        ty
    }

    /// `expr` for each kind of expression.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per node kind, so the dispatch reads in one place"
    )]
    fn expr_kind(&mut self, e: &ast::Expr, expected: Expectation) -> Checked {
        let node = |kind, ty| Ok(tir::Expr::new(kind, ty, e.span));
        // A form that no join converts yet is a made value: it is not a member.
        let expected = match expected {
            Expectation::Member if made_value(e) => Expectation::None,
            expected => expected,
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
            ExprKind::Range(range) => {
                let d = Self::range_outside(range, RangePlace::Elsewhere);
                let reported = self.diagnostics.error(d);
                // Only the diagnostics of the bounds matter. A bound can hold a `break` that
                // uses a label.
                for bound in [&range.start, &range.end].into_iter().flatten() {
                    drop(self.expr(bound, Expectation::Poisoned));
                }
                Err(Poisoned::untyped(reported))
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
                    parens: None,
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
                if !safe
                    && let Some(checked) =
                        self.type_path(e.span, base, name, None, (expected, false))
                {
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
                    parens: Some(*parens),
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

    /// An expression whose value is used: not `never`, and not `unit` where a value of another
    /// type is expected, nor an `if` without an `else` whose branch has a value.
    pub(super) fn value(&mut self, e: &ast::Expr, expected: Expectation) -> Checked {
        self.value_in(e, expected, false)
    }

    /// `value`; `returning` says that `e` is the value of a `return` in a function with no
    /// result. It changes the help of an `if` with no `else`.
    pub(super) fn value_in(
        &mut self,
        e: &ast::Expr,
        expected: Expectation,
        returning: bool,
    ) -> Checked {
        let checked = read_jump(self.expr(e, expected));
        self.value_of(e, checked, expected, returning)
    }

    /// `value_in` for `e`, already checked as `checked` against `expected`.
    pub(super) fn value_of(
        &mut self,
        e: &ast::Expr,
        checked: Checked,
        expected: Expectation,
        returning: bool,
    ) -> Checked {
        if type_of(&checked) == Some(Type::NEVER) {
            let at = self.never_at(e.span);
            return Err(Poisoned::untyped(self.no_value(at, NoValueCause::Never)));
        }
        self.refuse_unwanted(e, checked, expected, returning)
    }

    /// `checked`, the value of `e`, or E0043 when it is a `unit`: the context takes no `unit`, and
    /// has no expected type for `value` to hold `e` to.
    pub(super) fn refuse_unit(&mut self, e: &ast::Expr, checked: Checked) -> Checked {
        if type_of(&checked) == Some(Type::UNIT) {
            let cause = self.no_value_cause(e, checked.as_ref().ok(), (Type::UNIT, None), false);
            return Err(Poisoned::untyped(self.no_value(e.span, cause)));
        }
        checked
    }

    /// `checked`, the value of `e` in a context that expects `expected`, or E0043 when the context
    /// cannot take it (see `missing_value`). `returning` is as for `value_in`.
    pub(super) fn refuse_unwanted(
        &mut self,
        e: &ast::Expr,
        checked: Checked,
        expected: Expectation,
        returning: bool,
    ) -> Checked {
        match self.missing_value(e, &checked, expected, returning) {
            Some((at, cause)) => Err(Poisoned::untyped(self.no_value(at, cause))),
            None => checked,
        }
    }

    /// Where `checked`, the value of `e` in a context that expects `expected`, has no value that
    /// the context can take, and why. It is a valueless `if` at its tail (see `valueless`), or a
    /// `unit` where another type is expected. `returning` is as for `value_in`.
    pub(super) fn missing_value(
        &self,
        e: &ast::Expr,
        checked: &Checked,
        expected: Expectation,
        returning: bool,
    ) -> Option<(Span, NoValueCause)> {
        if let Some((at, if_let)) = valueless_value(checked) {
            return Some((at, NoValueCause::NoElse { if_let, returning }));
        }
        // An open `Any` variable takes the `unit`, as a type parameter does (`unit` is a value).
        let unwanted = type_of(checked) == Some(Type::UNIT)
            && expected.ty().is_some_and(|want| {
                let want = self.table.default_open(want);
                !takes_unit(want) && !open_any(want)
            });
        unwanted.then(|| {
            let value = checked.as_ref().ok();
            let cause = self.no_value_cause(e, value, (Type::UNIT, expected.ty()), returning);
            (e.span, cause)
        })
    }

    /// `checked`, or E0043 at the valueless `if` that gives its value (see `valueless`).
    pub(super) fn refuse_valueless(&mut self, checked: Checked) -> Checked {
        match valueless_value(&checked) {
            Some((at, if_let)) => Err(Poisoned::untyped(self.no_value(at, no_else(if_let)))),
            None => checked,
        }
    }

    /// Reports a value that is `unit` or `never`, used at `at`; `cause` says which (see
    /// `no_value_cause`).
    pub(super) fn no_value(&mut self, at: Span, cause: NoValueCause) -> Reported {
        self.error(DiagnosticKind::NoValue { at, cause })
    }

    /// Why `e`, whose checked form is `value`, has no value: an `if` or an `if let` that has no
    /// `else` at the end of its chain, or a call. A value of type `never` never finishes. `value`
    /// shows the callee, through the `mut` arguments of the call. `dbg` returns its argument, so
    /// the cause of `dbg(x)` is the cause of `x`.
    pub(super) fn no_value_cause(
        &self,
        e: &ast::Expr,
        value: Option<&tir::Expr>,
        (ty, want): (Type, Option<Type>),
        returning: bool,
    ) -> NoValueCause {
        if ty == Type::NEVER {
            return NoValueCause::Never;
        }
        if ty != Type::UNIT {
            return NoValueCause::Plain;
        }
        let name = match &e.kind {
            ExprKind::If { .. } | ExprKind::IfLet { .. } => {
                // A branch with no value of its own gives the `if` the value `unit`. Where an
                // optional is wanted, an `if` without an `else` has no value (see `unlifted`).
                let optional =
                    want.is_some_and(|want| matches!(want.kind(), TypeKind::Optional(_)));
                return match missing_else(e) {
                    Some(if_let)
                        if !optional && value.is_some_and(|value| valueless(value).is_none()) =>
                    {
                        NoValueCause::UnitIf {
                            if_let,
                            want: want.map(|want| self.type_name(want)),
                        }
                    }
                    Some(if_let) => NoValueCause::NoElse { if_let, returning },
                    None => NoValueCause::Plain,
                };
            }
            ExprKind::Call { callee, args, .. } if callee.name.as_str() == "dbg" => {
                if let [arg] = args.as_slice() {
                    let operand = value.and_then(|value| match &value.kind {
                        tir::ExprKind::Dbg { operand, .. } => Some(&**operand),
                        _ => None,
                    });
                    return self.no_value_cause(&arg.value, operand, (ty, want), returning);
                }
                callee
            }
            ExprKind::Call { callee, .. } => callee,
            ExprKind::MethodCall { method, .. } => method,
            _ => return NoValueCause::Plain,
        };
        let func = value
            .and_then(callee)
            .filter(|func| !self.signatures[func.0].span.in_prelude());
        NoValueCause::Call {
            name: name.name.to_string(),
            declared: func.map(|func| match self.written_ret(func) {
                Some((at, shown)) => (at, Some(shown)),
                None => (self.signatures[func.0].span, None),
            }),
        }
    }

    /// `e`, a `T`, wrapped to `want`, its optional. A `unit` whose value an `if` without an
    /// `else` gives has no value to wrap: E0043 at that `if` (see `unlifted`), unless no code uses
    /// the value of the join being settled (see `Body::dropped_join`).
    pub(super) fn lift(&mut self, e: tir::Expr, want: Type) -> Checked {
        match unlifted(&e).filter(|_| !self.body.dropped_join) {
            Some((at, if_let)) => Err(Poisoned::typed(want, self.no_value(at, no_else(if_let)))),
            None => Ok(wrap(e, want)),
        }
    }

    /// `checked`, a value that a later wrap lifts to its optional, as a loop lifts its `break`
    /// values. E0043 when it has no value to lift (see `lift`); the value keeps its type.
    pub(super) fn refuse_unlifted(&mut self, checked: Checked) -> Checked {
        match checked
            .as_ref()
            .ok()
            .and_then(|e| Some((unlifted(e)?, e.ty)))
        {
            Some(((at, if_let), ty)) => {
                Err(Poisoned::typed(ty, self.no_value(at, no_else(if_let))))
            }
            None => checked,
        }
    }

    /// `lift` for the value of `block`, a branch or a body. A block with no value gets a `unit`
    /// at `span` (see `wrap_block`).
    pub(super) fn lift_block(&mut self, block: &mut CheckedBlock, want: Type, span: Span) {
        let at = (block.as_ref().ok())
            .and_then(|block| block.value.as_deref())
            .and_then(unlifted);
        match at {
            Some((at, if_let)) => {
                *block = Err(Poisoned::typed(want, self.no_value(at, no_else(if_let))));
            }
            None => wrap_block(block, want, span),
        }
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
        self.close(e, checked, want)
    }

    /// `against`, as `settled` asks it for `e`, whose context is `expected`. A member keeps its
    /// variables open. A required type takes the value through the steps of `conversion`, else
    /// `against` reports it. A value with no type to meet shows each `Int` as `i64`, and an
    /// open `Any` is E0050 at its first source. Each variable of a report is poisoned.
    pub(super) fn close(
        &mut self,
        e: &ast::Expr,
        checked: Checked,
        expected: Expectation,
    ) -> (Checked, bool) {
        let checked = match (checked, expected) {
            (checked, Expectation::Member) => return (checked, false),
            (checked, Expectation::None | Expectation::Poisoned) => {
                return (self.close_open(checked, expected), false);
            }
            (Ok(value), Expectation::Required(want, _)) if value.ty != want => {
                match self.conversion(value.ty, want) {
                    Some(steps) => return (self.apply(value, &steps), false),
                    None => Ok(value),
                }
            }
            (checked, _) => checked,
        };
        let found = type_of(&checked);
        let (checked, reported) = self.against(e, checked, expected);
        if reported && let Some(found) = found {
            self.table.poison(found);
        }
        (checked, reported)
    }

    /// `close` with no type to meet: the type of `checked` with each open `Int` as `i64`, or
    /// E0050 at the first source of each open `Any`. A failed context reports none: its own
    /// report covers it. A value that it leaves open fails with no type, as a `None` does there.
    pub(super) fn close_open(&mut self, checked: Checked, expected: Expectation) -> Checked {
        let Ok(mut value) = checked else {
            return checked;
        };
        if !value.ty.holds_infer() {
            return Ok(value);
        }
        let unsettled = self.table.unsettled(value.ty);
        if expected == Expectation::Poisoned {
            value.ty = self.table.default_open(value.ty);
            if !unsettled.is_empty() || value.ty.holds_infer() {
                return Err(Poisoned::untyped(self.poisoned_context(value.span)));
            }
            return Ok(value);
        }
        let mut first = None;
        for unsettled in unsettled {
            let reported = self.report_unsettled(unsettled);
            first.get_or_insert(reported);
        }
        if let Some(reported) = first {
            return Err(Poisoned::untyped(reported));
        }
        value.ty = self.table.default_open(value.ty);
        Ok(value)
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
                    Ok(e) => self.lift(e, want),
                    Err(failed) => Err(Poisoned::typed(want, failed.reported())),
                };
                return (wrapped, false);
            }
            if let Ok(done) = self.any_conversion(e.span, checked, want, got, origin) {
                return done;
            }
            // The report shows each open part of `got` that `want` knows (`shown_against`).
            let shown = self.shown_against(got, want);
            let d = self.mismatch(e.span, Some(e), hint.shown(shown), shown, origin);
            return (Err(Poisoned::untyped(self.diagnostics.error(d))), true);
        }
        // A generic call that cannot meet `want` is reported with the type it found, as a
        // concrete call is with its type.
        if let Some(want) = want
            && let Some(found) = self.misfit_type(&checked).filter(|&found| found != want)
        {
            let found = self.shown_against(found, want);
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
            // A member's `None` is an `Any?`: the join gives its payload.
            Expectation::Member => {
                let ty = Type::optional(self.table.fresh(Kind::Any, span));
                return Ok(tir::Expr::new(tir::ExprKind::None, ty, span));
            }
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
        let (init, name, mutable, _) = self.body.let_init?;
        (init == span && name.as_str() != "_")
            .then(|| format!("{}{name}", if mutable { "mut " } else { "" }))
    }

    /// The name of the `let` whose whole initializer is at `span`, if any: the fix of a join
    /// writes its type after it.
    pub(super) fn let_name(&self, span: Span) -> Option<Span> {
        let (init, name, _, at) = self.body.let_init?;
        (init == span && name.as_str() != "_").then_some(at)
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

/// Whether `e` is a form that a join does not convert yet: an array literal with elements,
/// `[v; n]`, an operator, a `??` or a loop. As a member, it is checked as a made value, with no
/// type expected.
pub(super) fn made_value(e: &ast::Expr) -> bool {
    match &e.kind {
        ExprKind::ArrayLit(elements) => !elements.is_empty(),
        ExprKind::ArrayFill { .. }
        | ExprKind::Binary { .. }
        | ExprKind::Coalesce { .. }
        | ExprKind::Loop { .. } => true,
        _ => false,
    }
}

/// Whether `e` gives a made value (see `made_value`) at its value. Its value is that of its block,
/// branches and arms, and the argument of a `dbg` or a `Some`. Then the branches of `e` join by
/// the order of their ranks, not as members of one join (see `Checker::branches`).
pub(super) fn gives_made_value(e: &ast::Expr) -> bool {
    ensure_stack(|| {
        made_value(e)
            || match &e.kind {
                ExprKind::If {
                    then_block,
                    else_block,
                    ..
                }
                | ExprKind::IfLet {
                    then_block,
                    else_block,
                    ..
                } => (std::iter::once(then_block).chain(else_block))
                    .filter_map(super::tail)
                    .any(gives_made_value),
                ExprKind::Match { arms, .. } => arms.iter().any(|arm| gives_made_value(&arm.body)),
                ExprKind::Block(block) => super::tail(block).is_some_and(gives_made_value),
                ExprKind::Call { callee, args, .. }
                    if callee.name.as_str() == "dbg" || callee.name == Symbol::SOME =>
                {
                    matches!(args.as_slice(), [arg] if gives_made_value(&arg.value))
                }
                ExprKind::MethodCall {
                    method,
                    args,
                    safe: false,
                    ..
                } if method.name == Symbol::SOME => {
                    matches!(args.as_slice(), [arg] if gives_made_value(&arg.value))
                }
                _ => false,
            }
    })
}

/// Whether a context that wants `want` takes a `unit`: it wants `unit` or an optional `unit`, or
/// a hole of a call, which the `unit` binds. A type parameter of the body that is checked is no
/// hole: a `unit` is no value of it.
pub(super) fn takes_unit(want: Type) -> bool {
    match want.kind() {
        TypeKind::Unit | TypeKind::Param(_, HOLE_INDEX | REJECTED_INDEX) => true,
        TypeKind::Optional(payload) => takes_unit(payload),
        _ => false,
    }
}

/// Whether `want`, after `default_open`, is an open `Any` variable or an optional of one.
fn open_any(want: Type) -> bool {
    match want.kind() {
        TypeKind::Infer(_) => true,
        TypeKind::Optional(payload) => open_any(payload),
        _ => false,
    }
}

/// The cause of E0043 at a valueless `if` (see `valueless`); `if_let` says that it is an
/// `if let`.
pub(super) fn no_else(if_let: bool) -> NoValueCause {
    NoValueCause::NoElse {
        if_let,
        returning: false,
    }
}

/// `valueless` of `checked`, or the failed valueless `if` it is (see `Poisoned::valueless`).
fn valueless_value(checked: &Checked) -> Option<(Span, bool)> {
    checked
        .as_ref()
        .map_or_else(|failed| failed.valueless, valueless)
}

/// The span of the `if` without an `else` whose branch has a value, when `e` is one or gives the
/// value of one: through a block's value, a branch, an arm or a wrap. Such an `if` has no value.
/// Only a `unit` or a `unit?` can give one, so the walk stops at any other type. The second part
/// says that the `if` is an `if let`.
pub(super) fn valueless(e: &tir::Expr) -> Option<(Span, bool)> {
    without_value(e, false)
}

/// `valueless` of `e`, a `unit` that lifts to `unit?`. There each `if` without an `else` has no
/// value, whatever its branch has, as for an `i64?`.
fn unlifted(e: &tir::Expr) -> Option<(Span, bool)> {
    without_value(e, true)
}

/// The walk of `valueless`; with `lifted`, an `if` without an `else` has no value by itself.
fn without_value(e: &tir::Expr, lifted: bool) -> Option<(Span, bool)> {
    let unit = e.ty == Type::UNIT;
    if !unit && !optional_unit(e.ty) {
        return None;
    }
    // The tail of a `never` block is unreachable, so no code reads its value.
    let tail = |block: &tir::Block| {
        (block.value.as_deref())
            .filter(|_| block.ty != Type::NEVER)
            .and_then(|value| without_value(value, lifted))
    };
    // A branch with a value beside no `else`: an `if let` on an enum is a `match` of two arms.
    let has_value = |block: &tir::Block| !matches!(block.ty, Type::UNIT | Type::NEVER);
    match &e.kind {
        tir::ExprKind::If {
            then_block,
            else_block,
            ..
        }
        | tir::ExprKind::IfSome {
            then_block,
            else_block,
            ..
        } => {
            if else_block.is_none() && (lifted || has_value(then_block)) {
                let if_let = matches!(e.kind, tir::ExprKind::IfSome { .. });
                return Some((e.span, if_let));
            }
            tail(then_block).or_else(|| else_block.as_ref().and_then(tail))
        }
        tir::ExprKind::Match { arms, .. } => {
            // A made arm is the missing `else` of an `if let` on an enum.
            let made = lifted && arms.iter().any(|arm| arm.made);
            if made || unit && arms.iter().any(|arm| has_value(&arm.body)) {
                return Some((e.span, true));
            }
            arms.iter().find_map(|arm| tail(&arm.body))
        }
        tir::ExprKind::Block(block) => tail(block),
        tir::ExprKind::Wrap(inner) => without_value(inner, lifted),
        // Each of these gives its own value, which no `if` gives.
        tir::ExprKind::Int(_)
        | tir::ExprKind::Float(_)
        | tir::ExprKind::Bool(_)
        | tir::ExprKind::Char(_)
        | tir::ExprKind::Str(_)
        | tir::ExprKind::None
        | tir::ExprKind::Unit
        | tir::ExprKind::Local(_)
        | tir::ExprKind::Call { .. }
        | tir::ExprKind::InterfaceCall { .. }
        | tir::ExprKind::New { .. }
        | tir::ExprKind::Variant { .. }
        | tir::ExprKind::Field { .. }
        | tir::ExprKind::Struct { .. }
        | tir::ExprKind::StructField { .. }
        | tir::ExprKind::ToAny(_)
        | tir::ExprKind::DynCall { .. }
        | tir::ExprKind::Convert(_)
        | tir::ExprKind::Print { .. }
        | tir::ExprKind::Dbg { .. }
        | tir::ExprKind::Panic(_)
        | tir::ExprKind::ReadLine
        | tir::ExprKind::ArrayLit(_)
        | tir::ExprKind::ArrayFill { .. }
        | tir::ExprKind::Index { .. }
        | tir::ExprKind::Slice { .. }
        | tir::ExprKind::Len(_)
        | tir::ExprKind::Chars(_)
        | tir::ExprKind::Bytes(_)
        | tir::ExprKind::Words(_)
        | tir::ExprKind::FromChars(_)
        | tir::ExprKind::Math { .. }
        | tir::ExprKind::Unary { .. }
        | tir::ExprKind::Binary { .. }
        | tir::ExprKind::Interpolate { .. }
        | tir::ExprKind::Concat { .. }
        | tir::ExprKind::Compare { .. }
        | tir::ExprKind::Equal { .. }
        | tir::ExprKind::Is { .. }
        | tir::ExprKind::Loop(_)
        | tir::ExprKind::Break { .. }
        | tir::ExprKind::Continue { .. }
        | tir::ExprKind::Return(_)
        | tir::ExprKind::MutCall { .. } => None,
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
