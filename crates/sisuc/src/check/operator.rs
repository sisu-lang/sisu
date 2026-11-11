//! Operators: arithmetic, comparison, equality, `is` and `??`.

use super::any::{is_target, widen};
use super::expr::{fits, if_some, literal_type, local, operator, path, typed};
use super::generic::{Rank, mismatched};
use super::interface::Operation;
use super::reports::{Held, Reported};
use super::{
    Checked, Checker, Expectation, Lowered, Origin, Poisoned, as_optional, beyond_one_layer,
    block_expr, is_kept, known_type, optional_of, payload, type_of, wrap,
};
use crate::ast::{self, CompareOp, ExprKind};
use crate::diagnostic::{Diagnostic, DiagnosticKind, Expected, Literal, PeelIs, PeelLayers};
use crate::lang::{self, Dispatch};
use crate::source::Span;
use crate::tir::{self, Type, TypeArgs, TypeKind};

impl Checker<'_> {
    /// The type of unary `-`, its operand, `expected` passed down, and whether it was reported. A
    /// `u8` operand is an error; an `f64` keeps its type, and an `f64?` is checked against `f64`;
    /// any other is checked against `i64`. Unless `expected` is an `i64` or `i64?`, an operand that
    /// is none of the types `-` takes is reported against all of them; a `char` is not, so that its
    /// message keeps the conversion to `i64`.
    pub(super) fn negated(
        &mut self,
        span: Span,
        operand: &ast::Expr,
        expected: Expectation,
    ) -> Result<(Type, (Checked, bool)), Poisoned> {
        // A `None` expects an `i64`, as in `None + 1`. The operand is not a value part of the
        // value, so a hint does not reach it.
        let expected = if expected == Expectation::None && self.none_written(operand) {
            Expectation::Required(Type::I64, Origin::None)
        } else {
            expected.plain()
        };
        let before = self.diagnostics.mark();
        let checked = self.value(operand, expected);
        if let Some(ty) = type_of(&checked).filter(|ty| ty.has_param()) {
            return Err(Poisoned::untyped(self.param_lacks(operand.span, ty, "-")));
        }
        if type_of(&checked) == Some(Type::U8) {
            // A literal's "does not fit" is moot once its negation is an error. That literal is
            // a `u8` only because the context wants one, which the negation rejects.
            let literal = matches!(operand.kind, ExprKind::Int { .. });
            if literal {
                // The operator gives a new poisoned value, so no value outlives the cut reports.
                self.diagnostics.drop_since(before);
            }
            let reported = self
                .diagnostics
                .error(DiagnosticKind::NegatedU8 { at: span });
            return Err(Poisoned {
                mismatch: literal,
                ..Poisoned::untyped(reported)
            });
        }
        let want = if type_of(&checked).map(payload) == Some(Type::F64) {
            Type::F64
        } else {
            Type::I64
        };
        if expected.ty().map(payload) != Some(Type::I64)
            && let Some(got) = type_of(&checked)
                .filter(|&got| got != Type::CHAR && !NEGATABLE.contains(&payload(got)))
        {
            let d = self.takes_mismatch(operand.span, NEGATABLE, got);
            let reported = self.diagnostics.error(d);
            return Ok((want, (Err(Poisoned::untyped(reported)), true)));
        }
        let checked = self.against(operand, checked, Expectation::Required(want, Origin::None));
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
        let op = |i: usize| ops[i.saturating_sub(1)].to_string();
        let (checked, ty, mismatched) =
            self.ranked(|c| c.integers(&operands, Expectation::None, Operator::Compare, &op));
        let operands = checked.into_iter().collect::<Result<Vec<_>, _>>();
        match (operands, ty) {
            (Ok(operands), Some(Type::STRING)) => Ok(self.string_chain(span, operands, ops)),
            (Ok(operands), Some(ty)) => self.ordered_chain(span, ty, operands, ops),
            (operands, _) => typed(
                span,
                Type::BOOL,
                mismatched,
                operands
                    .map(|operands| tir::ExprKind::Compare {
                        operands,
                        ops: ops.to_vec(),
                    })
                    .map_err(|failed| failed.reported()),
            ),
        }
    }

    /// A comparison chain over `operands` of the type `ty`, a class, an enum or a type parameter
    /// that has `Ord`: a written `less` is a call, and a type parameter's is the `Compare` that
    /// `mono` lowers again for its type argument. A type that has `Ord` only through the poison
    /// of a broken block gives a poisoned expression, with no diagnostic.
    fn ordered_chain(
        &mut self,
        span: Span,
        ty: Type,
        operands: Vec<tir::Expr>,
        ops: &[CompareOp],
    ) -> Checked {
        // `integers` gives a type with `Ord`, which it has here only through a broken block.
        let Operation::Run(dispatch) = self.operation(ty, self.lang.ord) else {
            return Err(Poisoned::untyped(self.proof(span))); // proof: declaration
        };
        let (stmts, operands) = self.spill(operands);
        let mut cx = lang::Cx {
            locals: &mut self.body.locals,
        };
        let kind = lang::ordered(&mut cx, dispatch, ty, operands, ops, span);
        if matches!(dispatch, Dispatch::Method { .. }) {
            let written = Lowered::Operator(self.lang.ord, ops[0].text());
            self.spellings.insert(span, written);
        }
        let node = tir::Expr::new(kind, Type::BOOL, span);
        Ok(if stmts.is_empty() {
            node
        } else {
            block_expr(stmts, Some(node), Type::BOOL, span)
        })
    }

    /// Whether `ty` is a concrete type that no `Ord` orders, though it has the look of one that
    /// a program may compare: a `bool`, an array, an enum, a class or an `any` value.
    fn unordered(&mut self, ty: Type) -> bool {
        (ty == Type::BOOL
            || matches!(
                ty.kind(),
                TypeKind::Array(_) | TypeKind::Enum(..) | TypeKind::Class(..) | TypeKind::Any(..)
            ))
            && self.operation(ty, self.lang.ord) == Operation::Missing
    }

    /// Whether comparison operators run the interface `Ord` for the first operand's type `ty`: a
    /// type that has it, or has it through a broken block.
    fn compares_through_ord(&mut self, ty: Type) -> bool {
        matches!(
            self.operation(ty, self.lang.ord),
            Operation::Run(_) | Operation::Poison
        )
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
        expected: Expectation,
        compound: bool,
    ) -> Checked {
        let Some((tir_op, want)) = operator(op) else {
            return self.ranked(|c| c.equal(span, op == ast::BinaryOp::Ne, lhs, rhs));
        };
        if want == Type::BOOL {
            let lhs = self.operand(lhs, Expectation::Required(want, Origin::None));
            return self.arithmetic(span, (tir_op, op_span), want, lhs, rhs, Held::default());
        }
        let operator = if tir_op == tir::BinaryOp::Add {
            Operator::Plus { compound }
        } else {
            Operator::Arithmetic
        };
        let start = self.diagnostics.len();
        let op = |_| op.to_string();
        // The operands are not value parts of the value, so a hint does not reach them.
        let expected = expected.plain();
        let (checked, ty, mismatched) =
            self.ranked(|c| c.binary_operands(span, &[lhs, rhs], expected, operator, &op));
        if checked.iter().any(|c| type_of(c) == Some(Type::CHAR)) {
            let reported = self.error(DiagnosticKind::ArithmeticOnChar { at: op_span });
            return Err(Poisoned::untyped(reported));
        }
        let kept = checked
            .iter()
            .find_map(|c| c.as_ref().err().filter(|failed| failed.kept))
            .map(Poisoned::reported);
        let [l, r]: [Checked; 2] = checked
            .try_into()
            .unwrap_or_else(|_| panic!("`integers` should return one result per operand"));
        // Only an untyped operand is a failed part: a typed poisoned one reads as its type. With
        // an untyped right, a typed left of `sibling_rank` `Concrete` decides the type. An
        // untyped left decides nothing, so the second error waits.
        let failed = type_of(&l).is_none() || type_of(&r).is_none();
        let decides =
            !failed || (type_of(&l).is_some() && self.sibling_rank(lhs) == Rank::Concrete);
        if ty == Some(Type::STRING) {
            self.label_join_literals(start, [lhs, rhs]);
            return self.concat(span, (l, decides), r, mismatched);
        }
        if let Some(reported) = kept {
            return Err(Poisoned::kept_error(reported));
        }
        let kind = l
            .and_then(|l| {
                r.map(|r| tir::ExprKind::Binary {
                    op: tir_op,
                    lhs: Box::new(l),
                    rhs: Box::new(r),
                })
            })
            .map_err(|failed| failed.reported());
        match ty.filter(|_| decides) {
            Some(ty) => typed(span, ty, mismatched, kind),
            // An operand that decides nothing has failed, and so has one that leaves no type.
            None => Err(Poisoned::untyped(kind.expect_err(
                "an operator with no type should have a failed operand",
            ))),
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
        expected: Expectation,
        operator: Operator,
        op: &dyn Fn(usize) -> String,
    ) -> (Vec<Checked>, Option<Type>, bool) {
        let joins = operator == (Operator::Plus { compound: false });
        if !joins || literal_type(expected.ty()) == Type::I64 {
            return self.integers(operands, expected, operator, op);
        }
        if self.body.joins.contains(&span) {
            return self.integers(operands, Expectation::None, operator, op);
        }
        let (diagnostics, locals, pending, misfits) = (
            self.diagnostics.mark(),
            self.body.locals.len(),
            self.pending.len(),
            self.body.misfits.len(),
        );
        let result = self.integers(operands, expected, operator, op);
        if result.1 != Some(Type::STRING) {
            return result;
        }
        // The first check is dropped with its result, locals, uninferred literals, misfits and
        // findings. So none is left that nothing binds, none is judged twice, and no value
        // outlives the cut reports.
        self.diagnostics.drop_since(diagnostics);
        if !self.body.uninferred.is_empty() {
            for local in locals..self.body.locals.len() {
                #[cfg(test)]
                super::ROLLBACK_STEPS.set(super::ROLLBACK_STEPS.get() + 1);
                self.body.uninferred.remove(&tir::LocalId(local));
            }
        }
        self.body.locals.truncate(locals);
        self.pending.truncate(pending);
        self.body.misfits.truncate(misfits);
        self.body.joins.insert(span);
        self.integers(operands, Expectation::None, operator, op)
    }

    /// The operands of one number operator, checked in source order; the type they share,
    /// `None` when it is `Error`; and whether one was reported against it. The type is `u8`
    /// when `expected` is `u8` or `u8?`, and `f64` when it is `f64` or `f64?`. Otherwise the
    /// first concrete operand (of `sibling_rank` `Concrete`) is checked first, with no expected
    /// type, and gives it: `u8` beside a `u8`, `f64` beside an `f64`, `char` beside a `char` in
    /// a comparison, else `i64`; beside a `char` in arithmetic that `i64` is not passed on, and
    /// the later operands are checked with no expected type. Where `expected` is neither `i64`
    /// nor `i64?`, that operand, if it is none of the types the operator takes, is reported
    /// against all of them, not against `i64`, unless a `string` or `char` rule below reports
    /// it, an earlier operand has failed, or the first operand in source order is an integer
    /// literal. With none, it is `i64`, so `None + 1` expects an `i64`. An `f64` after an `i64`
    /// or `u8` first operand makes the type `f64`, and that operand is reported against it, as
    /// an operand before a `string` is in `+`. When `expected` is `i64` or `i64?`, an `f64`
    /// first operand gives `i64`, not `f64`, and that first operand is checked against
    /// `expected`, so an `f64` is reported in either order and at any depth. Once an operand is
    /// `Error`, the later ones are checked against `Error`. An operand of `arithmetic` that is
    /// a `char` is left unreported, as it was checked. Its caller reports the operator, so what
    /// the operator's type finds wrong with the other operands is dropped, in either order. In a
    /// comparison, a `char` after a number reports that number, as a `char` before one reports the
    /// number.
    ///
    /// The type is `string` beside a `string`, in a comparison and in `+`. In `+` a `string`
    /// drives from either side: an operand of another type is reported against `string`, with
    /// the unsafe fix to interpolate, the one before a `string` included, in place of what its
    /// own type was found to lack. A `string?` first operand makes the `+` a join and is
    /// reported against `string`, its label under the other operand only if that is a `string`.
    /// If operands come before a `string?` leader, they are checked with no expected type, and
    /// the `string?` is the operand reported, as when it comes first. A join after an `Error`
    /// is `Error`. An expected `u8` or `f64` is `binary_operands`' to drop.
    ///
    /// The first operand whose type names a type parameter is reported, as lacking `op` of its
    /// index, and makes the type `Error`. In a comparison, a leader that lacks `<` holds the
    /// other operands to its type, and its report waits: an operand of another type is the
    /// first mistake.
    #[expect(
        clippy::too_many_lines,
        reason = "one pass over the operands, so the rules that depend on each other read in one place"
    )]
    fn integers(
        &mut self,
        operands: &[&ast::Expr],
        expected: Expectation,
        operator: Operator,
        op: &dyn Fn(usize) -> String,
    ) -> (Vec<Checked>, Option<Type>, bool) {
        let arithmetic = operator != Operator::Compare;
        let compound = operator == (Operator::Plus { compound: true });
        // An `i64` that the context wants holds an `f64` operand to it, in either order.
        let wants_i64 = expected.ty().map(payload) == Some(Type::I64);
        let ranks: Vec<Rank> = operands.iter().map(|e| self.sibling_rank(e)).collect();
        let mut first = None;
        // Whether the operand checked first kept its E0050 (see `Poisoned::kept`).
        let mut kept_first = false;
        // No operand gives the type, so the operator takes `i64` by default, which no one wrote.
        let defaulted =
            literal_type(expected.ty()) == Type::I64 && !ranks.contains(&Rank::Concrete);
        let mut ty = if literal_type(expected.ty()) != Type::I64 {
            Some(literal_type(expected.ty()))
        } else if let Some(k) = ranks.iter().position(|&r| r == Rank::Concrete) {
            // An `i64` the context wants reaches an operator inside, so a nested `f64` is reported.
            let hint = if wants_i64 {
                expected
            } else {
                Expectation::None
            };
            let checked = self.value(operands[k], hint);
            let ty = type_of(&checked).map(|ty| match payload(ty) {
                Type::CHAR if !arithmetic => Type::CHAR,
                Type::STRING if operator != Operator::Arithmetic => Type::STRING,
                Type::F64 if wants_i64 => Type::I64,
                _ => literal_type(Some(ty)),
            });
            // An operand that kept its E0050 gives the operator no type, so the operator takes
            // its default `i64` again. The operands that wait for context get it as a hint.
            kept_first = is_kept(&checked);
            let ty = ty.or_else(|| kept_first.then_some(Type::I64));
            first = Some((k, checked));
            ty
        } else {
            Some(Type::I64)
        };
        // A class, an enum or a type parameter with `Ord` is compared through it, and the operands
        // after the first are held to its type.
        let by_ord = match first.as_ref().and_then(|f| type_of(&f.1)) {
            Some(leader) if operator == Operator::Compare && self.compares_through_ord(leader) => {
                ty = Some(leader);
                Some(leader)
            }
            _ => None,
        };
        // A leader with no `<` is a mistake, but a later one than an operand of another type: all
        // the operands are held to its type, and its report waits for them.
        let leader_ty = first.as_ref().and_then(|f| type_of(&f.1));
        if operator == Operator::Compare
            && by_ord.is_none()
            && let Some(leader) = leader_ty
            && (leader.has_param() || self.unordered(leader))
        {
            ty = Some(leader);
        }
        let (mut want, leader) = (ty, first.as_ref().map(|(k, _)| *k));
        // A leader of a type that the operator does not take, as found. The `i64` (or `u8` or
        // `f64`) that `ty` gets from it is no one's written type. The operands that wait for
        // context get it as a hint. A `char`, a leader that is its own `ty`, and a `string` or a
        // `string?` under `+`, which takes a `string`, are left out.
        let misfit_leader = leader_ty.is_some_and(|l| {
            ty != Some(l)
                && l != Type::CHAR
                && ty != Some(Type::STRING)
                && !operator.takes().contains(&l)
        });
        // A `string?` leader after other operands: they are checked with no expected type, so the
        // leader is the one reported, as when it comes first.
        let held = first
            .as_ref()
            .and_then(|f| type_of(&f.1))
            .is_some_and(|t| t != Type::STRING && payload(t) == Type::STRING);
        if held {
            want = None;
        }
        // An integer literal first is the operator's `i64`, as is an `i64` wanted: no list then.
        let takes = (!wants_i64 && !self.int_literal(operands[0]) && by_ord.is_none())
            .then(|| operator.takes());
        let by_char = arithmetic && first.as_ref().and_then(|f| type_of(&f.1)) == Some(Type::CHAR);
        let (mut checked, mut mismatched) = (Vec::with_capacity(operands.len()), false);
        // Whether an operand kept its E0050. The operands after it that wait for context still
        // get the default `i64` as a hint. The others are checked as after any failed operand.
        let mut kept_seen = false;
        // The operator's default `i64` is no one's written type, so it is a hint to the operands,
        // as the first side of `==` is for the second.
        let hinted = takes.is_some()
            && ((defaulted && ranks[0] != Rank::Literal) || kept_first || misfit_leader);
        // The diagnostics of the operator's type: mismatches, and a literal's, which that type
        // types. They wait until a `char` operand is ruled out.
        let mut deferred = Held::default();
        // The type of the operand checked last, as it was found, before it was held to `want`; and
        // the proof of the first concrete operand that was untyped as found, which its own
        // diagnostic reported.
        let (mut before, mut failed) = (None, None);
        // Whether the mismatch of the first operand waits to be labelled by the second operand,
        // if that is a `string`. That mismatch is the first report in `deferred`: the first
        // operand is concrete, so no report of its own is held before it.
        let mut unlabelled = false;
        // The proof of the first report of a lacking operator.
        let mut lacks = None;
        // Where the report of a lacking `Ord` stands, for an operand of the leader's type, once no
        // operand is of another type.
        let mut pending = None;
        for (i, e) in operands.iter().enumerate() {
            if (held || (misfit_leader && kept_seen)) && leader == Some(i) {
                want = ty;
            }
            // A call waits for a type, and the default `i64` is its hint. An array literal or a
            // `None` waits too, but no `i64` is one: it is checked as after any failed operand.
            if kept_seen
                && hinted
                && ranks[i] == Rank::Waits
                && !matches!(e.kind, ExprKind::ArrayLit(_))
                && !self.bare_none_written(e)
            {
                want = Some(Type::I64);
            }
            let start = self.diagnostics.mark();
            let value = match first.take_if(|(k, _)| *k == i) {
                Some((_, operand)) => operand,
                None if hinted => {
                    let hint = want.map_or(Expectation::Poisoned, |want| Expectation::Hint {
                        want,
                        siblings: want,
                        origin: Origin::None,
                    });
                    self.value(e, hint)
                }
                None if by_char => self.value(e, Expectation::None),
                None => self.value(e, Expectation::or_poisoned(want, Origin::None)),
            };
            if ranks[i] != Rank::Concrete {
                // Held for `restore` below, where a `char` operand may cut them with the operator.
                deferred.absorb(self.diagnostics.take_since(start));
            }
            let char_value = type_of(&value) == Some(Type::CHAR);
            let got = type_of(&value);
            // The leader of a comparison that no `Ord` orders has no `<`, though it is a concrete
            // type: an array, an enum, a class or a `bool`. A later operand of another type is
            // a mismatch, which `settled_by` reports.
            let unordered = |c: &mut Self, ty: Type| {
                !arithmetic && leader == Some(i) && by_ord.is_none() && c.unordered(ty)
            };
            // A comparison lacks `<` for a type parameter of the leader's type, or when there is no
            // leader type. A type parameter of another type is a mismatch, whatever its bounds.
            let lacking = got.filter(|&ty| {
                let param = ty.has_param()
                    && by_ord != Some(ty)
                    && (arithmetic || leader_ty.is_none_or(|l| payload(l) == payload(ty)));
                param || unordered(self, ty)
            });
            if let Some(param) = lacking {
                let reported = match lacks {
                    Some(reported) => reported,
                    None if arithmetic => self.param_lacks(e.span, param, op(i)),
                    None => {
                        // Reported below, once the operands after it are checked.
                        pending = Some((self.diagnostics.mark(), e.span, param, i));
                        match &value {
                            Err(poisoned) => poisoned.reported(),
                            Ok(_) => self.proof(e.span), // proof: held misfit
                        }
                    }
                };
                lacks = Some(reported);
                checked.push(Err(Poisoned::untyped(reported)));
                // The operands after it of a comparison are still held to the leader's type.
                if arithmetic {
                    want = None;
                }
                (ty, before) = (None, None);
                continue;
            }
            if std::mem::take(&mut unlabelled)
                && let Some(found) = before
            {
                deferred.replace_first(self.concat_mismatch(
                    operands[0],
                    found,
                    operands,
                    compound,
                    got,
                ));
            }
            let joins = matches!(operator, Operator::Plus { .. });
            let (operand, m) = if let Some(got) =
                got.filter(|&got| joins && want == Some(Type::STRING) && got != Type::STRING)
            {
                // The other operand's type, if it is checked already: the first's is `before`, and
                // a second that is the leader waits in `first`.
                let beside = first.as_ref().map_or(before, |f| type_of(&f.1));
                unlabelled = beside.is_none();
                deferred.push(self.concat_mismatch(e, got, operands, compound, beside));
                let reported = match &value {
                    Err(poisoned) => poisoned.reported(),
                    Ok(_) => self.proof(e.span), // proof: held misfit
                };
                (Err(Poisoned::untyped(reported)), true)
            } else if joins && !compound && got == Some(Type::STRING) && ty != Some(Type::STRING) {
                // A `string` in `+` makes it a join, whatever the operand before it, if any, was
                // held to: that operand is reported against `string` instead. Under an expected
                // `u8`, that the `+` is a join is all this check finds: `binary_operands` checks
                // it again with none.
                ty = Some(Type::STRING);
                if let Some(other) = before.take() {
                    // All that is deferred is that operand's, the first of two. It becomes untyped
                    // below, so no value outlives the dropped reports.
                    deferred.clear();
                    // Beside the first operand is this `string`, which `ty` now is.
                    let d = self.concat_mismatch(operands[0], other, operands, false, ty);
                    let reported = self.diagnostics.error(d);
                    checked[0] = Err(Poisoned::untyped(reported));
                    want = None;
                    (Err(Poisoned::untyped(reported)), true)
                } else {
                    // An `Error` before it may be the mistake, so the join is `Error` too.
                    (value, true)
                }
            } else if arithmetic && char_value {
                // The caller reports the operator, and drops the value.
                (value, false)
            } else if let Some(k) = leader.filter(|_| {
                char_value && !arithmetic && matches!(want, Some(Type::I64 | Type::U8 | Type::F64))
            }) {
                // A `char` after a number in a comparison: the number is the one converted.
                let reported = self.reject_leader(operands[k], &mut checked[k], Type::CHAR);
                want = None;
                (Err(Poisoned::untyped(reported)), true)
            } else if let Some(k) = leader.filter(|_| {
                got == Some(Type::F64) && matches!(want, Some(Type::I64 | Type::U8)) && !wants_i64
            }) {
                // An `f64` after an integer: the integer is the one converted, and the operands
                // after this one are held to `f64`.
                self.reject_leader(operands[k], &mut checked[k], Type::F64);
                ty = Some(Type::F64);
                want = Some(Type::F64);
                (value, true)
            } else if let Some(takes) = takes.filter(|_| {
                let later = leader.is_some() && operator != Operator::Compare;
                i == 0
                    && (defaulted || later && want != Some(Type::STRING))
                    && self.misfit_type(&value).is_some()
            }) {
                // A generic call first, where its concrete twin would lead: it is reported as
                // that leader is, against the operator's list. A comparison holds the operands
                // to the leader's type, which the call does not give. With no other leader, it is
                // left to its E0050; else the leader's type holds it. A `string` leader makes a
                // `+` a join, and the call is held to `string`, as the twin is.
                if operator == Operator::Compare {
                    (value, false)
                } else {
                    let start = self.diagnostics.mark();
                    let settled = self.settled_by(e, value, want, Some(takes));
                    // Held for `restore` below, as above.
                    deferred.absorb(self.diagnostics.take_since(start));
                    settled
                }
            } else {
                let start = self.diagnostics.mark();
                let settled = self.settled_by(e, value, want, takes.filter(|_| leader == Some(i)));
                // Held for `restore` below, as above.
                deferred.absorb(self.diagnostics.take_since(start));
                settled
            };
            if type_of(&operand).is_none() {
                want = None;
            }
            kept_seen |= is_kept(&operand);
            mismatched |= m;
            if got.is_none() && !is_kept(&operand) && ranks[i] == Rank::Concrete {
                failed = failed.or_else(|| operand.as_ref().err().map(Poisoned::reported));
            }
            checked.push(operand);
            before = got;
        }
        // A kept E0050 is the fault of its operand: the operator is `Error`, as a failed rule is.
        // A comparison is a `bool` whatever its operands are.
        mismatched |= arithmetic && kept_seen;
        // The missing `<` is the mistake only when no operand is of another type. A bound helps
        // only when the operands are one type, which they are then.
        if let Some((report, at, param, i)) = pending
            && !mismatched
        {
            let ord = self.lang.ord;
            let d = self.missing_operator(at, param, &op(i), ord);
            // A type with no `<` makes the chain `Error`, as a mismatch does.
            mismatched |= matches!(d, DiagnosticKind::NoOperator { .. });
            let reported = self.diagnostics.insert_at(report, d.into());
            checked[i] = Err(Poisoned::untyped(reported));
        }
        // The operator is rejected whatever stands beside the `char`, unless that is an `Error`:
        // then the operator may be the mistake, as for any other type, and the sum is `Error`.
        if arithmetic && checked.iter().any(|c| type_of(c) == Some(Type::CHAR)) {
            // The caller reports the `char` alone or, when an operand failed, gives `Error`.
            // Either way the result is a new poisoned value, so no value outlives the dropped
            // reports.
            drop(deferred);
            if let Some(reported) = failed {
                for c in &mut checked {
                    *c = Err(Poisoned::untyped(reported));
                }
                ty = None;
            }
        } else {
            self.diagnostics.restore(deferred);
        }
        (checked, ty, mismatched)
    }

    /// The operand `e`, checked as `checked`, reported against `ty`, the type of a later operand,
    /// and dropped to `Error`. Gives the proof of that report, or of the one that failed `e`.
    fn reject_leader(&mut self, e: &ast::Expr, checked: &mut Checked, ty: Type) -> Reported {
        let reported = match known_type(checked) {
            Ok(found) => {
                let d = self.mismatch(e.span, Some(e), ty, found, Origin::None);
                self.diagnostics.error(d)
            }
            Err(reported) => reported,
        };
        *checked = Err(Poisoned::untyped(reported));
        reported
    }

    /// `lhs op rhs`, its `lhs` already checked against `want`, the type `op` takes; `op` comes
    /// with its token's span. `held` are the diagnostics of `lhs`, reported once `rhs` is known to
    /// be no `char`: a `char` is reported alone, at `op`.
    pub(super) fn arithmetic(
        &mut self,
        span: Span,
        (op, op_span): (tir::BinaryOp, Span),
        want: Type,
        (l, l_mismatched): (Checked, bool),
        rhs: &ast::Expr,
        held: Held,
    ) -> Checked {
        let expected = Expectation::or_poisoned(type_of(&l).and(Some(want)), Origin::None);
        let checked = self.value(rhs, expected);
        // Only a compound assignment's number gets here with a `char` to its right.
        if want != Type::BOOL && type_of(&checked) == Some(Type::CHAR) {
            let reported = self.error(DiagnosticKind::ArithmeticOnChar { at: op_span });
            // The `char` is reported alone and the result is a new poisoned value, so no value
            // outlives the dropped reports.
            drop(held);
            return Err(Poisoned::untyped(reported));
        }
        self.diagnostics.restore(held);
        let (r, r_mismatched) = self.settled(rhs, checked, expected);
        let kind = l
            .and_then(|l| {
                r.map(|r| tir::ExprKind::Binary {
                    op,
                    lhs: Box::new(l),
                    rhs: Box::new(r),
                })
            })
            .map_err(|failed| failed.reported());
        typed(span, want, l_mismatched || r_mismatched, kind)
    }

    /// `settled`, except that a `checked` of a type that is none of `takes`, if given, is
    /// reported against all of them. Such a `checked` was found with no expected type, so no
    /// `mismatch` poison is in it for `settled` to pass on.
    pub(super) fn settled_by(
        &mut self,
        e: &ast::Expr,
        checked: Checked,
        want: Option<Type>,
        takes: Option<&[Type]>,
    ) -> (Checked, bool) {
        // An operand before it that failed has left `want` unset: one error is enough.
        // A generic call's misfit is held to `takes` by the type it found (see `Misfit`).
        if let Some(takes) = takes
            && want.is_some()
            && let Some(got) = (type_of(&checked).or_else(|| self.misfit_type(&checked)))
                .filter(|&got| !takes.contains(&payload(got)))
        {
            let d = self.takes_mismatch(e.span, takes, got);
            let reported = self.diagnostics.error(d);
            self.settle_misfit(&checked);
            return (Err(Poisoned::untyped(reported)), true);
        }
        self.settled(e, checked, Expectation::or_poisoned(want, Origin::None))
    }

    /// "expected `takes`, found `got`" at `at`: the types an operator takes, `got` not among them.
    pub(super) fn takes_mismatch(&self, at: Span, takes: &[Type], got: Type) -> Diagnostic {
        DiagnosticKind::Mismatch {
            want: Expected::Types(takes.iter().map(|&ty| self.type_name(ty)).collect()),
            got: self.type_name(got),
            at,
            origin: None,
            fix: None,
            conversion: false,
            layers: false,
        }
        .into()
    }

    /// `lhs == rhs`, or `!=`. The side of lower `sibling_rank` is checked first, the left on a tie
    /// unless it is the only `none_written`, and the other expects its type made optional,
    /// unless it is one. A side that is the other's type with two or more `?` layers taken off
    /// is held to it. A value converts through one `?` only. A `None` beside a value that is not
    /// optional is reported: the answer is fixed. So is a `u8` beside an `i64`, at the side
    /// checked second, a `char` beside another number, at the side that is not the `char`, and
    /// an `i64` or a `u8` beside an `f64`, at the side that is not the `f64`. A side that is
    /// `f64.nan` is warned about.
    fn equal(&mut self, span: Span, negated: bool, lhs: &ast::Expr, rhs: &ast::Expr) -> Checked {
        let swapped = self.sibling_rank(rhs) < self.sibling_rank(lhs)
            || (self.none_written(lhs) && !self.none_written(rhs));
        let (first, second) = if swapped { (rhs, lhs) } else { (lhs, rhs) };
        let checked = self.value(first, Expectation::None);
        // The first side's type is a hint to the second, as a sibling's is (see
        // `Expectation::Hint`): an `if` there reports a branch against it, as one checked first
        // reports against its first branch.
        let want = type_of(&checked).map_or(Expectation::Poisoned, |ty| Expectation::Hint {
            want: as_optional(ty),
            siblings: ty,
            origin: Origin::None,
        });
        let other = self.value(second, want);
        // A side converts through one `?` only: `x == 5` on an `i64??` holds `5` to `i64??`.
        if let (Some(a), Some(b)) = (type_of(&checked), type_of(&other))
            && let Some((side, want, got)) = [(second, a, b), (first, b, a)]
                .into_iter()
                .find(|&(_, want, got)| beyond_one_layer(want, got))
        {
            let d = self.mismatch(side.span, Some(side), want, got, Origin::None);
            return Err(Poisoned::untyped(self.diagnostics.error(d)));
        }
        // `second` is the `none_written`, if either is.
        if self.none_written(second)
            && let Some(ty) = type_of(&checked)
            && !matches!(ty.kind(), TypeKind::Optional(_))
        {
            let value = path(first);
            let reported = self
                .diagnostics
                .error(DiagnosticKind::NeverNone { value, at: span });
            return Err(Poisoned::untyped(reported));
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
                self.mismatch(first.span, Some(first), payload(got), want, Origin::None)
            } else {
                self.mismatch(second.span, Some(second), payload(want), got, Origin::None)
            };
            return Err(Poisoned::untyped(self.diagnostics.error(d)));
        }
        let (l, r) = if swapped {
            (other, checked)
        } else {
            (checked, other)
        };
        // `x == None` tests an optional, which any `T?` is, and needs no `Eq`. Else a side whose
        // type has no `Eq` has no `==`, unless the two types differ for good: E0047 reports that,
        // whatever the types are, a type parameter too. A side with no type (its error is
        // reported) does not say that the other needs `Eq`, except a type parameter, which always
        // did.
        let eq = self.lang.eq;
        // When the two sides have one type, its operation is asked once, for the check and the
        // call.
        let both = match (type_of(&l), type_of(&r)) {
            (Some(lt), Some(rt)) if lt == rt => Some((lt, self.operation(lt, eq))),
            _ => None,
        };
        // A second side that rejected the first's type is the mistake: it has no type to compare.
        if !self.none_written(second) && !mismatched(&l) && !mismatched(&r) {
            // A pair that converts to one `any` type is compared as two values of it, so only
            // the `any` side is asked for `Eq`.
            let (same, any) = match (type_of(&l), type_of(&r)) {
                (Some(lt), Some(rt)) if lt == rt || fits(lt, rt) || fits(rt, lt) => (true, None),
                (Some(lt), Some(rt)) => {
                    let any = self.any_join(lt, rt);
                    (any.is_some(), any)
                }
                _ => (false, None),
            };
            let typed = type_of(&l).is_some() && type_of(&r).is_some();
            for (side, checked) in [(lhs, &l), (rhs, &r)] {
                let Some(ty) = type_of(checked).filter(|&ty| {
                    (same && any.is_none_or(|any| any == ty)) || (!typed && ty.has_param())
                }) else {
                    continue;
                };
                let operation = match both {
                    Some((lt, operation)) if lt == ty => operation,
                    _ => self.operation(ty, eq),
                };
                if operation != Operation::Missing {
                    continue;
                }
                let op = if negated { "!=" } else { "==" };
                let d = self.missing_operator(side.span, ty, op, eq);
                return Err(Poisoned::untyped(self.error(d)));
            }
        }
        // A class or an enum with a written `Eq` is compared by a call of its `eq`. Any other
        // type keeps the node that `==` has always had. A type with no `Eq` reaches here only
        // in a comparison with `None`, which tests presence.
        let dispatch = match both {
            Some((_, Operation::Run(dispatch))) => dispatch,
            // The type has `Eq` only through a broken block, which is reported.
            Some((_, Operation::Poison)) => {
                let failed = (l.as_ref().err())
                    .or_else(|| r.as_ref().err())
                    .map(Poisoned::reported);
                let reported = failed.unwrap_or_else(|| self.proof(span)); // proof: declaration
                return Err(Poisoned::untyped(reported));
            }
            _ => Dispatch::Generated,
        };
        self.nan_comparison(negated, (lhs, &l), (rhs, &r));
        if matches!(dispatch, Dispatch::Method { .. }) {
            let written = Lowered::Operator(eq, if negated { "!=" } else { "==" });
            self.spellings.insert(span, written);
        }
        self.compared(span, l, r, |lhs, rhs| {
            lang::equal(dispatch, negated, lhs, rhs, span)
        })
    }

    /// `lhs is rhs`: each a class or its optional, of one class, or a type parameter that is
    /// bound to `AnyObject` or its optional. An enum value has no identity to test: two equal ones
    /// are the same value. An optional of an optional object is no object: one layer comes off
    /// first.
    pub(super) fn is(&mut self, span: Span, lhs: &ast::Expr, rhs: &ast::Expr) -> Checked {
        if self.none_written(lhs) || self.none_written(rhs) {
            let other = if self.none_written(lhs) { rhs } else { lhs };
            // It goes before the diagnostics of `other`, which the help waits for.
            let at = self.diagnostics.mark();
            let checked = self.value(other, Expectation::Poisoned);
            // `== None` compiles only beside an optional, or a side whose type is unknown.
            let optional =
                type_of(&checked).is_none_or(|ty| matches!(ty.kind(), TypeKind::Optional(_)));
            let tested = path(other).filter(|_| optional);
            let d = DiagnosticKind::IsTestsNone { tested, at: span }.into();
            return Err(Poisoned::untyped(self.diagnostics.insert_at(at, d)));
        }
        let (l, r) = (
            self.value(lhs, Expectation::None),
            self.value(rhs, Expectation::None),
        );
        // Each side peels its own extra layers: peeling one side leaves the other.
        let layers = [self.extra_layers(&l), self.extra_layers(&r)];
        for (side, checked, extra) in [(lhs, &l, layers[0]), (rhs, &r, layers[1])] {
            // Once an operand is `Error`, the later one is not held to `is`.
            let Some(ty) = type_of(checked) else { break };
            // The type under every `?` layer.
            let mut base = ty;
            while let TypeKind::Optional(inner) = base.kind() {
                base = inner;
            }
            // A type parameter, or an associated type of one, is an object when its bounds say
            // so: `mono` then makes it a class.
            let object_param = matches!(
                base.kind(),
                TypeKind::Param(..) | TypeKind::Assoc(..) | TypeKind::Any(..)
            ) && self.conforms(base, self.lang.any_object, TypeArgs::NONE);
            // An `any` of an interface that no class-only interface leads to may hold anything.
            if let TypeKind::Any(interface, _) = base.kind()
                && !object_param
            {
                let info = &self.interfaces[interface.0];
                let make = (!info.name.span.in_prelude()).then(|| info.name.name.to_string());
                let kind = DiagnosticKind::AnyMayNotHold {
                    found: self.type_name(ty),
                    at: side.span,
                    make,
                };
                return Err(Poisoned::untyped(self.diagnostics.error(kind)));
            }
            let nested = matches!(ty.kind(), TypeKind::Optional(p) if matches!(p.kind(), TypeKind::Optional(_)));
            // Peeling a layer helps only when an object is under the rest.
            if extra > 0 {
                let text = |e: &ast::Expr, extra: usize| {
                    let mut text = self.source.text(e.span).to_string();
                    for _ in 0..extra {
                        text = format!("({text} ?? None)");
                    }
                    text
                };
                let kind = DiagnosticKind::IsNeedsObjects {
                    found: self.type_name(ty),
                    at: side.span,
                    help: false,
                    add: None,
                    peel: Some(PeelIs {
                        text: format!("{} is {}", text(lhs, layers[0]), text(rhs, layers[1])),
                        layers: match layers {
                            [1, 1] => PeelLayers::OfEachSide,
                            _ if layers.iter().any(|&n| n > 1) => PeelLayers::Extra,
                            _ => PeelLayers::One,
                        },
                    }),
                };
                return Err(Poisoned::untyped(self.diagnostics.error(kind)));
            }
            if matches!(
                base.kind(),
                TypeKind::Array(_) | TypeKind::String | TypeKind::Enum(..)
            ) || !(object_param || ty.is_counted(&self.lowered_enums))
            {
                let kind = DiagnosticKind::IsNeedsObjects {
                    found: self.type_name(ty),
                    at: side.span,
                    help: !ty.has_param(),
                    // The bound alone fixes `T` and `T?`; `T??` would still need a layer peeled.
                    add: self
                        .add_bound(base, self.lang.any_object)
                        .filter(|_| !nested),
                    peel: None,
                };
                return Err(Poisoned::untyped(self.diagnostics.error(kind)));
            }
        }
        self.compared(span, l, r, |lhs, rhs| tir::ExprKind::Is { lhs, rhs })
    }

    /// The `?` layers of `checked` that `is` must peel before it can compare: all but one, when
    /// a class or an object parameter lies under them; else none.
    fn extra_layers(&mut self, checked: &Checked) -> usize {
        let Some(mut base) = type_of(checked) else {
            return 0;
        };
        let mut layers = 0_usize;
        while let TypeKind::Optional(inner) = base.kind() {
            base = inner;
            layers += 1;
        }
        let object = match base.kind() {
            TypeKind::Class(..) => true,
            TypeKind::Param(..) | TypeKind::Assoc(..) | TypeKind::Any(..) => {
                self.conforms(base, self.lang.any_object, TypeArgs::NONE)
            }
            _ => false,
        };
        if object { layers.saturating_sub(1) } else { 0 }
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
        // A generic call that cannot meet its context is compared by the type it found, as a
        // concrete call is by its type.
        let shown = |c: &Self, side: &Checked| type_of(side).or_else(|| c.misfit_type(side));
        if let (Some(lt), Some(rt)) = (shown(self, &l), shown(self, &r))
            && (type_of(&l).is_none() || type_of(&r).is_none())
            && !fits(lt, rt)
            && !fits(rt, lt)
        {
            let kind = DiagnosticKind::CannotCompare {
                left: self.type_name(lt),
                right: self.type_name(rt),
                at: span,
            };
            let reported = self.diagnostics.error(kind);
            self.settle_misfit(&l);
            self.settle_misfit(&r);
            return Err(Poisoned::untyped(reported));
        }
        let (l, r) = match (type_of(&l), type_of(&r)) {
            (Some(lt), Some(rt)) if lt != rt => {
                if fits(lt, rt) {
                    (l, r.map(|e| wrap(e, lt)))
                } else if fits(rt, lt) {
                    (l.map(|e| wrap(e, rt)), r)
                } else if self.converts_to_any(rt, lt) {
                    // A class beside an `any` converts to it first.
                    #[cfg(test)]
                    super::reached("make_any on the right");
                    (l, r.map(|e| self.make_any(e, lt)))
                } else if self.converts_to_any(lt, rt) {
                    #[cfg(test)]
                    super::reached("make_any on the left");
                    (l.map(|e| self.make_any(e, rt)), r)
                } else {
                    let kind = DiagnosticKind::CannotCompare {
                        left: self.type_name(lt),
                        right: self.type_name(rt),
                        at: span,
                    };
                    return Err(Poisoned::untyped(self.diagnostics.error(kind)));
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
            (Err(failed), _) | (_, Err(failed)) => {
                Err(Poisoned::typed(Type::BOOL, failed.reported()))
            }
        }
    }

    /// The right side of `??`, checked against `expected`: a value, or one that never finishes,
    /// but not `unit`. A value that kept its E0050 (see `Poisoned::kept`) is the fault, so the
    /// right side still has the type of the left side's value.
    fn fallback(&mut self, rhs: &ast::Expr, expected: Expectation) -> Checked {
        let checked = self.expr(rhs, expected);
        if type_of(&checked) == Some(Type::UNIT) {
            let reported = self.error(DiagnosticKind::NoValue {
                at: rhs.span,
                never: false,
            });
            return Err(Poisoned::untyped(reported));
        }
        if let Err(failed) = &checked
            && failed.kept
        {
            return Err(Poisoned::new(expected.ty().map(payload), failed.reported()));
        }
        checked
    }

    /// E0050 for the `None` at `none` that starts the right side of a `??` whose left side starts
    /// at `lhs_start`: the fix groups the left side.
    fn report_unregrouped(&mut self, none: Span, lhs_start: usize) -> Reported {
        self.error(DiagnosticKind::CannotInfer {
            literal: Literal::None,
            binding: None,
            ty: None,
            at: none,
            regroup: Some(lhs_start),
        })
    }

    /// `lhs ?? rhs`: `rhs` is a `T` or a `T?` beside the `T?` `lhs`, and gives the result its
    /// type, or never finishes, and the result is a `T`. A type written for an `any` result is
    /// what a default that does not join is reported against (see `leaves_written`).
    #[expect(
        clippy::too_many_lines,
        reason = "each form of the default side reads in one place"
    )]
    pub(super) fn coalesce(
        &mut self,
        span: Span,
        (lhs, rhs): (&ast::Expr, &ast::Expr),
        expected: Expectation,
    ) -> Checked {
        let scrutinee = self.value(lhs, Expectation::None);
        let Some(TypeKind::Optional(payload)) = type_of(&scrutinee).map(Type::kind) else {
            let reported = match known_type(&scrutinee) {
                Ok(_) => self
                    .diagnostics
                    .error(DiagnosticKind::CoalesceNotOptional { at: lhs.span }),
                Err(reported) => reported,
            };
            // Only its own diagnostics matter.
            drop(self.fallback(rhs, Expectation::Poisoned));
            return Err(Poisoned::untyped(reported));
        };
        // `??` groups to the right, so a `None` that starts a right `??` has no type to take:
        // `x ?? None ?? 0` is `x ?? (None ?? 0)`. Grouping the left side helps only when the
        // left side has two layers to take off, and a parenthesis before the `None` is a choice.
        if let ExprKind::Coalesce {
            lhs: none,
            rhs: tail,
        } = &rhs.kind
            && matches!(none.kind, ExprKind::NoneLit)
            && none.span.start() == rhs.span.start()
            && matches!(payload.kind(), TypeKind::Optional(_))
        {
            let reported = self.report_unregrouped(none.span, lhs.span.start());
            // The next `None` of a run is for the next fix to report.
            let mut tail: &ast::Expr = tail;
            while let ExprKind::Coalesce {
                lhs: next,
                rhs: after,
            } = &tail.kind
                && matches!(next.kind, ExprKind::NoneLit)
            {
                tail = after;
            }
            // Only its own diagnostics matter.
            drop(self.fallback(tail, Expectation::Poisoned));
            return Err(Poisoned::untyped(reported));
        }
        // A hint is no written type.
        let written =
            (expected.split_hint().0.ty()).filter(|&want| is_target(want) || is_target(payload));
        // The right side expects `P` for an optional `P`, else `P?`: a value converts through
        // one `?` only. That is a hint (see `Expectation::Hint`), unless that type is written. Then
        // the right side is checked against it, so `let r: any I = oa ?? None` reports at the
        // `None`, and an optional value there at the whole `??`.
        let other = match written {
            Some(want)
                if is_target(payload) && (want == payload || want == as_optional(payload)) =>
            {
                self.fallback(rhs, Expectation::Required(want, Origin::None))
            }
            _ => {
                let hint = Expectation::Hint {
                    want: as_optional(payload),
                    siblings: payload,
                    origin: Origin::None,
                };
                self.fallback(rhs, hint)
            }
        };
        // A value converts to an `any` payload, unless it is a wider `any` type, which the
        // payload converts to.
        let other = match type_of(&other) {
            Some(got) if got != payload && self.any_join(payload, got) != Some(got) => {
                match self.any_conversion(rhs.span, other, payload, got) {
                    // The conversion failed: its value is untyped, with the proof of its report.
                    Ok((failed, true)) => return failed,
                    Ok((converted, false)) | Err(converted) => converted,
                }
            }
            _ => other,
        };
        // A generic call that cannot meet its context joins by the type it found, as a concrete
        // call does by its type. The value stays `Error`.
        let (other, ty) = match type_of(&other).or_else(|| self.misfit_type(&other)) {
            // `a ?? panic("x")`: only the payload reaches the result.
            Some(Type::NEVER) => (other, payload),
            Some(ty) if ty == payload || ty == optional_of(payload) => (other, ty),
            // `x ?? 5` on an `i64??`: the payload of an optional `P` is wrapped to `P`.
            Some(ty) if optional_of(ty) == payload => (other.map(|e| wrap(e, payload)), payload),
            Some(ty) => {
                // An object and an `any` type that it converts to join to the `any` type.
                let Some(joined) = self.any_join(payload, ty) else {
                    let object =
                        (other.as_ref().ok()).and_then(|v| self.object_in_some(v, payload));
                    let leaves = written.and_then(|want| {
                        [(lhs, payload), (rhs, ty)]
                            .into_iter()
                            .find(|&(_, t)| !fits(want, t) && !self.converts_to_any(t, want))
                            .map(|(e, t)| (want, e, t))
                    });
                    let d = match (leaves, object) {
                        // With a written type, the first side that leaves it, as in a join.
                        (Some((want, e, t)), _) => {
                            let source = (e.span == rhs.span).then_some(rhs);
                            self.leaves_any(e.span, source, want, t, Origin::None)
                        }
                        // A class payload leaves an `any` default, as a member leaves a join.
                        _ if is_target(ty) && matches!(payload.kind(), TypeKind::Class(..)) => {
                            self.leaves_any(lhs.span, None, ty, payload, Origin::None)
                        }
                        (None, Some((at, want, object))) => {
                            self.leaves_any(at, None, want, object, Origin::None)
                        }
                        (None, None) => {
                            self.mismatch(rhs.span, Some(rhs), payload, ty, Origin::None)
                        }
                    };
                    let reported = self.diagnostics.error(d);
                    self.settle_misfit(&other);
                    return Err(Poisoned::untyped(reported));
                };
                self.instantiates = true;
                // A default that converts to the payload did so above: the payload converts to
                // this default's `any` type.
                (other, joined)
            }
            None => {
                let reported =
                    known_type(&other).expect_err("a value with no type should have failed");
                return Err(Poisoned::untyped(reported));
            }
        };
        let bind = self.fresh(payload);
        let read = local(bind, payload, lhs.span);
        let value = if read.ty == ty { read } else { widen(read, ty) };
        match (scrutinee, other) {
            (Ok(scrutinee), Ok(other)) => Ok(if_some(bind, scrutinee, value, Some(other), span)),
            (Err(failed), _) | (_, Err(failed)) => Err(Poisoned::typed(ty, failed.reported())),
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

impl Operator {
    /// The types the operator takes, in the order a mismatch lists them.
    pub(super) fn takes(self) -> &'static [Type] {
        match self {
            Operator::Compare => &[Type::I64, Type::U8, Type::F64, Type::CHAR, Type::STRING],
            Operator::Plus { .. } => &[Type::I64, Type::U8, Type::F64, Type::STRING],
            Operator::Arithmetic => &[Type::I64, Type::U8, Type::F64],
        }
    }
}

/// The types unary `-` takes. A `u8` is its own error: it cannot be negative.
pub(super) const NEGATABLE: &[Type] = &[Type::I64, Type::F64];
