//! Branches: `if`, `if let` and the join of branch types.

use super::expr::{fits, takes_unit};
use super::generic::Ahead;
use super::infer::Kind;
use super::join::{Heading, JoinForm, JoinMember, Place};
use super::pattern::LetPattern;
use super::reports::Reported;
use super::{
    Checked, CheckedBlock, Checker, Expectation, Misfit, Origin, Poisoned, block_type, is_jumped,
    is_kept, value_span,
};
use crate::ast;
use crate::diagnostic::Unwrap;
use crate::source::Span;
use crate::tir::{self, Type, TypeKind};

/// `span`, the span of an `if` with no `else`, when its `then` block has a value: such an `if` has
/// none (see `Poisoned::valueless`). `if_let` says that it is an `if let`, and goes with it.
fn valueless_join(
    (span, if_let): (Span, bool),
    else_block: Option<&ast::Block>,
    then: &CheckedBlock,
) -> Option<(Span, bool)> {
    let value = block_type(then).is_some_and(|ty| !matches!(ty, Type::UNIT | Type::NEVER));
    (else_block.is_none() && value).then_some((span, if_let))
}

/// The proof of the first failed branch among `blocks`, of a join that failed by a branch.
fn failed_branch<T>(blocks: &[Result<T, Poisoned>]) -> Reported {
    (blocks.iter().find_map(|block| block.as_ref().err()))
        .expect("a join that fails by a branch should have a failed branch")
        .reported()
}

