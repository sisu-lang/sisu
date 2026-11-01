//! Branches: `if`, `if let` and the join of branch types.

use super::any::is_target;
use super::expr::fits;
use super::generic::{Ahead, Rank};
use super::pattern::{LetPattern, arm_key};
use super::{
    Checked, CheckedBlock, Checker, Poisoned, UNKNOWN, as_optional, block_type, tail, value_span,
    wrap_block,
};
use crate::ast;
use crate::diagnostic::{Diagnostic, DiagnosticKind, Unwrap};
use crate::source::Span;
use crate::tir::{self, Type};

impl Checker<'_> {
    /// Without an `else`, `unit`. With one, the branches joined (see `branches`). A condition
    /// that is not `bool` makes the `if` `Error`, or `Never` when its branches are.
    pub(super) fn if_expr(
        &mut self,
        span: Span,
        cond: &ast::Expr,
        then_block: &ast::Block,
        else_block: Option<&ast::Block>,
        expected: Option<Type>,
    ) -> Checked {
        let (cond, mismatched) = self.operand(cond, Some(Type::BOOL));
        let (then, other, ty) = self.branches(
            span,
            then_block,
            else_block,
            expected,
            |_| None,
            |c, expected| c.block(then_block, expected),
        );
        // A failed join keeps its own `mismatch`, and branches that never end keep `Never`.
        let ty = match ty {
            Ok(Type::NEVER) if mismatched => Err(Poisoned::typed(Type::NEVER)),
            Ok(_) if mismatched => Err(Poisoned::error()),
            ty => ty,
        };
        match (cond, then, other.transpose(), ty) {
            (Ok(cond), Ok(then_block), Ok(else_block), Ok(ty)) => Ok(tir::Expr {
                kind: tir::ExprKind::If {
                    cond: Box::new(cond),
                    then_block,
                    else_block,
                },
                ty,
                span,
            }),
            (.., ty) => Err(ty.map_or_else(|poisoned| poisoned, Poisoned::typed)),
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
        expected: Option<Type>,
    ) -> Checked {
        // Ranking the `if let` for context may have checked its value (see `rank_value`).
        let (scrutinee, form, mismatched) = match self.kept(value.span, false) {
            Some((_, Ahead::Let(checked, form, mismatched))) => (checked, form, mismatched),
            _ => self.let_value(Unwrap::IfLet, pattern, value),
        };
        let mut checked = None;
        let bind = |c: &mut Self| Some(c.bind_pattern(Some(Unwrap::IfLet), &form, pattern, None));
        let (then, other, ty) = self.branches(
            span,
            then_block,
            else_block,
            expected,
            bind,
            |c, expected| {
                let (bound, block) =
                    c.let_block(Unwrap::IfLet, &form, pattern, then_block, expected);
                checked = Some(bound);
                block
            },
        );
        let bound = checked.expect("`branches` should check the first block, which sets it");
        // A failed join keeps its own `mismatch`, and branches that never end keep `Never`.
        let ty = match ty {
            Ok(Type::NEVER) if mismatched => Err(Poisoned::typed(Type::NEVER)),
            Ok(_) if mismatched => Err(Poisoned::error()),
            ty => ty,
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
                    pattern: Some(pattern),
                },
            ) => self.if_let_match(scrutinee, pattern, then_block, else_block),
            _ => return Err(Poisoned::typed(ty)),
        };
        Ok(tir::Expr { kind, ty, span })
    }

    /// The blocks of the `if` or `if let` at `span`, `then` checking the first, and their type:
    /// `unit` without an `else`; see `join`. `bind` binds what the first block sees, for the
    /// ranking of its value.
    fn branches(
        &mut self,
        span: Span,
        then_block: &ast::Block,
        else_block: Option<&ast::Block>,
        expected: Option<Type>,
        bind: impl FnOnce(&mut Self) -> Option<(LetPattern, Option<Checked>)>,
        then: impl FnOnce(&mut Self, Option<Type>) -> CheckedBlock,
    ) -> (CheckedBlock, Option<CheckedBlock>, Result<Type, Poisoned>) {
        let Some(else_block) = else_block else {
            return (then(self, None), None, Ok(Type::UNIT));
        };
        let mut then = Some(then);
        let (blocks, ty) = self.ranked(|c| {
            let sources = [
                c.block_branch(then_block, expected, bind),
                c.block_branch(else_block, expected, |_| None),
            ];
            c.join(span, &sources, expected, |c, i, expected| {
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
    /// A failed or `unit` branch sets what the branches after it expect only until a branch of
    /// another known type is checked. Then a `never` branch, and after it an `Error` one, takes
    /// the others' type. A `T` and a `T?` join to `T?`, the `T` branches wrapped; other types
    /// that differ are reported. A join that fails for a branch that rejected `expected` is a
    /// mismatch too. With no branches, the type is `never`. A branch with no expected type is
    /// quiet when the `if` or `match` is (see `Body::quiet_at`).
    pub(super) fn join(
        &mut self,
        span: Span,
        sources: &[Branch<'_>],
        expected: Option<Type>,
        mut check: impl FnMut(&mut Self, usize, Option<Type>) -> CheckedBlock,
    ) -> (Vec<CheckedBlock>, Result<Type, Poisoned>) {
        let mut order: Vec<usize> = (0..sources.len()).collect();
        if expected.is_none() {
            order.sort_by_key(|&i| sources[i].rank);
        }
        let mut checked: Vec<Option<CheckedBlock>> = sources.iter().map(|_| None).collect();
        let mut anchor = None;
        for i in order {
            let expected =
                expected.or_else(|| anchor.and_then(|a| after_branch(a, sources[i].exact())));
            let outer = self.pass_quiet(span, sources[i].tail.map(|v| v.span), expected);
            let block = check(self, i, expected);
            self.body.quiet_at = outer;
            // The first branch of a known type anchors the rest; a failed or `unit` one only
            // until then.
            match block_type(&block) {
                Some(Type::NEVER) => {}
                Some(t) if matches!(anchor, None | Some(None | Some(Type::UNIT))) => {
                    anchor = Some(Some(t));
                }
                None if anchor.is_none() => anchor = Some(None),
                _ => {}
            }
            checked[i] = Some(block);
        }
        let mut blocks: Vec<CheckedBlock> = checked
            .into_iter()
            .map(|block| block.expect("`join` should check every branch"))
            .collect();
        // Each branch converts to an `any` type that is expected, as a value does.
        if let Some(want) = expected.filter(|&want| want != UNKNOWN) {
            for (block, source) in blocks.iter_mut().zip(sources) {
                self.any_block(block, want, source.span);
            }
        }
        // The type so far, and the branch it came from, folded in source order.
        let (mut joined, mut error, mut failed) = (None::<(usize, Type)>, false, false);
        for (i, block) in blocks.iter().enumerate() {
            let Some(t) = block_type(block) else {
                error = true;
                continue;
            };
            match joined {
                _ if t == Type::NEVER => {}
                None => joined = Some((i, t)),
                // The side with more layers takes the other, one layer below it.
                Some((_, j)) if j == t || j == Type::optional(t) => {}
                Some((k, j)) if t == Type::optional(j) => {
                    joined = Some((k, t));
                }
                // An object and an `any` type that it converts to join to the `any` type.
                Some((k, j)) => {
                    if let Some(w) = self.any_join(j, t) {
                        joined = Some((if w == t { i } else { k }, w));
                    } else {
                        let d = self.join_mismatch(sources, expected, (k, j), (i, t));
                        self.diagnostics.push(d);
                        failed = true;
                        break;
                    }
                }
            }
        }
        // The join can grow past an earlier branch: each converts through one layer only.
        if let Some((k, j)) = joined.filter(|_| !failed) {
            let types: Vec<Option<Type>> = blocks.iter().map(block_type).collect();
            let short = types.into_iter().enumerate().find_map(|(i, t)| {
                let t = t?;
                (t != Type::NEVER && !fits(j, t) && !self.converts_to_any(t, j)).then_some((i, t))
            });
            if let Some(other) = short {
                let d = self.join_mismatch(sources, expected, (k, j), other);
                self.diagnostics.push(d);
                failed = true;
            }
        }
        let ty = match joined {
            _ if failed => None,
            Some((_, ty)) => {
                for (block, source) in blocks.iter_mut().zip(sources) {
                    if block_type(block).is_some_and(|t| t != ty && t != Type::NEVER) {
                        if is_target(ty) {
                            self.any_block(block, ty, source.span);
                        } else {
                            wrap_block(block, ty);
                        }
                    }
                }
                Some(ty)
            }
            None if error => None,
            None => Some(Type::NEVER),
        };
        // A join that fails with a type expected rejects that type, as a branch that does, and
        // then the whole is `Error`, whatever type the other branches joined to.
        let mismatch = failed && expected.is_some_and(|want| want != UNKNOWN)
            || blocks
                .iter()
                .any(|b| matches!(b, Err(Poisoned { mismatch: true, .. })));
        let ty = match ty {
            Some(ty) if !mismatch => Ok(ty),
            _ => Err(Poisoned { ty: None, mismatch }),
        };
        (blocks, ty)
    }

    /// The mistake when the branch `first`, of type `first_ty`, and the later branch `other`,
    /// of type `other_ty`, do not join. With a type expected, the branch that fits it neither as
    /// is nor as its payload is the mistake; without one, `other` is, against `first_ty`. A
    /// branch with no value is that mistake, not a type mismatch.
    fn join_mismatch(
        &self,
        sources: &[Branch<'_>],
        expected: Option<Type>,
        (first, first_ty): (usize, Type),
        (other, other_ty): (usize, Type),
    ) -> Diagnostic {
        let (first, other) = (&sources[first], &sources[other]);
        match expected.filter(|&want| want != UNKNOWN) {
            _ if first_ty == Type::UNIT => DiagnosticKind::NoValue {
                at: first.span,
                never: false,
            }
            .into(),
            _ if other_ty == Type::UNIT => DiagnosticKind::NoValue {
                at: other.span,
                never: false,
            }
            .into(),
            Some(want) if fits(want, first_ty) => {
                self.mismatch(other.span, other.tail, want, other_ty)
            }
            Some(want) => self.mismatch(first.span, first.tail, want, first_ty),
            None => self.mismatch(other.span, other.tail, first_ty, other_ty),
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
        expected: Option<Type>,
        bind: impl FnOnce(&mut Self) -> Option<(LetPattern, Option<Checked>)>,
    ) -> Branch<'a> {
        let mut fit = true;
        let ranked = expected
            .is_none()
            .then(|| self.block_rank(block, &mut fit, bind))
            .flatten();
        Branch {
            rank: ranked.map_or(Rank::Concrete, |ranked| ranked.sibling(fit)),
            optional: ranked.is_some_and(|ranked| ranked.optional),
            span: value_span(block),
            tail: tail(block),
        }
    }

    /// The branch for the body `e` of an arm that is not a block, the arm's value. See
    /// `block_branch`.
    pub(super) fn expr_branch<'a>(
        &mut self,
        arm: &ast::Arm,
        e: &'a ast::Expr,
        expected: Option<Type>,
        bind: impl FnOnce(&mut Self) -> Option<(LetPattern, Option<Checked>)>,
    ) -> Branch<'a> {
        let mut fit = true;
        let ranked = expected
            .is_none()
            .then(|| self.tail_rank(arm_key(arm), None, e, &mut fit, bind));
        Branch {
            rank: ranked.map_or(Rank::Concrete, |ranked| ranked.sibling(fit)),
            optional: ranked.is_some_and(|ranked| ranked.optional),
            span: e.span,
            tail: Some(e),
        }
    }
}

/// What one branch of an `if` expects after the other, of type `ty`, when the `if` expects
/// nothing: `ty` made optional, unless it is one or the branch is `exact`; `Error` after
/// `Error`; nothing after `unit` or `never`.
fn after_branch(ty: Option<Type>, exact: bool) -> Option<Type> {
    match ty {
        None => Some(UNKNOWN),
        Some(Type::UNIT | Type::NEVER) => None,
        Some(ty) if exact => Some(ty),
        Some(ty) => Some(as_optional(ty)),
    }
}
