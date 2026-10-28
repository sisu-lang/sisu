//! Operators through the prelude's interfaces. `==` and `!=` are `Eq`, and `<`, `<=`, `>` and `>=`
//! are `Ord`. This module makes the typed tree for them in one place. The checker calls it for an
//! operand type that it knows, and `mono` calls it again for an operand whose type a type argument
//! fixes.
//!
//! The caller chooses a [`Dispatch`]. A generated conformance keeps today's node: `Equal` and
//! `Compare`. A written one is a call of the conformance's method, with the spec's formulas:
//! `a != b` is `!a.eq(b)`, `a > b` is `b.less(a)`, `a <= b` is `!b.less(a)` and `a >= b` is
//! `!a.less(b)`. Operands run from left to right in every case.

use crate::source::Span;
use crate::tir::{BinaryOp, Block, UnaryOp};
use crate::tir::{CompareOp, Expr, ExprKind, FuncId, Local, LocalId, Stmt, Type, TypeArgs};

/// How an operator runs for one operand type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dispatch {
    /// The compiler writes the conformance: the node that the operator has always had.
    Generated,
    /// A written conformance: the call of its method `func`, with the type arguments `types` of
    /// the conformance's block.
    Method { func: FuncId, types: TypeArgs },
}

/// What the lowering of an operator may change: the locals of the function that holds it, for
/// the temporaries that keep the order of the operands.
pub(crate) struct Cx<'a> {
    pub(crate) locals: &'a mut Vec<Local>,
}

impl Cx<'_> {
    fn fresh(&mut self, ty: Type) -> LocalId {
        let id = LocalId(self.locals.len());
        self.locals.push(Local {
            name: None,
            ty,
            mutable: false,
        });
        id
    }
}

/// The node for `lhs == rhs`, or `lhs != rhs` when `negated`, of operands that share one type.
/// `span` is the span of the call that a written `eq` makes.
pub(crate) fn equal(
    dispatch: Dispatch,
    negated: bool,
    lhs: Box<Expr>,
    rhs: Box<Expr>,
    span: Span,
) -> ExprKind {
    let Dispatch::Method { func, types } = dispatch else {
        return ExprKind::Equal { negated, lhs, rhs };
    };
    let call = call(func, types, *lhs, *rhs, span);
    if negated {
        not(call, span).kind
    } else {
        call.kind
    }
}

/// The node for the comparison chain `operands[0] ops[0] operands[1] ...`, of operands that share
/// the type `ty`. A written `less` makes a call for each link. A chain of more than two operands
/// runs each operand once, in order, and a link that fails skips the operands after it.
pub(crate) fn ordered(
    cx: &mut Cx<'_>,
    dispatch: Dispatch,
    ty: Type,
    operands: Vec<Expr>,
    ops: &[CompareOp],
    span: Span,
) -> ExprKind {
    match dispatch {
        // Numbers compare in one node; `string`s do too when two compare.
        Dispatch::Generated if ty != Type::STRING || ops.len() == 1 => ExprKind::Compare {
            operands,
            ops: ops.to_vec(),
        },
        Dispatch::Generated => chain(
            cx,
            &|op, a, b| compare(op, a, b, span),
            operands,
            ops,
            ty,
            span,
        ),
        Dispatch::Method { func, types } => {
            let pair = |op, a, b| less(func, types, op, a, b, span);
            match <[Expr; 2]>::try_from(operands) {
                Ok([a, b]) => single(cx, &pair, ops[0], a, b, span),
                Err(operands) => chain(cx, &pair, operands, ops, ty, span),
            }
        }
    }
}

/// Builds the link `a op b` from two operands that are already read.
type Pair<'a> = &'a dyn Fn(CompareOp, Expr, Expr) -> Expr;

