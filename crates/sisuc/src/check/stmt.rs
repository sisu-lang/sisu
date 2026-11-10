//! Statements, blocks and scopes.

use rustc_hash::{FxHashMap, FxHashSet};

use super::any::{AnyJoin, is_target, widen};
use super::array::optional_array;
use super::enums::unwrapped;
use super::expr::{fits, local};
use super::generic::Ahead;
use super::pattern::LetPattern;
use super::prelude::PRELUDE;
use super::reports::Reported;
use super::unused_names::{Owner, Pending};
use super::{
    Binding, BindingKind, Checked, CheckedBlock, Checker, Declared, Expectation, LoopContext,
    LoopFrame, LoopKind, Poisoned, SelfType, block_expr, built_in_type, known_type, starts_in,
    tail, type_of,
};
use crate::ast::{self, StmtKind};
use crate::diagnostic::{
    ClashForm, ClashKind, DiagnosticKind, MismatchSource, NeverReassignedForm, RangePlace, Unwrap,
    UnwrapHelp, closest,
};
use crate::source::{Span, removal_end};
use crate::symbol::Symbol;
use crate::tir::{self, ExprKind, Type, TypeKind};
use std::slice::SliceIndex;

/// What `block_statements` has checked of a block, and what those statements leave.
pub(super) struct BlockSoFar {
    stmts: Vec<tir::Stmt>,
    /// `never` once a statement is, else the last statement's type; `None` is `Error`.
    ty: Option<Type>,
    /// The token of the first statement that failed.
    failed: Option<Reported>,
    /// Whether "unreachable code" is reported, or moot.
    warned: bool,
    /// The value, if it failed. It records whether the value rejected the type expected of it.
    /// It records what the value found and could not meet its context with (see `Misfit`).
    /// It records whether a generic call in it kept its E0050. A statement before it leaves none.
    value: Option<Poisoned>,
    /// How many of the block's statements are checked.
    done: usize,
    /// Whether the statement that ends the block with `never` is a failed `break` or `continue`
    /// outside a loop (see `Poisoned::jumped`).
    jumped: bool,
}

impl Default for BlockSoFar {
    fn default() -> Self {
        BlockSoFar {
            stmts: Vec::new(),
            ty: Some(Type::UNIT),
            failed: None,
            warned: false,
            value: None,
            done: 0,
            jumped: false,
        }
    }
}

/// The most values of one statement that share one fix.
// shortcut: past the cap the warnings offer no fix; store one fix per statement and share it if
// long `match` statements need fixes.
const SHARED_FIX_SITES: usize = 8;

