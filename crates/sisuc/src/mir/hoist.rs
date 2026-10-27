//! ADR 0021's hoist on mir: a loop that writes the elements of a local array, and never
//! shares or replaces it, makes it unique once at its pre-header, so its writes skip the
//! count check at the root.
//!
//! Only a retain raises a block's count, and every retain in mir is a `copy`, so a read in
//! place never bars a local. That covers all six borrow positions, where `codegen/hoist.rs`
//! covered only an `Index` base and a `Len` operand.
//!
//! The pass reads each block in a loop once, to list what the block writes and bars. A local
//! barred in a loop is barred in every loop around it, so the loops are solved bottom up: each
//! loop's states start from its largest inner loop's, and the smaller ones merge into them.
//! Then one pass drops the `make_unique` statements that leave each block, and one appends
//! them to the pre-headers.

use self::seam::{Blocks, BlocksMut, Roots, State, States};
use super::{Function, LocalId, Loop, Operand, Place, Rvalue, Statement, Terminator};

/// The locals that `lp` can make unique before it starts, sorted. A local `L` qualifies when
/// the loop's blocks, its nested loops' included, both:
///
/// - hold a level-0 `make_unique L`: the root of an element write to `L`;
/// - never copy `L` whole, move it, release it or assign it whole (a `let` or a binding
///   included). A read through a place rooted at `L`, a `take` and a store do not bar it, and
///   neither does a scope release, which only an edge leaving the loop runs for `L`.
#[cfg(test)]
pub(super) fn unique_roots(function: &Function, lp: &Loop) -> Vec<LocalId> {
    let top = function
        .loops
        .iter()
        .position(|other| other.header == lp.header)
        .expect("`lp` should be one of the function's loops");
    let plan = plan(&Blocks::new(&function.blocks), &function.loops, Some(top));
    let mut roots: Vec<_> = plan
        .arrive
        .into_iter()
        .filter(|&(at, _)| at == top)
        .map(|(_, local)| local)
        .collect();
    roots.sort_unstable_by_key(|local| local.0);
    roots
}

/// Moves each loop's level-0 `make_unique` of its `unique_roots` out of its blocks, to the end
/// of the pre-header of the outermost loop around it that still qualifies.
pub(super) fn hoist(function: &mut Function) {
    let Plan {
        innermost,
        leave,
        arrive,
    } = plan(&Blocks::new(&function.blocks), &function.loops, None);
    let mut blocks = BlocksMut::new(&mut function.blocks);
    for (block, lp) in innermost.iter().enumerate() {
        let Some(lp) = *lp else { continue };
        blocks.retain(block, &leave[lp]);
    }
    let mut arrive: Vec<(usize, LocalId)> = arrive
        .into_iter()
        .map(|(lp, local)| (function.loops[lp].pre_header.0, local))
        .collect();
    arrive.sort_unstable_by_key(|&(block, local)| (block, local.0));
    for group in arrive.chunk_by(|a, b| a.0 == b.0) {
        blocks.append(
            group[0].0,
            group.iter().map(|&(_, local)| {
                Statement::MakeUnique(Place {
                    local,
                    projections: Vec::new(),
                    borrow: false,
                    guard: false,
                })
            }),
        );
    }
}

/// Where the hoist moves each `make_unique`.
struct Plan {
    /// Each block's innermost loop.
    innermost: Vec<Option<usize>>,
    /// Per loop, the locals whose level-0 `make_unique` leaves the blocks it is innermost to.
    leave: Vec<Roots>,
    /// `(loop, local)`: a `make_unique` of `local` goes to the end of the loop's pre-header.
    arrive: Vec<(usize, LocalId)>,
}

/// The hoist's plan for `loops`, or with `top`, for that loop and the loops inside it, as if
/// no loop were around it.
fn plan(blocks: &Blocks<'_>, loops: &[Loop], top: Option<usize>) -> Plan {
    let nest = Nest::new(blocks.len(), loops, top);
    let mut own: Vec<Walk> = (0..loops.len()).map(|_| Walk::default()).collect();
    for (block, lp) in nest.innermost.iter().enumerate() {
        let Some(lp) = *lp else { continue };
        if nest.range.contains(&lp) {
            blocks.walk(block, &mut own[lp]);
        }
    }
    let (leave, arrive) = solve(&nest, &own);
    Plan {
        innermost: nest.innermost,
        leave,
        arrive,
    }
}