/// One link with two operands. A formula that swaps them (`>` and `<=`) binds both to
/// temporaries first, in source order, unless both are locals: a swapped call would run the right
/// operand first.
fn single(
    cx: &mut Cx<'_>,
    pair: Pair<'_>,
    op: CompareOp,
    a: Expr,
    b: Expr,
    span: Span,
) -> ExprKind {
    let swapped = matches!(op, CompareOp::Gt | CompareOp::Le);
    let both_read = matches!(a.kind, ExprKind::Local(_)) && matches!(b.kind, ExprKind::Local(_));
    if !swapped || both_read {
        return pair(op, a, b).kind;
    }
    let (ty_a, ty_b) = (a.ty, b.ty);
    let (bind_a, bind_b) = (cx.fresh(ty_a), cx.fresh(ty_b));
    let stmts = vec![
        Stmt::Let {
            local: bind_a,
            init: a,
        },
        Stmt::Let {
            local: bind_b,
            init: b,
        },
    ];
    let link = pair(op, read(bind_a, ty_a, span), read(bind_b, ty_b, span));
    block(stmts, link, span).kind
}

/// The chain over three operands or more, as nested blocks: operand `i` is bound inside the block
/// that follows the link `i - 2`, so a link that fails skips the operands after it. `a < b < c` is
/// `(block (let #0 a) (let #1 b) (&& (< #0 #1) (block (let #2 c) (< #1 #2))))`.
fn chain(
    cx: &mut Cx<'_>,
    pair: Pair<'_>,
    operands: Vec<Expr>,
    ops: &[CompareOp],
    ty: Type,
    span: Span,
) -> ExprKind {
    let mut lets = Vec::new();
    let reads: Vec<(LocalId, Span)> = operands
        .into_iter()
        .map(|init| {
            let bind = cx.fresh(ty);
            let at = init.span;
            lets.push(Stmt::Let { local: bind, init });
            (bind, at)
        })
        .collect();
    let read_at = |i: usize| read(reads[i].0, ty, reads[i].1);
    let link = |i: usize| pair(ops[i], read_at(i), read_at(i + 1));
    // From the innermost block out: each binds the right operand of the link it tests.
    let mut rest = link(ops.len() - 1);
    for i in (1..ops.len()).rev() {
        let bind = lets.pop().expect("each operand should have its `let`");
        let inner = block(vec![bind], rest, span);
        rest = and(link(i - 1), inner, span);
    }
    block(lets, rest, span).kind
}

/// The comparison `a op b` of two operands in one `Compare`.
fn compare(op: CompareOp, a: Expr, b: Expr, span: Span) -> Expr {
    Expr {
        kind: ExprKind::Compare {
            operands: vec![a, b],
            ops: vec![op],
        },
        ty: Type::BOOL,
        span,
    }
}

/// The link `a op b` through a written `less`.
fn less(func: FuncId, types: TypeArgs, op: CompareOp, a: Expr, b: Expr, span: Span) -> Expr {
    match op {
        CompareOp::Lt => call(func, types, a, b, span),
        CompareOp::Gt => call(func, types, b, a, span),
        CompareOp::Le => {
            let swapped = call(func, types, b, a, span);
            not(swapped, span)
        }
        CompareOp::Ge => {
            let direct = call(func, types, a, b, span);
            not(direct, span)
        }
    }
}

/// `receiver.method(argument)` for a method of a conformance, of type `bool`.
fn call(func: FuncId, types: TypeArgs, receiver: Expr, argument: Expr, span: Span) -> Expr {
    Expr {
        kind: ExprKind::Call {
            func,
            types,
            args: vec![receiver, argument],
        },
        ty: Type::BOOL,
        span,
    }
}

fn not(operand: Expr, span: Span) -> Expr {
    Expr {
        kind: ExprKind::Unary {
            op: UnaryOp::Not,
            operand: Box::new(operand),
        },
        ty: Type::BOOL,
        span,
    }
}

fn and(lhs: Expr, rhs: Expr, span: Span) -> Expr {
    Expr {
        kind: ExprKind::Binary {
            op: BinaryOp::And,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        },
        ty: Type::BOOL,
        span,
    }
}

fn read(local: LocalId, ty: Type, span: Span) -> Expr {
    Expr {
        kind: ExprKind::Local(local),
        ty,
        span,
    }
}

/// A block of `stmts` and then `value`, of type `bool`.
fn block(stmts: Vec<Stmt>, value: Expr, span: Span) -> Expr {
    Expr {
        kind: ExprKind::Block(Block {
            stmts,
            value: Some(Box::new(value)),
            ty: Type::BOOL,
        }),
        ty: Type::BOOL,
        span,
    }
}
