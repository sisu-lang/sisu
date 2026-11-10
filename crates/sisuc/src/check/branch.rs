//! Branches: `if`, `if let` and the join of branch types.

use super::any::{AnyJoin, is_target};
use super::expr::fits;
use super::generic::{Ahead, Rank};
use super::pattern::{LetPattern, arm_key};
use super::reports::Reported;
use super::{
    Checked, CheckedBlock, Checker, Expectation, Misfit, Poisoned, as_optional, block_type,
    is_jumped, is_kept, payload, tail, value_span, wrap_block,
};
use crate::ast;
use crate::diagnostic::{Diagnostic, DiagnosticKind, Unwrap};
use crate::source::Span;
use crate::tir::{self, Type, TypeKind};

/// The failed `if` or `match` whose join is `ty`: as typed as the join is. A join with a type
/// fails by a failed part, whose proof is `part`.
pub(super) fn failed_join(ty: Result<Type, Poisoned>, part: Option<Reported>) -> Checked {
    Err(match ty {
        Err(poisoned) => poisoned,
        Ok(ty) => Poisoned::typed(
            ty,
            part.expect("a join with a type should fail by a failed part"),
        ),
    })
}

/// The proof of the first failed branch among `blocks`, of a join that failed by a branch.
fn failed_branch(blocks: &[CheckedBlock]) -> Reported {
    (blocks.iter().find_map(|block| block.as_ref().err()))
        .expect("a join that fails by a branch should have a failed branch")
        .reported()
}