/// How the loops nest.
struct Nest {
    /// Each block's innermost loop.
    innermost: Vec<Option<usize>>,
    /// The loops solved: all of them, or `top` and the loops inside it.
    range: std::ops::Range<usize>,
    /// Each solved loop's loops directly inside it.
    children: Vec<Vec<usize>>,
    /// Whether each solved loop is outermost among them.
    outermost: Vec<bool>,
}

impl Nest {
    fn new(blocks: usize, loops: &[Loop], top: Option<usize>) -> Self {
        let mut innermost = vec![None; blocks];
        let mut parent = vec![None; loops.len()];
        for (i, lp) in loops.iter().enumerate() {
            // Pre-order: the loop around this one has written its blocks, this one not yet.
            parent[i] = innermost[lp.header.0];
            for block in &lp.blocks {
                innermost[block.0] = Some(i);
            }
        }
        let range = top.map_or(0..loops.len(), |top| {
            // In pre-order the loops inside `top` follow it, deeper than it.
            let mut depth = vec![0; loops.len()];
            for lp in 0..loops.len() {
                depth[lp] = parent[lp].map_or(0, |up: usize| depth[up] + 1);
            }
            let end = (top + 1..loops.len())
                .find(|&lp| depth[lp] <= depth[top])
                .unwrap_or(loops.len());
            top..end
        });
        let outermost: Vec<bool> = (0..loops.len())
            .map(|lp| Some(lp) == top || parent[lp].is_none())
            .collect();
        let mut children = vec![Vec::new(); loops.len()];
        for lp in range.clone() {
            if let Some(up) = parent[lp].filter(|_| !outermost[lp]) {
                children[up].push(lp);
            }
        }
        Nest {
            innermost,
            range,
            children,
            outermost,
        }
    }
}

/// Solves the loops of `nest`, inner loops first, from what each loop's own blocks write and
/// bar. A loop's `States` hold, for each local its blocks write or bar, whether it bars it.
/// A local a loop writes and does not bar arrives at the loop's pre-header, unless the loop
/// around it does not bar it either. Gives `Plan::leave` and `Plan::arrive`.
fn solve(nest: &Nest, own: &[Walk]) -> (Vec<Roots>, Vec<(usize, LocalId)>) {
    let mut states: Vec<States> = own.iter().map(|_| States::default()).collect();
    let mut leave: Vec<Roots> = own.iter().map(|_| Roots::default()).collect();
    let mut arrive = Vec::new();
    for lp in nest.range.clone().rev() {
        let mut inner: Vec<(usize, States)> = nest.children[lp]
            .iter()
            .map(|&child| (child, std::mem::take(&mut states[child])))
            .collect();
        // The largest inner loop's states become this loop's, so each entry moves only when
        // it joins a set at least twice as large.
        let largest = (0..inner.len()).max_by_key(|&i| inner[i].1.len());
        let (big, mut merged) = largest.map_or_else(
            || (None, States::default()),
            |i| {
                let (child, states) = inner.swap_remove(i);
                (Some(child), states)
            },
        );
        // Every bar first, so that an `Eligible` left in `merged` is eligible in this loop.
        let bars = own[lp].barred.iter().copied().chain(
            inner
                .iter()
                .flat_map(|(_, states)| states.iter())
                .filter(|&(_, state)| state == State::Barred)
                .map(|(local, _)| local),
        );
        for local in bars {
            if merged.insert(local, State::Barred) == Some(State::Eligible) {
                arrive.extend(big.map(|big| (big, local)));
            }
        }
        for (child, states) in &inner {
            for (local, state) in states.iter() {
                if state == State::Eligible {
                    match merged.get(local) {
                        Some(State::Barred) => arrive.push((*child, local)),
                        Some(State::Eligible) => {}
                        None => {
                            merged.insert(local, State::Eligible);
                        }
                    }
                }
            }
        }
        for &local in &own[lp].written {
            if merged.get(local) != Some(State::Barred) {
                merged.insert(local, State::Eligible);
                leave[lp].insert(local);
            }
        }
        if nest.outermost[lp] {
            arrive.extend(
                merged
                    .iter()
                    .filter(|&(_, state)| state == State::Eligible)
                    .map(|(local, _)| (lp, local)),
            );
        } else {
            states[lp] = merged;
        }
    }
    (leave, arrive)
}