impl Checker<'_> {
    /// `never` once a statement is `never`, else the type of the last statement (`unit` when
    /// there is none). Warns "unreachable code" on the first statement after a `never` one.
    /// Every statement is checked, also after one fails. `expected` is for the block's value.
    pub(super) fn block(&mut self, block: &ast::Block, expected: Expectation) -> CheckedBlock {
        // Ranking an argument may have checked all but the block's value (see `tail_rank`).
        let (scope, so_far) = match self.kept(block.span, true) {
            Some((_, Ahead::Block(scope, so_far))) => (scope, so_far),
            _ => (FxHashMap::default(), BlockSoFar::default()),
        };
        self.body.scopes.push(scope);
        self.resume_block(block, expected, so_far)
    }

    /// `block`, in the scope just opened for it, which it closes.
    pub(super) fn scoped_block(
        &mut self,
        block: &ast::Block,
        expected: Expectation,
    ) -> CheckedBlock {
        self.resume_block(block, expected, BlockSoFar::default())
    }

    /// `scoped_block`, its statements before `so_far.done` checked already.
    pub(super) fn resume_block(
        &mut self,
        block: &ast::Block,
        expected: Expectation,
        mut so_far: BlockSoFar,
    ) -> CheckedBlock {
        self.block_statements(block, expected, &mut so_far, block.stmts.len());
        let BlockSoFar {
            mut stmts,
            ty,
            failed,
            value,
            jumped,
            ..
        } = so_far;
        self.pop_scope();
        let ty = match (ty, failed) {
            (Some(ty), None) => ty,
            // A tail that rejected `expected` fails the block, unless a `never` came first: then
            // no code uses the tail's value, so its misfit is not handed up.
            (ty, failed) => {
                let value = (value.or_else(|| failed.map(Poisoned::untyped)))
                    .expect("a block with no type should have a failed statement");
                let none = ty.is_none();
                return Err(Poisoned {
                    ty,
                    mismatch: value.mismatch && none,
                    found: value.found.filter(|_| none),
                    kept: value.kept && none,
                    // The block is `never` exactly when `ty` is, and then `jumped` says why.
                    jumped,
                    reported: value.reported,
                });
            }
        };
        let ends_in_expr = tail(block).is_some();
        let value = match stmts.pop() {
            Some(tir::Stmt::Expr(e)) if ends_in_expr => Some(Box::new(e)),
            last => {
                stmts.extend(last);
                None
            }
        };
        Ok(tir::Block { stmts, value, ty })
    }

    /// Checks the statements of `block` from `so_far.done` up to `end`, at most its length, in
    /// its scope; `expected` is for its value.
    pub(super) fn block_statements(
        &mut self,
        block: &ast::Block,
        expected: Expectation,
        so_far: &mut BlockSoFar,
        end: usize,
    ) {
        let done = so_far.done;
        so_far.stmts.reserve(end - done);
        for (i, stmt) in (done..).zip(&block.stmts[done..end]) {
            if so_far.ty == Some(Type::NEVER) && !so_far.warned {
                self.warn(DiagnosticKind::UnreachableCode { at: stmt.span });
                so_far.warned = true;
            }
            let last = i + 1 == block.stmts.len();
            let (checked, invalid_jump) =
                self.jumps(|c| c.stmt(stmt, if last { expected } else { Expectation::None }));
            let stmt_ty = match checked {
                Ok(lowered) => {
                    if !last
                        && let (StmtKind::Expr(source), tir::Stmt::Expr(e)) = (&stmt.kind, &lowered)
                    {
                        self.discard(source, e);
                    }
                    // `let _ = e` lowers to an expression, but a `let` has no value.
                    let ty = if matches!(stmt.kind, StmtKind::Let { .. }) {
                        Type::UNIT
                    } else {
                        stmt_type(&lowered)
                    };
                    so_far.stmts.push(lowered);
                    Some(ty)
                }
                Err(poisoned) => {
                    so_far.failed.get_or_insert(poisoned.reported);
                    let ty = poisoned.ty;
                    if ty == Some(Type::NEVER) && so_far.ty != Some(Type::NEVER) {
                        so_far.jumped = poisoned.jumped;
                    }
                    so_far.value = last.then_some(poisoned);
                    // An invalid jump made the block `never` only by error; what follows is no
                    // new news. Any other failed `never` statement still leaves the block.
                    so_far.warned |= invalid_jump && ty == Some(Type::NEVER);
                    ty
                }
            };
            if so_far.ty != Some(Type::NEVER) {
                so_far.ty = stmt_ty;
            }
        }
        so_far.done = end;
    }

    /// Warns "unused value" at `source`, whose checked form is `checked`, unless its type is
    /// `unit` or `never`. An `if`, an `if let`, a `match` or a block is not itself the value: the
    /// tail of each branch or arm is. The warnings of one statement share one fix, which drops
    /// all of its values: a branch with no value beside one with a value is an error.
    pub(super) fn discard(&mut self, source: &ast::Expr, checked: &tir::Expr) {
        let mut unused = Vec::new();
        self.collect(source, checked, false, &mut unused);
        self.report_unused(unused);
    }

    /// Each value under `source` that `discard` warns about, pushed to `unused` in source order.
    /// Only the roots, `discard` and `discard_tail`, report, so an inner `if` joins its statement's
    /// fix.
    fn collect(
        &mut self,
        source: &ast::Expr,
        checked: &tir::Expr,
        arm: bool,
        unused: &mut Vec<Unused>,
    ) {
        // An optional join wraps a whole branch value, an inner `if` too.
        let checked = match &checked.kind {
            ExprKind::Wrap(inner) => inner,
            _ => checked,
        };
        match (&source.kind, &checked.kind) {
            (
                ast::ExprKind::If {
                    then_block,
                    else_block,
                    ..
                },
                ExprKind::If {
                    then_block: then_checked,
                    else_block: else_checked,
                    ..
                },
            )
            | (
                ast::ExprKind::IfLet {
                    then_block,
                    else_block,
                    ..
                },
                ExprKind::IfSome {
                    then_block: then_checked,
                    else_block: else_checked,
                    ..
                },
            ) => {
                self.collect_tail(then_block, then_checked, unused);
                if let (Some(source), Some(checked)) = (else_block, else_checked) {
                    self.collect_tail(source, checked, unused);
                }
            }
            (
                ast::ExprKind::IfLet {
                    then_block,
                    else_block,
                    ..
                },
                ExprKind::Match { arms, .. },
            ) => {
                self.collect_tail(then_block, &arms[0].body, unused);
                if let Some(source) = else_block {
                    self.collect_tail(source, &arms[1].body, unused);
                }
            }
            (ast::ExprKind::Match { arms: sources, .. }, ExprKind::Match { arms, .. }) => {
                for (source, arm) in sources.iter().zip(arms) {
                    if let ast::ExprKind::Block(source) = &source.body.kind {
                        self.collect_tail(source, &arm.body, unused);
                    } else {
                        let value = arm.body.value.as_ref();
                        let value = value.expect("an expression arm should keep its value");
                        self.collect(&source.body, value, true, unused);
                    }
                }
            }
            (ast::ExprKind::Block(source), ExprKind::Block(checked)) => {
                self.collect_tail(source, checked, unused);
            }
            // Its value comes from `break`s anywhere in its body, so the warning points at the
            // loop, from its loop label through `loop`.
            (ast::ExprKind::Loop { label, keyword, .. }, _) => {
                if !matches!(checked.ty, Type::UNIT | Type::NEVER) {
                    unused.push(Unused {
                        ty: self.type_name(checked.ty),
                        at: label.as_ref().map_or(*keyword, |l| l.span).to(*keyword),
                        site: source.span,
                        arm,
                    });
                }
            }
            _ => {
                debug_assert!(
                    !matches!(
                        source.kind,
                        ast::ExprKind::If { .. }
                            | ast::ExprKind::IfLet { .. }
                            | ast::ExprKind::Match { .. }
                    ),
                    "the checker should lower an `if` to `If`, an `if let` to `IfSome` or `Match`, and a `match` to `Match`"
                );
                // `dbg` is meant to be run, so its value may go unused.
                let dbg = matches!(checked.kind, ExprKind::Dbg { .. });
                if !dbg && !matches!(checked.ty, Type::UNIT | Type::NEVER) {
                    unused.push(Unused {
                        ty: self.type_name(checked.ty),
                        at: source.span,
                        site: source.span,
                        arm,
                    });
                }
            }
        }
    }

    /// `discard` of the value of `block`, whose checked form is `checked`, if it has one.
    pub(super) fn discard_tail(&mut self, block: &ast::Block, checked: &tir::Block) {
        let mut unused = Vec::new();
        self.collect_tail(block, checked, &mut unused);
        self.report_unused(unused);
    }

    /// `collect` of the value of `block`, whose checked form is `checked`, if it has one.
    fn collect_tail(&mut self, block: &ast::Block, checked: &tir::Block, unused: &mut Vec<Unused>) {
        if let (Some(source), Some(value)) = (tail(block), &checked.value) {
            self.collect(source, value, false, unused);
        }
    }

    /// Reports each of `unused`, one statement's values, with the fix that drops them all, unless
    /// there are more than `SHARED_FIX_SITES`: each fix lists every value, so N values cost N^2
    /// edits.
    fn report_unused(&mut self, unused: Vec<Unused>) {
        let sites: Vec<_> = if unused.len() <= SHARED_FIX_SITES {
            unused.iter().map(|u| (u.site, u.arm)).collect()
        } else {
            Vec::new()
        };
        for Unused { ty, at, arm, .. } in unused {
            self.warn(DiagnosticKind::UnusedValue {
                ty,
                at,
                arm,
                sites: sites.clone(),
            });
        }
    }

    /// The statement, or its type when it failed. `expected` is for an expression statement.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per statement kind, so the dispatch reads in one place"
    )]
    fn stmt(&mut self, stmt: &ast::Stmt, expected: Expectation) -> Result<tir::Stmt, Poisoned> {
        let unit = |failed: Poisoned| Poisoned::typed(Type::UNIT, failed.reported);
        match &stmt.kind {
            StmtKind::Let {
                mutable,
                name,
                ty,
                init,
            } => {
                let uses = self.aliases.uses;
                let declared = ty.as_ref().map(|ty| self.resolve(ty));
                let shown = ty.as_ref().and_then(|ty| self.shown_text(ty, uses));
                let init_expr = init;
                self.body.let_init = Some((init.span, name.name, mutable.is_some()));
                self.body.failed_let = matches!(declared, Some(None)).then_some(init.span);
                let init = match declared {
                    Some(declared) => {
                        if let (Some(want), Some(text)) = (declared, &shown) {
                            self.show(init.span, want, text);
                        }
                        self.expect(init, Expectation::or_poisoned(declared))
                    }
                    None => self.value(init, Expectation::None),
                };
                let uninferred = self.uninferred_of(init_expr);
                self.body.let_init = None;
                self.body.failed_let = None;
                // `let _ = e` binds nothing: the value is an expression statement, which
                // codegen releases at once.
                if name.name.as_str() == "_" {
                    return init.map(tir::Stmt::Expr).map_err(unit);
                }
                // A declared type that does not resolve (`Error`) binds no type. A declared
                // type binds its type also when the value failed, as for a `const`. So each
                // use of the name is still checked. Otherwise the name gets the type of the
                // initializer. A failed value keeps that type when it is still known (an `if`
                // with one failed branch).
                let ty = match (declared, &init) {
                    (Some(None), _) => None,
                    (Some(Some(declared)), Err(_)) => Some(declared),
                    _ => type_of(&init),
                };
                let kind = if mutable.is_some() {
                    BindingKind::Var
                } else {
                    BindingKind::Let
                };
                let local = self.declare(name, ty, kind, *mutable);
                if let Some(binding) = self
                    .body
                    .scopes
                    .last_mut()
                    .and_then(|s| s.get_mut(&name.name))
                {
                    binding.shown = shown;
                }
                if let (None, Some(uninferred)) = (ty, uninferred) {
                    self.body.uninferred.insert(local, uninferred);
                }
                Ok(tir::Stmt::Let {
                    local,
                    init: init.map_err(unit)?,
                })
            }
            StmtKind::LetElse {
                mutable,
                pattern,
                value,
                else_block,
            } => self.let_else(*mutable, pattern, value, else_block.as_ref()),
            StmtKind::While {
                label,
                keyword,
                cond: cond_ast,
                body,
            } => {
                // Only the literal: the AST has no parenthesis node, so `(true)` is it too.
                let endless = matches!(cond_ast.kind, ast::ExprKind::Bool(true));
                // W0011 reads what changed from here to the end of the body.
                let before = (self.diagnostics.errors(), self.body.writes, self.body.exits);
                let nans = self.body.nans;
                let cond = self.in_header(*keyword, label.clone(), |c| {
                    c.expect(cond_ast, Expectation::Required(Type::BOOL))
                });
                // W0005 reports a comparison with `nan` in it, and its fix changes the condition.
                let nan = self.body.nans != nans;
                let frame =
                    LoopFrame::new(LoopKind::While, *keyword, label.clone(), Expectation::None);
                let (checked, frame) = self.loop_body(frame, |c| c.block(body, Expectation::None));
                let broken = frame.broken;
                if endless {
                    // Writing `loop` over a failed loop (a failed jump, label or `break` value in
                    // it) could remove its error rather than fix it, and a safe fix may remove
                    // only its own diagnostic.
                    self.warn(DiagnosticKind::WhileTrue {
                        at: keyword.to(cond_ast.span),
                        fix: frame.failed.is_none(),
                    });
                }
                if let Ok(checked) = &checked {
                    self.discard_tail(body, checked);
                }
                let (cond, body) = match (cond, checked) {
                    (Ok(cond), Ok(body)) => (cond, body),
                    (cond, body) => {
                        // A failed loop (a failed jump, label or `break` value in it) has no
                        // type, so nothing after it reports.
                        if let Some(failed) = frame.failed {
                            return Err(Poisoned::untyped(failed));
                        }
                        let part = (cond.err().or_else(|| body.err()))
                            .expect("a `while` that fails should have a failed part");
                        let ty = if endless && !broken {
                            Type::NEVER
                        } else {
                            Type::UNIT
                        };
                        return Err(Poisoned::typed(ty, part.reported));
                    }
                };
                if endless {
                    // A bare `Loop`: `else Break` would give MIR an exit the checker denies.
                    let ty = if broken { Type::UNIT } else { Type::NEVER };
                    return Ok(tir::Stmt::Expr(tir::Expr::new(
                        ExprKind::Loop(body),
                        ty,
                        stmt.span,
                    )));
                }
                // The checker does not see a parser error that the parser kept the loop with.
                if !nan && !starts_in(&self.parse_errors, cond_ast.span.to(stmt.span)) {
                    self.unchanged_condition(cond_ast.span, &cond, before);
                }
                Ok(tir::Stmt::Expr(lower_loop(
                    body,
                    stmt.span,
                    |then_block, exit| ExprKind::If {
                        cond: Box::new(cond),
                        then_block,
                        else_block: Some(exit),
                    },
                )))
            }
            StmtKind::WhileLet {
                label,
                keyword,
                pattern,
                value,
                body,
            } => {
                let (scrutinee, form, _) = self.in_header(*keyword, label.clone(), |c| {
                    c.let_value(Unwrap::WhileLet, pattern, value)
                });
                let frame = LoopFrame::new(
                    LoopKind::WhileLet,
                    *keyword,
                    label.clone(),
                    Expectation::None,
                );
                let ((bound, checked), _) = self.loop_body(frame, |c| {
                    c.let_block(Unwrap::WhileLet, &form, pattern, body, Expectation::None)
                });
                if let Ok(checked) = &checked {
                    self.discard_tail(body, checked);
                }
                let (scrutinee, body) = match (scrutinee, checked) {
                    (Ok(scrutinee), Ok(body)) => (scrutinee, body),
                    (Err(failed), _) | (_, Err(failed)) => return Err(unit(failed)),
                };
                let pattern = match bound {
                    LetPattern::Some { bind } => {
                        return Ok(tir::Stmt::Expr(lower_loop(
                            body,
                            stmt.span,
                            |then_block, exit| ExprKind::IfSome {
                                bind,
                                scrutinee: Box::new(scrutinee),
                                then_block,
                                else_block: Some(exit),
                            },
                        )));
                    }
                    LetPattern::Match {
                        pattern: Ok(pattern),
                    } => pattern,
                    // The scrutinee did not fail, so the pattern failed by its own error.
                    LetPattern::Match { pattern: Err(own) } => {
                        let own = own.expect("a pattern at a typed position should fail by itself");
                        return Err(Poisoned::typed(Type::UNIT, own));
                    }
                };
                Ok(tir::Stmt::Expr(lower_loop(
                    body,
                    stmt.span,
                    |then_block, exit| {
                        self.if_let_match(scrutinee, pattern, then_block, Some(exit))
                    },
                )))
            }
            StmtKind::For {
                label,
                keyword,
                name,
                source,
                body,
            } => self.for_loop(stmt.span, (*keyword, label.as_ref()), name, source, body),
            StmtKind::Assign { target, value } => self.assign(stmt.span, target, value),
            StmtKind::CompoundAssign {
                op,
                op_span,
                target,
                value,
            } => self.compound_assign(stmt.span, (*op, *op_span), target, value),
            StmtKind::Expr(e) if e.is_jump() => self.jump_expr(e).map(tir::Stmt::Expr),
            StmtKind::Expr(e) => self.expr(e, expected).map(tir::Stmt::Expr),
        }
    }

    /// `let pattern = value else { else_block }`. The value and the pattern go through the code
    /// of `if let`. The block is checked in its own scope, before the pattern binds anything, so
    /// it cannot read the names, and it must not finish. The names go in the enclosing scope.
    /// A failed block or value fails the statement, but the names are bound all the same, so
    /// the lines after it report nothing of them. Without an `else_block`, `let_without_else`
    /// checks the statement.
    fn let_else(
        &mut self,
        mutable: Option<Span>,
        pattern: &ast::Pattern,
        value: &ast::Expr,
        else_block: Option<&ast::Block>,
    ) -> Result<tir::Stmt, Poisoned> {
        let unit = |reported| Poisoned::typed(Type::UNIT, reported);
        let Some(else_block) = else_block else {
            return self.let_without_else(pattern, value);
        };
        let word = Unwrap::LetElse;
        let (scrutinee, form, _) = self.let_value(word, pattern, value);
        let before = self.diagnostics.len();
        let block = self.block(else_block, Expectation::None);
        if let Ok(block) = &block
            && block.ty != Type::NEVER
        {
            let close = else_block.span.end();
            self.error(DiagnosticKind::LetElseCanFinish {
                at: Span::new(close - 1, close),
            });
        }
        let removal = Span::new(value.span.end(), else_block.span.end());
        // A `;` anywhere in the removed text, also one in a `';'` literal, makes the fix unsafe.
        let safe = mutable.is_none()
            && is_plain_exit(else_block)
            && self.diagnostics.len() == before
            && !self.comment_in(&[removal.range()])
            && !self.source.text(removal).contains(';');
        let (bound, _) = self.bind_pattern_as(
            Some(word),
            mutable,
            Some((removal, safe)),
            &form,
            pattern,
            None,
        );
        if let (
            Some(keyword),
            LetPattern::Match {
                pattern: Ok((_, bindings)),
            },
        ) = (mutable, &bound)
            && bindings.is_empty()
        {
            self.error(DiagnosticKind::Expected {
                what: "a pattern".to_string(),
                found: "`mut`".to_string(),
                at: keyword,
            });
        }
        let (scrutinee, else_block) = match (scrutinee, block) {
            (Ok(scrutinee), Ok(else_block)) => (scrutinee, else_block),
            (Err(failed), _) | (_, Err(failed)) => return Err(unit(failed.reported)),
        };
        let test = match bound {
            LetPattern::Some { bind } => tir::LetElseTest::Some { bind },
            LetPattern::Match {
                pattern: Ok((pattern, bindings)),
            } => tir::LetElseTest::Match {
                temp: self.fresh(scrutinee.ty),
                pattern,
                bindings,
            },
            // The scrutinee did not fail, so the pattern failed by its own error.
            LetPattern::Match { pattern: Err(own) } => {
                return Err(unit(
                    own.expect("a pattern at a typed position should fail by itself"),
                ));
            }
        };
        Ok(tir::Stmt::LetElse {
            scrutinee,
            test,
            else_block,
        })
    }

    /// `let pattern = value` with a pattern that is not a name and no `else`: always an error.
    /// The value and the pattern are checked as in `let_else`. Then E0174 says that the pattern
    /// always matches (the help is `match`) or can fail (the help is an `else`). A value that is
    /// no enum or optional (E0051) and a pattern with an error of its own report that error
    /// alone. A value with no type keeps its error and adds E0174. The names stay bound.
    fn let_without_else(
        &mut self,
        pattern: &ast::Pattern,
        value: &ast::Expr,
    ) -> Result<tir::Stmt, Poisoned> {
        let (scrutinee, form, mismatched) = self.let_value(Unwrap::Let, pattern, value);
        let ty = type_of(&scrutinee);
        let (bound, _) = self.bind_pattern_as(Some(Unwrap::Let), None, None, &form, pattern, None);
        // A value with no type leaves `always` false.
        let always = match (mismatched, ty) {
            (true, _) => None,
            (false, None) => Some(false),
            (false, Some(ty)) => match &bound {
                LetPattern::Match { pattern: Err(_) } => None,
                LetPattern::Match {
                    pattern: Ok((checked, _)),
                } => Some(self.always_matches(ty, checked)),
                LetPattern::Some { .. } => Some(false),
            },
        };
        let reported = match (always, &bound) {
            (Some(always), _) => self.error(DiagnosticKind::RefutablePattern {
                at: pattern.span,
                shown: self.source.text(pattern.span).to_string(),
                always,
            }),
            // E0051 fails the value; any other pattern failed by its own error.
            (None, LetPattern::Match { pattern: Err(own) }) => (scrutinee.err())
                .map(|failed| failed.reported)
                .or(*own)
                .expect("a pattern with no error of its own should be at an `Error` position"),
            (None, _) => unreachable!("only a failed pattern leaves `always` unknown"),
        };
        Err(Poisoned::typed(Type::UNIT, reported))
    }

    /// `for`, lowered to a `Loop` as in the spec's table: over an array `s` through `s[i]`
    /// with `i < s.len()`, over a range through `i < end`, and over `start..=end` through a
    /// flag, cleared on the last value, so `end` may be `i64::MAX`. The locals are the source's,
    /// the fresh ones, then the loop variable.
    fn for_loop(
        &mut self,
        span: Span,
        (keyword, label): (Span, Option<&ast::Ident>),
        name: &ast::Ident,
        source: &ast::ForSource,
        body: &ast::Block,
    ) -> Result<tir::Stmt, Poisoned> {
        let expr = |kind, ty| tir::Expr::new(kind, ty, span);
        let (header, bound) = self.in_header(keyword, label.cloned(), |c| match source {
            ast::ForSource::Array(e) => c.for_array(span, e),
            ast::ForSource::Range(range) => c.for_range(span, range),
        });
        let frame = LoopFrame::new(LoopKind::For, keyword, label.cloned(), Expectation::None);
        let ((x, checked), _) = self.loop_body(frame, |c| {
            c.unwrapping_block("for", name, bound, body, Expectation::None)
        });
        if let Ok(checked) = &checked {
            self.discard_tail(body, checked);
        }
        let (header, body) = match (header, checked) {
            (Ok(header), Ok(body)) => (header, body),
            (Err(reported), _) | (_, Err(Poisoned { reported, .. })) => {
                return Err(Poisoned::typed(Type::UNIT, reported));
            }
        };
        // The body keeps its own type, so codegen releases a counted tail value.
        let body_ty = body.ty;
        let ForHeader {
            mut stmts,
            counter,
            limit,
            go,
            item,
        } = header;
        let at = |local_id| local(local_id, Type::I64, span);
        // The step runs only while the counter is below `limit`, or not yet at it over `..=`,
        // so it cannot overflow and needs no check, which would keep `-O2` from folding a loop.
        let next = ExprKind::Binary {
            op: tir::BinaryOp::AddUnchecked,
            lhs: Box::new(at(counter)),
            rhs: Box::new(expr(ExprKind::Int(1), Type::I64)),
        };
        let step = tir::Stmt::Assign {
            place: tir::Place::Local(counter),
            value: expr(next, Type::I64),
        };
        let block = |stmts| tir::Block {
            stmts,
            value: None,
            ty: Type::UNIT,
        };
        // The counter moves on before the body runs, so `continue` needs nothing more. Over
        // `..=`, it stays at the last value, which `i64::MAX + 1` would overflow.
        let (moves, cond) = if let Some(go) = go {
            let last = ExprKind::Equal {
                negated: true,
                lhs: Box::new(at(counter)),
                rhs: Box::new(at(limit)),
            };
            let flag = || local(go, Type::BOOL, span);
            let moves = vec![
                tir::Stmt::Assign {
                    place: tir::Place::Local(go),
                    value: expr(last, Type::BOOL),
                },
                tir::Stmt::Expr(expr(
                    ExprKind::If {
                        cond: Box::new(flag()),
                        then_block: block(vec![step]),
                        else_block: None,
                    },
                    Type::UNIT,
                )),
            ];
            (moves, flag().kind)
        } else {
            let cond = ExprKind::Compare {
                operands: vec![at(counter), at(limit)],
                ops: vec![tir::CompareOp::Lt],
            };
            (vec![step], cond)
        };
        let mut then_stmts = vec![tir::Stmt::Let {
            local: x,
            init: item,
        }];
        then_stmts.extend(moves);
        then_stmts.push(tir::Stmt::Expr(expr(ExprKind::Block(body), body_ty)));
        let then_block = block(then_stmts);
        stmts.push(tir::Stmt::Expr(lower_loop(
            then_block,
            span,
            |then_block, exit| ExprKind::If {
                cond: Box::new(expr(cond, Type::BOOL)),
                then_block,
                else_block: Some(exit),
            },
        )));
        Ok(tir::Stmt::Expr(block_expr(stmts, None, Type::UNIT, span)))
    }

    /// The header of `for x in e`, and the type of `x`: the element type, `None` (`Error`) when
    /// `e` is no array.
    fn for_array(
        &mut self,
        span: Span,
        e: &ast::Expr,
    ) -> (Result<ForHeader, Reported>, Option<Type>) {
        // The subject is indexed in the loop: a `?.` fix of an E0062 in it leads to a `for` on an
        // optional, as an index does.
        let outer = std::mem::replace(&mut self.body.indexed, true);
        let checked = self.value(e, Expectation::None);
        self.body.indexed = outer;
        let (element, d) = match type_of(&checked).map(|ty| (ty, ty.kind())) {
            Some((_, TypeKind::Array(element))) => (Some(element), None),
            Some((_, TypeKind::Optional(payload)))
                if matches!(payload.kind(), TypeKind::Array(_)) =>
            {
                (None, Some(optional_array(e, UnwrapHelp::It)))
            }
            Some((ty, _)) => {
                let found = self.type_name(ty);
                let d = DiagnosticKind::ForNeedsArray { found, at: e.span };
                (None, Some(d.into()))
            }
            None => (None, None),
        };
        let reported = d.map(|d| self.diagnostics.error(d));
        let ty = type_of(&checked).unwrap_or(Type::UNIT);
        // A place that cannot change during the loop is read in place, not copied into a local
        // that parameter inference would take for a consume.
        let fixed = checked.as_ref().is_ok_and(|e| self.fixed_place(e));
        let array = (!fixed).then(|| self.fresh(ty));
        let len = self.fresh(Type::I64);
        let counter = self.fresh_var(Type::I64);
        let (checked, element) = match (checked, element) {
            (Ok(checked), Some(element)) => (checked, element),
            (checked, element) => {
                let failed = (checked.err().map(|failed| failed.reported))
                    .or(reported)
                    .expect("a `for` over no array should report it or fail its subject");
                return (Err(failed), element);
            }
        };
        let expr = |kind, ty| tir::Expr::new(kind, ty, span);
        let read = || match array {
            Some(array) => local(array, ty, e.span),
            None => reread(&checked),
        };
        let mut stmts = Vec::new();
        let (len_init, item_base) = (read(), read());
        if let Some(array) = array {
            stmts.push(tir::Stmt::Let {
                local: array,
                init: checked,
            });
        }
        stmts.push(tir::Stmt::Let {
            local: len,
            init: expr(ExprKind::Len(Box::new(len_init)), Type::I64),
        });
        stmts.push(tir::Stmt::Let {
            local: counter,
            init: expr(ExprKind::Int(0), Type::I64),
        });
        let item = ExprKind::Index {
            base: Box::new(item_base),
            index: Box::new(local(counter, Type::I64, span)),
        };
        let header = ForHeader {
            stmts,
            counter,
            limit: len,
            go: None,
            item: expr(item, element),
        };
        (Ok(header), Some(element))
    }

    /// Whether `e` names a value that no code can change while a `for` over it runs: a local
    /// without `mut`, or a field without `mut` of such a place (W0002 and ADR 0011).
    fn fixed_place(&self, e: &tir::Expr) -> bool {
        match &e.kind {
            ExprKind::Local(local) => !self.body.locals[local.0].mutable,
            ExprKind::Field { base, index } => {
                matches!(base.ty.kind(), TypeKind::Class(class, _)
                    if !self.classes[class.0].fields[*index].mutable)
                    && self.fixed_place(base)
            }
            _ => false,
        }
    }

    /// The header of `for x in start..end`, or `start..=end` when `inclusive`, and the type of
    /// `x`, `i64`.
    fn for_range(
        &mut self,
        span: Span,
        range: &ast::Range,
    ) -> (Result<ForHeader, Reported>, Option<Type>) {
        let inclusive = range.inclusive;
        let (Some(start), Some(end)) = (&range.start, &range.end) else {
            // A bound is missing: one report, and the bound that is there checks itself.
            let d = Self::range_outside(range, RangePlace::ForSource);
            let reported = self.diagnostics.error(d);
            for bound in [&range.start, &range.end].into_iter().flatten() {
                drop(self.expect(bound, Expectation::Required(Type::I64)));
            }
            return (Err(reported), Some(Type::I64));
        };
        let start = self.expect(start, Expectation::Required(Type::I64));
        let end = self.expect(end, Expectation::Required(Type::I64));
        let counter = self.fresh_var(Type::I64);
        let limit = self.fresh(Type::I64);
        let go = inclusive.then(|| self.fresh_var(Type::BOOL));
        let (start, end) = match (start, end) {
            (Ok(start), Ok(end)) => (start, end),
            (Err(failed), _) | (_, Err(failed)) => return (Err(failed.reported), Some(Type::I64)),
        };
        let mut stmts = vec![
            tir::Stmt::Let {
                local: counter,
                init: start,
            },
            tir::Stmt::Let {
                local: limit,
                init: end,
            },
        ];
        if let Some(go) = go {
            let at = |local_id| local(local_id, Type::I64, span);
            let some = ExprKind::Compare {
                operands: vec![at(counter), at(limit)],
                ops: vec![tir::CompareOp::Le],
            };
            stmts.push(tir::Stmt::Let {
                local: go,
                init: tir::Expr::new(some, Type::BOOL, span),
            });
        }
        let item = local(counter, Type::I64, span);
        let header = ForHeader {
            stmts,
            counter,
            limit,
            go,
            item,
        };
        (Ok(header), Some(Type::I64))
    }

    /// `return`, of type `never` even when it fails.
    pub(super) fn return_expr(&mut self, span: Span, value: Option<&ast::Expr>) -> Checked {
        self.body.exits += 1;
        let never = |reported| Poisoned::typed(Type::NEVER, reported);
        let value = match value {
            None => match self.body.ret {
                Some(ret) if ret != Type::UNIT => {
                    let want = self.type_name(ret);
                    let reported = self.error(DiagnosticKind::ReturnNeedsValue { want, at: span });
                    return Err(never(reported));
                }
                _ => None,
            },
            Some(value) if self.body.ret == Some(Type::UNIT) => {
                let checked = self.value(value, Expectation::None);
                let reported = match known_type(&checked) {
                    Ok(_) => self.error(DiagnosticKind::UnexpectedReturnValue { at: value.span }),
                    Err(reported) => reported,
                };
                return Err(never(reported));
            }
            Some(value) => {
                if let (Some(want), Some(text)) = (self.body.ret, self.body.ret_shown.clone()) {
                    self.show(value.span, want, &text);
                }
                let value = self
                    .expect(value, Expectation::or_poisoned(self.body.ret))
                    .map_err(|failed| never(failed.reported))?;
                Some(Box::new(value))
            }
        };
        Ok(tir::Expr::new(ExprKind::Return(value), Type::NEVER, span))
    }

    /// `f`, inside the header of the loop whose keyword is `keyword` and loop label `label`.
    fn in_header<T>(
        &mut self,
        keyword: Span,
        label: Option<ast::Ident>,
        f: impl FnOnce(&mut Self) -> T,
    ) -> T {
        self.body.loops.push(LoopContext::Header { keyword, label });
        let checked = f(self);
        self.body.loops.pop();
        checked
    }

    /// `f`, inside the body of `frame`'s loop, and the frame as the `break`s left it. The loop's
    /// loop label opens for the body even when an enclosing loop has it already, which is an
    /// error, and is reported when no jump names it.
    fn loop_body<T>(&mut self, frame: LoopFrame, f: impl FnOnce(&mut Self) -> T) -> (T, LoopFrame) {
        let outer = frame.label.as_ref().and_then(|l| self.open_label(l.name));
        let mut frame = frame;
        if let (Some(label), Some((at, outer))) = (&frame.label, outer) {
            // Deleting either loop label would fix the error, so neither warns as unused, and
            // neither loop gets a type: their jumps may have meant the other one.
            self.body.failed_labels.extend([outer, label.span]);
            let reported = self.error(DiagnosticKind::LabelReused {
                name: label.name.to_string(),
                at: label.span,
                outer,
            });
            self.fail_loops(at..=at, reported);
            frame.failed = Some(reported);
        }
        self.body.loops.push(LoopContext::Body(frame));
        let checked = f(self);
        let Some(LoopContext::Body(frame)) = self.body.loops.pop() else {
            unreachable!("a loop's body should pop the frame it pushed");
        };
        if let Some(label) = &frame.label
            && !frame.label_used
            && !self.body.failed_labels.contains(&label.span)
        {
            self.warn(DiagnosticKind::UnusedLabel {
                name: label.name.to_string(),
                at: label.span,
                remove: Span::new(
                    label.span.start(),
                    removal_end(
                        self.source.comments,
                        label.span.end(),
                        frame.keyword.start(),
                    ),
                ),
            });
        }
        (checked, frame)
    }

    /// The loop labels open around the code being checked, innermost last.
    pub(super) fn open_labels(&self) -> impl DoubleEndedIterator<Item = &ast::Ident> {
        (0..self.body.loops.len()).filter_map(|at| self.open_labels_at(at))
    }

    /// The loop label of the loop context at `at` in `body.loops`, if it has one.
    fn open_labels_at(&self, at: usize) -> Option<&ast::Ident> {
        match &self.body.loops[at] {
            LoopContext::Body(frame) => frame.label.as_ref(),
            LoopContext::Header { label, .. } => label.as_ref(),
        }
    }

    /// Where in `body.loops` the innermost open loop label `name` is, and its span.
    fn open_label(&self, name: Symbol) -> Option<(usize, Span)> {
        (0..self.body.loops.len()).rev().find_map(|at| {
            let label = self.open_labels_at(at)?;
            (label.name == name).then_some((at, label.span))
        })
    }

    /// Fails the loop bodies in `range` of `body.loops` by the error `reported`: they get no
    /// type, so the code after them reports nothing more.
    fn fail_loops(
        &mut self,
        range: impl SliceIndex<[LoopContext], Output = [LoopContext]>,
        reported: Reported,
    ) {
        for context in &mut self.body.loops[range] {
            if let LoopContext::Body(frame) = context {
                frame.failed.get_or_insert(reported);
            }
        }
    }

    /// Records that a failed jump named the innermost open loop label `name`, so that it is not
    /// reported unused, and where in `body.loops` that loop label is.
    fn named_by_failed_jump(&mut self, name: Symbol) -> usize {
        let (at, span) = self
            .open_label(name)
            .expect("a failed jump should name an open loop label");
        self.body.failed_labels.insert(span);
        at
    }

    /// Records that the `break` with the misspelled loop label `name` meant the open loop label
    /// `suggested`, by the error `reported`: that loop and the loops inside it get no type, and
    /// `suggested` is not reported unused.
    pub(super) fn break_meant(&mut self, suggested: &str, reported: Reported) {
        let at = self.named_by_failed_jump(Symbol::intern(suggested));
        self.fail_loops(at.., reported);
    }

    /// Skips a `break` value that is not checked, by the error `reported`: a jump in it may name
    /// any open loop, so every open loop gets no type and no open loop label is reported unused.
    fn skip_value(&mut self, reported: Reported) {
        let labels: Vec<Span> = self.open_labels().map(|l| l.span).collect();
        self.body.failed_labels.extend(labels);
        self.fail_loops(.., reported);
    }

    /// "no loop label `name`" for `name` after `keyword`, offering the closest open loop label,
    /// which counts as named. `break_start` is the start of the `break`, for the parentheses that
    /// its fix may need (see `wrap_break`); a `continue` takes no value, so it has none.
    fn missing_label(
        &mut self,
        name: &ast::Ident,
        keyword: &'static str,
        break_start: Option<usize>,
    ) -> Reported {
        let suggestion = closest(
            name.name.as_str(),
            self.open_labels().map(|l| l.name.as_str()),
        )
        .map(str::to_string);
        let wrap = break_start.and_then(|start| {
            suggestion
                .as_ref()
                .and_then(|_| self.wrap_break(name.span, start))
        });
        let reported = self.error(DiagnosticKind::MissingLabel {
            name: name.name.to_string(),
            keyword,
            at: name.span,
            suggestion: suggestion.clone(),
            wrap,
        });
        if let Some(label) = &suggestion {
            if keyword == "break" {
                self.break_meant(label, reported);
            } else {
                self.named_by_failed_jump(Symbol::intern(label));
            }
        }
        reported
    }

    /// What `name` is when a value or a type of that spelling is in scope: a binding, a
    /// function (the prelude's included), a constant, a class, an enum, an interface or a
    /// built-in type, and where it is declared, when that is in the source.
    fn in_scope(&self, name: Symbol) -> Option<(ClashKind, Option<Span>)> {
        // A broken item's declaration is not known.
        fn at<T>(declared: Declared<T>, span: impl FnOnce(T) -> Span) -> Option<Span> {
            match declared {
                Declared::Known(item) => Some(span(item)),
                Declared::Poisoned(_) => None,
            }
        }
        if let Some(binding) = self.body.scopes.iter().rev().find_map(|s| s.get(&name)) {
            // A local that hides a poisoned name is no binding, and the poisoned item gives no
            // clash.
            return (!binding.hides_name).then_some((ClashKind::Binding, Some(binding.span)));
        }
        if let Some(id) = self.names.function(name) {
            let span = at(id, |id| self.signatures[id.0].span);
            return Some((ClashKind::Function, span));
        }
        if PRELUDE.contains(&name.as_str()) {
            return Some((ClashKind::Function, None));
        }
        if let Some(constant) = self.names.constant(name) {
            return Some((
                ClashKind::Constant,
                at(constant, |id| self.const_infos[id.0].span),
            ));
        }
        if let Some(id) = self.names.class(name) {
            let span = at(id, |id| self.classes[id.0].name.span);
            return Some((ClashKind::Class, span));
        }
        if let Some(id) = self.names.enum_id(name) {
            let span = at(id, |id| self.enums[id.0].name.span);
            return Some((ClashKind::Enum, span));
        }
        if let Some(id) = self.names.interface(name) {
            let span = at(id, |id| self.interfaces[id.0].name.span);
            return Some((ClashKind::Interface, span));
        }
        if let Some(id) = self.names.alias(name) {
            let span = at(id, |id| self.aliases.infos[id].name.span);
            return Some((ClashKind::Alias, span));
        }
        built_in_type(name.as_str()).map(|_| (ClashKind::BuiltInType, None))
    }

    /// `break` or `continue`, of type `never` even outside a loop, and `break`'s `value`. In a
    /// loop header it is `Error`, so the condition or source around it reports nothing more.
    pub(super) fn jump(
        &mut self,
        span: Span,
        keyword: &'static str,
        label: Option<&ast::Ident>,
        value: Option<&ast::Expr>,
    ) -> Checked {
        self.body.exits += 1;
        let (target, depth) = self.target(span, keyword, label)?;
        if let Some(label) = label
            && let Some((kind, declared)) = self.in_scope(label.name)
        {
            let at = self.frame(target).label.as_ref().map(|l| l.span);
            let reported = self.error(DiagnosticKind::LabelClash {
                name: label.name.to_string(),
                at: label.span,
                declared: at.expect("a jump's loop label should be its target's"),
                form: ClashForm::Name { kind, declared },
            });
            // The loop label may have meant the name, so the loop gets no type, and the value
            // is not checked.
            self.frame(target).failed.get_or_insert(reported);
            if value.is_some() {
                self.skip_value(reported);
            }
            return Err(Poisoned::untyped(reported));
        }
        // Only a bare name right after an unlabeled `break` may be a misspelled loop label; a
        // parenthesized name spans its parentheses.
        let typo = value.and_then(|v| match v.kind {
            ast::ExprKind::Name(name)
                if label.is_none() && v.span.range().len() == name.as_str().len() =>
            {
                Some(ast::Ident { name, span: v.span })
            }
            _ => None,
        });
        let value = value
            .map(|v| {
                let start = label.is_none().then(|| v.span.start());
                let outer = std::mem::replace(&mut self.body.break_start, start);
                let checked = self.break_value(span, target, v, typo.as_ref());
                self.body.break_start = outer;
                checked.map(Box::new)
            })
            .transpose()?;
        let frame = self.frame(target);
        if value.is_none() && keyword == "break" && frame.kind == LoopKind::Loop {
            frame.bare.get_or_insert(span);
        }
        let kind = if keyword == "break" {
            ExprKind::Break { depth, value }
        } else {
            ExprKind::Continue { depth }
        };
        Ok(tir::Expr::new(kind, Type::NEVER, span))
    }

    /// The loop the jump at `span` acts on, the one with its loop `label` or else the innermost:
    /// its index in `body.loops`, and how many loop bodies lie between them. A `break` marks it
    /// broken, and a label marks it used. A loop header on the way, the target's own included,
    /// would be left, so the jump is E0082 there.
    fn target(
        &mut self,
        span: Span,
        keyword: &'static str,
        label: Option<&ast::Ident>,
    ) -> Result<(usize, usize), Poisoned> {
        let named = |own: Option<Symbol>| label.is_none_or(|label| own == Some(label.name));
        let mut header = None;
        let mut found = None;
        let mut depth = 0;
        for at in (0..self.body.loops.len()).rev() {
            match &mut self.body.loops[at] {
                LoopContext::Header {
                    keyword: loop_at,
                    label: own,
                } => {
                    header.get_or_insert(*loop_at);
                    // Only a loop label reaches the loop whose header holds the jump.
                    if label.is_some_and(|label| own.as_ref().is_some_and(|l| l.name == label.name))
                    {
                        found = Some(at);
                        break;
                    }
                }
                LoopContext::Body(frame) if named(frame.label.as_ref().map(|l| l.name)) => {
                    if header.is_some() {
                        found = Some(at);
                        break;
                    }
                    frame.broken |= keyword == "break";
                    frame.crossed |= keyword == "break" && depth > 0;
                    frame.label_used |= label.is_some();
                    return Ok((at, depth));
                }
                LoopContext::Body(_) => depth += 1,
            }
        }
        match header {
            // Without a loop label, a header with no loop body around it is the target's too.
            Some(loop_at) if found.is_some() || label.is_none() => {
                // The loops the jump would leave, its target included, get no type, so neither
                // the header nor the code after the target reports more.
                if let Some(label) = label {
                    self.named_by_failed_jump(label.name);
                }
                let reported = self.error(DiagnosticKind::JumpInLoopHeader {
                    keyword,
                    at: span,
                    loop_at,
                });
                self.fail_loops(found.unwrap_or(0).., reported);
                Err(Poisoned::untyped(reported))
            }
            // Only `continue` takes a name that is no open loop label; outside a loop, the
            // jump is the error.
            _ => match label {
                Some(label) if !self.body.loops.is_empty() => Err(Poisoned::untyped(
                    self.missing_label(label, "continue", None),
                )),
                _ => {
                    let reported =
                        self.error(DiagnosticKind::JumpOutsideLoop { keyword, at: span });
                    self.body.invalid_jump = true;
                    Err(Poisoned {
                        jumped: true,
                        ..Poisoned::typed(Type::NEVER, reported)
                    })
                }
            },
        }
    }

    /// `loop { body }` at `span`, of the type its `break`s give it.
    pub(super) fn loop_expr(
        &mut self,
        span: Span,
        (keyword, label): (Span, Option<&ast::Ident>),
        body: &ast::Block,
        expected: Expectation,
    ) -> Checked {
        let frame = LoopFrame::new(LoopKind::Loop, keyword, label.cloned(), expected);
        let (mut checked, mut frame) = self.loop_body(frame, |c| c.block(body, Expectation::None));
        self.join_deferred(&mut frame);
        if let Ok(checked) = &mut checked {
            self.discard_tail(body, checked);
            if let (true, Some((ty, _))) = (frame.widened, frame.first) {
                checked.each_expr_mut(&mut |e| wrap_breaks(e, 0, ty, frame.crossed));
            }
        }
        if let (Some(at), Some(valued)) = (frame.bare, frame.valued) {
            let reported = self.error(DiagnosticKind::BareBreak { at, valued });
            frame.failed.get_or_insert(reported);
        }
        let ty = frame.loop_type();
        match (checked, ty) {
            (Ok(body), Some(ty)) => Ok(tir::Expr::new(ExprKind::Loop(body), ty, span)),
            // A loop with no type failed by its frame, or has values that a `Poisoned`
            // expectation gives no type; one with a type failed by its body.
            (Ok(_), None) => Err(Poisoned::untyped(frame.failed.unwrap_or_else(|| {
                debug_assert_eq!(
                    frame.expected,
                    Expectation::Poisoned,
                    "the loop should have failed"
                );
                self.poisoned_context(span)
            }))),
            (Err(failed), ty) => Err(Poisoned::new(ty, frame.failed.unwrap_or(failed.reported))),
        }
    }

    /// The values of objects that did not join the first value of the loop (see `join_break`),
    /// now that every `break` is known: they join it as `if` branches do, also to an `any` type
    /// that the siblings of a hinted loop give (see `seeded_join`), or one is reported.
    fn join_deferred(&mut self, frame: &mut LoopFrame) {
        let deferred = std::mem::take(&mut frame.deferred);
        let Some((first, at)) = frame.first.filter(|_| !deferred.is_empty()) else {
            return;
        };
        // The first value is member 0, and each deferred value its place after it.
        let members: Vec<(usize, Type)> = std::iter::once((0, first))
            .chain((1..).zip(deferred.iter().map(|&(_, t)| t)))
            .collect();
        let span = |i: usize| match i.checked_sub(1) {
            Some(i) => deferred[i].0,
            None => frame.first_value,
        };
        let join = match self.any_join_all(&members) {
            joined @ AnyJoin::Joined { .. } => joined,
            failed => match self.seeded_join(frame.expected.siblings(), &members) {
                seeded @ AnyJoin::Joined { .. } => {
                    #[cfg(test)]
                    super::reached("seeded_join for a loop");
                    seeded
                }
                leaves @ AnyJoin::Leaves { .. } => leaves,
                AnyJoin::NoAny | AnyJoin::Fails { .. } => failed,
            },
        };
        // A value of an object reads no conversion from its expression.
        let d = match join {
            AnyJoin::Joined { ty } => {
                // The loop grew to this type, so `wrap_breaks` converts the values.
                if ty != first {
                    frame.first = Some((ty, at));
                    frame.widened = true;
                }
                self.instantiates = true;
                return;
            }
            AnyJoin::Leaves {
                ty,
                member: (i, got),
            } => self.leaves_any(span(i), None, ty, got),
            AnyJoin::Fails {
                earlier: (_, want),
                later: (i, got),
            } => self.mismatch_from(span(i), None, want, got, Some(MismatchSource::Break(at))),
            // Objects that join to no `any` type: each is not the first value's.
            AnyJoin::NoAny => {
                for &(at_value, got) in &deferred {
                    let source = Some(MismatchSource::Break(at));
                    let d = self.mismatch_from(at_value, None, first, got, source);
                    let reported = self.diagnostics.error(d);
                    frame.failed.get_or_insert(reported);
                }
                return;
            }
        };
        let reported = self.diagnostics.error(d);
        frame.failed.get_or_insert(reported);
    }

    /// The loop body open at `at` in `self.body.loops`.
    fn frame(&mut self, at: usize) -> &mut LoopFrame {
        match &mut self.body.loops[at] {
            LoopContext::Body(frame) => frame,
            LoopContext::Header { .. } => unreachable!("a jump's target should be a loop body"),
        }
    }

    /// `v`, the value of the `break` at `span` whose target is the loop body at `target`, checked
    /// by the loop's type rule. A failed value fails the loop. `typo` is `v` when it is a bare
    /// name right after an unlabeled `break`, which may be a misspelled loop label.
    fn break_value(
        &mut self,
        span: Span,
        target: usize,
        v: &ast::Expr,
        typo: Option<&ast::Ident>,
    ) -> Checked {
        let frame = self.frame(target);
        if frame.kind != LoopKind::Loop {
            let (keyword, loop_at) = (frame.kind.keyword(), frame.keyword);
            // `break outr`, a misspelled loop label, rather than a value.
            let reported = if let Some(name) = typo
                && self.in_scope(name.name).is_none()
                && closest(
                    name.name.as_str(),
                    self.open_labels().map(|l| l.name.as_str()),
                )
                .is_some()
            {
                self.missing_label(name, "break", Some(span.start()))
            } else {
                let reported = self.error(DiagnosticKind::BreakValueNotLoop {
                    keyword,
                    at: v.span,
                    loop_at,
                });
                self.skip_value(reported);
                reported
            };
            self.frame(target).failed.get_or_insert(reported);
            return Err(Poisoned::typed(Type::NEVER, reported));
        }
        let frame = self.frame(target);
        // With no expected type, an earlier value gives a hint, as the siblings of a hinted loop
        // do.
        let (hint, first_at) = match (frame.expected, frame.first) {
            (Expectation::None, Some((ty, at))) => {
                let hint = Expectation::Hint {
                    want: ty,
                    siblings: ty,
                };
                (hint, Some(at))
            }
            (expected, _) => (expected, None),
        };
        let from = first_at.map(|first| (v.span, first));
        let outer = std::mem::replace(&mut self.body.break_hint, from);
        self.body.label_typo = typo.map(|name| (name.span, span.start()));
        let checked = self.expr(v, hint);
        self.body.label_typo = None;
        self.body.break_hint = outer;
        // A failed value fails the loop, and gives it no type and no second report. A generic
        // call's misfit is joined by the type it found, as a concrete call is by its type.
        let misfit = self.misfit_type(&checked);
        if let (Err(failed), None) = (&checked, misfit) {
            self.frame(target).failed.get_or_insert(failed.reported);
            return checked;
        }
        let checked = match type_of(&checked).or(misfit) {
            Some(Type::UNIT) => Err(Poisoned::untyped(self.error(DiagnosticKind::NoValue {
                at: v.span,
                never: false,
            }))),
            Some(Type::NEVER) | None => checked,
            Some(got) => {
                // Checking `v` can set the first type, through a `break` of this loop inside it.
                let frame = self.frame(target);
                frame.valued.get_or_insert(span);
                let (expected, first) = (frame.expected, frame.first);
                let hinted = matches!(expected, Expectation::Hint { .. });
                if expected == Expectation::Poisoned {
                    checked
                } else {
                    let want = (expected.ty(), hinted, first);
                    self.join_break((v, span), (checked, got), want, target)
                }
            }
        };
        if let Err(failed) = &checked {
            self.frame(target).failed.get_or_insert(failed.reported);
        }
        checked
    }

    /// The value `checked` of the `break` `v`, of type `got`, joined with the loop's first value.
    /// A value that fits the expected type keeps its own, as a block's value does. The values join
    /// as `if` branches do: a `T` and a `T?` make a `T?`. An object and an `any` type that it
    /// converts to make the `any` type. The values before it are converted at the loop's end when
    /// the type grows. With `hinted`, `expected` is a hint (see `Expectation::Hint`).
    fn join_break(
        &mut self,
        (v, span): (&ast::Expr, Span),
        (checked, got): (Checked, Type),
        (expected, hinted, first): (Option<Type>, bool, Option<(Type, Span)>),
        target: usize,
    ) -> Checked {
        // A value that does not fit the expected type converts to it, or is the mismatch. No
        // value converts to an `any` hint: the join of the values does.
        let (checked, got) = match expected {
            Some(ty) if !fits(ty, got) && (!hinted || !is_target(ty)) => {
                (self.against(v, checked, Some(ty), None).0, ty)
            }
            _ => (checked, got),
        };
        // A misfit left is reported against the first value, as a value of its type is.
        if let Some((first, at)) = first
            && self.misfit_type(&checked).is_some()
        {
            return (self.against(v, checked, Some(first), Some(MismatchSource::Break(at)))).0;
        }
        let value = checked?;
        let Some((first, at)) = first else {
            let frame = self.frame(target);
            frame.first = Some((got, span));
            frame.first_value = v.span;
            return Ok(value);
        };
        // Without an expected type, a `T` and a `T?` do not join.
        let joined = if first == got || expected.is_some() && fits(first, got) {
            Some(first)
        } else if expected.is_some() && fits(got, first) {
            Some(got)
        } else {
            self.any_join(first, got)
        };
        let Some(joined) = joined else {
            // Objects and `any` values: a later `any` value can join them all.
            let object = |ty: Type| matches!(ty.kind(), TypeKind::Class(..) | TypeKind::Any(..));
            let objects = object(got) && object(first);
            if objects {
                self.frame(target).deferred.push((v.span, got));
                return Ok(value);
            }
            // A `Some` of an object is the mistake at the object, as in a branch.
            if let Some((at, want, object)) = self.object_in_some(&value, first) {
                let d = self.leaves_any(at, None, want, object);
                return Err(Poisoned::untyped(self.diagnostics.error(d)));
            }
            return self
                .against(v, Ok(value), Some(first), Some(MismatchSource::Break(at)))
                .0;
        };
        let checked = if got == joined {
            Ok(value)
        } else {
            self.against(v, Ok(value), Some(joined), None).0
        };
        if joined != first {
            let frame = self.frame(target);
            frame.widened = true;
            frame.first = Some((joined, span));
            // Converting the values before makes a table, as a conversion does.
            self.instantiates |= is_target(joined);
        }
        checked
    }

    /// `block`, its scope holding `name` bound to a `ty`, as `keyword` (`if let`, `while let` or
    /// `for`) binds it. The bound local, and the block.
    /// A `for _` binds a fresh unnamed local and declares no name.
    pub(super) fn unwrapping_block(
        &mut self,
        keyword: &'static str,
        name: &ast::Ident,
        ty: Option<Type>,
        block: &ast::Block,
        expected: Expectation,
    ) -> (tir::LocalId, CheckedBlock) {
        self.body.scopes.push(FxHashMap::default());
        let bind = self.bind_unwrapped(keyword, name, ty);
        (bind, self.scoped_block(block, expected))
    }

    /// `name` bound to a `ty` in the innermost scope, as `keyword` binds it, and its local.
    /// A `_` binds a fresh unnamed local and declares no name.
    pub(super) fn bind_unwrapped(
        &mut self,
        keyword: &'static str,
        name: &ast::Ident,
        ty: Option<Type>,
    ) -> tir::LocalId {
        if name.name.as_str() == "_" {
            self.fresh(ty.unwrap_or(Type::UNIT))
        } else {
            self.declare(name, ty, BindingKind::Unwrapped(keyword), None)
        }
    }

    /// A new local with no source name, for a value the lowering binds.
    pub(super) fn fresh(&mut self, ty: Type) -> tir::LocalId {
        self.fresh_local(ty, false)
    }

    /// `fresh`, mutable: a counter the lowering steps.
    fn fresh_var(&mut self, ty: Type) -> tir::LocalId {
        self.fresh_local(ty, true)
    }

    fn fresh_local(&mut self, ty: Type, mutable: bool) -> tir::LocalId {
        let local = tir::LocalId(self.body.locals.len());
        self.body.locals.push(tir::Local {
            name: None,
            ty,
            mutable,
        });
        local
    }

    /// When an operand exits and one before it is counted, binds every operand through the
    /// last exiting one to a fresh local, in source order, so each read of them comes after
    /// every exit. The bindings, and the operands with those reads in place.
    pub(super) fn spill(&mut self, operands: Vec<tir::Expr>) -> (Vec<tir::Stmt>, Vec<tir::Expr>) {
        self.spill_before(operands, false)
    }

    /// `spill` of `operands` that an expression after them follows, which exits when
    /// `later_exits`: then each operand is bound if one of them is counted, as that expression is
    /// the last exit. The expression itself is not an operand and stays where it is.
    pub(super) fn spill_before(
        &mut self,
        operands: Vec<tir::Expr>,
        later_exits: bool,
    ) -> (Vec<tir::Stmt>, Vec<tir::Expr>) {
        let last_exit = if later_exits {
            Some(operands.len())
        } else {
            operands.iter().rposition(tir::Expr::exits)
        };
        let first_counted = operands
            .iter()
            .position(|e| e.ty.may_be_counted(&self.lowered_enums));
        let Some(last_exit) = last_exit.filter(|&exit| first_counted.is_some_and(|c| c < exit))
        else {
            return (Vec::new(), operands);
        };
        let mut spills = Vec::new();
        let operands = operands
            .into_iter()
            .enumerate()
            .map(|(i, operand)| {
                if i > last_exit {
                    return operand;
                }
                let bind = self.fresh(operand.ty);
                let read = local(bind, operand.ty, operand.span);
                spills.push(tir::Stmt::Let {
                    local: bind,
                    init: operand,
                });
                read
            })
            .collect();
        (spills, operands)
    }

    /// Adds `name` to the innermost scope as a new local; `keyword` is the `mut` of a
    /// `let mut`. It replaces a binding of that name in the scope, except that a parameter whose
    /// name another parameter has is reported and poisoned. A name that milestone 8's prelude
    /// takes is reported, and so is a user's local that an intact enum takes. A poisoned enum
    /// does not exist, so it takes nothing. The binding still takes the name. A local of the
    /// prelude's own code may have an enum's name.
    pub(super) fn declare(
        &mut self,
        name: &ast::Ident,
        ty: Option<Type>,
        kind: BindingKind,
        keyword: Option<Span>,
    ) -> tir::LocalId {
        if !self.reserved(name)
            && !name.span.in_prelude()
            && (matches!(self.names.enum_id(name.name), Some(Declared::Known(_)))
                || self.alias_unhideable(name.name))
        {
            self.error(DiagnosticKind::EnumNamedLocal {
                name: name.name.to_string(),
                at: name.span,
                optional: self.alias_names_option(name.name),
            });
        }
        self.insert_local(name, ty, kind, keyword)
    }

    /// A `mut` parameter, which is a variable in the body: declared as a parameter, so that a
    /// repeated name is reported, then made a `Var` with no keyword, so that W0002's `let mut`
    /// form never names it.
    pub(super) fn declare_mut_param(
        &mut self,
        name: &ast::Ident,
        ty: Option<Type>,
    ) -> tir::LocalId {
        let local = self.declare(name, ty, BindingKind::Param, None);
        self.body.locals[local.0].mutable = true;
        if let Some(b) = self.lookup(name.name) {
            b.kind = BindingKind::Var;
            b.mut_param = true;
        }
        local
    }

    /// `declare` without its checks of the name.
    pub(super) fn insert_local(
        &mut self,
        name: &ast::Ident,
        ty: Option<Type>,
        kind: BindingKind,
        keyword: Option<Span>,
    ) -> tir::LocalId {
        let local = tir::LocalId(self.body.locals.len());
        self.body.locals.push(tir::Local {
            name: Some(name.name.to_string()),
            // An `Error` local never reaches `tir`: the error behind it stops the program.
            ty: ty.unwrap_or(Type::UNIT),
            mutable: kind == BindingKind::Var,
        });
        let shadows_mut = self.lookup(name.name).is_some_and(|b| b.keyword.is_some());
        let scope = self
            .body
            .scopes
            .last_mut()
            .expect("a scope should be open while a block is checked");
        let mut binding = Binding {
            local,
            ty,
            kind,
            span: name.span,
            keyword,
            reassigned: false,
            shadows_mut,
            written: 0,
            loop_named: false,
            read: false,
            mut_param: false,
            hides_name: false,
            also: Vec::new(),
            shown: None,
        };
        if kind == BindingKind::Param
            && let Some(first) = scope.get_mut(&name.name)
        {
            self.diagnostics.error(DiagnosticKind::AlreadyDeclared {
                name: name.name.to_string(),
                at: name.span,
                first: first.span,
            });
            // A use after it may mean either parameter, so neither is judged: the name becomes
            // `Error` and can be assigned, which also keeps W0002 off a `mut` parameter.
            first.reassigned = true;
            binding = Binding {
                ty: None,
                kind: BindingKind::Var,
                reassigned: true,
                ..binding
            };
        }
        // Nothing can reassign the binding it replaces from here on.
        if let Some(old) = scope.insert(name.name, binding) {
            self.retire(name.name, &old);
        }
        local
    }

    /// Closes the innermost scope, and retires each of its bindings.
    pub(super) fn pop_scope(&mut self) {
        let scope = self
            .body
            .scopes
            .pop()
            .expect("a scope should be open while a block is checked");
        for (name, b) in scope {
            self.retire(name, &b);
        }
    }

    /// Judges `b`, which `name` can no longer reach. When nothing read it, its name does not
    /// start with `_` and it is a function's or a method's, it waits for the pass (W0014). A
    /// parameter of a body with a `todo` does not wait. Otherwise a `let mut` that was never
    /// reassigned gets W0002, and so does a `mut` parameter, unless a requirement spells its `mut`.
    fn retire(&mut self, name: Symbol, b: &Binding) {
        let never = !b.reassigned && !b.loop_named;
        // The fix removes the `mut` and the white space up to the name, but not a comment.
        let remove = b.keyword.filter(|_| never).map(|k| {
            Span::new(
                k.start(),
                removal_end(self.source.comments, k.end(), b.span.start()),
            )
        });
        if let Some(Owner::Func(func)) = self.body.owner
            && !b.read
            && !b.mut_param
            && !name.as_str().starts_with('_')
        {
            let param = b.kind == BindingKind::Param;
            if param && self.body.todo {
                return;
            }
            // Only a `for` or pattern name can become `_`, in each place that binds it.
            let fix = match b.kind {
                BindingKind::Unwrapped(_) => std::iter::once(b.span)
                    .chain(b.also.iter().copied())
                    .collect(),
                _ => Vec::new(),
            };
            self.pending.push(Pending {
                func,
                param,
                name: name.to_string(),
                at: b.span,
                fix,
                remove,
            });
            return;
        }
        let form = match remove {
            Some(remove) => Some(NeverReassignedForm::Let { remove }),
            None if never => self.never_reassigned_param(b),
            None => None,
        };
        if let Some(form) = form {
            self.warn(DiagnosticKind::NeverReassigned {
                name: name.to_string(),
                at: b.span,
                form,
            });
        }
    }

    /// The form of W0002 for `b`, which nothing reassigns, when it is a `mut` parameter that a
    /// `todo` body does not hide, and that is not a method's for a requirement: the requirement
    /// spells its `mut`, or E0152 reports it.
    fn never_reassigned_param(&self, b: &Binding) -> Option<NeverReassignedForm> {
        if !b.mut_param || self.body.todo {
            return None;
        }
        if let Some(Owner::Func(func)) = self.body.owner
            && self.named_for_requirement.contains(&func)
        {
            return None;
        }
        Some(NeverReassignedForm::Param {
            class: b
                .ty
                .is_some_and(|ty| matches!(unwrapped(ty).kind(), TypeKind::Class(..))),
        })
    }

    /// Reports W0011 for the checked condition `cond` at `at` of a `while` whose body checked,
    /// when it reads a local, nothing since `before` (the error, write and exit counts at the
    /// start of the condition) wrote one, and nothing left the loop or reported an error.
    fn unchanged_condition(&mut self, at: Span, cond: &tir::Expr, before: (u32, u32, u32)) {
        let (errors, writes, exits) = before;
        if self.diagnostics.errors() != errors || self.body.exits != exits {
            return;
        }
        let Some(locals) = self.condition_locals(cond).filter(|l| !l.is_empty()) else {
            return;
        };
        if !locals.iter().all(|&id| self.unchanged(id, writes)) {
            return;
        }
        let mut names = Vec::with_capacity(locals.len());
        let mut lets = Vec::new();
        for id in locals {
            let name = self.local_name(id);
            // `self` has no binding.
            if let Some(b) = self.lookup(name) {
                b.loop_named = true;
                if matches!(b.kind, BindingKind::Let | BindingKind::Var) {
                    lets.push(name.to_string());
                }
            }
            names.push(name.to_string());
        }
        self.warn(DiagnosticKind::UnchangedLoopCondition { at, names, lets });
    }

    /// The locals `cond` reads, each once, in source order; `None` when it holds a node that
    /// can see a change the body makes through an object or a function (W0011's rule 1).
    fn condition_locals(&self, cond: &tir::Expr) -> Option<Vec<tir::LocalId>> {
        let mut locals = (Vec::new(), FxHashSet::default());
        self.read_locals(cond, &mut locals).then_some(locals.0)
    }

    /// Adds the locals `e` reads to `locals`, in order and with the set of those already in it;
    /// false at a node rule 1 does not allow.
    fn read_locals(&self, e: &tir::Expr, locals: &mut Locals) -> bool {
        match &e.kind {
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Char(_)
            | ExprKind::Str(_)
            | ExprKind::None => true,
            &ExprKind::Local(id) => {
                if locals.1.insert(id) {
                    locals.0.push(id);
                }
                true
            }
            ExprKind::Wrap(operand) | ExprKind::Unary { operand, .. } | ExprKind::Len(operand) => {
                self.read_locals(operand, locals)
            }
            ExprKind::Equal { lhs, rhs, .. } => {
                // `==` on a value that holds a class runs the class's written `eq`, which reads
                // fields that can change with no write of a local.
                // Both sides have one type, up to an optional, so `lhs` tells for both.
                let none = matches!(lhs.kind, ExprKind::None) || matches!(rhs.kind, ExprKind::None);
                (none || !self.holds(lhs.ty).class)
                    && self.read_locals(lhs, locals)
                    && self.read_locals(rhs, locals)
            }
            ExprKind::Binary { lhs, rhs, .. }
            | ExprKind::Concat { lhs, rhs }
            | ExprKind::Is { lhs, rhs }
            | ExprKind::Index {
                base: lhs,
                index: rhs,
            } => self.read_locals(lhs, locals) && self.read_locals(rhs, locals),
            ExprKind::Slice {
                base, start, end, ..
            } => {
                self.read_locals(base, locals)
                    && start
                        .iter()
                        .chain(end)
                        .all(|bound| self.read_locals(bound, locals))
            }
            ExprKind::Compare { operands, .. } => operands
                .iter()
                .all(|operand| self.read_locals(operand, locals)),
            _ => false,
        }
    }

    /// Whether the condition local `id` can be named by W0011: its type holds no poison, it is
    /// `self` or has a binding, and nothing wrote it after the write count `writes`.
    fn unchanged(&mut self, id: tir::LocalId, writes: u32) -> bool {
        if self.holds(self.body.locals[id.0].ty).poison {
            return false;
        }
        if id.0 == 0 && self.body.self_type != SelfType::Absent {
            return self.body.self_written <= writes;
        }
        // A local with no name is a checker temporary, so it has no binding.
        let name = self.body.locals[id.0].name.as_deref().map(Symbol::intern);
        name.and_then(|name| self.lookup(name))
            .is_some_and(|b| b.local == id && b.written <= writes)
    }

    /// The name of the local `id`, which `unchanged` accepted.
    fn local_name(&self, id: tir::LocalId) -> Symbol {
        let name = self.body.locals[id.0].name.as_deref();
        Symbol::intern(name.expect("a local that `unchanged` accepts should have a name"))
    }

    pub(super) fn lookup(&mut self, name: Symbol) -> Option<&mut Binding> {
        self.body
            .scopes
            .iter_mut()
            .rev()
            .find_map(|scope| scope.get_mut(&name))
    }
}

