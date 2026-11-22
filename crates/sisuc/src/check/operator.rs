//! Operators: arithmetic, comparison, equality, `is` and `??`.

use super::branch::whole_members;
use super::expr::{fits, if_some, local, open_any, operator, path, takes_unit, typed};
use super::generic::mismatched;
use super::infer::Kind;
use super::infer::LiteralCheck;
use super::interface::Operation;
use super::join::{Heading, JoinForm, JoinMember, Place};
use super::reports::{Held, Reported};
use super::{
    Checked, Checker, Expectation, Lowered, Origin, Poisoned, as_optional, beyond_one_layer,
    block_expr, known_type, payload, type_of,
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
    #[expect(
        clippy::too_many_lines,
        reason = "each form of the operand, and the join it waits for, reads in one place"
    )]
    pub(super) fn negated(
        &mut self,
        span: Span,
        operand: &ast::Expr,
        expected: Expectation,
    ) -> Result<(Type, (Checked, bool)), Poisoned> {
        // A member's `-` of a literal is a literal of an `Int` variable, as `integer` makes.
        if let (Expectation::Member, &ExprKind::Int { value, literal }) = (expected, &operand.kind)
        {
            let ty = self.int_variable(LiteralCheck {
                span: operand.span,
                literal,
                value: value.unsigned_abs(),
                negated: Some(span),
            });
            let literal = tir::Expr::new(tir::ExprKind::Int(value), ty, operand.span);
            return Ok((ty, (Ok(literal), false)));
        }
        // The operand is a member: `-` converts at its top. A `None` expects an `i64`, as in
        // `None + 1`.
        let given = if self.none_written(operand) {
            Expectation::Required(Type::I64, Origin::None)
        } else {
            Expectation::Member
        };
        let before = self.diagnostics.mark();
        let checked = self.value(operand, given);
        // No `-` takes a `unit`, and the operand may have no expected type.
        let checked = self.refuse_unit(operand, checked);
        // An open operand under a required number takes it, and is then checked as a closed one.
        // Any other open operand waits for its type: `-` gives that type, and the check of a `u8`
        // waits with it. An `Any` variable is an integer here, as no other open type has a `-`.
        let mut checked = checked;
        // An open operand is a join of one member: a nested join's members take part.
        if let Some(ty) = type_of(&checked)
            .filter(|&ty| matches!(self.table.resolve(ty).kind(), TypeKind::Infer(_)))
        {
            // A conflict of the branches of an `if` or a `match` is theirs: it reports at their
            // head, as with no `-`.
            let (form, head) = self
                .own_join(operand)
                .unwrap_or_else(|| (JoinForm::Operator, self.source.head(span, "-".len())));
            let values = std::slice::from_mut(&mut checked);
            match self.join_operands(form, head, (&[operand], values), None, |_, _, _| None) {
                Ok(joined) => {
                    self.unify(ty, joined);
                }
                Err(reported) => return Err(Poisoned::untyped(reported)),
            }
        }
        // A failed operand keeps its type for the checks below, as a valid one does.
        match &mut checked {
            Ok(value) => value.ty = self.table.resolve(value.ty),
            Err(failed) => failed.ty = failed.ty.map(|ty| self.table.resolve(ty)),
        }
        if let Some(ty) = type_of(&checked)
            && matches!(ty.kind(), TypeKind::Infer(_))
        {
            let number = (expected.ty().map(payload))
                .filter(|&t| matches!(t, Type::I64 | Type::U8 | Type::F64));
            // The operand of `-(-x)` already waits with the check of its own `-`.
            let negation = matches!(
                operand.kind,
                ExprKind::Unary {
                    op: ast::UnaryOp::Neg,
                    ..
                }
            );
            if let Some(number) = number
                && !negation
                && self.unify(ty, number)
            {
                match &mut checked {
                    Ok(value) => value.ty = number,
                    Err(failed) => failed.ty = Some(number),
                }
            } else {
                let check = LiteralCheck {
                    span,
                    literal: operand.span,
                    value: 0,
                    negated: Some(span),
                };
                let ty = if self.table.default_open(ty) == Type::I64 {
                    if !negation {
                        self.table.literal(ty, check);
                    }
                    ty
                } else {
                    let int = self.int_variable(check);
                    self.unify(ty, int);
                    int
                };
                let ty = self.close_number(ty, (span, span), expected)?;
                return Ok((ty, (checked, false)));
            }
        }
        // `-` converts at its top, so an operand that is still open in a part, as `-mk()` with
        // `mk<X>() -> [X]`, closes here: E0050 for a call that nothing binds.
        let checked = self.close_open(checked, Expectation::None);
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
            // `i64(` and `)` give an `i64`, so the context must expect one or nothing.
            let convert = (!literal
                && expected != Expectation::Member
                && matches!(expected.ty(), None | Some(Type::I64)))
            .then_some(operand.span);
            let reported = self
                .diagnostics
                .error(DiagnosticKind::NegatedU8 { at: span, convert });
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
                .map(|got| self.table.default_open(got))
                .filter(|&got| got != Type::CHAR && !NEGATABLE.contains(&payload(got)))
        {
            let d = self.takes_mismatch(operand.span, NEGATABLE, got);
            let reported = self.diagnostics.error(d);
            return Ok((want, (Err(Poisoned::untyped(reported)), true)));
        }
        let origin = expected.origin_for(Some(want));
        let checked = self.close(operand, checked, Expectation::Required(want, origin));
        Ok((want, checked))
    }

    /// The form and the head of the join of branches or of elements that `e` is, at its value.
    pub(super) fn own_join(&self, e: &ast::Expr) -> Option<(JoinForm, Span)> {
        match &e.kind {
            ExprKind::If {
                else_block: Some(_),
                ..
            }
            | ExprKind::IfLet {
                else_block: Some(_),
                ..
            } => Some((JoinForm::If, self.source.head(e.span, "if".len()))),
            ExprKind::Match { .. } => {
                Some((JoinForm::Match, self.source.head(e.span, "match".len())))
            }
            ExprKind::ArrayLit(elements) if !elements.is_empty() => {
                Some((JoinForm::Array, self.source.head(e.span, "[".len())))
            }
            ExprKind::Block(block) => super::tail(block).and_then(|tail| self.own_join(tail)),
            _ => None,
        }
    }

    /// Closes `ty`, the open type of the number that an operator at `at` gives. A member keeps it
    /// open. A required number gives it that number, so that a literal check that waits on it is
    /// reported. Any other context closes it as a join does (see `close_join`), and the caller
    /// takes the value to a required type. `whole` is the span of the operands.
    fn close_number(
        &mut self,
        ty: Type,
        (at, whole): (Span, Span),
        expected: Expectation,
    ) -> Result<Type, Poisoned> {
        match expected {
            Expectation::Member => Ok(ty),
            Expectation::Required(want, _) if self.unify(ty, payload(want)) => {
                Ok(self.table.resolve(ty))
            }
            Expectation::Required(..) => self.close_join(ty, (at, whole), Expectation::None),
            _ => self.close_join(ty, (at, whole), expected),
        }
    }

    /// The join of the operands `es` of the operator at `head`, its token, checked as `values`:
    /// each is a member, so a nested join's members take part. Operands of one closed type join
    /// with no work. A conflict finds each operand's own type (its join alone, open variables at
    /// their defaults) and gives them to `kind`: a report of `kind` is the operator's, in place of
    /// the conflict. Else a required `number` reports each operand that does not convert to it
    /// (Q33). Else the conflict reports at `head` (ADR 0052).
    pub(super) fn join_operands(
        &mut self,
        form: JoinForm,
        head: Span,
        (es, values): (&[&ast::Expr], &mut [Checked]),
        number: Option<(Type, Origin)>,
        kind: impl FnOnce(&mut Self, &mut [Checked], &[Option<Type>]) -> Option<Reported>,
    ) -> Result<Type, Reported> {
        let types: Vec<Option<Type>> = (values.iter())
            .map(|v| v.as_ref().ok().map(|v| self.table.resolve(v.ty)))
            .collect();
        if let [Some(first), rest @ ..] = types.as_slice()
            && !first.holds_infer()
            && rest.iter().all(|ty| *ty == Some(*first))
        {
            return self.valued(values, *first);
        }
        // Operands of one closed number type and open `Int`s, as `i + 1`, join as they unify. A
        // literal that does not fit reports when its variable settles, as in the join.
        let closed = |ty: &Option<Type>| matches!(*ty, Some(Type::I64 | Type::U8 | Type::F64));
        let int = |c: &Self, ty: &Option<Type>| {
            ty.is_some_and(|ty| {
                matches!(ty.kind(), TypeKind::Infer(_)) && c.table.default_open(ty) == Type::I64
            })
        };
        if let Some(Some(target)) = (types.iter().find(|ty| closed(ty))).or_else(|| types.first())
            && (types.iter()).all(|ty| (*ty == Some(*target) && closed(ty)) || int(self, ty))
        {
            for ty in (types.iter().flatten()).filter(|ty| *ty != target) {
                let unified = self.unify(*ty, *target);
                debug_assert!(unified, "an `Int` should unify with a number or an `Int`");
            }
            return self.valued(values, self.table.resolve(*target));
        }
        let spans: Vec<Span> = es.iter().map(|e| e.span).collect();
        let heading = Heading {
            head,
            whole: spans[0].to(spans[spans.len() - 1]),
            returns: None,
        };
        let mut members = whole_members(values, &spans);
        let (joined, parked) = self.join_form_holding(
            form,
            (heading, |_: &mut Self| None),
            &mut members,
            (Expectation::Member, None),
            true,
        );
        let Some(parked) = parked else {
            let ty = joined.map_err(|failed| failed.reported())?;
            return self.valued(values, ty);
        };
        let own: Vec<Option<Type>> = (values.iter_mut().zip(&spans))
            .map(|(value, &label)| self.member_own_type(value, label))
            .collect();
        if let Some(reported) = kind(self, values, &own) {
            return Err(reported);
        }
        if let Some((ty, origin)) = number {
            let mut members = whole_members(values, &spans);
            let required = Expectation::Required(ty, origin);
            let joined = self.join_form(form, heading, &mut members, required, None);
            return joined.map_err(|failed| failed.reported());
        }
        let reported = joined
            .expect_err("a held conflict should fail its join")
            .reported();
        self.diagnostics.restore(parked);
        Err(reported)
    }

    /// `ty`, the type that the operands `values` join to, or E0043 at a valueless `if` in one of
    /// them when it is `unit`: such an `if` has no value (see `refuse_settled_valueless`).
    fn valued(&mut self, values: &mut [Checked], ty: Type) -> Result<Type, Reported> {
        if self.table.resolve(ty) == Type::UNIT
            && let Some(reported) =
                (values.iter_mut()).find_map(|value| self.refuse_settled_valueless(value))
        {
            return Err(reported);
        }
        Ok(ty)
    }

    /// The own type of the member `value` at `label`: its join alone, open variables at their
    /// defaults. Nothing is bound.
    fn member_own_type(&mut self, value: &mut Checked, label: Span) -> Option<Type> {
        let mut member = [JoinMember {
            value,
            at: Place::Whole,
            label,
        }];
        let ty = self.join_type(&mut member, None, None)?;
        // The variable of a nested `if` is the type its branches bind it to.
        let snapshot = self.table.snapshot();
        self.member_converts(&mut member[0], ty, false);
        let ty = self.table.resolve(ty);
        self.table.rollback(snapshot);
        Some(self.table.default_open(ty))
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
        // The join reports at the first operator token.
        let gap = Span::new(operands[0].span.end(), operands[1].span.start());
        let text = ops[0].text();
        let head = (self.source.text(gap).find(text)).map_or(gap, |at| {
            Span::new(gap.start() + at, gap.start() + at + text.len())
        });
        let (checked, ty, mismatched) = self.integers(
            &operands,
            (head, Expectation::None),
            Operator::Compare,
            (&op, None),
        );
        let operands = checked.into_iter().collect::<Result<Vec<_>, _>>();
        match (operands, ty) {
            (Ok(operands), Some(Type::STRING)) if !mismatched => {
                Ok(self.string_chain(span, operands, ops))
            }
            (Ok(operands), Some(ty)) if !mismatched => self.ordered_chain(span, ty, operands, ops),
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

    /// Whether comparison operators run the interface `Ord` for the first operand's type `ty`: a
    /// type that has it, or has it through a broken block.
    fn compares_through_ord(&mut self, ty: Type) -> bool {
        matches!(
            self.operation(ty, self.lang.ord),
            Operation::Run(_) | Operation::Poison
        )
    }

    /// `lhs op rhs`. A logic operator holds each side to `bool`. A number operator joins its two
    /// operands as members (see `integers`). A compound operator stands for a compound
    /// assignment, a statement that a rewrite of the operator would break. `op` comes with its
    /// token's span, where the join reports and where arithmetic on a `char` is reported.
    pub(super) fn binary(
        &mut self,
        span: Span,
        (op, op_span): (ast::BinaryOp, Span),
        lhs: &ast::Expr,
        rhs: &ast::Expr,
        expected: Expectation,
        (compound, read): (bool, Option<Checked>),
    ) -> Checked {
        let Some((tir_op, want)) = operator(op) else {
            return self.equal(span, (op == ast::BinaryOp::Ne, op_span), lhs, rhs);
        };
        if want == Type::BOOL {
            let lhs = self.operand(lhs, Expectation::Required(want, Origin::None));
            return self.arithmetic(span, (tir_op, op_span), want, lhs, rhs, Held::default());
        }
        let operator = if tir_op == tir::BinaryOp::Add {
            Operator::Plus { compound }
        } else {
            Operator::Arithmetic { compound }
        };
        let start = self.diagnostics.len();
        let op = |_| op.to_string();
        let (checked, ty, mismatched) =
            self.integers(&[lhs, rhs], (op_span, expected), operator, (&op, read));
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
        // Only an untyped operand is a failed part: a typed poisoned one reads as its type.
        // `integers` gives the type that the others decide, if any ("Poisoned members").
        if ty == Some(Type::STRING) {
            self.label_join_literals(start, [lhs, rhs]);
            return self.concat(span, (l, true), r, mismatched);
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
        match ty {
            Some(ty) => typed(span, ty, mismatched, kind),
            // An operand that decides nothing has failed, and so has one that leaves no type.
            None => Err(Poisoned::untyped(kind.expect_err(
                "an operator with no type should have a failed operand",
            ))),
        }
    }

    /// The operands of one number operator, its token at `head`, each checked as a member of one
    /// join (ADR 0052); the type they join to, `None` when it is `Error`; and whether a rule of
    /// the operator reported. A `None` expects that number, else `i64`.
    ///
    /// Before the join, each operand of a closed type meets the rules of the operator's kind
    /// (`kind_rules`). A conflict meets them again with each operand's own type. Else under a
    /// required number each operand that does not convert to it is reported (Q33), and else the
    /// conflict reports at `head`.
    ///
    /// Arithmetic gives its type open to a member and to a required number, which it meets,
    /// and else closes it as a join does. A comparison closes its type: a `string` and a type
    /// with `Ord` compare, and any other type lacks `<`.
    #[expect(
        clippy::too_many_lines,
        reason = "one pass over the operands, so the rules that depend on each other read in one place"
    )]
    fn integers(
        &mut self,
        operands: &[&ast::Expr],
        (head, expected): (Span, Expectation),
        operator: Operator,
        (op, mut read): (&dyn Fn(usize) -> String, Option<Checked>),
    ) -> (Vec<Checked>, Option<Type>, bool) {
        let arithmetic = operator != Operator::Compare;
        let whole =
            (operands.first().zip(operands.last())).map_or(head, |(a, b)| a.span.to(b.span));
        // A compound assignment's conflict reports at its operator: its place does not convert.
        let number = match expected {
            Expectation::Required(ty, origin)
                if arithmetic
                    && !operator.compound()
                    && matches!(payload(ty), Type::I64 | Type::U8 | Type::F64) =>
            {
                Some((payload(ty), origin))
            }
            _ => None,
        };
        let (ty, origin) = number.unwrap_or((Type::I64, Origin::None));
        let none = Expectation::Required(ty, origin);
        // Arithmetic of integer literals only, under a required number or with no context, has
        // that number or `i64`. So its operands take it at once, with no `Int` variables
        // (`1 + 1 + 1`). A literal that does not fit the number stays a member, and reports as
        // one does.
        let mut literals = match expected {
            _ if !arithmetic || operator.compound() => None,
            Expectation::None => Some((Type::I64, Origin::None)),
            _ => number,
        };
        if literals.is_some() && !operands.iter().all(|e| self.int_literal(e)) {
            literals = None;
        }
        let mut checked: Vec<Checked> = Vec::with_capacity(operands.len());
        for e in operands {
            // The place of a compound assignment is read once, before its value.
            if let Some(read) = read.take() {
                checked.push(read);
                continue;
            }
            checked.push(if self.none_operand(e) {
                Ok(unit_value(e.span)) // replaced below
            } else {
                let expected = match (literals, &e.kind) {
                    (Some((ty, _)), ExprKind::Int { .. }) => self.beside_closed(e, Some(ty)),
                    (Some((ty, origin)), ExprKind::Binary { .. }) => {
                        Expectation::Required(ty, origin)
                    }
                    _ => self.beside_closed(e, checked.first().and_then(type_of)),
                };
                self.value(e, expected)
            });
        }
        // A `None` has no number to be. Beside a `char` the operator is the mistake. Beside a
        // `unit` a comparison holds it to `unit`. Beside a `string?` it is one, so that the
        // `string?` is the mistake. Else it meets the operator's number.
        let others: Vec<Type> = (operands.iter().zip(&checked))
            .filter(|(e, _)| !self.none_operand(e))
            .filter_map(|(_, c)| type_of(c))
            .collect();
        let want = if arithmetic && others.contains(&Type::CHAR) {
            Expectation::Member
        } else if !arithmetic && others.contains(&Type::UNIT) {
            Expectation::Required(Type::UNIT, Origin::None)
        } else if let Some(&ty) = others
            .iter()
            .find(|&&t| t != payload(t) && !t.holds_infer())
        {
            Expectation::Required(ty, Origin::None)
        } else {
            none
        };
        for (i, e) in operands.iter().enumerate() {
            if self.none_operand(e) {
                checked[i] = self.value(e, want);
            }
        }
        if arithmetic {
            for (e, c) in operands.iter().zip(&mut checked) {
                *c = self.refuse_unit(e, std::mem::replace(c, Ok(unit_value(e.span))));
            }
        }
        let own: Vec<Option<Type>> = (checked.iter())
            .map(|c| type_of(c).map(|ty| self.table.resolve(ty)))
            .map(|ty| ty.filter(|ty| !ty.holds_infer()))
            .collect();
        // An untyped operand is left out of the join. The others join, so that a nested join in
        // them settles. Their join types the operator only when one of them states its type
        // ("Poisoned members"): `a + nope` with `a: u8` is `u8`. The untyped operand may have
        // been meant as a `string`, so the rules of the operator's kind wait.
        if let Some(reported) = checked.iter().find_map(untyped) {
            // A `None` that meets no number is the operator's mismatch, as `1 < None` is.
            let none_failed =
                (operands.iter().zip(&checked)).any(|(e, c)| c.is_err() && self.none_operand(e));
            let typed: Vec<usize> = (0..checked.len())
                .filter(|&i| untyped(&checked[i]).is_none())
                .collect();
            let mut joined = None;
            if !typed.is_empty() {
                let es: Vec<&ast::Expr> = typed.iter().map(|&i| operands[i]).collect();
                let mut values: Vec<Checked> = (typed.iter())
                    .map(|&i| std::mem::replace(&mut checked[i], Err(Poisoned::untyped(reported))))
                    .collect();
                // A lone operand that is a join of its own reports as that join.
                let (form, head) = (typed.len() == 1)
                    .then(|| self.own_join(es[0]))
                    .flatten()
                    .unwrap_or_else(|| (operator.join_form(), head));
                // Its report, if any, is a true one.
                joined = (self.join_operands(form, head, (&es, &mut values), None, |_, _, _| None))
                    .ok()
                    .map(|ty| self.table.resolve(ty))
                    .filter(|ty| !ty.holds_infer() && typed.iter().any(|&i| own[i].is_some()));
                for (&i, value) in typed.iter().zip(values) {
                    checked[i] = value;
                }
            }
            self.poison_open(&checked);
            if !arithmetic {
                // `a < nope` of a `T` with no `Ord` is a mistake whatever `nope` is. An operand
                // that rejected a type inside it is the first mistake.
                let inner =
                    (checked.iter()).any(|c| matches!(c, Err(Poisoned { mismatch: true, .. })));
                if let Some(ty) = joined.filter(|_| !inner)
                    && !(ty == Type::STRING || ty == Type::NEVER || self.compares_through_ord(ty))
                {
                    let d = self.missing_operator(
                        operands[typed[0]].span,
                        ty,
                        &op(typed[0]),
                        self.lang.ord,
                    );
                    self.diagnostics.error(d);
                }
                return (fail_all(checked, reported), None, none_failed);
            }
            if let Some(ty) = joined.filter(|ty| operator.takes().contains(ty)) {
                return (checked, Some(ty), false);
            }
            return (fail_all(checked, reported), None, none_failed);
        }
        if !arithmetic && own.contains(&Some(Type::UNIT)) {
            return self.units_compared(operands, checked);
        }
        // The caller reports the operator.
        if arithmetic && on_char(operator, &own) {
            return (checked, Some(Type::CHAR), false);
        }
        if self.kind_rules(operands, (&mut checked, &own), operator, op, false) {
            self.poison_open(&checked);
            return (checked, None, true);
        }
        let errors = self.diagnostics.errors();
        let joined = self.join_operands(
            operator.join_form(),
            head,
            (operands, &mut checked),
            number,
            |c, checked, own| {
                // The caller reports the `char` of an operand that a join hid, as `integers` does.
                if arithmetic && on_char(operator, own) {
                    return Some(c.error(DiagnosticKind::ArithmeticOnChar { at: head }));
                }
                (c.kind_rules(operands, (checked, own), operator, op, true)).then(|| c.proof(head)) // proof: kind rule
            },
        );
        let ty = match joined {
            Ok(ty) => self.table.resolve(ty),
            Err(reported) => {
                self.poison_open(&checked);
                return (fail_all(checked, reported), None, true);
            }
        };
        // `mk() + mk()` is a number, as `1 + 1` is.
        let ty = if matches!(ty.kind(), TypeKind::Infer(_))
            && self.table.default_open(ty) != Type::I64
        {
            let int = self.table.fresh(Kind::Int, head);
            if self.unify(ty, int) {
                self.table.resolve(ty)
            } else {
                ty
            }
        } else {
            ty
        };
        if arithmetic {
            let closed = self.table.default_open(ty);
            // A shape that no number has is the mistake, whatever its open parts are.
            if !matches!(closed.kind(), TypeKind::Infer(_)) && !operator.takes().contains(&closed) {
                // `u8? + u8?`: the joined type is the mistake, at each operand of that type
                // alone. A `1` beside an `i64?` is no mistake.
                let own: Vec<Option<Type>> = (checked.iter())
                    .map(|value| match value {
                        Ok(tir::Expr {
                            kind: tir::ExprKind::Wrap(inner),
                            ..
                        }) => self.table.default_open(inner.ty),
                        _ => closed,
                    })
                    .map(Some)
                    .collect();
                // A failed operand has reported: the others are no mistake of their own.
                if !self.kind_rules(operands, (&mut checked, &own), operator, op, true)
                    && checked.iter().all(Result::is_ok)
                {
                    let d = self.takes_mismatch(operands[0].span, operator.takes(), closed);
                    let reported = self.diagnostics.error(d);
                    checked[0] = Err(Poisoned::untyped(reported));
                }
                return (checked, None, true);
            }
            return match self.close_number(ty, (head, whole), expected) {
                Ok(ty) => (checked, Some(ty), false),
                Err(failed) => (fail_all(checked, failed.reported()), None, true),
            };
        }
        let ty = match self.close_join(ty, (head, whole), Expectation::None) {
            Ok(ty) => ty,
            Err(failed) => return (fail_all(checked, failed.reported()), None, true),
        };
        if ty == Type::STRING || ty == Type::NEVER || self.compares_through_ord(ty) {
            return (checked, Some(ty), false);
        }
        // A member that the join could not close, as `[300]` beside a `[u8]`, is the first
        // mistake: the operator's is not reported.
        if self.diagnostics.errors() > errors {
            return (checked, None, true);
        }
        // The operand of the joined type lacks the operator: `a < Some(a)` gives `T?`.
        let at = (0..own.len()).find(|&i| own[i] == Some(ty)).unwrap_or(0);
        let ord = self.lang.ord;
        let d = self.missing_operator(operands[at].span, ty, &op(at), ord);
        let reported = self.diagnostics.error(d);
        checked[at] = Err(Poisoned::untyped(reported));
        (checked, None, true)
    }

    /// The expectation of the operand `e` beside an operand of type `beside`. An integer literal
    /// beside a closed number type takes that type: it needs no `Int` variable. It is checked as
    /// a literal written for that type, which reports one that does not fit, as the join would.
    /// Any other operand is a member.
    fn beside_closed(&self, e: &ast::Expr, beside: Option<Type>) -> Expectation {
        match beside.map(|ty| self.table.resolve(ty)) {
            Some(ty @ (Type::I64 | Type::U8 | Type::F64))
                if matches!(e.kind, ExprKind::Int { .. }) =>
            {
                Expectation::Required(ty, Origin::None)
            }
            _ => Expectation::Member,
        }
    }

    /// The expectation of the side `e` of `==`, `!=` or `??` beside a side of type `beside`. A
    /// side that waits for a type to pick its method (`id(s.name())`) takes a closed `beside` as
    /// its type, which picks the method. Any other side is as `beside_closed` says.
    fn beside_side(&mut self, e: &ast::Expr, beside: Option<Type>) -> Expectation {
        match beside.map(|ty| self.table.resolve(ty)) {
            Some(ty) if !ty.holds_infer() && self.waits_for_pick(e) => {
                Expectation::Required(ty, Origin::None)
            }
            _ => self.beside_closed(e, beside),
        }
    }

    /// Whether `e` is a `None`, or a block whose value is one: it waits for the operator's
    /// number, as `1 + None` does.
    fn none_operand(&self, e: &ast::Expr) -> bool {
        match &e.kind {
            ExprKind::Block(block) => super::tail(block).is_some_and(|e| self.none_operand(e)),
            _ => self.none_written(e),
        }
    }

    /// Poisons each open variable of `checked` after its operator failed, so that no E0050
    /// follows the report.
    fn poison_open(&mut self, checked: &[Checked]) {
        for ty in checked.iter().filter_map(type_of) {
            if self.table.resolve(ty).holds_infer() {
                self.table.poison(ty);
            }
        }
    }

    /// The rules of the kind of `operator` on the operands `operands`, checked as `checked`.
    /// `own` is the type of each that the rules read (`None` for one they leave). An operand that
    /// a rule reports fails. Whether a rule reported. In `conflict`, the operands did not join.
    /// - Arithmetic on a type parameter lacks the operator.
    /// - `+` with a `string`: each operand of another type is reported against `string`.
    /// - A `char` beside a number in a comparison: the number is reported against `char`.
    /// - Arithmetic on a type it does not take: the list of the types it takes.
    /// - A `X?` of a type `X` that the operator takes: "expected `X`".
    /// - In a conflict of a comparison, a type parameter beside another type: E0047.
    fn kind_rules(
        &mut self,
        operands: &[&ast::Expr],
        (checked, own): (&mut [Checked], &[Option<Type>]),
        operator: Operator,
        op: &dyn Fn(usize) -> String,
        conflict: bool,
    ) -> bool {
        let arithmetic = operator != Operator::Compare;
        let typed = |i: usize| own[i].filter(|&ty| ty != Type::NEVER);
        if arithmetic
            && let Some(i) = (0..own.len()).find(|&i| typed(i).is_some_and(Type::has_param))
        {
            let ty = own[i].expect("the operand should be typed");
            checked[i] = Err(Poisoned::untyped(self.param_lacks(
                operands[i].span,
                ty,
                op(i),
            )));
            return true;
        }
        let mut reported = false;
        if matches!(operator, Operator::Plus { .. }) && own.contains(&Some(Type::STRING)) {
            // A compound assignment's number place does not convert: its value is the mistake.
            let number = |t: Type| matches!(payload(t), Type::I64 | Type::U8 | Type::F64);
            if let Some(place) = typed(0).filter(|&t| operator.compound() && number(t)) {
                let want = Expectation::Required(place, Origin::None);
                let value = std::mem::replace(&mut checked[1], Ok(unit_value(operands[1].span)));
                checked[1] = self.close(operands[1], value, want).0;
                return true;
            }
            // A place that `+=` does not take is the mistake, as `x -= 1` with a `bool` `x` is.
            if let Some(place) =
                typed(0).filter(|&t| operator.compound() && payload(t) != Type::STRING)
            {
                let d = self.takes_mismatch(operands[0].span, operator.takes(), place);
                checked[0] = Err(Poisoned::untyped(self.diagnostics.error(d)));
                return true;
            }
            let start = self.diagnostics.len();
            for i in 0..own.len() {
                if let Some(got) = typed(i).filter(|&t| t != Type::STRING) {
                    let beside = own
                        .iter()
                        .enumerate()
                        .find_map(|(k, &t)| t.filter(|_| k != i));
                    let d = self.concat_mismatch(
                        operands[i],
                        got,
                        operands,
                        operator.compound(),
                        beside,
                    );
                    checked[i] = Err(Poisoned::untyped(self.diagnostics.error(d)));
                    reported = true;
                }
            }
            if let [lhs, rhs] = operands {
                self.label_join_literals(start, [lhs, rhs]);
            }
            return reported;
        }
        let number = |t: Type| matches!(t, Type::I64 | Type::U8 | Type::F64);
        if !arithmetic
            && own.contains(&Some(Type::CHAR))
            && let Some(i) = (0..own.len()).find(|&i| typed(i).is_some_and(number))
        {
            // The first number is the mistake; the others fail with it.
            let reported = self.reject_leader(operands[i], &mut checked[i], Type::CHAR);
            for k in (0..own.len()).filter(|&k| k != i && typed(k).is_some_and(number)) {
                checked[k] = Err(Poisoned::untyped(reported));
            }
            return true;
        }
        for i in 0..own.len() {
            let Some(got) = typed(i) else { continue };
            let taken = operator.takes().contains(&payload(got));
            // A `None` beside a `X?` is not the mistake.
            if got != payload(got) && taken && !self.none_operand(operands[i]) {
                let want = Expectation::Required(payload(got), Origin::None);
                let value = std::mem::replace(&mut checked[i], Ok(unit_value(operands[i].span)));
                checked[i] = self.against(operands[i], value, want).0;
                reported = true;
            } else if arithmetic && !taken && !reported {
                let d = self.takes_mismatch(operands[i].span, operator.takes(), got);
                checked[i] = Err(Poisoned::untyped(self.diagnostics.error(d)));
                reported = true;
            }
        }
        if reported || !conflict || arithmetic {
            return reported;
        }
        // A type parameter beside another type cannot be compared, whatever its bounds.
        let pair = (own.iter().enumerate()).find_map(|(k, &a)| {
            let a = a?;
            (own[k + 1..].iter().enumerate()).find_map(|(j, &b)| {
                let b = b?;
                (a != b && (a.has_param() || b.has_param())).then_some((k, k + 1 + j, a, b))
            })
        });
        let Some((k, j, left, right)) = pair else {
            return false;
        };
        let kind = DiagnosticKind::CannotCompare {
            left: self.type_name(left),
            right: self.type_name(right),
            at: operands[k].span.to(operands[j].span),
        };
        let reported = self.diagnostics.error(kind);
        checked[k] = Err(Poisoned::untyped(reported));
        checked[j] = Err(Poisoned::untyped(reported));
        true
    }

    /// The operands of a comparison, checked as `checked`, one of which is a `unit`. An open
    /// operand, such as `[]`, is checked against `unit`. Beside an operand of a type that takes
    /// no `unit`, each `unit` operand has no value to compare: E0043. Otherwise each operand of
    /// another type, such as `unit?`, is reported against `unit`.
    fn units_compared(
        &mut self,
        operands: &[&ast::Expr],
        checked: Vec<Checked>,
    ) -> (Vec<Checked>, Option<Type>, bool) {
        let unit = Expectation::Required(Type::UNIT, Origin::None);
        let mut mismatched = false;
        let mut opened = Vec::with_capacity(checked.len());
        for (e, c) in operands.iter().zip(checked) {
            let open = type_of(&c)
                .map(|ty| self.table.resolve(ty))
                .filter(|ty| ty.holds_infer());
            opened.push(match open {
                Some(ty) if self.unify(ty, Type::UNIT) => c.map(|mut value| {
                    value.ty = Type::UNIT;
                    value
                }),
                // A number literal is its default type: `g() < 1` compares an `i64`.
                Some(ty) if !self.table.default_open(ty).holds_infer() => {
                    let closed = self.close_join(ty, (e.span, e.span), Expectation::None);
                    c.and_then(|mut value| {
                        value.ty = closed?;
                        Ok(value)
                    })
                }
                Some(_) => {
                    let (operand, m) = self.close(e, c, unit);
                    mismatched |= m;
                    operand
                }
                None => c,
            });
        }
        let mut checked = opened;
        let other =
            (checked.iter().filter_map(type_of)).find(|&ty| !takes_unit(ty) && ty != Type::NEVER);
        if other.is_some() {
            for (e, c) in operands.iter().zip(&mut checked) {
                if type_of(c) == Some(Type::UNIT) {
                    let cause = self.no_value_cause(e, c.as_ref().ok(), (Type::UNIT, other), false);
                    *c = Err(Poisoned::untyped(self.no_value(e.span, cause)));
                }
            }
            return (checked, None, false);
        }
        let mut held = Vec::with_capacity(checked.len());
        for (e, c) in operands.iter().zip(checked) {
            let misfit = type_of(&c).is_some_and(|ty| ty != Type::UNIT && ty != Type::NEVER);
            let c = if misfit {
                let (operand, m) = self.close(e, c, unit);
                mismatched |= m;
                operand
            } else {
                c
            };
            held.push(c);
        }
        if let Err(reported) = self.valued(&mut held, Type::UNIT) {
            return (fail_all(held, reported), None, true);
        }
        (held, Some(Type::UNIT), mismatched)
    }

    /// The operand `e`, checked as `checked`, reported against `ty`, the type of a later operand,
    /// and dropped to `Error`. Gives the proof of that report, or of the one that failed `e`.
    fn reject_leader(&mut self, e: &ast::Expr, checked: &mut Checked, ty: Type) -> Reported {
        let reported = match known_type(checked) {
            Ok(found) => {
                // An open number shows as its default: `'a' < 1` finds an `i64`.
                let found = self.table.default_open(found);
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

    /// "expected `takes`, found `got`" at `at`: the types an operator takes, `got` not among them.
    /// A comparison also takes `unit`, which `units_compared` checks apart from the others.
    pub(super) fn takes_mismatch(&self, at: Span, takes: &[Type], got: Type) -> Diagnostic {
        let unit = (takes == Operator::Compare.takes()).then_some(Type::UNIT);
        let listed = takes.iter().copied().chain(unit);
        DiagnosticKind::Mismatch {
            want: Expected::Types(listed.map(|ty| self.type_name(ty)).collect()),
            got: self.open_name(got),
            at,
            origin: None,
            fix: None,
            conversion: false,
            layers: false,
        }
        .into()
    }

    /// `lhs == rhs`, or `!=`, its token at `head`. Each side is a member of one join (ADR 0052),
    /// so neither side is checked against the other. Before the join, these rules read the own
    /// type of each side.
    /// - A `unit` beside a value of another type has no value to compare.
    /// - A side can be the other's type with two or more `?` layers taken off. It is held to that
    ///   type, as a value converts through one `?` only.
    /// - A `None` beside a value that is not optional is reported, as the answer is fixed.
    /// - A number beside a `char` is reported against `char`.
    ///
    /// Sides that do not join report at `head` when both are numbers, else E0047. A side that is
    /// `f64.nan` is warned about.
    #[expect(
        clippy::too_many_lines,
        reason = "the checks of one operator, in the order that they report"
    )]
    fn equal(
        &mut self,
        span: Span,
        (negated, head): (bool, Span),
        lhs: &ast::Expr,
        rhs: &ast::Expr,
    ) -> Checked {
        let sides = [lhs, rhs];
        // Each side is checked once, as a member: a call that cannot infer its parameter there
        // stays open, and the join of the two sides gives it the type of the other. A side that
        // waits for a type to pick its method is checked last (see `beside_side`).
        let mut values = self.ranked(|c| {
            let late = c.waits_for_pick(lhs) && !c.waits_for_pick(rhs);
            let (early, last) = if late { (rhs, lhs) } else { (lhs, rhs) };
            let first = c.value(early, Expectation::Member);
            let expected = c.beside_side(last, type_of(&first));
            let second = c.value(last, expected);
            if late {
                [second, first]
            } else {
                [first, second]
            }
        });
        let types = [
            self.member_own_type(&mut values[0], lhs.span),
            self.member_own_type(&mut values[1], rhs.span),
        ];
        // A `unit` beside a value of another type has no value to compare. An open side, as
        // `none()`, takes it.
        if let [Some(a), Some(b)] = types
            && let Some((side, value, beside)) = [(0, a, b), (1, b, a)]
                .into_iter()
                .find(|&(_, ty, beside)| {
                    ty == Type::UNIT && !takes_unit(beside) && !open_any(beside)
                })
                .map(|(i, _, beside)| (sides[i], &values[i], beside))
        {
            let value = value.as_ref().ok();
            let cause = self.no_value_cause(side, value, (Type::UNIT, Some(beside)), false);
            let reported = self.no_value(side.span, cause);
            self.poison_open(&values);
            return Err(Poisoned::untyped(reported));
        }
        // A side converts through one `?` only: `x == 5` on an `i64??` holds `5` to `i64??`.
        if let [Some(a), Some(b)] = types
            && let Some((side, want, got)) = [(rhs, a, b), (lhs, b, a)]
                .into_iter()
                .find(|&(_, want, got)| beyond_one_layer(want, got))
        {
            let d = self.mismatch(side.span, Some(side), want, got, Origin::None);
            self.poison_open(&values);
            return Err(Poisoned::untyped(self.diagnostics.error(d)));
        }
        // A `None` beside a value that is not optional: the answer is fixed.
        let none = (0..2).find(|&i| self.none_written(sides[i]));
        if let Some(i) = none
            && let Some(ty) = types[1 - i].filter(|ty| !ty.holds_infer())
            && !matches!(ty.kind(), TypeKind::Optional(_))
            && !self.none_written(sides[1 - i])
        {
            let value = path(sides[1 - i]);
            let reported = self
                .diagnostics
                .error(DiagnosticKind::NeverNone { value, at: span });
            self.poison_open(&values);
            return Err(Poisoned::untyped(reported));
        }
        // A number beside a `char` is the mistake, as in a comparison.
        let number = |ty: Type| matches!(ty, Type::I64 | Type::U8 | Type::F64);
        if let [Some(a), Some(b)] = types.map(|t| t.map(payload))
            && (a == Type::CHAR || b == Type::CHAR)
            && let Some(i) = [a, b].iter().position(|&t| number(t))
        {
            let reported = self.reject_leader(sides[i], &mut values[i], Type::CHAR);
            self.poison_open(&values);
            return Err(Poisoned::untyped(reported));
        }
        // A side with no type (its error is reported) does not say that the other needs `Eq`,
        // except a type parameter, which always did.
        if let Some(failed) = values.iter().find_map(untyped) {
            if !values.iter().any(mismatched)
                && let Some(ty) = types.into_iter().flatten().find(|ty| ty.has_param())
                && self.operation(ty, self.lang.eq) == Operation::Missing
            {
                let side = if types[0] == Some(ty) { lhs } else { rhs };
                let op = if negated { "!=" } else { "==" };
                let d = self.missing_operator(side.span, ty, op, self.lang.eq);
                self.error(d);
            }
            self.poison_open(&values);
            return Err(Poisoned::typed(Type::BOOL, failed));
        }
        let joined = self.join_operands(
            JoinForm::Compare,
            head,
            (&sides, &mut values),
            None,
            |c, _, own| {
                // A bare `Any` variable left open is a failed member's: it is no type to report.
                let bare = |t: Type| matches!(t.kind(), TypeKind::Infer(_));
                if let [Some(a), Some(b)] = *own
                    && !(number(payload(a)) && number(payload(b)))
                    && !bare(a)
                    && !bare(b)
                {
                    let kind = DiagnosticKind::CannotCompare {
                        left: c.open_name(a),
                        right: c.open_name(b),
                        at: span,
                    };
                    return Some(c.diagnostics.error(kind));
                }
                None
            },
        );
        // A failed side has reported: the close is silent, as in a failed context.
        let close = if values.iter().any(Result::is_err) {
            Expectation::Poisoned
        } else {
            Expectation::None
        };
        let ty = match joined.map(|ty| self.table.resolve(ty)) {
            Ok(ty) => self.close_join(ty, (head, span), close)?,
            Err(reported) => {
                self.poison_open(&values);
                return Err(Poisoned::untyped(reported));
            }
        };
        // `x == None` tests an optional, which any `T?` is, and needs no `Eq`. Else a type with
        // no `Eq` has no `==`.
        let eq = self.lang.eq;
        let operation = self.operation(ty, eq);
        if none.is_none() && operation == Operation::Missing {
            let op = if negated { "!=" } else { "==" };
            // It is at the first side of that type or its payload, as at the `any` side of
            // `1 == d`.
            let side = (0..2)
                .find(|&i| {
                    (types[i].map(|t| self.table.resolve(t)))
                        .is_some_and(|t| t == ty || as_optional(t) == ty)
                })
                .map_or(lhs, |i| sides[i]);
            let d = self.missing_operator(side.span, ty, op, eq);
            return Err(Poisoned::untyped(self.error(d)));
        }
        // A side that failed with a type took part in the join, and has reported. Its type may
        // be a guess, so the comparison states no type (ADR 0049).
        if let Some(failed) = values.iter().find_map(|value| value.as_ref().err()) {
            return Err(Poisoned::untyped(failed.reported()));
        }
        let [mut l, mut r] = values;
        for value in [&mut l, &mut r].into_iter().flatten() {
            value.ty = self.table.resolve(value.ty);
        }
        // A class or an enum with a written `Eq` is compared by a call of its `eq`. Any other
        // type keeps the node that `==` has always had. A type with no `Eq` reaches here only
        // in a comparison with `None`, which tests presence.
        let dispatch = match operation {
            Operation::Run(dispatch) => dispatch,
            // The type has `Eq` only through a broken block, which is reported.
            Operation::Poison => return Err(Poisoned::untyped(self.proof(span))), // proof: declaration
            Operation::Missing => Dispatch::Generated,
        };
        self.record_compared(lhs, rhs, [type_of(&l), type_of(&r)]);
        self.nan_comparison(negated, (lhs, &l), (rhs, &r));
        if matches!(dispatch, Dispatch::Method { .. }) {
            let written = Lowered::Operator(eq, if negated { "!=" } else { "==" });
            self.spellings.insert(span, written);
        }
        // The join gave both sides its type.
        if let (Ok(a), Ok(b)) = (&mut l, &mut r) {
            (a.ty, b.ty) = (ty, ty);
        }
        self.compared(span, l, r, |lhs, rhs| {
            lang::equal(dispatch, negated, lhs, rhs, span)
        })
    }

    /// Records that `==` reads the values of the two sides of types `types`, which are
    /// `first` and `second` as written. A comparison with `None` tests presence and reads no
    /// field.
    fn record_compared(&mut self, first: &ast::Expr, second: &ast::Expr, types: [Option<Type>; 2]) {
        if !self.none_written(first) && !self.none_written(second) {
            for ty in types.into_iter().flatten() {
                self.record_shown(ty, self.lang.eq);
            }
        }
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
                TypeKind::Array(_) | TypeKind::String | TypeKind::Struct(..) | TypeKind::Enum(..)
            ) || !(object_param || ty.is_counted(&self.lowered_enums, &self.lowered_structs))
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
                left: self.open_name(lt),
                right: self.open_name(rt),
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
                    (l, r.and_then(|e| self.lift(e, lt)))
                } else if fits(rt, lt) {
                    (l.and_then(|e| self.lift(e, rt)), r)
                } else if self.converts_to_any(rt, lt) {
                    // A class beside an `any` converts to it first.
                    #[cfg(test)]
                    super::reached("make_any on the right");
                    (l, r.and_then(|e| self.make_any(e, lt)))
                } else if self.converts_to_any(lt, rt) {
                    #[cfg(test)]
                    super::reached("make_any on the left");
                    (l.and_then(|e| self.make_any(e, rt)), r)
                } else {
                    let kind = DiagnosticKind::CannotCompare {
                        left: self.open_name(lt),
                        right: self.open_name(rt),
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
    /// but not a `unit` where `expected` is another type. A value that kept its E0050 (see
    /// `Poisoned::kept`) is the fault, so the right side still has the type of the left side's
    /// value.
    fn fallback(&mut self, rhs: &ast::Expr, expected: Expectation) -> Checked {
        let checked = self.expr(rhs, expected);
        let checked = self.refuse_unwanted(rhs, checked, expected, false);
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
        let mut scrutinee = self.value(lhs, Expectation::Member);
        let head = operator_head(&self.source, [lhs.span, rhs.span], "??");
        // An open left side is a join of one member: the join of `??` takes its payload, not it.
        // A nested join's members take part, and its conflict is its own.
        if let Ok(ty) = known_type(&scrutinee)
            && self.table.resolve(ty).holds_infer()
        {
            let (form, head) = self.own_join(lhs).unwrap_or((JoinForm::Coalesce, head));
            let values = std::slice::from_mut(&mut scrutinee);
            match self.join_operands(form, head, (&[lhs], values), None, |_, _, _| None) {
                Ok(joined) => {
                    self.unify(ty, joined);
                }
                Err(reported) => scrutinee = Err(Poisoned::untyped(reported)),
            }
            if let Ok(value) = &mut scrutinee {
                value.ty = self.table.resolve(value.ty);
            }
        }
        // A value that the join cannot make optional closes as it would alone, and so does an
        // `Any` variable in it: only its own value gives it a type (`None ?? 0`, as on `main`).
        // An `Int` variable stays open for the join (`Some(1) ?? o`).
        if let Some(ty) = type_of(&scrutinee)
            && let open = self.table.resolve(ty)
            && open.holds_infer()
            && (!matches!(open.kind(), TypeKind::Optional(_))
                || self.table.default_open(open).holds_infer())
            // A failed left side has reported: its close is silent.
            && let close = if scrutinee.is_err() { Expectation::Poisoned } else { Expectation::None }
            && let Err(failed) = self.close_join(ty, (lhs.span, lhs.span), close)
        {
            scrutinee = Err(failed);
        }
        let lhs_type = type_of(&scrutinee).map(|ty| self.table.resolve(ty));
        let Some(TypeKind::Optional(payload)) = lhs_type.map(Type::kind) else {
            let before = self.diagnostics.len();
            let default = self.fallback(rhs, Expectation::Poisoned);
            let reported = match known_type(&scrutinee) {
                // The fix deletes the default. So the default needs no report of its own, which
                // the fix would hide. It needs no jump either: a `break` types its loop, and each
                // jump changes how the code flows. A poisoned default has no `tir` to read: it
                // follows an earlier error, or only the left side could type it, as `[]` and
                // `None`. A left side that ends in `}` would end a `let ... else` value in `}`
                // (E0173), so it gets no fix either. Both exits of the check sort the reports, so
                // they are in text order.
                Ok(_) => self.diagnostics.error(DiagnosticKind::CoalesceNotOptional {
                    at: lhs.span,
                    default: (self.diagnostics.len() == before
                        && matches!(&default, Ok(e) if !e.exits())
                        && !self.source.text(lhs.span).ends_with('}'))
                    .then(|| Span::new(lhs.span.end(), rhs.span.end())),
                }),
                // A failed left side beside a closed optional default: the whole is of the
                // default's type, whatever the payload is.
                Err(reported) => match type_of(&default).map(|ty| self.table.resolve(ty)) {
                    Some(ty) if !ty.holds_infer() && matches!(ty.kind(), TypeKind::Optional(_)) => {
                        return Err(Poisoned::typed(ty, reported));
                    }
                    _ => reported,
                },
            };
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
        // The payload is read from a local that the value binds, so the left member is that read:
        // a whole value, which converts at its top (`oe ?? e` with an `Eye?` and an `any Seen`).
        let bind = self.fresh(payload);
        let mut read = match &scrutinee {
            Ok(_) => Ok(local(bind, payload, lhs.span)),
            Err(failed) => Err(Poisoned::typed(payload, failed.reported())),
        };
        // In a failed context too the default is a member: the join is silent (ADR 0049).
        // A context of another type blames the left side, which the open literal of the default
        // joins: the literal takes the payload's type only when the context agrees.
        let inner = if expected.ty().is_none_or(|ty| super::payload(ty) == payload) {
            self.beside_side(rhs, Some(payload))
        } else {
            Expectation::Member
        };
        let mut other = self.fallback(rhs, inner);
        // A failed default whose type is a nested join's variable lost the values that give it
        // its type: it has none.
        if let Err(failed) = &mut other
            && let Some(ty) = failed.ty
            && self.table.default_open(ty).holds_infer()
        {
            self.table.poison(ty);
            failed.ty = None;
        }
        // A default with no type gives the whole none, as the payload is not its type.
        if let Err(failed) = &other
            && failed.ty.is_none()
        {
            return Err(Poisoned::untyped(failed.reported()));
        }
        let heading = Heading {
            head,
            whole: span,
            returns: None,
        };
        let mut members = [
            JoinMember {
                value: &mut read,
                at: Place::Whole,
                label: lhs.span,
            },
            JoinMember {
                value: &mut other,
                at: Place::Whole,
                label: rhs.span,
            },
        ];
        // Sides of one type keep it, as the branches of an `if` do: the context converts the whole.
        let given = match expected {
            Expectation::Required(want, _)
                if self.lifted_whole(&members, want) || self.agreed_whole(&members, want) =>
            {
                Expectation::None
            }
            Expectation::Required(..) | Expectation::Poisoned => expected,
            _ => Expectation::Member,
        };
        let ty = (self.join_form(JoinForm::Coalesce, heading, &mut members, given, None))
            .and_then(|ty| self.close_join(ty, (span, span), expected))?;
        match (scrutinee, read, other) {
            (Ok(scrutinee), Ok(mut value), Ok(other)) => {
                // The node takes the type of the read, which the join may have closed.
                value.ty = self.table.resolve(value.ty);
                self.body.settled_joins.insert(span);
                Ok(if_some(bind, scrutinee, value, Some(other), span))
            }
            (Err(failed), ..) | (_, Err(failed), _) | (.., Err(failed)) => {
                Err(Poisoned::typed(ty, failed.reported()))
            }
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
    /// `- * / %`; `compound` for a compound assignment's.
    Arithmetic { compound: bool },
}

impl Operator {
    /// Whether the operator is that of a compound assignment, whose left side is its place.
    fn compound(self) -> bool {
        matches!(
            self,
            Operator::Plus { compound: true } | Operator::Arithmetic { compound: true }
        )
    }

    /// The form of the join of its two operands.
    fn join_form(self) -> JoinForm {
        match self {
            Operator::Compare => JoinForm::Compare,
            op if op.compound() => JoinForm::Compound { assigned: 0 },
            _ => JoinForm::Operator,
        }
    }

    /// The types the operator takes, in the order a mismatch lists them.
    pub(super) fn takes(self) -> &'static [Type] {
        match self {
            Operator::Compare => &[Type::I64, Type::U8, Type::F64, Type::CHAR, Type::STRING],
            Operator::Plus { .. } => &[Type::I64, Type::U8, Type::F64, Type::STRING],
            Operator::Arithmetic { .. } => &[Type::I64, Type::U8, Type::F64],
        }
    }
}

/// The types unary `-` takes. A `u8` is its own error: it cannot be negative.
pub(super) const NEGATABLE: &[Type] = &[Type::I64, Type::F64];

/// The span of the operator token `text` between the operands at `left` and `right`: the head of
/// their join. The gap holds no other token, but it can hold a comment after the token.
fn operator_head(source: &super::Source<'_>, [left, right]: [Span; 2], text: &str) -> Span {
    let gap = Span::new(left.end(), right.start());
    (source.text(gap).find(text)).map_or(gap, |at| {
        Span::new(gap.start() + at, gap.start() + at + text.len())
    })
}

/// A `unit` value at `span`, in the place of a value that is not checked yet.
fn unit_value(span: Span) -> tir::Expr {
    tir::Expr::new(tir::ExprKind::Unit, Type::UNIT, span)
}

/// Each of `checked` failed, as `reported` proves: the operator's operands after its report.
fn fail_all(checked: Vec<Checked>, reported: Reported) -> Vec<Checked> {
    (checked.into_iter())
        .map(|_| Err(Poisoned::untyped(reported)))
        .collect()
}

/// The proof of the failure of `checked` when it failed with no type.
fn untyped(checked: &Checked) -> Option<Reported> {
    match checked {
        Err(failed) if failed.ty.is_none() => Some(failed.reported()),
        _ => None,
    }
}

/// Whether arithmetic over operands of the types `own` is on a `char`: a `char` beside a
/// `string` under `+` is a value that the `string` takes, and is reported against it.
fn on_char(operator: Operator, own: &[Option<Type>]) -> bool {
    own.contains(&Some(Type::CHAR))
        && !(matches!(operator, Operator::Plus { .. }) && own.contains(&Some(Type::STRING)))
}