impl Checker<'_> {
    /// The failed `if` or `match` whose join is `ty`: as typed as the join is. A join with a type
    /// fails by a failed part, whose proof is `part`. The open variables of that type report
    /// nothing more, as a nested join leaves its type open.
    pub(super) fn failed_join(
        &mut self,
        ty: Result<Type, Poisoned>,
        part: Option<Reported>,
    ) -> Checked {
        Err(match ty {
            Err(poisoned) => poisoned,
            Ok(ty) => {
                self.table.poison(ty);
                Poisoned::typed(
                    ty,
                    part.expect("a join with a type should fail by a failed part"),
                )
            }
        })
    }

    /// Without an `else`, `unit`. With one, the branches joined (see `branches`), also when the
    /// condition failed.
    pub(super) fn if_expr(
        &mut self,
        span: Span,
        cond: &ast::Expr,
        then_block: &ast::Block,
        else_block: Option<&ast::Block>,
        expected: Expectation,
    ) -> Checked {
        let dropped = self.body.dropped == Some(span);
        let (cond, _) = self.operand(cond, Expectation::Required(Type::BOOL, Origin::Condition));
        let (then, other, ty) = self.branches(
            span,
            (then_block, else_block),
            (expected, dropped, cond.is_err()),
            |c, expected| c.block(then_block, expected),
        );
        let valueless = valueless_join((span, false), else_block, &then);
        let part = (cond.as_ref().err())
            .or_else(|| then.as_ref().err())
            .or_else(|| other.as_ref().and_then(|other| other.as_ref().err()))
            .map(Poisoned::reported);
        match (cond, then, other.transpose(), ty) {
            (Ok(cond), Ok(then_block), Ok(else_block), Ok(ty)) => Ok(tir::Expr::new(
                tir::ExprKind::If {
                    cond: Box::new(cond),
                    then_block,
                    else_block,
                },
                ty,
                span,
            )),
            (.., ty) => self.failed_join(ty, part).map_err(|failed| Poisoned {
                valueless,
                ..failed
            }),
        }
    }

    /// `if let pattern = value { ... }`, typed as `if`; what `pattern` binds is in scope in the
    /// first block. On an optional it is an `IfSome`, on an enum a `Match` (`if_let_match`).
    /// A `value` that is neither is reported, and the branches are still joined: what `pattern`
    /// binds is then untyped, so a branch that uses it fails.
    pub(super) fn if_let(
        &mut self,
        span: Span,
        pattern: &ast::Pattern,
        value: &ast::Expr,
        then_block: &ast::Block,
        else_block: Option<&ast::Block>,
        expected: Expectation,
    ) -> Checked {
        let dropped = self.body.dropped == Some(span);
        // Ranking the `if let` for context may have checked its value (see `rank_value`).
        let (scrutinee, form) =
            if let Some((_, Ahead::Let(checked, form))) = self.kept(value.span, false) {
                (checked, form)
            } else {
                let (checked, form, _) = self.let_value(Unwrap::IfLet, pattern, value);
                (checked, form)
            };
        let mut checked = None;
        let (then, other, ty) = self.branches(
            span,
            (then_block, else_block),
            (expected, dropped, scrutinee.is_err()),
            |c, expected| {
                let (bound, block) =
                    c.let_block(Unwrap::IfLet, &form, pattern, then_block, expected);
                checked = Some(bound);
                block
            },
        );
        let bound = checked.expect("`branches` should check the first block, which sets it");
        let ty = ty?;
        let valueless = valueless_join((span, true), else_block, &then);
        let kind = match (scrutinee, then, other.transpose(), bound) {
            (Ok(scrutinee), Ok(then_block), Ok(else_block), LetPattern::Some { bind }) => {
                tir::ExprKind::IfSome {
                    bind,
                    scrutinee: Box::new(scrutinee),
                    then_block,
                    else_block,
                }
            }
            (
                Ok(scrutinee),
                Ok(then_block),
                Ok(else_block),
                LetPattern::Match {
                    pattern: Ok(pattern),
                },
            ) => self.if_let_match(scrutinee, pattern, then_block, else_block),
            (scrutinee, then, other, bound) => {
                // A pattern at an `Error` position fails with the scrutinee, which comes first.
                let pattern = match bound {
                    LetPattern::Match { pattern: Err(own) } => own,
                    LetPattern::Match { pattern: Ok(_) } | LetPattern::Some { .. } => None,
                };
                let part = (scrutinee.err().map(|p| p.reported()))
                    .or_else(|| then.err().map(|p| p.reported()))
                    .or_else(|| other.err().map(|p| p.reported()))
                    .or(pattern)
                    .expect("a failed `if let` should have a failed part");
                self.table.poison(ty);
                return Err(Poisoned {
                    valueless,
                    ..Poisoned::typed(ty, part)
                });
            }
        };
        Ok(tir::Expr::new(kind, ty, span))
    }

    /// The blocks of the `if` or `if let` at `span`, `then` checking the first, and their type:
    /// `unit` without an `else`; else their join (see `join_blocks`). With `dropped`, no code uses
    /// the value (see `Body::dropped`). `head_failed` says that the condition or the value of an
    /// `if let` failed (see `join_blocks`).
    fn branches(
        &mut self,
        span: Span,
        (then_block, else_block): (&ast::Block, Option<&ast::Block>),
        (expected, dropped, head_failed): (Expectation, bool, bool),
        then: impl FnOnce(&mut Self, Expectation) -> CheckedBlock,
    ) -> (CheckedBlock, Option<CheckedBlock>, Result<Type, Poisoned>) {
        let Some(else_block) = else_block else {
            // Without an `else`, no code uses the value of the `then` block.
            self.body.dropped = Some(then_block.span);
            return (then(self, Expectation::None), None, Ok(Type::UNIT));
        };
        let inner = self.branches_expect(expected);
        let binding = self.let_name(span);
        self.body.dropped = dropped.then_some(then_block.span);
        let first = then(self, branch_expectation(inner));
        self.body.dropped = dropped.then_some(else_block.span);
        let other = self.block(else_block, branch_expectation(inner));
        let heading = Heading {
            head: self.source.head(span, "if".len()),
            whole: span,
            returns: None,
        };
        let spans = [value_span(then_block), value_span(else_block)];
        let (blocks, ty) = self.join_blocks(
            (JoinForm::If, heading, binding),
            vec![first, other],
            (&spans, dropped, head_failed),
            expected,
        );
        let [first, other] =
            <[_; 2]>::try_from(blocks).unwrap_or_else(|_| panic!("a join should give two blocks"));
        (first, Some(other), ty)
    }

    /// What the branches of a join under `expected` take. A required type that is a bare variable,
    /// as the element of a `[]` beside the join, is no type to meet: the branches are members, and
    /// the join gives the variable their type (see `join_blocks`).
    pub(super) fn branches_expect(&self, expected: Expectation) -> Expectation {
        match expected {
            Expectation::Required(want, _) if self.bare_variable(want) => Expectation::None,
            _ => expected,
        }
    }

    /// Whether `ty` is an inference variable with no type yet, or an optional of one.
    pub(super) fn bare_variable(&self, ty: Type) -> bool {
        let mut ty = self.table.resolve(ty);
        while let TypeKind::Optional(inner) = ty.kind() {
            ty = self.table.resolve(inner);
        }
        matches!(ty.kind(), TypeKind::Infer(_))
    }

    /// The join of the values of `blocks`, the branches of an `if` or the arms of a `match`, whose
    /// values are at `spans`: `join_form` with the `form`, its `heading` and the name `binding` of
    /// the `let` it is the value of. With `dropped`, no code uses the value (see
    /// `Body::dropped_join`). Under `Member` it is a nested join, which has no type of its own:
    /// its branches are members of the join around it, which settles them. With `head_failed`, a
    /// failed condition or scrutinee makes the nested join a poisoned member, which the join around
    /// it does not settle. So its branches join by themselves in a failed context, and their
    /// conflict is reported. Its type is not closed: a literal keeps an open type, which the
    /// poison silences, and the join around it gives no report for it (ADR 0049). A join with no
    /// type to meet closes its open variables (see `close_join`). A join of `never` branches of
    /// which one is a failed `break` or `continue` keeps that mark (see `Poisoned::jumped`).
    pub(super) fn join_blocks(
        &mut self,
        (form, heading, binding): (JoinForm, Heading, Option<Span>),
        blocks: Vec<CheckedBlock>,
        (spans, dropped, head_failed): (&[Span], bool, bool),
        expected: Expectation,
    ) -> (Vec<CheckedBlock>, Result<Type, Poisoned>) {
        let mut values: Vec<Checked> = (blocks.into_iter().zip(spans))
            .map(|(block, &at)| {
                block.map(|mut block| {
                    let ty = block.ty;
                    // A nested branch with no value keeps where it ends, so that the join around
                    // it reports its `unit` there, not at the whole `if`.
                    if expected == Expectation::Member && block.value.is_none() && ty == Type::UNIT
                    {
                        block.value = Some(Box::new(tir::Expr::new(tir::ExprKind::Unit, ty, at)));
                    }
                    tir::Expr::new(tir::ExprKind::Block(block), ty, at)
                })
            })
            .collect();
        let ty = self.join_whole(
            (form, heading, binding),
            &mut values,
            (spans, dropped, head_failed),
            expected,
        );
        let blocks = (values.into_iter())
            .map(|value| {
                value.map(|e| match e.kind {
                    tir::ExprKind::Block(block) => block,
                    _ => panic!("a join should settle a block as a block"),
                })
            })
            .collect();
        (blocks, ty)
    }

    /// `join_blocks` for `values`, the members of a join that are each its own value: the value of
    /// a branch, or of a `break` (see `Checker::loop_expr`), labeled by the span in `spans`.
    pub(super) fn join_whole(
        &mut self,
        (form, heading, binding): (JoinForm, Heading, Option<Span>),
        values: &mut [Checked],
        (spans, dropped, head_failed): (&[Span], bool, bool),
        expected: Expectation,
    ) -> Result<Type, Poisoned> {
        let ty = if expected == Expectation::Member {
            let never = (values.iter()).all(|v| v.as_ref().is_ok_and(|e| e.ty == Type::NEVER));
            if never {
                Ok(Type::NEVER)
            } else if head_failed {
                let mut members = whole_members(values, spans);
                let outer = std::mem::replace(&mut self.body.dropped_join, dropped);
                let ty =
                    self.join_form(form, heading, &mut members, Expectation::Poisoned, binding);
                self.body.dropped_join = outer;
                ty.map_err(Some)
            } else if values.iter().all(Result::is_ok)
                && matches!(form, JoinForm::Array)
                && self.spills(values.iter().flatten())
            {
                // The array reads a spilled element from a local, which hides the element's value
                // positions from the join around the array. So the elements join now, and their
                // type stays open for that join.
                let mut members = whole_members(values, spans);
                let outer = std::mem::replace(&mut self.body.dropped_join, dropped);
                let ty = self.join_form(form, heading, &mut members, Expectation::None, binding);
                self.body.dropped_join = outer;
                ty.map_err(Some)
            } else if values.iter().all(Result::is_ok) {
                Ok(self.table.fresh(Kind::Any, heading.whole))
            } else {
                // A failed branch makes the nested join poison, typed as its members join, else
                // untyped by that branch (`None`).
                let mut members = whole_members(values, spans);
                self.join_type(&mut members, None, None).ok_or(None)
            }
        } else {
            // A branch whose generic call found a type that cannot meet its context joins by that
            // type, as its concrete twin does (see `Misfit`). The whole stays `Error`.
            for value in values.iter_mut() {
                if self.misfit_type(value).is_some()
                    && let Err(poisoned @ Poisoned { found: Some(_), .. }) = value
                {
                    poisoned.ty = poisoned.found.map(|misfit| misfit.ty);
                }
            }
            let mut members = whole_members(values, spans);
            // An array does not lift, and reports each `unit` element: its join meets the
            // required element type (see `agreed_kind`).
            let whole = !matches!(form, JoinForm::Array);
            let expected = match expected {
                Expectation::Required(want, _)
                    if whole
                        && (self.lifted_whole(&members, want)
                            || self.agreed_whole(&members, want)
                            || self.unit_whole(&members, want)) =>
                {
                    Expectation::None
                }
                _ => self.hinted_join(&mut members, expected),
            };
            let outer = std::mem::replace(&mut self.body.dropped_join, dropped);
            let ty = self.join_form(form, heading, &mut members, expected, binding);
            self.body.dropped_join = outer;
            ty.map_err(Some)
        };
        let ty = ty.map_err(|p| p.unwrap_or_else(|| Poisoned::untyped(failed_branch(values))));
        let ty = self.misfit_join(values, ty);
        let ty = match ty {
            // A failed branch has reported: the close is silent, as in a failed context.
            Ok(ty) if self.table.resolve(ty).holds_infer() => {
                let close = match expected {
                    Expectation::None | Expectation::Hint { .. }
                        if values.iter().any(Result::is_err) =>
                    {
                        Expectation::Poisoned
                    }
                    expected => expected,
                };
                self.close_join(ty, (heading.whole, heading.whole), close)
            }
            ty => ty,
        };
        match ty {
            Ok(Type::NEVER) if values.iter().any(is_jumped) => Err(Poisoned {
                jumped: true,
                ..Poisoned::typed(Type::NEVER, failed_branch(values))
            }),
            Ok(ty) => Ok(ty),
            // A join that fails with a type required, with no failed branch, rejected that type.
            Err(poisoned) => {
                let mismatch = poisoned.mismatch
                    || matches!(expected, Expectation::Required(..))
                        && values.iter().all(Result::is_ok)
                    || (values.iter()).any(|b| matches!(b, Err(Poisoned { mismatch: true, .. })));
                Err(Poisoned {
                    // A join that rejected its type is `Error`, as a value that does: no type
                    // outlives its report (ADR 0049).
                    ty: poisoned.ty.filter(|_| !mismatch),
                    mismatch,
                    // A join that has no type is kept when a branch is: that call's E0050 is the
                    // one report.
                    kept: poisoned.ty.is_none() && values.iter().any(is_kept),
                    ..poisoned
                })
            }
        }
    }

    /// `ty`, the join of `blocks`, with each misfit branch untyped again. A join that failed has
    /// reported each misfit. Else the first misfit carries the joined type to the code that uses
    /// the value, which reports it, and the whole is `Error`.
    fn misfit_join<T>(
        &mut self,
        blocks: &mut [Result<T, Poisoned>],
        ty: Result<Type, Poisoned>,
    ) -> Result<Type, Poisoned> {
        let mut carried = None;
        for block in blocks.iter_mut() {
            let Err(poisoned) = block else {
                continue;
            };
            let (Some(found), Some(_)) = (poisoned.found, poisoned.ty) else {
                continue;
            };
            poisoned.ty = None;
            match (&carried, &ty) {
                (None, Ok(joined)) => {
                    let found = Misfit {
                        ty: *joined,
                        ..found
                    };
                    carried = Some((found, poisoned.reported()));
                }
                _ => self.settle_misfit(block),
            }
        }
        match carried {
            Some((found, reported)) => Err(Poisoned {
                found: Some(found),
                ..Poisoned::untyped(reported)
            }),
            None => ty,
        }
    }

    /// Whether `members` all have one type that `want`, a required optional, lifts: then they
    /// keep that type, as the value of a block does, and the context lifts the whole value.
    pub(super) fn lifted_whole(&self, members: &[JoinMember<'_>], want: Type) -> bool {
        let mut types = (members.iter())
            .filter_map(JoinMember::ty)
            .map(|ty| self.table.resolve(ty))
            .filter(|&ty| ty != Type::NEVER);
        let Some(own) = types.next() else {
            return false;
        };
        own != want && fits(want, own) && types.all(|ty| ty == own)
    }

    /// Whether `members` all have one known type other than `want`. Then they keep that type, as
    /// the value of a block does. The context reports the whole value once, with its own code
    /// (E0094 for a `char` in arithmetic). A misfit counts with the type it found.
    pub(super) fn agreed_whole(&self, members: &[JoinMember<'_>], want: Type) -> bool {
        // A failed member has reported, so the members do not agree. A misfit has not.
        if (members.iter()).any(|member| matches!(member.value, Err(Poisoned { found: None, .. })))
        {
            return false;
        }
        let mut types = (members.iter())
            .filter_map(JoinMember::ty)
            .map(|ty| self.table.resolve(ty))
            .filter(|&ty| ty != Type::NEVER);
        let Some(own) = types.next() else {
            return false;
        };
        own != want && !own.holds_infer() && types.all(|ty| ty == own)
    }

    /// Whether each of `members` is `unit` or `never`, one at least `unit`, and `want` takes no
    /// `unit`. Then the context reports the whole join once, with the cause of its `unit`.
    fn unit_whole(&self, members: &[JoinMember<'_>], want: Type) -> bool {
        let mut types = (members.iter()).map(|m| m.ty().map(|ty| self.table.resolve(ty)));
        !takes_unit(want)
            && types.all(|ty| matches!(ty, Some(Type::UNIT | Type::NEVER)))
            && (members.iter()).any(|m| m.ty().map(|ty| self.table.resolve(ty)) == Some(Type::UNIT))
    }

    /// The expectation that the join of `members` under `expected` takes. Under a hint it is no
    /// type when the members join by themselves. Else it is the type of its siblings or the
    /// hint's type, required, the first that the members join to. The siblings' type comes
    /// first, as it adds no `?`. A join that takes none of them has no type to meet, and is
    /// reported as a conflict.
    fn hinted_join(
        &mut self,
        members: &mut [JoinMember<'_>],
        expected: Expectation,
    ) -> Expectation {
        let Expectation::Hint {
            want,
            siblings,
            origin,
        } = expected
        else {
            return expected;
        };
        // A join with no member that has a value takes no type from a hint: it is untyped
        // (ADR 0049).
        let valued = |m: &JoinMember<'_>| m.ty().is_some_and(|ty| ty != Type::NEVER);
        if !members.iter().any(valued) || self.join_type(members, None, None).is_some() {
            return Expectation::None;
        }
        [siblings, want]
            .into_iter()
            .find(|&ty| self.join_type(members, None, Some(ty)).is_some())
            .map_or(Expectation::None, |ty| Expectation::Required(ty, origin))
    }

    /// Closes the open variables of `ty`, the type of a join at `at` that is no member. A value
    /// with no type to meet closes them so (see `close_open`). A hint first gives an open `Int` its
    /// type, as it gives a literal. Each variable that the close defaults is bound to its default.
    /// `whole` is the span of the join's members: a call inside it that waits reports there.
    pub(super) fn close_join(
        &mut self,
        ty: Type,
        (at, whole): (Span, Span),
        expected: Expectation,
    ) -> Result<Type, Poisoned> {
        match expected {
            Expectation::Member | Expectation::Required(..) => return Ok(ty),
            Expectation::Hint { want, .. } => {
                let snapshot = self.table.snapshot();
                if self.conversion(ty, want).is_some() {
                    self.keep_snapshot(snapshot);
                } else {
                    self.table.rollback(snapshot);
                }
            }
            Expectation::None | Expectation::Poisoned => {}
        }
        let open = tir::Expr::new(tir::ExprKind::Unit, self.table.resolve(ty), at);
        let closed = self.close_open_in(Ok(open), whole, expected)?.ty;
        self.unify(ty, closed);
        Ok(closed)
    }
}
/// What a member of a join expects, when the join expects `expected`: a required type or a hint
/// as it is, else it is a member. The members are the branches of an `if`, the arms of a
/// `match`, the elements of an array and the `break` values of a loop.
pub(super) fn branch_expectation(expected: Expectation) -> Expectation {
    match expected {
        Expectation::Required(..) | Expectation::Hint { .. } => expected,
        Expectation::None | Expectation::Poisoned | Expectation::Member => Expectation::Member,
    }
}

/// Each of `values`, the value of the branch at its span in `spans`, as a whole member of a join.
pub(super) fn whole_members<'a>(values: &'a mut [Checked], spans: &[Span]) -> Vec<JoinMember<'a>> {
    (values.iter_mut().zip(spans))
        .map(|(value, &label)| JoinMember {
            value,
            at: Place::Whole,
            label,
        })
        .collect()
}