/// The locals a condition reads, in source order, and the set of them.
type Locals = (Vec<tir::LocalId>, FxHashSet<tir::LocalId>);

/// What a `for` binds before its `Loop`: `stmts` bind the source and the counter, which runs
/// to `limit`, and `item` is the value of the loop variable for the current counter. Over
/// `..=`, `go` says whether a value is left, and the counter runs through `limit`.
struct ForHeader {
    stmts: Vec<tir::Stmt>,
    counter: tir::LocalId,
    limit: tir::LocalId,
    go: Option<tir::LocalId>,
    item: tir::Expr,
}

/// Whether `block` holds one statement that leaves and reports nothing else when it goes: a
/// `return` with no value or with a number, `bool` or `char` literal, or a `continue` with no
/// label. A `break` is not one: it gives its `loop` a type, and the errors of that type come
/// after the block.
fn is_plain_exit(block: &ast::Block) -> bool {
    let [stmt] = block.stmts.as_slice() else {
        return false;
    };
    let StmtKind::Expr(e) = &stmt.kind else {
        return false;
    };
    match &e.kind {
        ast::ExprKind::Return { value, .. } => value.as_ref().is_none_or(|v| {
            matches!(
                v.kind,
                ast::ExprKind::Int { .. } | ast::ExprKind::Bool(_) | ast::ExprKind::Char(_)
            )
        }),
        ast::ExprKind::Continue { label } => label.is_none(),
        _ => false,
    }
}

