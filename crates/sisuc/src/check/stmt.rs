//! Statements, blocks and scopes.

use rustc_hash::FxHashMap;

use super::array::optional_array;
use super::expr::{fits, local};
use super::generic::Ahead;
use super::pattern::LetPattern;
use super::prelude::PRELUDE;
use super::{
    Binding, BindingKind, Checked, CheckedBlock, Checker, Declared, LoopContext, LoopFrame,
    LoopKind, Poisoned, UNKNOWN, block_expr, built_in_type, tail, type_of, wrap,
};
use crate::ast::{self, StmtKind};
use crate::diagnostic::{ClashForm, ClashKind, DiagnosticKind, MismatchSource, Unwrap, closest};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, ExprKind, Type, TypeKind};
use std::slice::SliceIndex;

/// What `block_statements` has checked of a block, and what those statements leave.
pub(super) struct BlockSoFar {
    stmts: Vec<tir::Stmt>,
    /// `never` once a statement is, else the last statement's type; `None` is `Error`.
    ty: Option<Type>,
    failed: bool,
    /// Whether "unreachable code" is reported, or moot.
    warned: bool,
    /// Whether the value rejected the type expected of it.
    mismatch: bool,
    /// How many of the block's statements are checked.
    done: usize,
}

impl Default for BlockSoFar {
    fn default() -> Self {
        BlockSoFar {
            stmts: Vec::new(),
            ty: Some(Type::UNIT),
            failed: false,
            warned: false,
            mismatch: false,
            done: 0,
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
    pub(super) fn block(&mut self, block: &ast::Block, expected: Option<Type>) -> CheckedBlock {
        // Ranking an argument may have checked all but the block's value (see `tail_waits`).
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
        expected: Option<Type>,
    ) -> CheckedBlock {
        self.resume_block(block, expected, BlockSoFar::default())
    }

    /// `scoped_block`, its statements before `so_far.done` checked already.
    fn resume_block(
        &mut self,
        block: &ast::Block,
        expected: Option<Type>,
        mut so_far: BlockSoFar,
    ) -> CheckedBlock {
        self.block_statements(block, expected, &mut so_far, block.stmts.len());
        let BlockSoFar {
            mut stmts,
            ty,
            failed,
            mismatch,
            ..
        } = so_far;
        self.pop_scope();
        let ty = match ty {
            Some(ty) if !failed => ty,
            // A tail that rejected `expected` fails the block, unless a `never` came first.
            ty => {
                let mismatch = mismatch && ty.is_none();
                return Err(Poisoned { ty, mismatch });
            }
        };
        let ends_in_expr = matches!(
            block.stmts.last(),
            Some(ast::Stmt {
                kind: StmtKind::Expr(_),
                ..
            })
        );
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
        expected: Option<Type>,
        so_far: &mut BlockSoFar,
        end: usize,
    ) {
        let done = so_far.done;
        for (i, stmt) in (done..).zip(&block.stmts[done..end]) {
            if so_far.ty == Some(Type::NEVER) && !so_far.warned {
                self.report(DiagnosticKind::UnreachableCode { at: stmt.span });
                so_far.warned = true;
            }
            let last = i + 1 == block.stmts.len();
            let outer_jump = std::mem::take(&mut self.body.invalid_jump);
            let checked = self.stmt(stmt, expected.filter(|_| last));
            let invalid_jump = self.body.invalid_jump;
            // Nested blocks leave their flag set, so an enclosing statement sees it too.
            self.body.invalid_jump |= outer_jump;
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
                    so_far.failed = true;
                    so_far.mismatch = last && poisoned.mismatch;
                    // An invalid jump made the block `never` only by error; what follows is no
                    // new news. Any other failed `never` statement still leaves the block.
                    so_far.warned |= invalid_jump && poisoned.ty == Some(Type::NEVER);
                    poisoned.ty
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
                if !matches!(checked.ty, Type::UNIT | Type::NEVER) {
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
            self.report(DiagnosticKind::UnusedValue {
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
    fn stmt(&mut self, stmt: &ast::Stmt, expected: Option<Type>) -> Result<tir::Stmt, Poisoned> {
        let unit = || Poisoned::typed(Type::UNIT);
        match &stmt.kind {
            StmtKind::Let {
                mutable,
                name,
                ty,
                init,
            } => {
                let declared = ty.as_ref().map(|ty| self.resolve(ty));
                let init = match declared {
                    Some(declared) => self.expect(init, declared),
                    None => self.value(init, None),
                };
                // `let _ = e` binds nothing: the value is an expression statement, which
                // codegen releases at once.
                if name.name.as_str() == "_" {
                    return init.map(tir::Stmt::Expr).map_err(|_| unit());
                }
                // The initializer's type, which a failed one keeps only while it is still
                // known (`if` with one failed branch); an unknown declared type is `Error`.
                let ty = match declared {
                    Some(None) => None,
                    _ => type_of(&init),
                };
                let kind = if mutable.is_some() {
                    BindingKind::Var
                } else {
                    BindingKind::Let
                };
                let local = self.declare(name, ty, kind, *mutable);
                Ok(tir::Stmt::Let {
                    local,
                    init: init.map_err(|_| unit())?,
                })
            }
            StmtKind::While {
                label,
                keyword,
                cond: cond_ast,
                body,
            } => {
                // Only the literal: the AST has no parenthesis node, so `(true)` is it too.
                let endless = matches!(cond_ast.kind, ast::ExprKind::Bool(true));
                let cond = self.in_header(*keyword, label.clone(), |c| {
                    c.expect(cond_ast, Some(Type::BOOL))
                });
                let frame = LoopFrame::new(LoopKind::While, *keyword, label.clone(), None);
                let (checked, frame) = self.loop_body(frame, |c| c.block(body, None));
                let broken = frame.broken;
                if endless {
                    // Writing `loop` over a failed loop (a failed jump, label or `break` value in
                    // it) could remove its error rather than fix it, and a safe fix may remove
                    // only its own diagnostic.
                    self.report(DiagnosticKind::WhileTrue {
                        at: keyword.to(cond_ast.span),
                        fix: !frame.failed,
                    });
                }
                if let Ok(checked) = &checked {
                    self.discard_tail(body, checked);
                }
                let (Ok(cond), Ok(body)) = (cond, checked) else {
                    // A failed loop (a failed jump, label or `break` value in it) has no type, so
                    // nothing after it reports.
                    return Err(if frame.failed {
                        Poisoned::error()
                    } else if endless && !broken {
                        Poisoned::typed(Type::NEVER)
                    } else {
                        unit()
                    });
                };
                if endless {
                    // A bare `Loop`: `else Break` would give MIR an exit the checker denies.
                    let ty = if broken { Type::UNIT } else { Type::NEVER };
                    return Ok(tir::Stmt::Expr(tir::Expr {
                        kind: ExprKind::Loop(body),
                        ty,
                        span: stmt.span,
                    }));
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
                let frame = LoopFrame::new(LoopKind::WhileLet, *keyword, label.clone(), None);
                let ((bound, checked), _) = self.loop_body(frame, |c| {
                    c.let_block(Unwrap::WhileLet, form, pattern, body, None)
                });
                if let Ok(checked) = &checked {
                    self.discard_tail(body, checked);
                }
                let (Ok(scrutinee), Ok(body)) = (scrutinee, checked) else {
                    return Err(unit());
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
                        pattern: Some(pattern),
                    } => pattern,
                    LetPattern::Match { pattern: None } => return Err(unit()),
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
            StmtKind::Return(value) => self.return_stmt(stmt.span, value.as_ref()),
            StmtKind::Break { label, value } => {
                self.jump(stmt.span, "break", label.as_ref(), value.as_ref())
            }
            StmtKind::Continue { label } => self.jump(stmt.span, "continue", label.as_ref(), None),
            StmtKind::Assign { target, value } => self.assign(stmt.span, target, value),
            StmtKind::CompoundAssign {
                op,
                op_span,
                target,
                value,
            } => self.compound_assign(stmt.span, (*op, *op_span), target, value),
            StmtKind::Expr(e) => self.expr(e, expected).map(tir::Stmt::Expr),
        }
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
        let expr = |kind, ty| tir::Expr { kind, ty, span };
        let (header, bound) = self.in_header(keyword, label.cloned(), |c| match source {
            ast::ForSource::Array(e) => c.for_array(span, e),
            ast::ForSource::Range {
                start,
                end,
                inclusive,
            } => c.for_range(span, start, end, *inclusive),
        });
        let frame = LoopFrame::new(LoopKind::For, keyword, label.cloned(), None);
        let ((x, checked), _) = self.loop_body(frame, |c| {
            c.unwrapping_block("for", name, bound, body, None)
        });
        if let Ok(checked) = &checked {
            self.discard_tail(body, checked);
        }
        let (Some(header), Ok(body)) = (header, checked) else {
            return Err(Poisoned::typed(Type::UNIT));
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
    fn for_array(&mut self, span: Span, e: &ast::Expr) -> (Option<ForHeader>, Option<Type>) {
        let checked = self.value(e, None);
        let (element, d) = match type_of(&checked).map(|ty| (ty, ty.kind())) {
            Some((_, TypeKind::Array(element))) => (Some(element), None),
            Some((_, TypeKind::Optional(payload)))
                if matches!(payload.kind(), TypeKind::Array(_)) =>
            {
                (None, Some(optional_array(e)))
            }
            Some((ty, _)) => {
                let found = self.type_name(ty);
                let d = DiagnosticKind::ForNeedsArray { found, at: e.span };
                (None, Some(d.into()))
            }
            None => (None, None),
        };
        self.diagnostics.extend(d);
        let ty = type_of(&checked).unwrap_or(Type::UNIT);
        // A place that cannot change during the loop is read in place, not copied into a local
        // that parameter inference would take for a consume.
        let fixed = checked.as_ref().is_ok_and(|e| self.fixed_place(e));
        let array = (!fixed).then(|| self.fresh(ty));
        let len = self.fresh(Type::I64);
        let counter = self.fresh_var(Type::I64);
        let (Ok(checked), Some(element)) = (checked, element) else {
            return (None, element);
        };
        let expr = |kind, ty| tir::Expr { kind, ty, span };
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
        (Some(header), Some(element))
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
        start: &ast::Expr,
        end: &ast::Expr,
        inclusive: bool,
    ) -> (Option<ForHeader>, Option<Type>) {
        let start = self.expect(start, Some(Type::I64));
        let end = self.expect(end, Some(Type::I64));
        let counter = self.fresh_var(Type::I64);
        let limit = self.fresh(Type::I64);
        let go = inclusive.then(|| self.fresh_var(Type::BOOL));
        let (Ok(start), Ok(end)) = (start, end) else {
            return (None, Some(Type::I64));
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
                init: tir::Expr {
                    kind: some,
                    ty: Type::BOOL,
                    span,
                },
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
        (Some(header), Some(Type::I64))
    }

    /// `return`, of type `never` even when it fails.
    fn return_stmt(
        &mut self,
        span: Span,
        value: Option<&ast::Expr>,
    ) -> Result<tir::Stmt, Poisoned> {
        let never = || Poisoned::typed(Type::NEVER);
        let value = match value {
            None => match self.body.ret {
                Some(ret) if ret != Type::UNIT => {
                    let want = self.type_name(ret);
                    self.report(DiagnosticKind::ReturnNeedsValue { want, at: span });
                    return Err(never());
                }
                _ => None,
            },
            Some(value) if self.body.ret == Some(Type::UNIT) => {
                if type_of(&self.value(value, None)).is_some() {
                    self.report(DiagnosticKind::UnexpectedReturnValue { at: value.span });
                }
                return Err(never());
            }
            Some(value) => {
                let value = self.expect(value, self.body.ret).map_err(|_| never())?;
                Some(Box::new(value))
            }
        };
        Ok(tir::Stmt::Expr(tir::Expr {
            kind: ExprKind::Return(value),
            ty: Type::NEVER,
            span,
        }))
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
            self.fail_loops(at..=at);
            frame.failed = true;
            self.report(DiagnosticKind::LabelReused {
                name: label.name.to_string(),
                at: label.span,
                outer,
            });
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
            self.report(DiagnosticKind::UnusedLabel {
                name: label.name.to_string(),
                at: label.span,
                remove: Span::new(label.span.start(), frame.keyword.start()),
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

    /// Fails the loop bodies in `range` of `body.loops`: they get no type, so the code after
    /// them reports nothing more.
    fn fail_loops(&mut self, range: impl SliceIndex<[LoopContext], Output = [LoopContext]>) {
        for context in &mut self.body.loops[range] {
            if let LoopContext::Body(frame) = context {
                frame.failed = true;
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
    /// `suggested`: that loop and the loops inside it get no type, and `suggested` is not
    /// reported unused.
    pub(super) fn break_meant(&mut self, suggested: &str) {
        let at = self.named_by_failed_jump(Symbol::intern(suggested));
        self.fail_loops(at..);
    }

    /// Skips a `break` value that is not checked: a jump in it may name any open loop, so every
    /// open loop gets no type and no open loop label is reported unused.
    fn skip_value(&mut self) {
        let labels: Vec<Span> = self.open_labels().map(|l| l.span).collect();
        self.body.failed_labels.extend(labels);
        self.fail_loops(..);
    }

    /// "no loop label `name`" for `name` after `keyword`, offering the closest open loop label,
    /// which counts as named.
    fn missing_label(&mut self, name: &ast::Ident, keyword: &'static str) {
        let suggestion = closest(
            name.name.as_str(),
            self.open_labels().map(|l| l.name.as_str()),
        )
        .map(str::to_string);
        if let Some(label) = &suggestion {
            if keyword == "break" {
                self.break_meant(label);
            } else {
                self.named_by_failed_jump(Symbol::intern(label));
            }
        }
        self.report(DiagnosticKind::MissingLabel {
            name: name.name.to_string(),
            keyword,
            at: name.span,
            suggestion,
        });
    }

    /// What `name` is when a value or a type of that spelling is in scope: a binding, a
    /// function (the prelude's included), a constant, a class, an enum or a built-in type, and
    /// where it is declared, when that is in the source.
    fn in_scope(&self, name: Symbol) -> Option<(ClashKind, Option<Span>)> {
        // A broken item's declaration is not known.
        fn at<T>(declared: &Declared<T>, span: impl FnOnce(&T) -> Span) -> Option<Span> {
            match declared {
                Declared::Known(item) => Some(span(item)),
                Declared::Broken => None,
            }
        }
        if let Some(binding) = self.body.scopes.iter().rev().find_map(|s| s.get(&name)) {
            return Some((ClashKind::Binding, Some(binding.span)));
        }
        if let Some(id) = self.function_ids.get(&name) {
            let span = at(id, |id| self.signatures[id.0].span);
            return Some((ClashKind::Function, span));
        }
        if PRELUDE.contains(&name.as_str()) {
            return Some((ClashKind::Function, None));
        }
        if let Some(constant) = self.constants.get(&name) {
            return Some((ClashKind::Constant, at(constant, |info| info.span)));
        }
        if let Some(id) = self.class_ids.get(&name) {
            let span = at(id, |id| self.classes[id.0].name.span);
            return Some((ClashKind::Class, span));
        }
        if let Some(id) = self.enum_ids.get(&name) {
            let span = at(id, |id| self.enums[id.0].name.span);
            return Some((ClashKind::Enum, span));
        }
        built_in_type(name.as_str()).map(|_| (ClashKind::BuiltInType, None))
    }

    /// `break` or `continue`, of type `never` even outside a loop, and `break`'s `value`. In a
    /// loop header it is `Error`, so the condition or source around it reports nothing more.
    fn jump(
        &mut self,
        span: Span,
        keyword: &'static str,
        label: Option<&ast::Ident>,
        value: Option<&ast::Expr>,
    ) -> Result<tir::Stmt, Poisoned> {
        let (target, depth) = self.target(span, keyword, label)?;
        if let Some(label) = label
            && let Some((kind, declared)) = self.in_scope(label.name)
        {
            let at = self.frame(target).label.as_ref().map(|l| l.span);
            self.report(DiagnosticKind::LabelClash {
                name: label.name.to_string(),
                at: label.span,
                declared: at.expect("a jump's loop label should be its target's"),
                form: ClashForm::Name { kind, declared },
            });
            // The loop label may have meant the name, so the loop gets no type, and the value
            // is not checked.
            self.frame(target).failed = true;
            if value.is_some() {
                self.skip_value();
            }
            return Err(Poisoned::error());
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
        Ok(tir::Stmt::Expr(tir::Expr {
            kind,
            ty: Type::NEVER,
            span,
        }))
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
                self.fail_loops(found.unwrap_or(0)..);
                if let Some(label) = label {
                    self.named_by_failed_jump(label.name);
                }
                self.report(DiagnosticKind::JumpInLoopHeader {
                    keyword,
                    at: span,
                    loop_at,
                });
                Err(Poisoned::error())
            }
            // Only `continue` takes a name that is no open loop label; outside a loop, the
            // jump is the error.
            _ => match label {
                Some(label) if !self.body.loops.is_empty() => {
                    self.missing_label(label, "continue");
                    Err(Poisoned::error())
                }
                _ => {
                    self.report(DiagnosticKind::JumpOutsideLoop { keyword, at: span });
                    self.body.invalid_jump = true;
                    Err(Poisoned::typed(Type::NEVER))
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
        expected: Option<Type>,
    ) -> Checked {
        let frame = LoopFrame::new(LoopKind::Loop, keyword, label.cloned(), expected);
        let (mut checked, mut frame) = self.loop_body(frame, |c| c.block(body, None));
        if let Ok(checked) = &mut checked {
            self.discard_tail(body, checked);
            if let (true, Some((ty, _))) = (frame.widened, frame.first) {
                checked.each_expr_mut(&mut |e| wrap_breaks(e, 0, ty, frame.crossed));
            }
        }
        if let (Some(at), Some(valued)) = (frame.bare, frame.valued) {
            self.report(DiagnosticKind::BareBreak { at, valued });
            frame.failed = true;
        }
        let ty = frame.loop_type();
        match (checked, ty) {
            (Ok(body), Some(ty)) => Ok(tir::Expr {
                kind: ExprKind::Loop(body),
                ty,
                span,
            }),
            (_, ty) => Err(Poisoned::of(ty)),
        }
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
            frame.failed = true;
            let (keyword, loop_at) = (frame.kind.keyword(), frame.keyword);
            // `break outr`, a misspelled loop label, rather than a value.
            if let Some(name) = typo
                && self.in_scope(name.name).is_none()
                && closest(
                    name.name.as_str(),
                    self.open_labels().map(|l| l.name.as_str()),
                )
                .is_some()
            {
                self.missing_label(name, "break");
            } else {
                self.skip_value();
                self.report(DiagnosticKind::BreakValueNotLoop {
                    keyword,
                    at: v.span,
                    loop_at,
                });
            }
            return Err(Poisoned::typed(Type::NEVER));
        }
        let frame = self.frame(target);
        let hint = frame
            .expected
            .map(|ty| (ty, None))
            .or(frame.first.map(|(ty, at)| (ty, Some(at))));
        let from = hint
            .and_then(|(_, first)| first)
            .map(|first| (v.span, first));
        let outer = std::mem::replace(&mut self.body.break_hint, from);
        self.body.label_typo = typo.map(|name| name.span);
        let checked = self.expr(v, hint.map(|(ty, _)| ty));
        self.body.label_typo = None;
        self.body.break_hint = outer;
        // A failed value fails the loop, and gives it no type and no second report.
        if checked.is_err() {
            self.frame(target).failed = true;
            return checked;
        }
        let checked = match type_of(&checked) {
            Some(Type::UNIT) => {
                self.report(DiagnosticKind::NoValue {
                    at: v.span,
                    never: false,
                });
                Err(Poisoned::error())
            }
            Some(Type::NEVER) | None => checked,
            Some(got) => {
                // Checking `v` can set the first type, through a `break` of this loop inside it.
                let frame = self.frame(target);
                frame.valued.get_or_insert(span);
                match (frame.expected, frame.first) {
                    (Some(ty), _) if ty == UNKNOWN => checked,
                    // A value that fits the expected type keeps its own, as a block's value does,
                    // and joins the others as `if` branches do: a `T` and a `T?` make a `T?`.
                    (Some(ty), Some((first, _))) if fits(ty, got) && fits(first, got) => {
                        self.against(v, checked, Some(first), None).0
                    }
                    (Some(ty), first) if fits(ty, got) => {
                        frame.widened = first.is_some();
                        frame.first = Some((got, span));
                        checked
                    }
                    (Some(ty), _) => self.against(v, checked, Some(ty), None).0,
                    (None, Some((first, at))) => {
                        self.against(v, checked, Some(first), Some(MismatchSource::Break(at)))
                            .0
                    }
                    (None, None) => {
                        frame.first = Some((got, span));
                        checked
                    }
                }
            }
        };
        if checked.is_err() {
            self.frame(target).failed = true;
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
        expected: Option<Type>,
    ) -> (tir::LocalId, CheckedBlock) {
        self.body.scopes.push(FxHashMap::default());
        let bind = if name.name.as_str() == "_" {
            self.fresh(ty.unwrap_or(Type::UNIT))
        } else {
            self.declare(name, ty, BindingKind::Unwrapped(keyword), None)
        };
        (bind, self.scoped_block(block, expected))
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
        let last_exit = operands.iter().rposition(tir::Expr::exits);
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
    /// or an enum takes is reported, and the binding still takes it.
    pub(super) fn declare(
        &mut self,
        name: &ast::Ident,
        ty: Option<Type>,
        kind: BindingKind,
        keyword: Option<Span>,
    ) -> tir::LocalId {
        if !self.reserved(name) && self.enum_ids.contains_key(&name.name) {
            self.report(DiagnosticKind::EnumNamedLocal {
                name: name.name.to_string(),
                at: name.span,
            });
        }
        self.insert_local(name, ty, kind, keyword)
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
        };
        if kind == BindingKind::Param
            && let Some(first) = scope.get(&name.name)
        {
            self.diagnostics.push(
                DiagnosticKind::AlreadyDeclared {
                    name: name.name.to_string(),
                    at: name.span,
                    first: first.span,
                }
                .into(),
            );
            // A use after it may mean either parameter, so neither is judged: the name becomes
            // `Error` and can be assigned.
            binding = Binding {
                ty: None,
                kind: BindingKind::Var,
                ..binding
            };
        }
        // Nothing can reassign the binding it replaces from here on.
        if let Some(old) = scope.insert(name.name, binding) {
            self.never_reassigned(name.name, &old);
        }
        local
    }

    /// Closes the innermost scope, warning about each `let mut` that was never reassigned.
    pub(super) fn pop_scope(&mut self) {
        let scope = self
            .body
            .scopes
            .pop()
            .expect("a scope should be open while a block is checked");
        for (name, b) in scope {
            self.never_reassigned(name, &b);
        }
    }

    /// Warns when `b`, which `name` can no longer reach, is a `let mut` that was never
    /// reassigned.
    fn never_reassigned(&mut self, name: Symbol, b: &Binding) {
        if let Some(keyword) = b.keyword
            && !b.reassigned
        {
            self.report(DiagnosticKind::NeverReassigned {
                name: name.to_string(),
                at: b.span,
                keyword,
            });
        }
    }

    pub(super) fn lookup(&mut self, name: Symbol) -> Option<&mut Binding> {
        self.body
            .scopes
            .iter_mut()
            .rev()
            .find_map(|scope| scope.get_mut(&name))
    }
}

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
    tir::Expr {
        kind,
        ty: e.ty,
        span: e.span,
    }
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
        let placeholder = tir::Expr {
            kind: ExprKind::None,
            ty,
            span: value.span,
        };
        let payload = std::mem::replace(&mut **value, placeholder);
        **value = wrap(payload, ty);
    }
}

/// The type a statement gives its block when it comes last.
fn stmt_type(stmt: &tir::Stmt) -> Type {
    match stmt {
        tir::Stmt::Let { .. } | tir::Stmt::Assign { .. } | tir::Stmt::Append { .. } => Type::UNIT,
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
    let expr = |kind, ty| tir::Expr { kind, ty, span };
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