/// The local that `stmt` makes unique whole, if it is a `make_unique` with no projection.
fn level_0(stmt: &Statement) -> Option<LocalId> {
    match stmt {
        Statement::MakeUnique(place) if place.projections.is_empty() => Some(place.local),
        Statement::MakeUnique(_)
        | Statement::Assign { .. }
        | Statement::Take { .. }
        | Statement::Store { .. }
        | Statement::Release(_)
        | Statement::ScopeRelease(_)
        | Statement::Check(_)
        | Statement::End(_)
        | Statement::TakeApart { .. }
        | Statement::Free(_) => None,
    }
}

/// The locals a loop's blocks make unique at level 0, and those they bar.
#[derive(Default)]
struct Walk {
    written: Vec<LocalId>,
    barred: Vec<LocalId>,
}

impl Walk {
    fn stmt(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Assign { dest, value } => {
                self.barred.extend(*dest);
                self.rvalue(value);
            }
            Statement::Take { dest, .. } => {
                self.barred.push(*dest);
            }
            Statement::Store { value, .. } => self.operand(value),
            Statement::Release(local) | Statement::Free(local) => {
                self.barred.push(*local);
            }
            Statement::TakeApart {
                scrutinee,
                tokens,
                bindings,
                rest: _,
            } => {
                self.barred.push(*scrutinee);
                self.barred
                    .extend(tokens.iter().chain(bindings).map(|&(local, _)| local));
            }
            Statement::MakeUnique(_) => self.written.extend(level_0(stmt)),
            Statement::ScopeRelease(_) | Statement::Check(_) | Statement::End(_) => {}
        }
    }

    fn rvalue(&mut self, value: &Rvalue) {
        match value {
            Rvalue::Copy(place) if place.projections.is_empty() => {
                self.barred.push(place.local);
            }
            Rvalue::Copy(_) | Rvalue::CopyStr(_) | Rvalue::Borrow(_) | Rvalue::ReadLine => {}
            Rvalue::Use(operand)
            | Rvalue::Unwrap(operand)
            | Rvalue::Wrap(operand)
            | Rvalue::Chars(operand)
            | Rvalue::Bytes(operand)
            | Rvalue::Words(operand)
            | Rvalue::FromChars(operand)
            | Rvalue::Len(operand)
            | Rvalue::LenWords(operand)
            | Rvalue::Sum { base: operand, .. }
            | Rvalue::Convert(operand)
            | Rvalue::Unary { operand, .. }
            | Rvalue::Print { operand, .. } => self.operand(operand),
            Rvalue::Call { args: operands, .. }
            | Rvalue::New { args: operands, .. }
            | Rvalue::Variant { args: operands, .. }
            | Rvalue::ArrayLit(operands)
            | Rvalue::Interpolate {
                parts: operands, ..
            }
            | Rvalue::Math { operands, .. } => {
                for operand in operands {
                    self.operand(operand);
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
                self.operand(lhs);
                self.operand(rhs);
            }
        }
    }

    /// A move bars its local. `Use` and `Guarded` do too: the ownership pass makes each a move
    /// or a copy.
    fn operand(&mut self, operand: &Operand) {
        match operand {
            Operand::Move(local) | Operand::Use(local) | Operand::Guarded(local) => {
                self.barred.push(*local);
            }
            Operand::Const(_) | Operand::Read(_) => {}
        }
    }

    fn term(&mut self, term: &Terminator) {
        match term {
            Terminator::Branch { cond: operand, .. }
            | Terminator::SwitchValue { operand, .. }
            | Terminator::Return(Some(operand))
            | Terminator::Panic {
                message: operand, ..
            } => self.operand(operand),
            Terminator::Goto(_)
            | Terminator::IfPresent { .. }
            | Terminator::SwitchTag { .. }
            | Terminator::TestPresent { .. }
            | Terminator::Return(None)
            | Terminator::Unreachable => {}
        }
    }
}

#[cfg(test)]
thread_local! {
    /// How many statements and terminators the hoist has stepped through on this thread,
    /// counted where the blocks hand them out: each one walked, each one kept or dropped and
    /// each one appended.
    pub(super) static STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many keys the hoist's `States` and `Roots` have touched on this thread, counted at
    /// the key: each hash of one, each comparison of two and each read of the local in one.
    pub(super) static PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Counts a statement or terminator stepped through.
#[cfg_attr(
    not(test),
    expect(clippy::missing_const_for_fn, reason = "counts only in tests")
)]
fn step() {
    #[cfg(test)]
    STEPS.set(STEPS.get() + 1);
}