impl Checker<'_> {
    /// Without an `else`, `unit`. With one, the branches joined (see `branches`). A condition
    /// that is not `bool` makes the `if` untyped, or `Never` when its branches are.
    pub(super) fn if_expr(
        &mut self,
        span: Span,
        cond: &ast::Expr,
        then_block: &ast::Block,
        else_block: Option<&ast::Block>,
        expected: Expectation,
    ) -> Checked {
        let (cond, mismatched) = self.operand(cond, Expectation::Required(Type::BOOL));
        let (then, other, ty) = self.branches(
            then_block,
            else_block,
            expected,
            |_| None,
            |c, expected| c.block(then_block, expected),
        );
        // A failed join keeps its own `mismatch`, and branches that never end keep `Never`.
        let ty = match (ty, &cond) {
            (Ok(ty), Err(failed)) if mismatched => Err(Poisoned::new(
                (ty == Type::NEVER).then_some(ty),
                failed.reported(),
            )),
            (ty, _) => ty,
        };
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
            (.., ty) => failed_join(ty, part),
        }
    }

    /// `if let pattern = value { ... }`, typed as `if`; what `pattern` binds is in scope in the
    /// first block. On an optional it is an `IfSome`, on an enum a `Match` (`if_let_match`).
    /// A `value` that is neither makes it `Error`, or `Never` when its branches are.
    pub(super) fn if_let(
        &mut self,
        span: Span,
        pattern: &ast::Pattern,
        value: &ast::Expr,
        then_block: &ast::Block,
        else_block: Option<&ast::Block>,
        expected: Expectation,
    ) -> Checked {
        // Ranking the `if let` for context may have checked its value (see `rank_value`).
        let (scrutinee, form, mismatched) = match self.kept(value.span, false) {
            Some((_, Ahead::Let(checked, form, mismatched))) => (checked, form, mismatched),
            _ => self.let_value(Unwrap::IfLet, pattern, value),
        };
        let mut checked = None;
        let bind = |c: &mut Self| Some(c.bind_pattern(Some(Unwrap::IfLet), &form, pattern, None));
        let (then, other, ty) =
            self.branches(then_block, else_block, expected, bind, |c, expected| {
                let (bound, block) =
                    c.let_block(Unwrap::IfLet, &form, pattern, then_block, expected);
                checked = Some(bound);
                block
            });
        let bound = checked.expect("`branches` should check the first block, which sets it");
        // A failed join keeps its own `mismatch`, and branches that never end keep `Never`.
        let ty = match (ty, &scrutinee) {
            (Ok(ty), Err(failed)) if mismatched => Err(Poisoned::new(
                (ty == Type::NEVER).then_some(ty),
                failed.reported(),
            )),
            (ty, _) => ty,
        };
        let ty = ty?;
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
                return Err(Poisoned::typed(ty, part));
            }
        };
        Ok(tir::Expr::new(kind, ty, span))
    }

    /// The blocks of an `if` or `if let`, `then` checking the first, and their type:
    /// `unit` without an `else`; see `join`. `bind` binds what the first block sees, for the
    /// ranking of its value.
    fn branches(
        &mut self,
        then_block: &ast::Block,
        else_block: Option<&ast::Block>,
        expected: Expectation,
        bind: impl FnOnce(&mut Self) -> Option<(LetPattern, Option<Checked>)>,
        then: impl FnOnce(&mut Self, Expectation) -> CheckedBlock,
    ) -> (CheckedBlock, Option<CheckedBlock>, Result<Type, Poisoned>) {
        let Some(else_block) = else_block else {
            return (then(self, Expectation::None), None, Ok(Type::UNIT));
        };
        let (expected, siblings) = expected.split_hint();
        let mut then = Some(then);
        let (blocks, ty) = self.ranked(|c| {
            let sources = [
                c.block_branch(then_block, expected, bind),
                c.block_branch(else_block, expected, |_| None),
            ];
            c.join(&sources, (expected, siblings), |c, i, expected| {
                if i == 0 {
                    let then = then
                        .take()
                        .expect("`join` should check the `then` branch once");
                    then(c, expected)
                } else {
                    c.block(else_block, expected)
                }
            })
        });
        let [first, other] =
            <[_; 2]>::try_from(blocks).unwrap_or_else(|_| panic!("`join` should give two blocks"));
        (first, Some(other), ty)
    }

    /// The `sources.len()` branches of the `if` or arms of the `match` at `span`, `check(i)`
    /// checking branch `i`, and their type. With no `expected` type, the branch whose value is of
    /// lowest `Rank` is checked first, the first in source order on a tie. Each branch after it
    /// expects the type of the first branch of a known type that is not `never`, made optional,
    /// unless its value waits and makes no optional by itself: it expects that type as it is.
    /// That type is a hint (see `Expectation::Hint`). In a hinted `if` or `match`, `siblings`, the
    /// type of the hinted value's siblings, is that first type. A failed or `unit` branch sets
    /// what the branches after it expect only until a branch of another known type is checked.
    /// Then a `never` branch, and after it an `Error` one, takes the others' type. A `T` and a
    /// `T?` join to `T?`, the `T` branches wrapped; other types that differ are reported. A join
    /// that fails for a branch that rejected `expected` is a mismatch too. With no branches, the
    /// type is `never`. A `never` join with a branch that only a failed `break` or `continue`
    /// makes `never` is `Err` with that mark (see `Poisoned::jumped`): one such branch is enough.
    pub(super) fn join(
        &mut self,
        sources: &[Branch<'_>],
        (expected, siblings): (Expectation, Option<Type>),
        mut check: impl FnMut(&mut Self, usize, Expectation) -> CheckedBlock,
    ) -> (Vec<CheckedBlock>, Result<Type, Poisoned>) {
        let mut order: Vec<usize> = (0..sources.len()).collect();
        if expected == Expectation::None {
            order.sort_by_key(|&i| sources[i].rank);
        }
        let mut checked: Vec<Option<CheckedBlock>> = sources.iter().map(|_| None).collect();
        let mut anchor = siblings.map(Some);
        for i in order {
            let carried = if expected == Expectation::None {
                anchor.map_or(Expectation::None, |a| after_branch(a, sources[i].exact()))
            } else {
                expected
            };
            // A type that the branches before give is a hint, not an expected type.
            let carried = match (carried, anchor.flatten()) {
                (Expectation::Required(want), Some(siblings)) if expected == Expectation::None => {
                    Expectation::Hint { want, siblings }
                }
                (carried, _) => carried,
            };
            let block = check(self, i, carried);
            // The first branch of a known type anchors the rest; a failed or `unit` one only
            // until then. An `any` branch widens the anchor, which only the hints read.
            match (block_type(&block), anchor) {
                (Some(Type::NEVER), _) => {}
                (Some(t), None | Some(None | Some(Type::UNIT))) => anchor = Some(Some(t)),
                (Some(t), Some(Some(a))) if t != a && t != Type::UNIT => {
                    if let Some(wider) = self.any_join(a, t) {
                        anchor = Some(Some(wider));
                    }
                }
                (None, None) => anchor = Some(None),
                _ => {}
            }
            checked[i] = Some(block);
        }
        let mut blocks: Vec<CheckedBlock> = checked
            .into_iter()
            .map(|block| block.expect("`join` should check every branch"))
            .collect();
        // Each branch converts to an `any` type that is expected, as a value does.
        if let Some(want) = expected.ty() {
            for (block, source) in blocks.iter_mut().zip(sources) {
                self.any_block(block, want, source.span);
            }
        }
        let (joined, error, failed) = self.fold_branches(sources, (expected, siblings), &blocks);
        // A branch with no type makes the join untyped: its repair can change the join.
        let ty = match joined {
            _ if failed.is_some() || error => None,
            Some(ty) => {
                for (block, source) in blocks.iter_mut().zip(sources) {
                    if block_type(block).is_some_and(|t| t != ty && t != Type::NEVER) {
                        if is_target(ty) {
                            unwrap_some_object(block, ty);
                            self.any_block(block, ty, source.span);
                        } else {
                            wrap_block(block, ty);
                        }
                    }
                }
                Some(ty)
            }
            None => Some(Type::NEVER),
        };
        // A join that fails with a type expected rejects that type, as a branch that does, and
        // then the whole is `Error`, whatever type the other branches joined to.
        let mismatch = failed.is_some() && expected.ty().is_some()
            || blocks
                .iter()
                .any(|b| matches!(b, Err(Poisoned { mismatch: true, .. })));
        // A branch whose generic call found a type that cannot meet its context joins by that
        // type, but the whole stays `Error`. The first such branch carries the joined type to
        // the code that uses the value. A failed join reported the mistake for all of them.
        let mut found = None;
        for block in &blocks {
            if self.misfit_type(block).is_none() {
                continue;
            }
            match (&found, ty) {
                (None, Some(ty)) if failed.is_none() => {
                    found = block
                        .as_ref()
                        .err()
                        .and_then(|p| p.found)
                        .map(|m| Misfit { ty, ..m });
                }
                _ => self.settle_misfit(block),
            }
        }
        let ty = match ty {
            // A join that is `never` only because a branch is (a failed `break` or `continue`)
            // keeps that mark.
            Some(Type::NEVER) if !mismatch && found.is_none() && blocks.iter().any(is_jumped) => {
                Err(Poisoned {
                    jumped: true,
                    ..Poisoned::typed(Type::NEVER, failed_branch(&blocks))
                })
            }
            Some(ty) if !mismatch && found.is_none() => Ok(ty),
            _ => Err(Poisoned {
                mismatch,
                found,
                // A join that has no type is kept when a branch is: that call's E0050 is the one
                // report.
                kept: ty.is_none() && blocks.iter().any(is_kept),
                ..Poisoned::untyped(failed.unwrap_or_else(|| failed_branch(&blocks)))
            }),
        };
        (blocks, ty)
    }

    /// The type that the `blocks` join to, whether a block has no type, and the proof of the report
    /// when the join failed. With an `any` branch, the join is that type (see `any_join_all`),
    /// whatever the order. Else the types fold in source order: a `T` and a `T?` make a `T?`, and
    /// other types that differ are reported. Branches that do not join by themselves join to an
    /// `any` type that `siblings` gives (see `seeded_join`). A `Some` of an object joins an `any`
    /// type as the object, and the join is then optional. With an `any` type in reach, the branch
    /// that does not convert to it is the mistake (see `leaves_any`).
    fn fold_branches(
        &mut self,
        sources: &[Branch<'_>],
        (expected, siblings): (Expectation, Option<Type>),
        blocks: &[CheckedBlock],
    ) -> (Option<Type>, bool, Option<Reported>) {
        // A branch's misfit joins by the type it found (see `Misfit`), as a concrete one would.
        let types: Vec<Option<Type>> = (blocks.iter())
            .map(|b| block_type(b).or_else(|| self.misfit_type(b)))
            .collect();
        let error = types.contains(&None);
        let members: Vec<(usize, Type)> = (types.iter().enumerate())
            .filter_map(|(i, t)| t.filter(|&t| t != Type::NEVER).map(|t| (i, t)))
            .collect();
        if let Some(d) = self.leaves_written(sources, blocks, expected, &members) {
            return (None, error, Some(self.diagnostics.error(d)));
        }
        let failed = match self.any_join_all(&members) {
            AnyJoin::Joined { ty } => return (Some(ty), error, None),
            AnyJoin::NoAny => match fold_in_order(&members) {
                Ok(joined) => return (joined.map(|(_, ty)| ty), error, None),
                Err((earlier, later)) => AnyJoin::Fails { earlier, later },
            },
            failed => failed,
        };
        // A `Some` of an object joins an `any` type as the object, and a `None` as no member; each
        // makes the join optional.
        let objects: Vec<(usize, Type)> = (members.iter())
            .filter_map(|&(i, t)| match some_object(&blocks[i]) {
                Some(Unwrapped::Object(o)) => Some((i, o)),
                Some(Unwrapped::Nothing) => None,
                None => Some((i, t)),
            })
            .collect();
        let made = |ty| {
            if objects == members {
                ty
            } else {
                as_optional(ty)
            }
        };
        let failed = if objects == members {
            failed
        } else {
            match self.any_join_all(&objects) {
                AnyJoin::Joined { ty } => return (Some(made(ty)), error, None),
                leaves @ AnyJoin::Leaves { .. } => leaves,
                AnyJoin::NoAny | AnyJoin::Fails { .. } => failed,
            }
        };
        // With an `any` type in reach, the mistake is the member that leaves it.
        let failed = match self.seeded_join(siblings, &objects) {
            AnyJoin::Joined { ty } => {
                #[cfg(test)]
                super::reached("seeded_join for a branch");
                return (Some(made(ty)), error, None);
            }
            leaves @ AnyJoin::Leaves { .. } => leaves,
            AnyJoin::NoAny | AnyJoin::Fails { .. } => failed,
        };
        let d = match failed {
            AnyJoin::Leaves {
                member: (i, Type::UNIT),
                ..
            } => DiagnosticKind::NoValue {
                at: sources[i].span,
                never: false,
            }
            .into(),
            AnyJoin::Leaves { ty, member: (i, t) } => {
                let (at, tail, want, got) = self.reported(&sources[i], &blocks[i], ty, t);
                self.leaves_any(at, tail, want, got)
            }
            AnyJoin::Fails { earlier, later } => {
                // A hint that is no `any` type names the branch that leaves it, as an expected
                // type does.
                let want = match (expected, siblings.filter(|&ty| !is_target(ty))) {
                    (Expectation::None, Some(ty)) => Expectation::Required(ty),
                    _ => expected,
                };
                self.join_mismatch((sources, blocks), want, earlier, later)
            }
            AnyJoin::NoAny | AnyJoin::Joined { .. } => {
                unreachable!("a join that failed should name its mistake")
            }
        };
        (None, error, Some(self.diagnostics.error(d)))
    }

    /// The mistake when the branch `first`, of type `first_ty`, and the later branch `other`,
    /// of type `other_ty`, do not join. With a type expected, the branch that fits it neither as
    /// is nor as its payload is the mistake; without one, `other` is, against `first_ty`. A
    /// branch with no value is that mistake, not a type mismatch.
    fn join_mismatch(
        &mut self,
        (sources, blocks): (&[Branch<'_>], &[CheckedBlock]),
        expected: Expectation,
        (first, first_ty): (usize, Type),
        (other, other_ty): (usize, Type),
    ) -> Diagnostic {
        let mut report = |i: usize, want: Type, got: Type| {
            let (at, tail, want, got) = self.reported(&sources[i], &blocks[i], want, got);
            self.leaves_any(at, tail, want, got)
        };
        match expected.ty() {
            _ if first_ty == Type::UNIT => DiagnosticKind::NoValue {
                at: sources[first].span,
                never: false,
            }
            .into(),
            _ if other_ty == Type::UNIT => DiagnosticKind::NoValue {
                at: sources[other].span,
                never: false,
            }
            .into(),
            Some(want) if fits(want, first_ty) => report(other, want, other_ty),
            Some(want) => report(first, want, first_ty),
            None => report(other, first_ty, other_ty),
        }
    }

    /// With a written type and an `any` type in the join, the mistake as a join with no `any` type
    /// reports it: the first member, in source order, that does not fit the written type or
    /// convert to it, against that type.
    fn leaves_written(
        &mut self,
        sources: &[Branch<'_>],
        blocks: &[CheckedBlock],
        expected: Expectation,
        members: &[(usize, Type)],
    ) -> Option<Diagnostic> {
        let want = expected.ty()?;
        if !is_target(want) && !members.iter().any(|&(_, t)| is_target(t)) {
            return None;
        }
        // A branch with no value is the mistake first, as in `join_mismatch`.
        if let Some(&(i, _)) = members.iter().find(|&&(_, t)| t == Type::UNIT) {
            let at = sources[i].span;
            return Some(DiagnosticKind::NoValue { at, never: false }.into());
        }
        for &(i, t) in members {
            if !fits(want, t) && !self.converts_to_any(t, want) {
                let (at, tail, want, got) = self.reported(&sources[i], &blocks[i], want, t);
                return Some(self.leaves_any(at, tail, want, got));
            }
        }
        None
    }

    /// Where the branch `source`, of type `got`, is reported as a mistake against `want`, with
    /// its expression, and the two types to name (see `object_in_some`).
    fn reported<'a>(
        &mut self,
        source: &Branch<'a>,
        block: &CheckedBlock,
        want: Type,
        got: Type,
    ) -> (Span, Option<&'a ast::Expr>, Type, Type) {
        let value = block.as_ref().ok().and_then(|b| b.value.as_deref());
        match value.and_then(|value| self.object_in_some(value, want)) {
            Some((at, want, object)) => (at, None, want, object),
            None => (source.span, source.tail, want, got),
        }
    }

    /// Where `value`, a `Some` of an object that leaves `want`, an object or an optional object,
    /// is the mistake, and the two types to name, when the object does not convert to the
    /// payload of `want`: the object, against that payload, as when the `Some` checks its
    /// argument against it. A `Some` of a `Some` looks through each layer that `want` has.
    pub(super) fn object_in_some(
        &mut self,
        value: &tir::Expr,
        want: Type,
    ) -> Option<(Span, Type, Type)> {
        #[cfg(test)]
        super::OBJECT_IN_SOME_CALLS.set(super::OBJECT_IN_SOME_CALLS.get() + 1);
        let object = |ty: Type| matches!(ty.kind(), TypeKind::Class(..) | TypeKind::Any(..));
        let want = payload(want);
        match &value.kind {
            tir::ExprKind::Wrap(inner)
                if object(inner.ty)
                    && object(want)
                    && inner.ty != want
                    && !self.converts_to_any(inner.ty, want) =>
            {
                Some((inner.span, want, inner.ty))
            }
            // A `Some` of a `Some` checks its argument against an optional payload.
            tir::ExprKind::Wrap(inner) if matches!(want.kind(), TypeKind::Optional(_)) => {
                self.object_in_some(inner, want)
            }
            _ => None,
        }
    }
}

/// A branch of an `if` or an arm of a `match`, as `join` sees its source: the `Rank` of its
/// value, whether it can make an optional by itself (`Ranked::optional`), where the value is, and
/// the expression it ends in, if any.
pub(super) struct Branch<'a> {
    rank: Rank,
    optional: bool,
    span: Span,
    tail: Option<&'a ast::Expr>,
}

impl Branch<'_> {
    /// Whether the branch expects the type of the others as it is, not made optional: its value
    /// waits and makes no optional by itself.
    fn exact(&self) -> bool {
        self.rank == Rank::Waits && !self.optional
    }
}

impl Checker<'_> {
    /// The branch for the value of `block`. Its value is ranked only when no type is expected,
    /// as `join` orders the branches only then. `bind` binds what the block sees (see
    /// `block_rank`).
    pub(super) fn block_branch<'a>(
        &mut self,
        block: &'a ast::Block,
        expected: Expectation,
        bind: impl FnOnce(&mut Self) -> Option<(LetPattern, Option<Checked>)>,
    ) -> Branch<'a> {
        let mut fit = true;
        let ranked = (expected == Expectation::None)
            .then(|| self.block_rank(block, &mut fit, bind))
            .flatten();
        Branch {
            rank: ranked.map_or(Rank::Concrete, |ranked| ranked.sibling(fit)),
            optional: ranked.is_some_and(|ranked| ranked.optional),
            span: value_span(block),
            tail: tail(block),
        }
    }

    /// The branch for `e`, an element of an array literal that joins as a branch does (see
    /// `hinted_elements`).
    pub(super) fn value_branch<'a>(&mut self, e: &'a ast::Expr) -> Branch<'a> {
        let (rank, optional) = self.sibling_ranked(e);
        Branch {
            rank,
            optional,
            span: e.span,
            tail: Some(e),
        }
    }

    /// The branch for the body `e` of an arm that is not a block, the arm's value. See
    /// `block_branch`.
    pub(super) fn expr_branch<'a>(
        &mut self,
        arm: &ast::Arm,
        e: &'a ast::Expr,
        expected: Expectation,
        bind: impl FnOnce(&mut Self) -> Option<(LetPattern, Option<Checked>)>,
    ) -> Branch<'a> {
        let mut fit = true;
        let ranked = (expected == Expectation::None)
            .then(|| self.tail_rank(arm_key(arm), None, e, &mut fit, bind));
        Branch {
            rank: ranked.map_or(Rank::Concrete, |ranked| ranked.sibling(fit)),
            optional: ranked.is_some_and(|ranked| ranked.optional),
            span: e.span,
            tail: Some(e),
        }
    }
}

