//! Statements, blocks and scopes.

use rustc_hash::FxHashMap;

use super::array::optional_array;
use super::expr::local;
use super::pattern::LetPattern;
use super::{
    Binding, BindingKind, CheckedBlock, Checker, LoopContext, Poisoned, block_expr, tail, type_of,
};
use crate::ast::{self, StmtKind};
use crate::diagnostic::{DiagnosticKind, Unwrap};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, ExprKind, Type, TypeKind};

/// The most values of one statement that share one fix.
// shortcut: past the cap the warnings offer no fix; store one fix per statement and share it if
// long `match` statements need fixes.
const SHARED_FIX_SITES: usize = 8;

impl Checker<'_> {
    /// `never` once a statement is `never`, else the type of the last statement (`unit` when
    /// there is none). Warns "unreachable code" on the first statement after a `never` one.
    /// Every statement is checked, also after one fails. `expected` is for the block's value.
    pub(super) fn block(&mut self, block: &ast::Block, expected: Option<Type>) -> CheckedBlock {
        self.body.scopes.push(FxHashMap::default());
        self.scoped_block(block, expected)
    }

    /// `block`, in the scope just opened for it, which it closes.
    pub(super) fn scoped_block(
        &mut self,
        block: &ast::Block,
        expected: Option<Type>,
    ) -> CheckedBlock {
        let mut stmts = Vec::new();
        let mut ty = Some(Type::UNIT);
        let (mut failed, mut warned, mut mismatch) = (false, false, false);
        for (i, stmt) in block.stmts.iter().enumerate() {
            if ty == Some(Type::NEVER) && !warned {
                self.report(DiagnosticKind::UnreachableCode { at: stmt.span });
                warned = true;
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
                    stmts.push(lowered);
                    Some(ty)
                }
                Err(poisoned) => {
                    failed = true;
                    mismatch = last && poisoned.mismatch;
                    // An invalid jump made the block `never` only by error; what follows is no
                    // new news. Any other failed `never` statement still leaves the block.
                    warned |= invalid_jump && poisoned.ty == Some(Type::NEVER);
                    poisoned.ty
                }
            };
            if ty != Some(Type::NEVER) {
                ty = stmt_ty;
            }
        }
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

    /// Warns "unused value" at `source`, whose checked form is `checked`, unless its type is
    /// `unit` or `never`. An `if`, an `if let`, a `match` or a block is not itself the value: the
    /// tail of each branch or arm is. The warnings of one statement share one fix, which drops
    /// all of its values: a branch with no value beside one with a value is an error.
    pub(super) fn discard(&mut self, source: &ast::Expr, checked: &tir::Expr) {
        let mut unused = Vec::new();
        self.collect(source, checked, false, &mut unused);
        self.report_unused(unused);
    }

    /// The `(type, span, arm)` of each value under `source` that `discard` warns about, pushed to
    /// `unused` in source order, where `arm` is whether `source` is a `match` arm's whole body.
    /// Only the roots, `discard` and `discard_tail`, report, so an inner `if` joins its statement's
    /// fix.
    fn collect(
        &mut self,
        source: &ast::Expr,
        checked: &tir::Expr,
        arm: bool,
        unused: &mut Vec<(String, Span, bool)>,
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
                    let ty = self.type_name(checked.ty);
                    unused.push((ty, source.span, arm));
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
    fn collect_tail(
        &mut self,
        block: &ast::Block,
        checked: &tir::Block,
        unused: &mut Vec<(String, Span, bool)>,
    ) {
        if let (Some(source), Some(value)) = (tail(block), &checked.value) {
            self.collect(source, value, false, unused);
        }
    }

    /// Reports each of `unused`, one statement's values, with the fix that drops them all, unless
    /// there are more than `SHARED_FIX_SITES`: each fix lists every value, so N values cost N^2
    /// edits.
    fn report_unused(&mut self, unused: Vec<(String, Span, bool)>) {
        let sites: Vec<_> = if unused.len() <= SHARED_FIX_SITES {
            unused.iter().map(|&(_, at, arm)| (at, arm)).collect()
        } else {
            Vec::new()
        };
        for (ty, at, arm) in unused {
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
                cond: cond_ast,
                body,
            } => {
                // Only the literal: the AST has no parenthesis node, so `(true)` is it too.
                let endless = matches!(cond_ast.kind, ast::ExprKind::Bool(true));
                let header = keyword_span(stmt.span, "while");
                let cond = self.in_header(header, |c| c.expect(cond_ast, Some(Type::BOOL)));
                let (checked, broken) = self.loop_body(|c| c.block(body, None));
                if let Ok(checked) = &checked {
                    self.discard_tail(body, checked);
                }
                let (Ok(cond), Ok(body)) = (cond, checked) else {
                    return Err(if endless && !broken {
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
                pattern,
                value,
                body,
            } => {
                let header = keyword_span(stmt.span, "while");
                let (scrutinee, form, _) =
                    self.in_header(header, |c| c.let_value(Unwrap::WhileLet, pattern, value));
                let ((bound, checked), _) =
                    self.loop_body(|c| c.let_block(Unwrap::WhileLet, form, pattern, body, None));
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
            StmtKind::For { name, source, body } => self.for_loop(stmt.span, name, source, body),
            StmtKind::Return(value) => self.return_stmt(stmt.span, value.as_ref()),
            StmtKind::Break => self.jump(stmt.span, "break", ExprKind::Break),
            StmtKind::Continue => self.jump(stmt.span, "continue", ExprKind::Continue),
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
        name: &ast::Ident,
        source: &ast::ForSource,
        body: &ast::Block,
    ) -> Result<tir::Stmt, Poisoned> {
        let expr = |kind, ty| tir::Expr { kind, ty, span };
        let (header, bound) = self.in_header(keyword_span(span, "for"), |c| match source {
            ast::ForSource::Array(e) => c.for_array(span, e),
            ast::ForSource::Range {
                start,
                end,
                inclusive,
            } => c.for_range(span, start, end, *inclusive),
        });
        let ((x, checked), _) =
            self.loop_body(|c| c.unwrapping_block("for", name, bound, body, None));
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
        let array = self.fresh(ty);
        let len = self.fresh(Type::I64);
        let counter = self.fresh_var(Type::I64);
        let (Ok(checked), Some(element)) = (checked, element) else {
            return (None, element);
        };
        let expr = |kind, ty| tir::Expr { kind, ty, span };
        let read = || local(array, ty, e.span);
        let stmts = vec![
            tir::Stmt::Let {
                local: array,
                init: checked,
            },
            tir::Stmt::Let {
                local: len,
                init: expr(ExprKind::Len(Box::new(read())), Type::I64),
            },
            tir::Stmt::Let {
                local: counter,
                init: expr(ExprKind::Int(0), Type::I64),
            },
        ];
        let item = ExprKind::Index {
            base: Box::new(read()),
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

    /// `f`, inside the header of the loop whose keyword is `keyword`.
    fn in_header<T>(&mut self, keyword: Span, f: impl FnOnce(&mut Self) -> T) -> T {
        self.body.loops.push(LoopContext::Header(keyword));
        let checked = f(self);
        self.body.loops.pop();
        checked
    }

    /// `f`, inside a loop's body, and whether a `break` targeted that loop.
    fn loop_body<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> (T, bool) {
        self.body.loops.push(LoopContext::Body { broken: false });
        let checked = f(self);
        let broken = matches!(
            self.body.loops.pop(),
            Some(LoopContext::Body { broken: true })
        );
        (checked, broken)
    }

    /// `break` or `continue`, of type `never` even outside a loop. In a loop header it is
    /// `Error`, so the condition or source around it reports nothing more.
    fn jump(
        &mut self,
        span: Span,
        keyword: &'static str,
        kind: ExprKind,
    ) -> Result<tir::Stmt, Poisoned> {
        match self.body.loops.last_mut() {
            Some(LoopContext::Body { broken }) => *broken |= keyword == "break",
            Some(&mut LoopContext::Header(loop_at)) => {
                self.report(DiagnosticKind::JumpInLoopHeader {
                    keyword,
                    at: span,
                    loop_at,
                });
                return Err(Poisoned::error());
            }
            None => {
                self.report(DiagnosticKind::JumpOutsideLoop { keyword, at: span });
                self.body.invalid_jump = true;
                return Err(Poisoned::typed(Type::NEVER));
            }
        }
        Ok(tir::Stmt::Expr(tir::Expr {
            kind,
            ty: Type::NEVER,
            span,
        }))
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
            .position(|e| e.ty.is_counted(&self.lowered_enums));
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
        stmts: vec![tir::Stmt::Expr(expr(ExprKind::Break, Type::NEVER))],
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

/// The `keyword` that starts a loop statement at `span`.
fn keyword_span(span: Span, keyword: &str) -> Span {
    Span::new(span.start(), span.start() + keyword.len())
}