/// Counts a key touched.
#[cfg_attr(
    not(test),
    expect(clippy::missing_const_for_fn, reason = "counts only in tests")
)]
fn probe() {
    #[cfg(test)]
    PROBES.set(PROBES.get() + 1);
}

/// The data the hoist reads and writes. The fields are private and no block or statement
/// reference leaves the module, so every statement stepped through and every key touched is
/// counted.
mod seam {
    use rustc_hash::{FxHashMap, FxHashSet};

    use self::key::Key;
    use super::{Walk, level_0, step};
    use crate::mir::{Block, LocalId, Statement};

    /// A local as the key of a `States` or a `Roots`. Its field is private and it cannot be
    /// cloned, so a scan of a set counts each key it hashes, compares or reads.
    mod key {
        use std::hash::{Hash, Hasher};

        use super::super::probe;
        use crate::mir::LocalId;

        pub(super) struct Key(LocalId);

        impl Key {
            /// The key of `local`, made to look it up or to insert it.
            pub(super) const fn new(local: LocalId) -> Self {
                Self(local)
            }

            pub(super) fn local(&self) -> LocalId {
                probe();
                self.0
            }
        }

        impl PartialEq for Key {
            fn eq(&self, other: &Self) -> bool {
                probe();
                self.0 == other.0
            }
        }

        impl Eq for Key {}

        impl Hash for Key {
            fn hash<H: Hasher>(&self, state: &mut H) {
                probe();
                self.0.hash(state);
            }
        }
    }

    pub(super) struct Blocks<'a> {
        blocks: &'a [Block],
    }

    impl<'a> Blocks<'a> {
        pub(super) const fn new(blocks: &'a [Block]) -> Self {
            Self { blocks }
        }

        pub(super) const fn len(&self) -> usize {
            self.blocks.len()
        }

        /// Adds what `block`'s statements and terminator write and bar to `walk`.
        pub(super) fn walk(&self, block: usize, walk: &mut Walk) {
            let block = &self.blocks[block];
            for stmt in &block.stmts {
                step();
                walk.stmt(stmt);
            }
            step();
            walk.term(&block.term);
        }
    }

    pub(super) struct BlocksMut<'a> {
        blocks: &'a mut [Block],
    }

    impl<'a> BlocksMut<'a> {
        pub(super) const fn new(blocks: &'a mut [Block]) -> Self {
            Self { blocks }
        }

        /// Drops the level-0 `make_unique` statements of `block` whose local is in `leave`.
        pub(super) fn retain(&mut self, block: usize, leave: &Roots) {
            self.blocks[block].stmts.retain(|stmt| {
                step();
                level_0(stmt).is_none_or(|local| !leave.contains(local))
            });
        }

        /// Appends `stmts` to the end of `block`'s statements.
        pub(super) fn append(&mut self, block: usize, stmts: impl Iterator<Item = Statement>) {
            self.blocks[block].stmts.extend(stmts.inspect(|_| step()));
        }
    }

    /// Whether a loop bars a local, or writes it and does not bar it.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum State {
        Eligible,
        Barred,
    }

    /// A loop's state for each local its blocks write or bar.
    #[derive(Default)]
    pub(super) struct States {
        map: FxHashMap<Key, State>,
    }

    impl States {
        pub(super) fn len(&self) -> usize {
            self.map.len()
        }

        pub(super) fn get(&self, local: LocalId) -> Option<State> {
            self.map.get(&Key::new(local)).copied()
        }

        pub(super) fn insert(&mut self, local: LocalId, state: State) -> Option<State> {
            self.map.insert(Key::new(local), state)
        }

        pub(super) fn iter(&self) -> impl Iterator<Item = (LocalId, State)> + '_ {
            self.map.iter().map(|(key, &state)| (key.local(), state))
        }
    }

    /// The locals whose `make_unique` leaves a loop's blocks.
    #[derive(Default)]
    pub(super) struct Roots {
        set: FxHashSet<Key>,
    }

    impl Roots {
        pub(super) fn insert(&mut self, local: LocalId) {
            self.set.insert(Key::new(local));
        }

        pub(super) fn contains(&self, local: LocalId) -> bool {
            self.set.contains(&Key::new(local))
        }
    }
}
