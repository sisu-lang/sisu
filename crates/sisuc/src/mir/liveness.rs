//! Sparse per-local liveness: for each counted local, from each block where a read comes before
//! any write, walk back over predecessors and stop at a block that writes the local whole
//! (Boissinot et al.'s path exploration). Then one backward scan of the local's mentions
//! answers, for each statement that mentions it, whether it is live after that statement. The
//! cost is the blocks each local is live in, summed over locals, plus the mentions.
//!
//! A read through any projection uses its root, and so do `check`, `make_unique`, `take` and a
//! store through a place rooted at it: `f(a); a[0] = 1` keeps `a` live across the call. A
//! whole assignment writes the local. `end` and `release?` neither read nor write it.

use rustc_hash::FxHashMap;

use self::entries::{Mention, Stamps};

use super::{
    BlockId, Function, LocalId, Operand, Place, Projection, Rvalue, Statement, Terminator,
};

/// Whether each counted local is live after each statement that mentions it.
pub(super) struct Liveness {
    after: FxHashMap<(LocalId, BlockId, usize), bool>,
}

#[cfg(test)]
thread_local! {
    /// How many liveness entries have been read on this thread: a mention's field, a block's
    /// stamp or a stored answer. A search counts each entry it compares, a lookup one.
    pub(super) static CONSULTED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Counts one read of a liveness entry.
fn consulted() {
    #[cfg(test)]
    CONSULTED.set(CONSULTED.get() + 1);
}

/// The entries liveness searches. Their fields are private, so every read goes through a
/// counted accessor.
mod entries {
    use super::{BlockId, consulted};

    /// A statement that reads or writes a local. One that does both is noted twice, its read
    /// first, as an assignment evaluates its value before it writes its destination.
    #[derive(Clone, Copy)]
    pub(super) struct Mention {
        block: BlockId,
        at: usize,
        reads: bool,
    }

    impl Mention {
        pub(super) fn new(block: BlockId, at: usize, reads: bool) -> Self {
            Self { block, at, reads }
        }

        pub(super) fn block(self) -> BlockId {
            consulted();
            self.block
        }

        pub(super) fn at(self) -> usize {
            consulted();
            self.at
        }

        pub(super) fn reads(self) -> bool {
            consulted();
            self.reads
        }
    }

    /// Per block, the last local (as its index plus one) stamped there: each local's walk
    /// reads and writes only the blocks it reaches, and nothing is cleared between locals.
    pub(super) struct Stamps(Vec<usize>);

    impl Stamps {
        pub(super) fn new(blocks: usize) -> Self {
            Self(vec![0; blocks])
        }

        pub(super) fn has(&self, block: BlockId, stamp: usize) -> bool {
            consulted();
            self.0[block.0] == stamp
        }

        pub(super) fn set(&mut self, block: BlockId, stamp: usize) {
            self.0[block.0] = stamp;
        }
    }
}

impl Liveness {
    /// Whether a later statement can read `local`'s value after statement `at`, before
    /// anything writes it whole. Statement `at` (a block's length for its terminator) must
    /// mention `local`.
    pub(super) fn live_after(&self, local: LocalId, at: (BlockId, usize)) -> bool {
        consulted();
        *self
            .after
            .get(&(local, at.0, at.1))
            .expect("the statement should mention the local")
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
                mentions[local.0].push(Mention::new(block_id, at, reads));
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
    let mut mentioned = Stamps::new(function.blocks.len());
    let mut live_out = Stamps::new(function.blocks.len());
    let mut stack = Vec::new();
    let mut after = FxHashMap::default();
    for (local, list) in mentions.iter().enumerate() {
        let stamp = local + 1;
        let local = LocalId(local);
        for block in list.chunk_by(|a, b| a.block() == b.block()) {
            let first = block[0];
            mentioned.set(first.block(), stamp);
            // A read before any write: the local is live on entry to the block.
            if first.reads() {
                stack.push(first.block());
            }
        }
        while let Some(block) = stack.pop() {
            for &pred in &preds[block.0] {
                // A predecessor that mentions the local either read it first, and was
                // pushed above, or writes it before any read, which ends the walk.
                if !live_out.has(pred, stamp) {
                    live_out.set(pred, stamp);
                    if !mentioned.has(pred, stamp) {
                        stack.push(pred);
                    }
                }
            }
        }
        // Backward: a statement's answer is the next mentioning statement's read in its
        // block, or whether the local is live out of the block.
        let mut next: Option<Mention> = None;
        for statement in list
            .chunk_by(|a, b| (a.block(), a.at()) == (b.block(), b.at()))
            .rev()
        {
            let m = statement[0];
            let (block, at) = (m.block(), m.at());
            let live = match next {
                Some(n) if n.block() == block => n.reads(),
                _ => live_out.has(block, stamp),
            };
            after.insert((local, block, at), live);
            next = Some(m);
        }
    }
    Liveness { after }
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