/// A new expression for the fixed place `e` (see `fixed_place`).
fn reread(e: &tir::Expr) -> tir::Expr {
    let kind = match &e.kind {
        ExprKind::Local(local) => ExprKind::Local(*local),
        ExprKind::Field { base, index } => ExprKind::Field {
            base: Box::new(reread(base)),
            index: *index,
        },
        _ => unreachable!("a fixed place should be a local or a field of one"),
    };
    tir::Expr::new(kind, e.ty, e.span)
}

/// Wraps into `ty` each value of another type, `never` aside, that a `break` in `e` gives the
/// loop `depth` loops out from `e`: the `T` values of a loop that a `T?` value widened. It enters
/// nested loops only when `crossed`: when a `break` in one targets that loop.
fn wrap_breaks(e: &mut tir::Expr, depth: usize, ty: Type, crossed: bool) {
    let nested = matches!(e.kind, ExprKind::Loop(_));
    if nested && !crossed {
        return;
    }
    #[cfg(test)]
    if nested {
        super::WRAP_LOOPS.set(super::WRAP_LOOPS.get() + 1);
    }
    let inner = depth + usize::from(nested);
    e.each_child_mut(&mut |child| wrap_breaks(child, inner, ty, crossed));
    if let ExprKind::Break {
        depth: target,
        value: Some(value),
    } = &mut e.kind
        && *target == depth
        && value.ty != ty
        && value.ty != Type::NEVER
    {
        let placeholder = tir::Expr::new(ExprKind::None, ty, value.span);
        let payload = std::mem::replace(&mut **value, placeholder);
        **value = widen(payload, ty);
        #[cfg(test)]
        if !crossed {
            super::WIDENED_BREAKS.set(super::WIDENED_BREAKS.get() + 1);
        }
    }
}

