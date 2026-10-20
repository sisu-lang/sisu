//! Elaboration: each scope release (`release? x`) is decided by the set of states `S` that
//! reaches it, which the certifier's dataflow gives (`certify::scope_release_states`).
//!
//! | `S` | Becomes | Why |
//! | --- | --- | --- |
//! | a subset of {empty} | removed | the local owns nothing on any path |
//! | exactly {owned} | `release x` | owned on every path, so nothing to test |
//! | anything else | kept as `release? x` | conditional, or a violation |
//!
//! {owned, empty} is the conditional case. A set that holds borrowed or ended (ended includes a
//! value already released) is a rule-1 violation; it is left alone so that the debug certifier
//! reports it, never hidden by a removal.
//!
//! A release that the dataflow does not decide stays: one of a local whose region breaks a
//! rule, of every local of a function that breaks rule 6, and one in an unreachable block.
//!
//! Two facts make a kept `release? x` sound:
//!
//! 1. Every path into it that carries empty ends in one of three ways, and each leaves a value
//!    that a release skips: a move of `x`, which stores zero because `x` is flagged (a
//!    `release?` of it is left); the `None` edge of a presence branch, where the slot holds
//!    `None`; or entry with `x` never assigned, which lowering never lets reach a `release?`.
//! 2. A released value never reaches it: a release leaves the local ended, which the certifier
//!    rejects at a scope release, so a slot that a static `release x` leaves dangling is never
//!    tested.

use super::certify::{self, EMPTY, OWNED};
use super::{BlockId, Function, Statement};

/// Whether each local of `function` is flagged, indexed by `LocalId`: a `release?` still names
/// it, so a move of it must leave the zero value in its slot.
pub(crate) fn flagged(function: &Function) -> Vec<bool> {
    let mut flagged = vec![false; function.locals.len()];
    for stmt in function.blocks.iter().flat_map(|block| &block.stmts) {
        if let Statement::ScopeRelease(local) = stmt {
            flagged[local.0] = true;
        }
    }
    flagged
}

/// Removes each scope release of `function` that releases nothing, and makes each one of an
/// owned local a plain release.
pub(super) fn elaborate(function: &mut Function) {
    let states = certify::scope_release_states(function, &flagged(function));
    for (b, block) in function.blocks.iter_mut().enumerate() {
        block.stmts = std::mem::take(&mut block.stmts)
            .into_iter()
            .enumerate()
            .filter_map(|(at, stmt)| match stmt {
                Statement::ScopeRelease(local) => match states.get(&(BlockId(b), at)) {
                    Some(&set) if set & !EMPTY == 0 => None,
                    Some(&OWNED) => Some(Statement::Release(local)),
                    Some(_) | None => Some(stmt),
                },
                Statement::Assign { .. }
                | Statement::Take { .. }
                | Statement::Store { .. }
                | Statement::Release(_)
                | Statement::MakeUnique(_)
                | Statement::Check(_)
                | Statement::End(_) => Some(stmt),
            })
            .collect();
    }
}
