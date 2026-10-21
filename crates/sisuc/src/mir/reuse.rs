//! The reuse pass: each boxed `F` field of a construction takes a token of `F` that every path
//! into the construction holds, and its box is built into the token's block (ADR 0030).
//!
//! It is one forward walk in reverse post-order. A block's tokens on entry are the intersection
//! of its predecessors' tokens on exit. A block with a predecessor that is not earlier in the
//! order, a back edge, starts with none, so a construction in a loop never builds into a token
//! twice. `take_apart` adds its tokens, `free t` removes `t`, and a construction removes each
//! token it takes. The `free` of a token that a construction took on every path into it frees
//! nothing, and the elaboration removes it.
//!
//! Each set is a sorted vector of at most `MAX_OPEN` tokens. A `take_apart` that would make one
//! larger drops the lowest ids, the outermost, which are freed as any token no construction
//! takes. Dropping a token costs reuse, never soundness, and bounds the walk by the blocks times
//! `MAX_OPEN`, however deep the arms nest.

use super::{BlockId, Function, LocalId, Rvalue, Statement};
use crate::tir::{self, EnumId, TypeKind};

/// The most tokens a set holds.
pub(super) const MAX_OPEN: usize = 16;

/// Gives each construction of `function` the tokens its boxed fields take.
pub(super) fn reuse(function: &mut Function, program: &tir::Program) {
    for (b, mut set) in available(function, program).into_iter().enumerate() {
        for at in 0..function.blocks[b].stmts.len() {
            let taken = step(&mut set, &function.blocks[b].stmts[at], function, program);
            if let (
                Some(taken),
                Statement::Assign {
                    value: Rvalue::Variant { reuse, .. },
                    ..
                },
            ) = (taken, &mut function.blocks[b].stmts[at])
            {
                *reuse = taken;
            }
        }
    }
}

/// The tokens each block of `function` holds on entry, indexed by block, each sorted. A block no
/// path from the entry reaches holds none.
pub(super) fn available(function: &Function, program: &tir::Program) -> Vec<Vec<LocalId>> {
    let blocks = function.blocks.len();
    let order = reverse_post_order(function);
    // An unreachable predecessor never runs, so it holds back nothing.
    let mut preds = vec![Vec::new(); blocks];
    for &block in &order {
        for to in function.blocks[block.0].term.successors() {
            preds[to.0].push(block.0);
        }
    }
    let mut entry = vec![Vec::new(); blocks];
    let mut exit: Vec<Vec<LocalId>> = vec![Vec::new(); blocks];
    for &block in &order {
        // A back-edge predecessor comes later in the order, so its exit is still empty here, and
        // so is the intersection.
        let mut set = match preds[block.0].split_first() {
            Some((&first, rest)) => {
                let mut set = exit[first].clone();
                for &p in rest {
                    set.retain(|token| exit[p].binary_search_by_key(&token.0, |t| t.0).is_ok());
                }
                set
            }
            None => Vec::new(),
        };
        entry[block.0].clone_from(&set);
        for stmt in &function.blocks[block.0].stmts {
            step(&mut set, stmt, function, program);
        }
        exit[block.0] = set;
    }
    entry
}

/// Applies `stmt` to `set`: for a construction, the token each of its fields takes.
fn step(
    set: &mut Vec<LocalId>,
    stmt: &Statement,
    function: &Function,
    program: &tir::Program,
) -> Option<Vec<Option<LocalId>>> {
    match stmt {
        Statement::Assign {
            value: Rvalue::Variant { id, variant, .. },
            ..
        } => return Some(take(set, function, program, *id, *variant)),
        Statement::TakeApart { tokens, .. } => {
            set.extend(tokens.iter().map(|(token, _)| *token));
            set.sort_unstable_by_key(|token| token.0);
            set.dedup();
            set.drain(..set.len().saturating_sub(MAX_OPEN));
        }
        Statement::Free(token) => set.retain(|t| t != token),
        Statement::Assign { .. }
        | Statement::Take { .. }
        | Statement::Store { .. }
        | Statement::Release(_)
        | Statement::ScopeRelease(_)
        | Statement::MakeUnique(_)
        | Statement::Check(_)
        | Statement::End(_) => {}
    }
    None
}

/// The token each field of variant `variant` of enum `id` takes from `set`, which loses them: a
/// boxed `F` field takes the token of `F` with the lowest id. A field held inline and a boxed
/// `F?` take none.
fn take(
    set: &mut Vec<LocalId>,
    function: &Function,
    program: &tir::Program,
    id: EnumId,
    variant: usize,
) -> Vec<Option<LocalId>> {
    program.enums[id.0].variants[variant]
        .fields
        .iter()
        .map(|field| {
            if !field.boxed || !matches!(field.ty.kind(), TypeKind::Enum(_)) {
                return None;
            }
            let at = set
                .iter()
                .position(|token| function.locals[token.0].ty == field.ty)?;
            Some(set.remove(at))
        })
        .collect()
}

/// The blocks that a path from the entry reaches, each before its successors except along a
/// back edge.
fn reverse_post_order(function: &Function) -> Vec<BlockId> {
    let mut seen = vec![false; function.blocks.len()];
    let mut order = Vec::with_capacity(function.blocks.len());
    seen[0] = true;
    let mut stack = vec![(BlockId(0), function.blocks[0].term.successors())];
    while let Some((block, successors)) = stack.last_mut() {
        let block = *block;
        match successors.next() {
            Some(next) if !seen[next.0] => {
                seen[next.0] = true;
                stack.push((next, function.blocks[next.0].term.successors()));
            }
            Some(_) => {}
            None => {
                order.push(block);
                stack.pop();
            }
        }
    }
    order.reverse();
    order
}