/// The type that the typed `members` join to in source order, and the branch it came from, or
/// the two branches that do not join: a `T` and a `T?` make a `T?`, and other types that differ
/// do not join.
fn fold_in_order(members: &[(usize, Type)]) -> Result<Option<(usize, Type)>, Pair> {
    let mut joined = None::<(usize, Type)>;
    for &(i, t) in members {
        match joined {
            None => joined = Some((i, t)),
            // The side with more layers takes the other, one layer below it.
            Some((_, j)) if j == t || j == Type::optional(t) => {}
            Some((k, j)) if t == Type::optional(j) => joined = Some((k, t)),
            Some(earlier) => return Err((earlier, (i, t))),
        }
    }
    // The join can grow past an earlier branch: each converts through one layer only.
    if let Some((k, j)) = joined
        && let Some(&other) = members.iter().find(|&&(_, t)| !fits(j, t))
    {
        return Err(((k, j), other));
    }
    Ok(joined)
}

/// A branch value that joins an `any` type without its optional layer (see `fold_branches`).
enum Unwrapped {
    /// A `Some` of an object joins as the object, of this type.
    Object(Type),
    /// A `None` joins as no member.
    Nothing,
}

/// How the value of `block` joins an `any` type, when it is a `Some` of an object or a `None`.
fn some_object(block: &CheckedBlock) -> Option<Unwrapped> {
    let value = block.as_ref().ok()?.value.as_deref()?;
    match &value.kind {
        tir::ExprKind::Wrap(inner)
            if matches!(inner.ty.kind(), TypeKind::Class(..) | TypeKind::Any(..)) =>
        {
            Some(Unwrapped::Object(inner.ty))
        }
        tir::ExprKind::None => Some(Unwrapped::Nothing),
        _ => None,
    }
}

