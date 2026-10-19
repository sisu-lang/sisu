//! Sparse per-local liveness: for each counted local, from each block where a read comes before
//! any write, walk back over predecessors and stop at a block that writes the local whole
//! (Boissinot et al.'s path exploration). No set is built per block: the cost is the blocks
//! each local is live in, summed over locals, plus its mentions.
//!
//! A read through any projection uses its root, and so do `check`, `make_unique`, `take` and a
//! store through a place rooted at it: `f(a); a[0] = 1` keeps `a` live across the call. A
//! whole assignment writes the local. `end` and `release?` neither read nor write it.

use rustc_hash::FxHashSet;

use super::{
    BlockId, Function, LocalId, Operand, Place, Projection, Rvalue, Statement, Terminator,
};

/// Where each counted local is live.
pub(super) struct Liveness {
    /// Each counted local's statements that read or write it, in block and then statement
    /// order; a block's terminator is at the block's length.
    mentions: Vec<Vec<Mention>>,
    /// The locals live at the end of each block they are live out of.
    live_out: FxHashSet<(LocalId, BlockId)>,
}

/// A statement that reads or writes a local. One that does both is noted twice, its read
/// first, as an assignment evaluates its value before it writes its destination.
#[derive(Clone, Copy)]
struct Mention {
    block: BlockId,
    at: usize,
    reads: bool,
}

impl Liveness {
    /// Whether a later statement can read `local`'s value after statement `at`, before
    /// anything writes it whole.
    pub(super) fn live_after(&self, local: LocalId, at: (BlockId, usize)) -> bool {
        let mentions = &self.mentions[local.0];
        let next = mentions.partition_point(|m| (m.block.0, m.at) <= (at.0.0, at.1));
        match mentions.get(next) {
            Some(m) if m.block == at.0 => m.reads,
            _ => self.live_out.contains(&(local, at.0)),
        }
    }
}

/// The liveness of every counted local of `function`.
pub(super) fn liveness(function: &Function) -> Liveness {
    let mut mentions = vec![Vec::new(); function.locals.len()];
    let mut preds = vec![Vec::new(); function.blocks.len()];
    for (b, block) in function.blocks.iter().enumerate() {
        let block_id = BlockId(b);
        let mut note = |at: usize, local: LocalId, reads: bool| {
            if function.locals[local.0].counted {
                mentions[local.0].push(Mention {
                    block: block_id,
                    at,
                    reads,
                });
            }
        };
        for (at, stmt) in block.stmts.iter().enumerate() {
            statement(stmt, &mut |local| note(at, local, true));
            if let Some(dest) = written(stmt) {
                note(at, dest, false);
            }
        }
        terminator(&block.term, &mut |local| {
            note(block.stmts.len(), local, true);
        });
        for to in block.term.successors() {
            preds[to.0].push(block_id);
        }
    }
    let mut live_out = FxHashSet::default();
    let mut stack = Vec::new();
    for (local, list) in mentions.iter().enumerate() {
        let local = LocalId(local);
        // The blocks whose first mention reads: the local is live on entry to them.
        stack.extend(
            list.iter()
                .enumerate()
                .filter(|&(i, m)| m.reads && (i == 0 || list[i - 1].block != m.block))
                .map(|(_, m)| m.block),
        );
        while let Some(block) = stack.pop() {
            for &pred in &preds[block.0] {
                // A predecessor that mentions the local either read it first, and was
                // pushed above, or writes it before any read, which ends the walk.
                let mentioned = list.binary_search_by_key(&pred.0, |m| m.block.0).is_ok();
                if live_out.insert((local, pred)) && !mentioned {
                    stack.push(pred);
                }
            }
        }
    }
    Liveness { mentions, live_out }
}

/// The local `stmt` writes whole, if any.
fn written(stmt: &Statement) -> Option<LocalId> {
    match stmt {
        Statement::Assign { dest, .. } => *dest,
        Statement::Take { dest, .. } => Some(*dest),
        Statement::Store { .. }
        | Statement::Release(_)
        | Statement::ScopeRelease(_)
        | Statement::MakeUnique(_)
        | Statement::Check(_)
        | Statement::End(_) => None,
    }
}

/// Calls `read` with each local `stmt` reads.
fn statement(stmt: &Statement, read: &mut impl FnMut(LocalId)) {
    match stmt {
        Statement::Assign { value, .. } => rvalue(value, read),
        Statement::Take { place, .. } | Statement::MakeUnique(place) | Statement::Check(place) => {
            self::place(place, read);
        }
        Statement::Store { place, value } => {
            self::place(place, read);
            operand(value, read);
        }
        // A release consumes the value, so it reads it.
        Statement::Release(local) => read(*local),
        Statement::ScopeRelease(_) | Statement::End(_) => {}
    }
}

fn rvalue(value: &Rvalue, read: &mut impl FnMut(LocalId)) {
    match value {
        Rvalue::Use(op)
        | Rvalue::Unwrap(op)
        | Rvalue::Wrap(op)
        | Rvalue::Chars(op)
        | Rvalue::Bytes(op)
        | Rvalue::Words(op)
        | Rvalue::FromChars(op)
        | Rvalue::Len(op)
        | Rvalue::LenWords(op)
        | Rvalue::Sum { base: op, .. }
        | Rvalue::Convert(op)
        | Rvalue::Unary { operand: op, .. }
        | Rvalue::Print { operand: op, .. } => operand(op, read),
        Rvalue::Copy(place) | Rvalue::Borrow(place) => self::place(place, read),
        Rvalue::CopyStr(_) | Rvalue::ReadLine => {}
        Rvalue::Call { args, .. }
        | Rvalue::New { args, .. }
        | Rvalue::Variant { args, .. }
        | Rvalue::ArrayLit(args)
        | Rvalue::Interpolate { parts: args, .. }
        | Rvalue::Math { operands: args, .. } => {
            for op in args {
                operand(op, read);
            }
        }
        Rvalue::ArrayFill {
            value: lhs,
            len: rhs,
            ..
        }
        | Rvalue::Concat { lhs, rhs }
        | Rvalue::Contains {
            base: lhs,
            value: rhs,
        }
        | Rvalue::Binary { lhs, rhs, .. }
        | Rvalue::Compare { lhs, rhs, .. }
        | Rvalue::Equal { lhs, rhs, .. }
        | Rvalue::Is { lhs, rhs } => {
            operand(lhs, read);
            operand(rhs, read);
        }
    }
}

fn terminator(term: &Terminator, read: &mut impl FnMut(LocalId)) {
    match term {
        Terminator::Branch { cond: op, .. }
        | Terminator::SwitchValue { operand: op, .. }
        | Terminator::Return(Some(op))
        | Terminator::Panic { message: op, .. } => operand(op, read),
        Terminator::IfPresent { optional, .. } => read(*optional),
        Terminator::SwitchTag { place, .. } => self::place(place, read),
        Terminator::Goto(_) | Terminator::Return(None) | Terminator::Unreachable => {}
    }
}

fn operand(op: &Operand, read: &mut impl FnMut(LocalId)) {
    match op {
        Operand::Read(place) => self::place(place, read),
        Operand::Move(local) | Operand::Use(local) | Operand::Guarded(local) => read(*local),
        Operand::Const(_) => {}
    }
}

/// The root and every index operand of `place`.
fn place(place: &Place, read: &mut impl FnMut(LocalId)) {
    read(place.local);
    for projection in &place.projections {
        if let Projection::Index { index, .. } = projection {
            operand(index, read);
        }
    }
}
