//! W0010: a function whose body cannot finish without calling itself. One walk over the typed
//! tree, in evaluation order, gives each node its `Flow`.

use crate::source::Span;
use crate::tir::{self, BinaryOp, ExprKind};

/// The self-calls that end a path, in source order, when every path through `function`, whose
/// id is `id`, calls it before it can return; `None` otherwise.
pub(super) fn unconditional(function: &tir::Function, id: tir::FuncId) -> Option<Vec<Span>> {
    let mut calls = Vec::new();
    let flow = Walk { id }.block(&function.body, true, &mut calls);
    flow.recurses.then(|| {
        calls.sort_by_key(|span| span.start());
        calls
    })
}

/// What the paths through a node reach. Both are never true together.
#[derive(Clone, Copy)]
struct Flow {
    /// Every path reaches a self-call.
    recurses: bool,
    /// Some path reaches a way out before a self-call.
    escapes: bool,
}

impl Flow {
    /// A node that every path passes through, such as a literal.
    const PASS: Flow = Flow {
        recurses: false,
        escapes: false,
    };

    /// A way out of the function or of a loop.
    const ESCAPE: Flow = Flow {
        recurses: false,
        escapes: true,
    };

    /// A self-call.
    const RECURSE: Flow = Flow {
        recurses: true,
        escapes: false,
    };

    /// `self`, then `next`.
    fn then(self, next: Flow) -> Flow {
        Flow {
            recurses: self.recurses || (!self.escapes && next.recurses),
            escapes: self.escapes || (!self.recurses && next.escapes),
        }
    }

    /// `self` or `other`, whichever runs.
    fn or(self, other: Flow) -> Flow {
        Flow {
            recurses: self.recurses && other.recurses,
            escapes: self.escapes || other.escapes,
        }
    }
}

struct Walk {
    id: tir::FuncId,
}