/// Makes the `Some` of an object that ends `block` the object, which a join then converts to
/// its optional `any` type `ty`, and a `None` that ends it a `None` of `ty` (see
/// `fold_branches`).
fn unwrap_some_object(block: &mut CheckedBlock, ty: Type) {
    let (Some(_), Ok(done)) = (some_object(block), block) else {
        return;
    };
    let value = done
        .value
        .as_mut()
        .expect("`some_object` should have found a value");
    match std::mem::replace(&mut value.kind, tir::ExprKind::None) {
        tir::ExprKind::Wrap(inner) => {
            #[cfg(test)]
            super::reached("unwrap_some_object of a Some");
            **value = *inner;
        }
        none => {
            #[cfg(test)]
            super::reached("unwrap_some_object of a None");
            value.kind = none;
            value.ty = ty;
        }
    }
    done.ty = value.ty;
}

/// Two members of a join, each an index and a type, that do not join: the earlier, then the
/// later.
type Pair = ((usize, Type), (usize, Type));

/// What one branch of an `if` expects after the other, of type `ty`, when the `if` expects
/// nothing: `ty` made optional, unless it is one or the branch is `exact`; `Error` after
/// `Error`; nothing after `unit` or `never`.
fn after_branch(ty: Option<Type>, exact: bool) -> Expectation {
    match ty {
        None => Expectation::Poisoned,
        Some(Type::UNIT | Type::NEVER) => Expectation::None,
        Some(ty) if exact => Expectation::Required(ty),
        Some(ty) => Expectation::Required(as_optional(ty)),
    }
}
