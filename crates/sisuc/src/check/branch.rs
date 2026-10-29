//! Branches: `if`, `if let` and the join of branch types.

use super::expr::{fits, rank};
use super::generic::Ahead;
use super::pattern::LetPattern;
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
        let (then, other, ty) =
            self.branches(span, then_block, else_block, expected, |c, expected| {
                c.block(then_block, expected)
            });
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
        // Ranking the `if let` for context may have checked its value (see `waits`).
        let (scrutinee, form, mismatched) = match self.kept(value.span, false) {
            Some((_, Ahead::Let(checked, form, mismatched))) => (checked, form, mismatched),
            _ => self.let_value(Unwrap::IfLet, pattern, value),
        };
        let mut checked = None;
        let (then, other, ty) =
            self.branches(span, then_block, else_block, expected, |c, expected| {
                let (bound, block) =
                    c.let_block(Unwrap::IfLet, &form, pattern, then_block, expected);
                checked = Some(bound);
                block
            });
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
    /// `unit` without an `else`; see `join`.
    fn branches(
        &mut self,
        span: Span,
        then_block: &ast::Block,
        else_block: Option<&ast::Block>,
        expected: Option<Type>,
        then: impl FnOnce(&mut Self, Option<Type>) -> CheckedBlock,
    ) -> (CheckedBlock, Option<CheckedBlock>, Result<Type, Poisoned>) {
        let Some(else_block) = else_block else {
            return (then(self, None), None, Ok(Type::UNIT));
        };
        let mut then = Some(then);
        let sources = [Branch::of_block(then_block), Branch::of_block(else_block)];
        let (blocks, ty) = self.join(span, &sources, expected, |c, i, expected| {
            if i == 0 {
                let then = then
                    .take()
                    .expect("`join` should check the `then` branch once");
                then(c, expected)
            } else {
                c.block(else_block, expected)
            }
        });
        let [first, other] =
            <[_; 2]>::try_from(blocks).unwrap_or_else(|_| panic!("`join` should give two blocks"));
        (first, Some(other), ty)
    }

    /// The `sources.len()` branches of the `if` or arms of the `match` at `span`, `check(i)`
    /// checking branch `i`, and their type. With no `expected` type, the branch whose value is of
    /// lowest `rank` is checked first, the first in source order on a tie. Each branch after it
    /// expects the type of the first branch of a known type that is not `never`, made optional.
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
            let expected = expected.or_else(|| anchor.and_then(after_branch));
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
                Some((k, j)) => {
                    let d = self.join_mismatch(sources, expected, (k, j), (i, t));
                    self.diagnostics.push(d);
                    failed = true;
                    break;
                }
            }
        }
        // The join can grow past an earlier branch: each converts through one layer only.
        if let Some((k, j)) = joined.filter(|_| !failed) {
            let short = blocks.iter().enumerate().find_map(|(i, block)| {
                let t = block_type(block)?;
                (t != Type::NEVER && !fits(j, t)).then_some((i, t))
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
                for block in &mut blocks {
                    if block_type(block).is_some_and(|t| t != ty && t != Type::NEVER) {
                        wrap_block(block, ty);
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

/// The `rank` of `block`'s value; a block that ends in no expression is concrete.
fn block_rank(block: &ast::Block) -> u8 {
    tail(block).map_or(0, rank)
}

/// A branch of an `if` or an arm of a `match`, as `join` sees its source: the `rank` of its
/// value, where the value is, and the expression it ends in, if any.
pub(super) struct Branch<'a> {
    rank: u8,
    span: Span,
    tail: Option<&'a ast::Expr>,
}

impl<'a> Branch<'a> {
    pub(super) fn of_block(block: &'a ast::Block) -> Self {
        Branch {
            rank: block_rank(block),
            span: value_span(block),
            tail: tail(block),
        }
    }

    pub(super) fn of_expr(e: &'a ast::Expr) -> Self {
        Branch {
            rank: rank(e),
            span: e.span,
            tail: Some(e),
        }
    }
}

/// What one branch of an `if` expects after the other, of type `ty`, when the `if` expects
/// nothing: `ty` made optional, unless it is one; `Error` after `Error`; nothing after `unit`
/// or `never`.
fn after_branch(ty: Option<Type>) -> Option<Type> {
    match ty {
        None => Some(UNKNOWN),
        Some(Type::UNIT | Type::NEVER) => None,
        Some(ty) => Some(as_optional(ty)),
    }
}