impl Walk {
    /// `open`: some path reaches `e` without a self-call. A self-call that `open` reaches is
    /// pushed to `calls`.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per node kind, with no `_` arm, so a new node fails the build here"
    )]
    fn walk(&self, e: &tir::Expr, open: bool, calls: &mut Vec<Span>) -> Flow {
        match &e.kind {
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Char(_)
            | ExprKind::Str(_)
            | ExprKind::None
            | ExprKind::Local(_)
            | ExprKind::ReadLine => Flow::PASS,
            ExprKind::Call { func, args, .. } if *func == self.id => {
                let flow = self.all(args, open, calls);
                if open && !flow.recurses {
                    calls.push(e.span);
                }
                flow.then(Flow::RECURSE)
            }
            ExprKind::Call { args, .. }
            | ExprKind::New { args, .. }
            | ExprKind::Variant { args, .. }
            | ExprKind::Math { operands: args, .. }
            | ExprKind::ArrayLit(args)
            | ExprKind::Interpolate { parts: args, .. } => self.all(args, open, calls),
            ExprKind::Field { base: operand, .. }
            | ExprKind::Wrap(operand)
            | ExprKind::Convert(operand)
            | ExprKind::Print { operand, .. }
            | ExprKind::Len(operand)
            | ExprKind::Chars(operand)
            | ExprKind::Bytes(operand)
            | ExprKind::Words(operand)
            | ExprKind::FromChars(operand)
            | ExprKind::Sum(operand)
            | ExprKind::Unary { operand, .. } => self.walk(operand, open, calls),
            ExprKind::Binary {
                op: BinaryOp::And | BinaryOp::Or,
                lhs,
                rhs,
            } => {
                let first = self.walk(lhs, open, calls);
                let second = self.walk(rhs, open && !first.recurses, calls);
                first.then(second.or(Flow::PASS))
            }
            ExprKind::Binary { lhs, rhs, .. }
            | ExprKind::Concat { lhs, rhs }
            | ExprKind::Equal { lhs, rhs, .. }
            | ExprKind::Is { lhs, rhs }
            | ExprKind::ArrayFill {
                value: lhs,
                len: rhs,
            }
            | ExprKind::Index {
                base: lhs,
                index: rhs,
            }
            | ExprKind::Contains {
                base: lhs,
                value: rhs,
            } => self.all([&**lhs, &**rhs], open, calls),
            // Each link after the first may stop the chain, so the third and later operands may
            // not run.
            ExprKind::Compare { operands, .. } => {
                let (first, later) = operands.split_at(operands.len().min(2));
                let mut flow = self.all(first, open, calls);
                for operand in later {
                    let next = self.walk(operand, open && !flow.recurses, calls);
                    flow = flow.then(next.or(Flow::PASS));
                }
                flow
            }
            ExprKind::If {
                cond: scrutinee,
                then_block,
                else_block,
            }
            | ExprKind::IfSome {
                scrutinee,
                then_block,
                else_block,
                ..
            } => {
                let first = self.walk(scrutinee, open, calls);
                let open = open && !first.recurses;
                let then = self.block(then_block, open, calls);
                let other = else_block
                    .as_ref()
                    .map_or(Flow::PASS, |block| self.block(block, open, calls));
                first.then(then.or(other))
            }
            ExprKind::Match {
                scrutinee, arms, ..
            } => {
                let first = self.walk(scrutinee, open, calls);
                let open = open && !first.recurses;
                // The arms are tried in order. `called`: every path that goes on to the next arm
                // has called the function in a guard. W0006 marks each arm that no path reaches,
                // such as every arm after `_`, and an `if let` that always matches is an error.
                let mut called = false;
                let arms = arms.iter().filter(|arm| arm.reachable).map(|arm| {
                    let reached = if called { Flow::RECURSE } else { Flow::PASS };
                    let open = open && !called;
                    let guard = arm
                        .guard
                        .as_ref()
                        .map_or(Flow::PASS, |guard| self.walk(guard, open, calls));
                    let body = self.block(&arm.body, open && !guard.recurses, calls);
                    // A path that a pattern can miss skips the guard.
                    called |= guard.recurses && always_matches(&arm.pattern);
                    reached.then(guard).then(body)
                });
                first.then(arms.reduce(Flow::or).unwrap_or(Flow::PASS))
            }
            ExprKind::Loop(body) => {
                let body = self.block(body, open, calls);
                Flow {
                    recurses: body.recurses,
                    escapes: !body.recurses,
                }
            }
            ExprKind::Panic(operand)
            | ExprKind::Break {
                value: Some(operand),
                ..
            }
            | ExprKind::Return(Some(operand)) => self.walk(operand, open, calls).then(Flow::ESCAPE),
            ExprKind::Break { value: None, .. }
            | ExprKind::Continue { .. }
            | ExprKind::Return(None) => Flow::ESCAPE,
            ExprKind::Block(block) => self.block(block, open, calls),
        }
    }

    /// `exprs`, in sequence.
    fn all<'e>(
        &self,
        exprs: impl IntoIterator<Item = &'e tir::Expr>,
        open: bool,
        calls: &mut Vec<Span>,
    ) -> Flow {
        exprs.into_iter().fold(Flow::PASS, |flow, e| {
            flow.then(self.walk(e, open && !flow.recurses, calls))
        })
    }

    /// The block's statements, then its value, in sequence.
    fn block(&self, block: &tir::Block, open: bool, calls: &mut Vec<Span>) -> Flow {
        let mut flow = Flow::PASS;
        for stmt in &block.stmts {
            let open = open && !flow.recurses;
            flow = flow.then(match stmt {
                tir::Stmt::Let { init: e, .. } | tir::Stmt::Expr(e) => self.walk(e, open, calls),
                // The place's operands run before the value, as in MIR.
                tir::Stmt::Assign { place, value } | tir::Stmt::Append { place, value } => {
                    let place = self.place(place, open, calls);
                    place.then(self.walk(value, open && !place.recurses, calls))
                }
            });
        }
        match &block.value {
            Some(value) => flow.then(self.walk(value, open && !flow.recurses, calls)),
            None => flow,
        }
    }

    /// The place's operands, from its root outwards.
    fn place(&self, place: &tir::Place, open: bool, calls: &mut Vec<Span>) -> Flow {
        match place {
            tir::Place::Local(_) => Flow::PASS,
            tir::Place::Field { base, .. } => self.walk(base, open, calls),
            tir::Place::Index { base, index } => {
                let base = self.place(base, open, calls);
                base.then(self.walk(index, open && !base.recurses, calls))
            }
        }
    }
}

/// Whether `pattern` matches every value of its type. `false` when it cannot tell, such as for a
/// variant of an enum that has only one, so that a path may skip the arm.
fn always_matches(pattern: &tir::Pattern) -> bool {
    match &pattern.kind {
        tir::PatternKind::Wild | tir::PatternKind::Bind(_) => true,
        tir::PatternKind::Or(alternatives) => alternatives.iter().any(always_matches),
        tir::PatternKind::Variant { .. }
        | tir::PatternKind::Bool(_)
        | tir::PatternKind::Range { .. }
        | tir::PatternKind::Str(_)
        | tir::PatternKind::Some(_)
        | tir::PatternKind::None => false,
    }
}