/// The type a statement gives its block when it comes last.
fn stmt_type(stmt: &tir::Stmt) -> Type {
    match stmt {
        tir::Stmt::Let { .. }
        | tir::Stmt::Assign { .. }
        | tir::Stmt::Append { .. }
        | tir::Stmt::Push { .. }
        | tir::Stmt::LetElse { .. } => Type::UNIT,
        tir::Stmt::Expr(e) => e.ty,
    }
}

/// `while c { b }` is `Loop { If c { b } else { Break } }`, and `while let` the same with an
/// `IfSome`: `branch` makes that node of the body and the `Break` block.
fn lower_loop(
    body: tir::Block,
    span: Span,
    branch: impl FnOnce(tir::Block, tir::Block) -> ExprKind,
) -> tir::Expr {
    let expr = |kind, ty| tir::Expr::new(kind, ty, span);
    let exit = tir::Block {
        stmts: vec![tir::Stmt::Expr(expr(
            ExprKind::Break {
                depth: 0,
                value: None,
            },
            Type::NEVER,
        ))],
        value: None,
        ty: Type::NEVER,
    };
    // The `else` branch is `never`, so the branch has the body's type.
    let ty = body.ty;
    let branch = expr(branch(body, exit), ty);
    let looped = tir::Block {
        stmts: Vec::new(),
        value: Some(Box::new(branch)),
        ty,
    };
    expr(ExprKind::Loop(looped), Type::UNIT)
}

/// A value that `discard` warns about: its type, the span the warning points at, the span its
/// fix drops, and whether that is a `match` arm's whole body.
struct Unused {
    ty: String,
    at: Span,
    site: Span,
    arm: bool,
}
