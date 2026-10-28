//! Operators: arithmetic, comparison, equality, `is` and `??`.

use super::expr::{
    fits, if_some, int_literal, literal_type, local, none_literal, operator, path, rank, typed,
};
use super::interface::Operation;
use super::{
    Checked, Checker, Poisoned, UNKNOWN, as_optional, beyond_one_layer, block_expr, optional_of,
    payload, type_of, wrap,
};
use crate::ast::{self, CompareOp, ExprKind};
use crate::diagnostic::{Diagnostic, DiagnosticKind, Literal};
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
        expected: Option<Type>,
    ) -> Result<(Type, (Checked, bool)), Poisoned> {
        // A `None` expects an `i64`, as in `None + 1`.
        let expected = expected.or_else(|| none_literal(operand).then_some(Type::I64));
        let before = self.diagnostics.len();
        let checked = self.value(operand, expected);
        if let Some(ty) = type_of(&checked).filter(|ty| ty.has_param()) {
            self.param_lacks(operand.span, ty, "-", None);
            return Err(Poisoned::error());
        }
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
        if expected.map(payload) != Some(Type::I64)
            && let Some(got) = type_of(&checked)
                .filter(|&got| got != Type::CHAR && !NEGATABLE.contains(&payload(got)))
        {
            let d = self.takes_mismatch(operand.span, &NEGATABLE, got);
            self.diagnostics.push(d);
            return Ok((want, (Err(Poisoned::error()), true)));
        }
        let checked = self.against(operand, checked, Some(want), None);
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
        let (checked, ty, mismatched) = self.integers(&operands, None, Operator::Compare, &op);
        let operands = checked.into_iter().collect::<Result<Vec<_>, _>>().ok();
        match (operands, ty) {
            (Some(operands), Some(Type::STRING)) => Ok(self.string_chain(span, operands, ops)),
            (Some(operands), Some(ty)) => self.ordered_chain(span, ty, operands, ops),
            (operands, _) => typed(
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
        let Operation::Run(dispatch) = self.operation(ty, self.lang.ord) else {
            return Err(Poisoned::error());
        };
        let (stmts, operands) = self.spill(operands);
        let mut cx = lang::Cx {
            locals: &mut self.body.locals,
        };
        let kind = lang::ordered(&mut cx, dispatch, ty, operands, ops, span);
        let node = tir::Expr {
            kind,
            ty: Type::BOOL,
            span,
        };
        Ok(if stmts.is_empty() {
            node
        } else {
            block_expr(stmts, Some(node), Type::BOOL, span)
        })
    }

    /// Whether `ty` is a concrete type that no `Ord` orders, though it has the look of one that
    /// a program may compare: a `bool`, an array, an enum or a class.
    fn unordered(&mut self, ty: Type) -> bool {
        (ty == Type::BOOL
            || matches!(
                ty.kind(),
                TypeKind::Array(_) | TypeKind::Enum(..) | TypeKind::Class(..)
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
        expected: Option<Type>,
        compound: bool,
    ) -> Checked {
        let Some((tir_op, want)) = operator(op) else {
            return self.equal(span, op == ast::BinaryOp::Ne, lhs, rhs);
        };
        if want == Type::BOOL {
            let lhs = self.operand(lhs, Some(want));
            return self.arithmetic(span, (tir_op, op_span), want, lhs, rhs, Vec::new());
        }
        let operator = if tir_op == tir::BinaryOp::Add {
            Operator::Plus { compound }
        } else {
            Operator::Arithmetic
        };
        let start = self.diagnostics.len();
        let op = |_| op.to_string();
        let (checked, ty, mismatched) =
            self.binary_operands(span, &[lhs, rhs], expected, operator, &op);
        if checked.iter().any(|c| type_of(c) == Some(Type::CHAR)) {
            self.report(DiagnosticKind::ArithmeticOnChar { at: op_span });
            return Err(Poisoned::error());
        }
        let [l, r]: [Checked; 2] = checked
            .try_into()
            .unwrap_or_else(|_| panic!("`integers` should return one result per operand"));
        if ty == Some(Type::STRING) {
            self.label_join_literals(start, [lhs, rhs]);
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
        op: &dyn Fn(usize) -> String,
    ) -> (Vec<Checked>, Option<Type>, bool) {
        let joins = operator == (Operator::Plus { compound: false });
        if !joins || literal_type(expected) == Type::I64 {
            return self.integers(operands, expected, operator, op);
        }
        if self.body.joins.contains(&span) {
            return self.integers(operands, None, operator, op);
        }
        let (diagnostics, locals, pending) = (
            self.diagnostics.len(),
            self.body.locals.len(),
            self.pending.len(),
        );
        let result = self.integers(operands, expected, operator, op);
        if result.1 != Some(Type::STRING) {
            return result;
        }
        // The first check is dropped, and its locals and their findings with it, so none is left
        // that nothing binds, and none is judged twice.
        self.diagnostics.truncate(diagnostics);
        self.body.locals.truncate(locals);
        self.pending.truncate(pending);
        self.body.joins.insert(span);
        self.integers(operands, None, operator, op)
    }

    /// The operands of one number operator, checked in source order; the type they share, `None`
    /// when it is `Error`; and whether one was reported against it. The type is `u8` when
    /// `expected` is `u8` or `u8?`, and `f64` when it is `f64` or `f64?`. Otherwise the first
    /// concrete operand (of `rank` 0) is checked first, with no expected type, and gives it: `u8`
    /// beside a `u8`, `f64` beside an `f64`, `char` beside a `char` in a comparison, else `i64`;
    /// beside a `char` in arithmetic that `i64` is not passed on, and the later operands are
    /// checked with no expected type. Where `expected` is neither `i64` nor `i64?`, that operand,
    /// if it is none of the types the operator takes, is reported against all of them, not against
    /// `i64`, unless a `string` or `char` rule below reports it, an earlier operand has failed, or
    /// the first operand in source order is an integer literal. With none, it is `i64`, so
    /// `None + 1` expects an `i64`. An `f64` after an `i64` or `u8` first operand makes the type
    /// `f64`, and that operand is reported against it, as an operand before a `string` is in `+`.
    /// When `expected` is `i64` or `i64?`, an `f64` first operand gives `i64`, not `f64`, and that
    /// first operand is checked against `expected`, so an `f64` is reported in either order and at
    /// any depth. Once an operand is `Error`, the later ones are checked against `Error`. An
    /// operand of `arithmetic` that is a `char` is left unreported, as the poison `typed` `char`:
    /// its caller reports the operator, so what the operator's type finds wrong with the other
    /// operands is dropped, in either order. In a comparison, a `char` after a number reports that
    /// number, as a `char` before one reports the number.
    ///
    /// The type is `string` beside a `string`, in a comparison and in `+`. In `+` a `string`
    /// drives from either side: an operand of another type is reported against `string`, with
    /// the unsafe fix to interpolate, the one before a `string` included, in place of what its own
    /// type was found to lack. A `string?` first operand makes the `+` a join and is reported
    /// against `string`, its label under the other operand only if that is a `string`. If
    /// operands come before a `string?` leader, they are checked with no expected type, and the
    /// `string?` is the operand reported, as when it comes first. A join
    /// after an `Error` is `Error`. An expected `u8` or `f64` is `binary_operands`' to drop.
    ///
    /// The first operand whose type names a type parameter is reported, as lacking `op` of its
    /// index, and makes the type `Error`.
    #[expect(
        clippy::too_many_lines,
        reason = "one pass over the operands, so the rules that depend on each other read in one place"
    )]
    fn integers(
        &mut self,
        operands: &[&ast::Expr],
        expected: Option<Type>,
        operator: Operator,
        op: &dyn Fn(usize) -> String,
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
        // A class, an enum or a type parameter with `Ord` is compared through it, and the operands
        // after the first are held to its type.
        let by_ord = match first.as_ref().and_then(|f| type_of(&f.1)) {
            Some(leader) if operator == Operator::Compare && self.compares_through_ord(leader) => {
                ty = Some(leader);
                Some(leader)
            }
            _ => None,
        };
        // A leader with no `<` is the mistake: the operands before it are not held to its type.
        if operator == Operator::Compare
            && by_ord.is_none()
            && let Some(leader) = first.as_ref().and_then(|f| type_of(&f.1))
            && self.unordered(leader)
        {
            ty = None;
        }
        let (mut want, leader) = (ty, first.as_ref().map(|(k, _)| *k));
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
        let takes =
            (!wants_i64 && !int_literal(operands[0]) && by_ord.is_none()).then(|| operator.takes());
        let by_char = arithmetic && first.as_ref().and_then(|f| type_of(&f.1)) == Some(Type::CHAR);
        let (mut checked, mut mismatched) = (Vec::with_capacity(operands.len()), false);
        // The diagnostics of the operator's type: mismatches, and a literal's, which that type
        // types. They wait until a `char` operand is ruled out.
        let mut deferred = Vec::new();
        // The type of the operand checked last, as it was found, before it was held to `want`; and
        // whether a concrete operand was `Error` as found, which its own diagnostic reported.
        let (mut before, mut failed) = (None, false);
        // A mismatch of the first operand, in `deferred` at the index, that waits to be labelled
        // by the second operand, if that is a `string`.
        let mut unlabelled = None;
        let mut lacks = false;
        // The type under every `?` of the first operand that has one, whether each later one has
        // it too, and where the report of a lacking `Ord` stands.
        let (mut shared, mut one_type) = (None, true);
        let mut helped = None;
        for (i, e) in operands.iter().enumerate() {
            if held && leader == Some(i) {
                want = ty;
            }
            let start = self.diagnostics.len();
            let value = match first.take_if(|(k, _)| *k == i) {
                Some((_, operand)) => operand,
                None => self.value(e, (!by_char).then(|| want.unwrap_or(UNKNOWN))),
            };
            if rank(e) != 0 {
                deferred.extend(self.diagnostics.drain(start..));
            }
            let char_value = type_of(&value) == Some(Type::CHAR);
            let got = type_of(&value);
            if let Some(ty) = got.map(payload) {
                one_type &= *shared.get_or_insert(ty) == ty;
            }
            // The leader of a comparison that no `Ord` orders has no `<`, though it is a concrete
            // type: an array, an enum, a class or a `bool`. A later operand of another type is
            // a mismatch, which `settled_by` reports.
            let unordered = |c: &mut Self, ty: Type| {
                !arithmetic && leader == Some(i) && by_ord.is_none() && c.unordered(ty)
            };
            let lacking =
                got.filter(|&ty| (ty.has_param() && by_ord != Some(ty)) || unordered(self, ty));
            if let Some(param) = lacking {
                if !std::mem::replace(&mut lacks, true) {
                    // Reported with no help here, where the operands after it have not been
                    // checked; the help comes below, once they are known to be one type.
                    helped = (!arithmetic).then_some((self.diagnostics.len(), e.span, param, i));
                    if arithmetic {
                        self.param_lacks(e.span, param, op(i), None);
                    } else {
                        let ord = self.lang.ord;
                        let d = self.missing_operator(e.span, param, &op(i), ord, false, false);
                        // A type with no `<` makes the chain `Error`, as a mismatch does.
                        mismatched |= matches!(d, DiagnosticKind::NoOperator { .. });
                        self.report(d);
                    }
                }
                checked.push(Err(Poisoned::error()));
                (want, ty, before) = (None, None, None);
                continue;
            }
            if let (Some(at), Some(found)) = (unlabelled.take(), before) {
                deferred[at] = self.concat_mismatch(operands[0], found, operands, compound, got);
            }
            let joins = matches!(operator, Operator::Plus { .. });
            let (operand, m) = if let Some(got) =
                got.filter(|&got| joins && want == Some(Type::STRING) && got != Type::STRING)
            {
                // The other operand's type, if it is checked already: the first's is `before`, and
                // a second that is the leader waits in `first`.
                let beside = first.as_ref().map_or(before, |f| type_of(&f.1));
                unlabelled = beside.is_none().then_some(deferred.len());
                deferred.push(self.concat_mismatch(e, got, operands, compound, beside));
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
                    // Beside the first operand is this `string`, which `ty` now is.
                    let d = self.concat_mismatch(operands[0], other, operands, false, ty);
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
                self.reject_leader(operands[k], &mut checked[k], Type::CHAR);
                want = None;
                (Err(Poisoned::error()), true)
            } else if let Some(k) = leader.filter(|_| {
                got == Some(Type::F64) && matches!(want, Some(Type::I64 | Type::U8)) && !wants_i64
            }) {
                // An `f64` after an integer: the integer is the one converted, and the operands
                // after this one are held to `f64`.
                self.reject_leader(operands[k], &mut checked[k], Type::F64);
                ty = Some(Type::F64);
                want = Some(Type::F64);
                (value, true)
            } else {
                let start = self.diagnostics.len();
                let settled = self.settled_by(e, value, want, takes.filter(|_| leader == Some(i)));
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
        // A bound helps only when the operands are one type: else the mistake is the operands.
        if let Some((report, at, param, i)) = helped
            && one_type
        {
            let ord = self.lang.ord;
            self.diagnostics[report] = self
                .missing_operator(at, param, &op(i), ord, false, true)
                .into();
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

    /// The operand `e`, checked as `checked`, reported against `ty`, the type of a later operand,
    /// and dropped to `Error`.
    fn reject_leader(&mut self, e: &ast::Expr, checked: &mut Checked, ty: Type) {
        if let Some(found) = type_of(checked) {
            let d = self.mismatch(e.span, Some(e), ty, found);
            self.diagnostics.push(d);
        }
        *checked = Err(Poisoned::error());
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
        held: Vec<Diagnostic>,
    ) -> Checked {
        let expected = type_of(&l).and(Some(want));
        let checked = self.value(rhs, Some(expected.unwrap_or(UNKNOWN)));
        // Only a compound assignment's number gets here with a `char` to its right.
        if want != Type::BOOL && type_of(&checked) == Some(Type::CHAR) {
            self.report(DiagnosticKind::ArithmeticOnChar { at: op_span });
            return Err(Poisoned::error());
        }
        self.diagnostics.extend(held);
        let (r, r_mismatched) = self.settled(rhs, checked, expected);
        let kind = l.ok().zip(r.ok()).map(|(l, r)| tir::ExprKind::Binary {
            op,
            lhs: Box::new(l),
            rhs: Box::new(r),
        });
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
        if let Some(takes) = takes
            && want.is_some()
            && let Some(got) = type_of(&checked).filter(|&got| !takes.contains(&payload(got)))
        {
            let d = self.takes_mismatch(e.span, takes, got);
            self.diagnostics.push(d);
            return (Err(Poisoned::error()), true);
        }
        self.settled(e, checked, want)
    }

    /// "expected `takes`, found `got`" at `at`: the types an operator takes, `got` not among them.
    pub(super) fn takes_mismatch(&self, at: Span, takes: &[Type], got: Type) -> Diagnostic {
        DiagnosticKind::Mismatch {
            want: takes.iter().map(|&ty| self.type_name(ty)).collect(),
            got: self.type_name(got),
            at,
            source: None,
            fix: None,
            conversion: false,
            layers: false,
        }
        .into()
    }

    /// `lhs == rhs`, or `!=`. The side of lower `rank` is checked first, the left on a tie
    /// unless it is the only `none_literal`, and the other expects its type made optional,
    /// unless it is one. A side that is the other's type with two or more `?` layers taken off
    /// is held to it. A value converts through one `?` only. A `None` beside a value that is not
    /// optional is reported: the answer is fixed. So is a `u8` beside an `i64`, at the side
    /// checked second, a `char` beside another number, at the side that is not the `char`, and
    /// an `i64` or a `u8` beside an `f64`, at the side that is not the `f64`. A side that is
    /// `f64.nan` is warned about.
    fn equal(&mut self, span: Span, negated: bool, lhs: &ast::Expr, rhs: &ast::Expr) -> Checked {
        let swapped = rank(rhs) < rank(lhs) || (none_literal(lhs) && !none_literal(rhs));
        let (first, second) = if swapped { (rhs, lhs) } else { (lhs, rhs) };
        let checked = self.value(first, None);
        let want = type_of(&checked).map_or_else(|| UNKNOWN, as_optional);
        let other = self.value(second, Some(want));
        // A side converts through one `?` only: `x == 5` on an `i64??` holds `5` to `i64??`.
        if let (Some(a), Some(b)) = (type_of(&checked), type_of(&other))
            && let Some((side, want, got)) = [(second, a, b), (first, b, a)]
                .into_iter()
                .find(|&(_, want, got)| beyond_one_layer(want, got))
        {
            let d = self.mismatch(side.span, Some(side), want, got);
            self.diagnostics.push(d);
            return Err(Poisoned::error());
        }
        // `second` is the `none_literal`, if either is.
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
        // `x == None` tests an optional, which any `T?` is, and needs no `Eq`. Else a side whose
        // type has no `Eq` has no `==`, unless the two types differ for good: E0047 reports that.
        // A side with no type (its error is reported) does not say that the other needs `Eq`,
        // except a type parameter, which always did.
        let eq = self.lang.eq;
        // When the two sides have one type, its operation is asked once, for the check and the
        // call.
        let both = match (type_of(&l), type_of(&r)) {
            (Some(lt), Some(rt)) if lt == rt => Some((lt, self.operation(lt, eq))),
            _ => None,
        };
        if !none_literal(second) {
            let same = match (type_of(&l), type_of(&r)) {
                (Some(lt), Some(rt)) => lt == rt || fits(lt, rt) || fits(rt, lt),
                _ => false,
            };
            for (side, checked) in [(lhs, &l), (rhs, &r)] {
                let Some(ty) = type_of(checked).filter(|ty| same || ty.has_param()) else {
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
                // A bound helps only when the sides are one type: else the mistake is the operands.
                let alike = type_of(&l)
                    .zip(type_of(&r))
                    .is_none_or(|(a, b)| payload(a) == payload(b));
                let d = self.missing_operator(side.span, ty, op, eq, false, alike);
                self.report(d);
                return Err(Poisoned::error());
            }
        }
        // A class or an enum with a written `Eq` is compared by a call of its `eq`. Any other
        // type keeps the node that `==` has always had. A type with no `Eq` reaches here only
        // in a comparison with `None`, which tests presence.
        let dispatch = match both {
            Some((_, Operation::Run(dispatch))) => dispatch,
            Some((_, Operation::Poison)) => return Err(Poisoned::error()),
            _ => Dispatch::Generated,
        };
        self.nan_comparison(negated, (lhs, &l), (rhs, &r));
        self.compared(span, l, r, |lhs, rhs| {
            lang::equal(dispatch, negated, lhs, rhs, span)
        })
    }

    /// `lhs is rhs`: each a class or its optional, of one class, or a type parameter that is
    /// bound to `AnyObject` or its optional. An enum value has no identity to test: two equal ones
    /// are the same value. An optional of an optional object is no object: one layer comes off
    /// first.
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
            // The type under every `?` layer.
            let mut base = ty;
            while let TypeKind::Optional(inner) = base.kind() {
                base = inner;
            }
            // A type parameter is an object when its bounds say so: `mono` then makes it a class.
            let object_param = matches!(base.kind(), TypeKind::Param(..))
                && self.conforms(base, self.lang.any_object, TypeArgs::NONE);
            let nested = matches!(ty.kind(), TypeKind::Optional(p) if matches!(p.kind(), TypeKind::Optional(_)));
            // Peeling a layer helps only when an object is under the rest.
            if nested && (matches!(base.kind(), TypeKind::Class(..)) || object_param) {
                let text = |e: &ast::Expr| self.source.file.text(e.span).to_string();
                let peeled = |e: &ast::Expr| {
                    if std::ptr::eq(e, side) {
                        format!("({} ?? None)", text(e))
                    } else {
                        text(e)
                    }
                };
                let kind = DiagnosticKind::IsNeedsObjects {
                    found: self.type_name(ty),
                    at: side.span,
                    help: false,
                    add: None,
                    peel: Some(format!("{} is {}", peeled(lhs), peeled(rhs))),
                };
                self.diagnostics.push(kind.into());
                return Err(Poisoned::error());
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
            drop(self.fallback(rhs, UNKNOWN));
            return Err(Poisoned::error());
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
            self.report(DiagnosticKind::CannotInfer {
                literal: Literal::None,
                binding: None,
                at: none.span,
                regroup: Some(lhs.span.start()),
            });
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
            drop(self.fallback(tail, UNKNOWN));
            return Err(Poisoned::error());
        }
        // The right side expects `P` for an optional `P`, else `P?`: a value converts through
        // one `?` only.
        let other = self.fallback(rhs, as_optional(payload));
        let (other, ty) = match type_of(&other) {
            // `a ?? panic("x")`: only the payload reaches the result.
            Some(Type::NEVER) => (other, payload),
            Some(ty) if ty == payload || ty == optional_of(payload) => (other, ty),
            // `x ?? 5` on an `i64??`: the payload of an optional `P` is wrapped to `P`.
            Some(ty) if optional_of(ty) == payload => (other.map(|e| wrap(e, payload)), payload),
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
pub(super) const NEGATABLE: [Type; 2] = [Type::I64, Type::F64];
