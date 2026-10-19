//! Operators: arithmetic, comparison, equality, `is` and `??`.

use super::expr::{fits, if_some, literal_type, local, none_literal, operator, path, rank, typed};
use super::{Checked, Checker, Poisoned, UNKNOWN, optional_of, payload, type_of, wrap};
use crate::ast::{self, CompareOp, ExprKind};
use crate::diagnostic::DiagnosticKind;
use crate::source::Span;
use crate::tir::{self, Type, TypeKind};

impl Checker<'_> {
    /// The type of unary `-`, its operand, `expected` passed down, and whether it was reported. A
    /// `u8` operand is an error; an `f64` keeps its type, and an `f64?` is checked against `f64`;
    /// any other is checked against `i64`.
    pub(super) fn negated(
        &mut self,
        span: Span,
        operand: &ast::Expr,
        expected: Option<Type>,
    ) -> Result<(Type, (Checked, bool)), Poisoned> {
        // A bare `None` expects an `i64`, as in `None + 1`.
        let expected = expected.or_else(|| none_literal(operand).then_some(Type::I64));
        let before = self.diagnostics.len();
        let checked = self.value(operand, expected);
        if type_of(&checked) == Some(Type::U8) {
            // A literal's "does not fit" is moot once its negation is an error.
            if matches!(operand.kind, ExprKind::Int { .. }) {
                self.diagnostics.truncate(before);
            }
            self.diagnostics
                .push(DiagnosticKind::NegatedU8 { at: span }.into());
            return Err(Poisoned::error());
        }
        let want = if type_of(&checked).map(payload) == Some(Type::F64) {
            Type::F64
        } else {
            Type::I64
        };
        let checked = self.against(operand, checked, Some(want));
        Ok((want, checked))
    }

    /// A comparison chain: every operand is of one number type, or every one a `string`.
    pub(super) fn compare(
        &mut self,
        span: Span,
        operands: &[ast::Expr],
        ops: &[CompareOp],
    ) -> Checked {
        let operands: Vec<_> = operands.iter().collect();
        let (checked, ty, mismatched) = self.integers(&operands, None, Operator::Compare);
        let operands = checked.into_iter().collect::<Result<Vec<_>, _>>().ok();
        match operands {
            Some(operands) if ty == Some(Type::STRING) => {
                Ok(self.string_chain(span, operands, ops))
            }
            operands => typed(
                span,
                Type::BOOL,
                mismatched,
                operands.map(|operands| tir::ExprKind::Compare {
                    operands,
                    ops: ops.to_vec(),
                }),
            ),
        }
    }

    /// Once an operand is `Error`, the other is checked against `Error` too: with that
    /// operand's type unknown, the operator may be the mistake. So `x + true` draws no
    /// diagnostic when `x` is `Error`, and `true && 1` draws one. A `compound` operator stands
    /// for a compound assignment, a statement that a rewrite of the operator would break. `op`
    /// comes with its token's span, where arithmetic on a `char` is reported.
    pub(super) fn binary(
        &mut self,
        span: Span,
        (op, op_span): (ast::BinaryOp, Span),
        lhs: &ast::Expr,
        rhs: &ast::Expr,
        expected: Option<Type>,
        compound: bool,
    ) -> Checked {
        let Some((tir_op, want)) = operator(op) else {
            return self.equal(span, op == ast::BinaryOp::Ne, lhs, rhs);
        };
        if want == Type::BOOL {
            let lhs = self.operand(lhs, Some(want));
            return self.arithmetic(span, (tir_op, op_span), want, lhs, rhs);
        }
        let operator = if tir_op == tir::BinaryOp::Add {
            Operator::Plus { compound }
        } else {
            Operator::Arithmetic
        };
        let (checked, ty, mismatched) = self.binary_operands(span, &[lhs, rhs], expected, operator);
        if checked.iter().any(|c| type_of(c) == Some(Type::CHAR)) {
            self.report(DiagnosticKind::ArithmeticOnChar { at: op_span });
            return Err(Poisoned::error());
        }
        let [l, r]: [Checked; 2] = checked
            .try_into()
            .unwrap_or_else(|_| panic!("`integers` should return one result per operand"));
        if ty == Some(Type::STRING) {
            return self.concat(span, l, r, mismatched);
        }
        let kind = l.ok().zip(r.ok()).map(|(l, r)| tir::ExprKind::Binary {
            op: tir_op,
            lhs: Box::new(l),
            rhs: Box::new(r),
        });
        match ty {
            Some(ty) => typed(span, ty, mismatched, kind),
            None => Err(Poisoned::error()),
        }
    }

    /// `integers` for the binary operator at `span`. A `+` that a `string` makes a join under an
    /// expected `u8` or `f64` is checked again with none, so that no operand is held to a type it
    /// was never going to meet: its `string` meets that type once, where the caller checks it. The
    /// join is remembered, so an operand checked again checks a join inside it once, with no
    /// expected type: each level of nesting adds a check, where it would double them.
    fn binary_operands(
        &mut self,
        span: Span,
        operands: &[&ast::Expr],
        expected: Option<Type>,
        operator: Operator,
    ) -> (Vec<Checked>, Option<Type>, bool) {
        let joins = operator == (Operator::Plus { compound: false });
        if !joins || literal_type(expected) == Type::I64 {
            return self.integers(operands, expected, operator);
        }
        if self.body.joins.contains(&span) {
            return self.integers(operands, None, operator);
        }
        let (diagnostics, locals) = (self.diagnostics.len(), self.body.locals.len());
        let result = self.integers(operands, expected, operator);
        if result.1 != Some(Type::STRING) {
            return result;
        }
        // The first check is dropped, and its locals with it, so none is left that nothing binds.
        self.diagnostics.truncate(diagnostics);
        self.body.locals.truncate(locals);
        self.body.joins.insert(span);
        self.integers(operands, None, operator)
    }

    /// The operands of one number operator, checked in source order; the type they share,
    /// `None` when it is `Error`; and whether one was reported against it. The type is `u8`
    /// when `expected` is `u8` or `u8?`, and `f64` when it is `f64` or `f64?`. Otherwise the
    /// first concrete operand (of `rank` 0) is checked first, with no expected type, and gives
    /// it: `u8` beside a `u8`, `f64` beside an `f64`, `char` beside a `char` in a comparison,
    /// else `i64`; that `i64` beside a `char` in arithmetic is no type the later operands are
    /// checked against. With none, it is `i64`, so `None + 1` expects an `i64`. An `f64` after an
    /// `i64` or `u8` first operand makes the type `f64`, and that operand is reported against
    /// it, as an operand before a `string` is in `+`. When `expected` is `i64` or `i64?`, an
    /// `f64` first operand gives `i64`, not `f64`, and that first operand is checked against
    /// `expected`, so an `f64` is reported in either order and at any depth.
    /// Once an operand is `Error`, the later ones are checked against `Error`. An operand of
    /// `arithmetic` that is a `char` is left unreported, as the poison `typed` `char`: its
    /// caller reports the operator, so what the operator's type finds wrong with the other
    /// operands is dropped, in either order. In a comparison, a `char` after a number reports
    /// that number, as a `char` before one reports the number.
    ///
    /// The type is `string` beside a `string`, in a comparison and in `+`. In `+` a `string`
    /// drives from either side: an operand of another type is reported against `string`, with
    /// the unsafe fix to interpolate, the one before a `string` included, in place of what its own
    /// type was found to lack. A join after an `Error` is `Error`. An expected `u8` or `f64` is
    /// `binary_operands`' to drop.
    fn integers(
        &mut self,
        operands: &[&ast::Expr],
        expected: Option<Type>,
        operator: Operator,
    ) -> (Vec<Checked>, Option<Type>, bool) {
        let arithmetic = operator != Operator::Compare;
        let compound = operator == (Operator::Plus { compound: true });
        // An `i64` that the context wants holds an `f64` operand to it, in either order.
        let wants_i64 = expected.map(payload) == Some(Type::I64);
        let mut first = None;
        let mut ty = if literal_type(expected) != Type::I64 {
            Some(literal_type(expected))
        } else if let Some(k) = operands.iter().position(|e| rank(e) == 0) {
            // An `i64` the context wants reaches an operator inside, so a nested `f64` is reported.
            let checked = self.value(operands[k], expected.filter(|_| wants_i64));
            let ty = type_of(&checked).map(|ty| match payload(ty) {
                Type::CHAR if !arithmetic => Type::CHAR,
                Type::STRING if operator != Operator::Arithmetic => Type::STRING,
                Type::F64 if wants_i64 => Type::I64,
                _ => literal_type(Some(ty)),
            });
            first = Some((k, checked));
            ty
        } else {
            Some(Type::I64)
        };
        let (mut want, leader) = (ty, first.as_ref().map(|(k, _)| *k));
        let by_char = arithmetic && first.as_ref().and_then(|f| type_of(&f.1)) == Some(Type::CHAR);
        let (mut checked, mut mismatched) = (Vec::with_capacity(operands.len()), false);
        // The diagnostics of the operator's type: mismatches, and a literal's, which that type
        // types. They wait until a `char` operand is ruled out.
        let mut deferred = Vec::new();
        // The type of the operand checked last, as it was found, before it was held to `want`.
        let mut before = None;
        // Whether a concrete operand was `Error` as found, which its own diagnostic reported.
        let mut failed = false;
        for (i, e) in operands.iter().enumerate() {
            let start = self.diagnostics.len();
            let value = match first.take_if(|(k, _)| *k == i) {
                Some((_, operand)) => operand,
                None => self.value(e, Some(want.filter(|_| !by_char).unwrap_or(UNKNOWN))),
            };
            if rank(e) != 0 {
                deferred.extend(self.diagnostics.drain(start..));
            }
            let char_value = type_of(&value) == Some(Type::CHAR);
            let got = type_of(&value);
            let joins = matches!(operator, Operator::Plus { .. });
            let (operand, m) = if let Some(got) =
                got.filter(|&got| joins && want == Some(Type::STRING) && got != Type::STRING)
            {
                deferred.push(self.concat_mismatch(e, got, operands, compound));
                (Err(Poisoned::error()), true)
            } else if joins && !compound && got == Some(Type::STRING) && ty != Some(Type::STRING) {
                // A `string` in `+` makes it a join, whatever the operand before it, if any, was
                // held to: that operand is reported against `string` instead. Under an expected
                // `u8`, that the `+` is a join is all this check finds: `binary_operands` checks
                // it again with none.
                ty = Some(Type::STRING);
                if let Some(other) = before.take() {
                    // All that is deferred is that operand's, the first of two.
                    deferred.clear();
                    let d = self.concat_mismatch(operands[0], other, operands, false);
                    self.diagnostics.push(d);
                    checked[0] = Err(Poisoned::error());
                    want = None;
                    (Err(Poisoned::error()), true)
                } else {
                    // An `Error` before it may be the mistake, so the join is `Error` too.
                    (value, true)
                }
            } else if arithmetic && char_value {
                (Err(Poisoned::typed(Type::CHAR)), false)
            } else if let Some(k) = leader.filter(|_| {
                char_value && !arithmetic && matches!(want, Some(Type::I64 | Type::U8 | Type::F64))
            }) {
                // A `char` after a number in a comparison: the number is the one converted.
                if let Some(got) = type_of(&checked[k]) {
                    let d = self.mismatch(operands[k].span, Some(operands[k]), Type::CHAR, got);
                    self.diagnostics.push(d);
                }
                checked[k] = Err(Poisoned::error());
                want = None;
                (Err(Poisoned::error()), true)
            } else if let Some(k) = leader.filter(|_| {
                got == Some(Type::F64) && matches!(want, Some(Type::I64 | Type::U8)) && !wants_i64
            }) {
                // An `f64` after an integer: the integer is the one converted, and the operands
                // after this one are held to `f64`.
                if let Some(found) = type_of(&checked[k]) {
                    let d = self.mismatch(operands[k].span, Some(operands[k]), Type::F64, found);
                    self.diagnostics.push(d);
                }
                checked[k] = Err(Poisoned::error());
                ty = Some(Type::F64);
                want = Some(Type::F64);
                (value, true)
            } else {
                let start = self.diagnostics.len();
                let settled = self.settled(e, value, want);
                deferred.extend(self.diagnostics.drain(start..));
                settled
            };
            if type_of(&operand).is_none() {
                want = None;
            }
            mismatched |= m;
            failed |= got.is_none() && rank(e) == 0;
            checked.push(operand);
            before = got;
        }
        // The operator is rejected whatever stands beside the `char`, unless that is an `Error`:
        // then the operator may be the mistake, as for any other type, and the sum is `Error`.
        if !(arithmetic && checked.iter().any(|c| type_of(c) == Some(Type::CHAR))) {
            self.diagnostics.append(&mut deferred);
        } else if failed {
            for c in &mut checked {
                *c = Err(Poisoned::error());
            }
            ty = None;
        }
        (checked, ty, mismatched)
    }

    /// `lhs op rhs`, its `lhs` already checked against `want`, the type `op` takes; `op` comes
    /// with its token's span.
    pub(super) fn arithmetic(
        &mut self,
        span: Span,
        (op, op_span): (tir::BinaryOp, Span),
        want: Type,
        (l, l_mismatched): (Checked, bool),
        rhs: &ast::Expr,
    ) -> Checked {
        let expected = type_of(&l).and(Some(want));
        let checked = self.value(rhs, Some(expected.unwrap_or(UNKNOWN)));
        // Only a compound assignment's number gets here with a `char` to its right.
        if want != Type::BOOL && type_of(&checked) == Some(Type::CHAR) {
            self.report(DiagnosticKind::ArithmeticOnChar { at: op_span });
            return Err(Poisoned::error());
        }
        let (r, r_mismatched) = self.settled(rhs, checked, expected);
        let kind = l.ok().zip(r.ok()).map(|(l, r)| tir::ExprKind::Binary {
            op,
            lhs: Box::new(l),
            rhs: Box::new(r),
        });
        typed(span, want, l_mismatched || r_mismatched, kind)
    }

    /// `lhs == rhs`, or `!=`. The side of lower `rank` is checked first, the left on a tie,
    /// and the other expects its type made optional. A bare `None` beside a value that is not
    /// optional is reported: the answer is fixed. So is a `u8` beside an `i64`, at the side
    /// checked second, a `char` beside another number, at the side that is not the `char`, and
    /// an `i64` or a `u8` beside an `f64`, at the side that is not the `f64`. A side that is
    /// `f64.nan` is warned about.
    fn equal(&mut self, span: Span, negated: bool, lhs: &ast::Expr, rhs: &ast::Expr) -> Checked {
        let swapped = rank(rhs) < rank(lhs);
        let (first, second) = if swapped { (rhs, lhs) } else { (lhs, rhs) };
        let checked = self.value(first, None);
        let want = type_of(&checked).map_or_else(|| UNKNOWN, optional_of);
        let other = self.value(second, Some(want));
        // `second` is the bare `None`, if either is.
        if none_literal(second)
            && let Some(ty) = type_of(&checked)
            && !matches!(ty.kind(), TypeKind::Optional(_))
        {
            let value = path(first);
            self.diagnostics
                .push(DiagnosticKind::NeverNone { value, at: span }.into());
            return Err(Poisoned::error());
        }
        // The numbers are `i64`, `u8`, `f64` and `char`: none is compared with another.
        let number = |ty: Type| matches!(ty, Type::I64 | Type::U8 | Type::F64 | Type::CHAR);
        if let (Some(want), Some(got)) = (type_of(&checked), type_of(&other))
            && number(payload(want))
            && number(payload(got))
            && payload(want) != payload(got)
        {
            // A number beside a `char` is the one converted, and an integer beside an `f64`; else
            // the second is held to the first.
            let d = if payload(got) == Type::CHAR
                || (payload(got) == Type::F64 && payload(want) != Type::CHAR)
            {
                self.mismatch(first.span, Some(first), payload(got), want)
            } else {
                self.mismatch(second.span, Some(second), payload(want), got)
            };
            self.diagnostics.push(d);
            return Err(Poisoned::error());
        }
        let (l, r) = if swapped {
            (other, checked)
        } else {
            (checked, other)
        };
        self.nan_comparison(negated, (lhs, &l), (rhs, &r));
        self.compared(span, l, r, |lhs, rhs| tir::ExprKind::Equal {
            negated,
            lhs,
            rhs,
        })
    }

    /// `lhs is rhs`: each a class or its optional, of one class. An enum value has no identity
    /// to test: two equal ones are the same value.
    pub(super) fn is(&mut self, span: Span, lhs: &ast::Expr, rhs: &ast::Expr) -> Checked {
        if none_literal(lhs) || none_literal(rhs) {
            let other = if none_literal(lhs) { rhs } else { lhs };
            // It goes before the diagnostics of `other`, which the help waits for.
            let at = self.diagnostics.len();
            let checked = self.value(other, Some(UNKNOWN));
            // `== None` compiles only beside an optional, or a side whose type is unknown.
            let optional =
                type_of(&checked).is_none_or(|ty| matches!(ty.kind(), TypeKind::Optional(_)));
            let tested = path(other).filter(|_| optional);
            let d = DiagnosticKind::IsTestsNone { tested, at: span }.into();
            self.diagnostics.insert(at, d);
            return Err(Poisoned::error());
        }
        let (l, r) = (self.value(lhs, None), self.value(rhs, None));
        for (side, checked) in [(lhs, &l), (rhs, &r)] {
            // Once an operand is `Error`, the later one is not held to `is`.
            let Some(ty) = type_of(checked) else { break };
            if matches!(
                payload(ty).kind(),
                TypeKind::Array(_) | TypeKind::String | TypeKind::Enum(_)
            ) || !ty.is_counted(&self.lowered_enums)
            {
                let kind = DiagnosticKind::IsNeedsObjects {
                    found: self.type_name(ty),
                    at: side.span,
                };
                self.diagnostics.push(kind.into());
                return Err(Poisoned::error());
            }
        }
        self.compared(span, l, r, |lhs, rhs| tir::ExprKind::Is { lhs, rhs })
    }

    /// The `bool` node `kind` makes of `l` and `r`, of one type once a `T` beside a `T?` is
    /// wrapped; other types that differ are reported.
    fn compared(
        &mut self,
        span: Span,
        l: Checked,
        r: Checked,
        kind: impl FnOnce(Box<tir::Expr>, Box<tir::Expr>) -> tir::ExprKind,
    ) -> Checked {
        let (l, r) = match (type_of(&l), type_of(&r)) {
            (Some(lt), Some(rt)) if lt != rt => {
                if fits(lt, rt) {
                    (l, r.map(|e| wrap(e, lt)))
                } else if fits(rt, lt) {
                    (l.map(|e| wrap(e, rt)), r)
                } else {
                    let kind = DiagnosticKind::CannotCompare {
                        left: self.type_name(lt),
                        right: self.type_name(rt),
                        at: span,
                    };
                    self.diagnostics.push(kind.into());
                    return Err(Poisoned::error());
                }
            }
            _ => (l, r),
        };
        match (l, r) {
            (Ok(l), Ok(r)) => Ok(self.spilled(span, Type::BOOL, vec![l, r], |operands| {
                let [lhs, rhs]: [tir::Expr; 2] = operands.try_into().expect(
                    "`spilled` should hand its closure one operand per input, and there are two",
                );
                kind(Box::new(lhs), Box::new(rhs))
            })),
            _ => Err(Poisoned::typed(Type::BOOL)),
        }
    }

    /// The right side of `??`, checked against `expected`: a value, or one that never finishes,
    /// but not `unit`.
    fn fallback(&mut self, rhs: &ast::Expr, expected: Type) -> Checked {
        let checked = self.expr(rhs, Some(expected));
        if type_of(&checked) == Some(Type::UNIT) {
            self.report(DiagnosticKind::NoValue {
                at: rhs.span,
                never: false,
            });
            return Err(Poisoned::error());
        }
        checked
    }

    /// `lhs ?? rhs`: `rhs` is a `T` or a `T?` beside the `T?` `lhs`, and gives the result its
    /// type, or never finishes, and the result is a `T`.
    pub(super) fn coalesce(&mut self, span: Span, lhs: &ast::Expr, rhs: &ast::Expr) -> Checked {
        let scrutinee = self.value(lhs, None);
        let Some(TypeKind::Optional(payload)) = type_of(&scrutinee).map(Type::kind) else {
            if type_of(&scrutinee).is_some() {
                self.diagnostics
                    .push(DiagnosticKind::CoalesceNotOptional { at: lhs.span }.into());
            }
            // Only its own diagnostics matter.
            let _ = self.fallback(rhs, UNKNOWN);
            return Err(Poisoned::error());
        };
        let optional = optional_of(payload);
        let other = self.fallback(rhs, optional);
        let ty = match type_of(&other) {
            // `a ?? panic("x")`: only the payload reaches the result.
            Some(Type::NEVER) => payload,
            Some(ty) if ty == payload || ty == optional => ty,
            Some(ty) => {
                let d = self.mismatch(rhs.span, Some(rhs), payload, ty);
                self.diagnostics.push(d);
                return Err(Poisoned::error());
            }
            None => return Err(Poisoned::error()),
        };
        let bind = self.fresh(payload);
        let read = local(bind, payload, lhs.span);
        let value = if read.ty == ty { read } else { wrap(read, ty) };
        match (scrutinee, other) {
            (Ok(scrutinee), Ok(other)) => Ok(if_some(bind, scrutinee, value, Some(other), span)),
            _ => Err(Poisoned::typed(ty)),
        }
    }
}

/// The operator whose operands `integers` checks.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Operator {
    /// A comparison chain.
    Compare,
    /// `+`, which also joins `string`s; `compound` for a compound assignment's.
    Plus { compound: bool },
    /// `- * / %`.
    Arithmetic,
}
