//! The certifier: it checks every function after the ownership pass, in debug builds, against
//! the spec's six rules. Each counted local has a set of states at each point: owned, empty,
//! borrowed or ended. A forward dataflow over blocks joins the sets by union.
//!
//! The dataflow is sparse, per local. Dominators come from Lengauer and Tarjan's algorithm.
//! A local's region is the blocks its scope block dominates, and its nodes are the blocks
//! that mention it, its `None` edge targets, its scope block and their iterated dominance
//! frontier within the region. Every other block of the region holds the set of its nearest
//! dominating node, so a node answers for the returns, the exits and the root's kills of the
//! blocks it covers with range queries over the dominator tree's pre-order. The work per local
//! is its nodes and their edges, not the blocks between them.
//!
//! An owned or borrowed value that leaves its region is an error, so a cycle that leaks is
//! caught whether or not `Function::loops` records it.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fmt;

use rustc_hash::FxHashMap;

use self::graph::Graph;
use super::{
    BlockId, Function, LocalId, Operand, Place, Projection, Rvalue, Statement, Terminator,
};

/// The first point where a function breaks a rule, and the local that breaks it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Violation {
    pub(crate) block: BlockId,
    pub(crate) local: LocalId,
    pub(crate) rule: Rule,
}

/// One variant per rule of the spec, and per region check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rule {
    /// A block that its scope block does not dominate mentions the local.
    Scope,
    /// Rule 1: a scope release of a local that may be borrowed or ended, or another consume
    /// of one that may not be owned.
    Consume,
    /// Rule 2: a read of a local that may be empty or ended.
    Read,
    /// Rule 3: the root of the local's borrow is consumed, assigned or released while it is
    /// borrowed, or a statement reads a root in place and then consumes, assigns or releases it.
    Borrow,
    /// Rule 4: a `return` where the local may be owned.
    Return,
    /// Rule 5: an assignment to a local that may be owned.
    Assign,
    /// Rule 6: a `take` and the store to its place are not one pair in one block.
    Take,
    /// The borrow of a local that may be owned ends.
    End,
    /// A borrow reaches a `return`, or leaves the local's region, still borrowed.
    Unended,
    /// An owned value leaves the local's region: it can never be consumed.
    Escape,
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Rule::Scope => "mentioned where its scope block does not dominate",
            Rule::Consume => "consumed where it may not own a reference",
            Rule::Read => "read where it may be empty or ended",
            Rule::Borrow => "its borrow's root is consumed, assigned or released while it is read",
            Rule::Return => "may still own a reference at `return`",
            Rule::Assign => "assigned where it may own a reference",
            Rule::Take => "a `take` without exactly one store to its place in its block",
            Rule::End => "its borrow ends where it may own a reference",
            Rule::Unended => "still borrowed outside its arm",
            Rule::Escape => "may still own a reference where its scope ends",
        })
    }
}

/// The states a local may be in at a point, as bits; no bit is a point not reached yet.
type States = u8;
const OWNED: States = 1;
const EMPTY: States = 2;
const BORROWED: States = 4;
const ENDED: States = 8;

/// No block, or no number.
const NONE: usize = usize::MAX;

/// Checks `function` after the ownership pass: the first violation, if any. Rule 6 is checked
/// first, then the statements that read a root in place and then kill it, then each counted
/// local in order.
pub(crate) fn certify(function: &Function) -> Result<(), Violation> {
    let graph = Graph::new(function);
    let tree = Tree::new(&graph);
    takes(function, &tree)?;
    let mentions = mentions(function, &tree)?;
    let shape = Shape::new(function, &graph, &tree);
    let mut roots = FxHashMap::default();
    let mut scratch = Scratch::new(function.blocks.len());
    for (i, local) in function.locals.iter().enumerate() {
        if local.counted {
            let local = LocalId(i);
            let root = root(function, local);
            let root = root.map(|root| {
                &*roots
                    .entry(root)
                    .or_insert_with(|| Kills::new(&mentions[root.0], &tree))
            });
            let region = Region {
                function,
                graph: &graph,
                tree: &tree,
                shape: &shape,
                local,
                scope: function.locals[i].scope.0,
                events: &mentions[i],
                root,
            };
            region.check(&mut scratch)?;
        }
    }
    Ok(())
}

/// The root of `local`'s borrow chain, when it is a `match` binding: the owners followed from
/// it to a local that is not a binding.
fn root(function: &Function, local: LocalId) -> Option<LocalId> {
    let mut root = *function.owners.get(&local)?;
    // A chain is at most as long as the map, so a cycle cannot loop for ever.
    for _ in 0..function.owners.len() {
        match function.owners.get(&root) {
            Some(&owner) => root = owner,
            None => break,
        }
    }
    Some(root)
}

/// Where a binding's root is consumed, assigned or released.
struct Kills {
    /// In block and statement order.
    events: Vec<Event>,
    /// The pre-order numbers of their blocks, sorted.
    pres: Vec<usize>,
}

impl Kills {
    fn new(events: &[Event], tree: &Tree) -> Self {
        let events: Vec<Event> = events
            .iter()
            .filter(|event| event.effect.kills())
            .copied()
            .collect();
        let mut pres: Vec<usize> = events.iter().map(|event| tree.pre[event.block]).collect();
        pres.sort_unstable();
        pres.dedup();
        Kills { events, pres }
    }
}

#[cfg(test)]
thread_local! {
    /// How many control-flow edges the certifier has been handed on this thread.
    pub(super) static EDGES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The control-flow graph. Its fields are private, so every edge read is counted.
mod graph {
    use super::Function;

    pub(super) struct Graph {
        succs: Vec<Vec<usize>>,
        preds: Vec<Vec<usize>>,
    }

    /// Counts `edges` edges handed out.
    fn handed(edges: &[usize]) -> &[usize] {
        #[cfg(test)]
        super::EDGES.set(super::EDGES.get() + edges.len());
        edges
    }

    impl Graph {
        pub(super) fn new(function: &Function) -> Self {
            let succs: Vec<Vec<usize>> = function
                .blocks
                .iter()
                .map(|block| block.term.successors().map(|to| to.0).collect())
                .collect();
            let mut preds = vec![Vec::new(); succs.len()];
            for (from, tos) in succs.iter().enumerate() {
                for &to in tos {
                    preds[to].push(from);
                }
            }
            Self { succs, preds }
        }

        pub(super) fn len(&self) -> usize {
            self.succs.len()
        }

        pub(super) fn succs(&self, block: usize) -> &[usize] {
            handed(&self.succs[block])
        }

        pub(super) fn preds(&self, block: usize) -> &[usize] {
            handed(&self.preds[block])
        }
    }
}

/// The dominator tree of the blocks reachable from `bb0`, numbered in pre-order: `a`
/// dominates `b` when `pre[a] <= pre[b] <= last[a]`. An unreachable block has `pre` `NONE`.
struct Tree {
    idom: Vec<usize>,
    pre: Vec<usize>,
    /// The largest pre-order number in each block's subtree.
    last: Vec<usize>,
    depth: Vec<usize>,
    /// The block at each pre-order number.
    at: Vec<usize>,
}

impl Tree {
    fn new(graph: &Graph) -> Self {
        let n = graph.len();
        let idom = dominators(graph);
        let mut children = vec![Vec::new(); n];
        for (block, &parent) in idom.iter().enumerate() {
            if parent != NONE {
                children[parent].push(block);
            }
        }
        let mut tree = Tree {
            idom,
            pre: vec![NONE; n],
            last: vec![NONE; n],
            depth: vec![0; n],
            at: Vec::new(),
        };
        let mut stack = vec![(0, 0)];
        tree.pre[0] = 0;
        tree.at.push(0);
        while let Some((block, next)) = stack.pop() {
            if let Some(&child) = children[block].get(next) {
                stack.push((block, next + 1));
                tree.pre[child] = tree.at.len();
                tree.depth[child] = tree.depth[block] + 1;
                tree.at.push(child);
                stack.push((child, 0));
            } else {
                tree.last[block] = tree.at.len() - 1;
            }
        }
        tree
    }

    fn reachable(&self, block: usize) -> bool {
        self.pre[block] != NONE
    }

    /// Whether `a` dominates `b`, which must be reachable.
    fn dominates(&self, a: usize, b: usize) -> bool {
        self.pre[a] <= self.pre[b] && self.pre[b] <= self.last[a]
    }
}

/// Each reachable block's immediate dominator, by Lengauer and Tarjan's algorithm, the simple
/// version with path compression; `NONE` for `bb0` and an unreachable block. Both walks are
/// iterative, since a `match` of many arms nests as deep as it has arms.
fn dominators(graph: &Graph) -> Vec<usize> {
    let n = graph.len();
    // Depth-first numbering from bb0.
    let mut number = vec![NONE; n];
    let mut vertex = Vec::new();
    let mut parent = Vec::new();
    let mut stack = vec![(0, NONE, 0)];
    while let Some((block, from, next)) = stack.pop() {
        if next == 0 {
            if number[block] != NONE {
                continue;
            }
            number[block] = vertex.len();
            vertex.push(block);
            parent.push(from);
        }
        if let Some(&to) = graph.succs(block).get(next) {
            stack.push((block, from, next + 1));
            if number[to] == NONE {
                stack.push((to, number[block], 0));
            }
        }
    }
    let count = vertex.len();
    let mut semi: Vec<usize> = (0..count).collect();
    let mut label: Vec<usize> = (0..count).collect();
    let mut ancestor = vec![NONE; count];
    let mut dom = vec![NONE; count];
    let mut bucket = vec![Vec::new(); count];
    let mut path = Vec::new();
    for w in (1..count).rev() {
        for &pred in graph.preds(vertex[w]) {
            if number[pred] != NONE {
                let u = eval(number[pred], &mut ancestor, &mut label, &semi, &mut path);
                semi[w] = semi[w].min(semi[u]);
            }
        }
        bucket[semi[w]].push(w);
        ancestor[w] = parent[w];
        for v in std::mem::take(&mut bucket[parent[w]]) {
            let u = eval(v, &mut ancestor, &mut label, &semi, &mut path);
            dom[v] = if semi[u] < semi[v] { u } else { parent[w] };
        }
    }
    for w in 1..count {
        if dom[w] != semi[w] {
            dom[w] = dom[dom[w]];
        }
    }
    let mut idom = vec![NONE; n];
    for w in 1..count {
        idom[vertex[w]] = vertex[dom[w]];
    }
    idom
}

/// The vertex of least semi-dominator on `v`'s path in the forest, compressing the path.
fn eval(
    v: usize,
    ancestor: &mut [usize],
    label: &mut [usize],
    semi: &[usize],
    path: &mut Vec<usize>,
) -> usize {
    if ancestor[v] == NONE {
        return v;
    }
    let mut x = v;
    while ancestor[ancestor[x]] != NONE {
        path.push(x);
        x = ancestor[x];
    }
    // From the top of the path down, as the recursive `compress` returns.
    while let Some(x) = path.pop() {
        let a = ancestor[x];
        if semi[label[a]] < semi[label[x]] {
            label[x] = label[a];
        }
        ancestor[x] = ancestor[a];
    }
    label[v]
}

/// What a statement does to one local, in the order its operands run.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Effect {
    /// A read that leaves the value in place for a copy or a scalar result.
    Read,
    /// A read of the root of a place that the statement uses in place.
    ReadInPlace,
    /// A `release` or a move.
    Consume,
    ScopeRelease,
    Assign,
    /// `b = borrow p`.
    AssignBorrowed,
    End,
}

impl Effect {
    /// Whether the effect consumes, assigns or releases the local.
    fn kills(self) -> bool {
        match self {
            Effect::Consume | Effect::ScopeRelease | Effect::Assign | Effect::AssignBorrowed => {
                true
            }
            Effect::Read | Effect::ReadInPlace | Effect::End => false,
        }
    }
}

/// One effect of statement `at` of `block` (the block's length for its terminator).
#[derive(Clone, Copy)]
struct Event {
    block: usize,
    at: usize,
    effect: Effect,
}

/// Calls `f` with each local `stmt` mentions, its effect and the place it is the root of.
fn statement<'a>(stmt: &'a Statement, f: &mut impl FnMut(LocalId, Effect, Option<&'a Place>)) {
    match stmt {
        Statement::Assign { dest, value } => {
            rvalue(value, f);
            if let Some(dest) = *dest {
                let borrowed = matches!(value, Rvalue::Borrow(_));
                f(
                    dest,
                    if borrowed {
                        Effect::AssignBorrowed
                    } else {
                        Effect::Assign
                    },
                    None,
                );
            }
        }
        Statement::Take { dest, place } => {
            self::place(place, Effect::ReadInPlace, f);
            f(*dest, Effect::Assign, None);
        }
        Statement::Store { place, value } => {
            self::place(place, Effect::ReadInPlace, f);
            operand(value, f);
        }
        Statement::Release(local) => f(*local, Effect::Consume, None),
        Statement::ScopeRelease(local) => f(*local, Effect::ScopeRelease, None),
        Statement::MakeUnique(place) | Statement::Check(place) => {
            self::place(place, Effect::ReadInPlace, f);
        }
        Statement::End(local) => f(*local, Effect::End, None),
    }
}

fn rvalue<'a>(value: &'a Rvalue, f: &mut impl FnMut(LocalId, Effect, Option<&'a Place>)) {
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
        | Rvalue::Print { operand: op, .. } => operand(op, f),
        Rvalue::Copy(place) => self::place(place, Effect::Read, f),
        Rvalue::Borrow(place) => self::place(place, Effect::ReadInPlace, f),
        Rvalue::CopyStr(_) | Rvalue::ReadLine => {}
        Rvalue::Call { args, .. }
        | Rvalue::New { args, .. }
        | Rvalue::Variant { args, .. }
        | Rvalue::ArrayLit(args)
        | Rvalue::Interpolate { parts: args, .. }
        | Rvalue::Math { operands: args, .. } => {
            for op in args {
                operand(op, f);
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
            operand(lhs, f);
            operand(rhs, f);
        }
    }
}

fn terminator<'a>(term: &'a Terminator, f: &mut impl FnMut(LocalId, Effect, Option<&'a Place>)) {
    match term {
        Terminator::Branch { cond: op, .. }
        | Terminator::SwitchValue { operand: op, .. }
        | Terminator::Return(Some(op))
        | Terminator::Panic { message: op, .. } => operand(op, f),
        Terminator::IfPresent { optional, .. } => f(*optional, Effect::Read, None),
        Terminator::SwitchTag { place, .. } => self::place(place, Effect::ReadInPlace, f),
        Terminator::Goto(_) | Terminator::Return(None) | Terminator::Unreachable => {}
    }
}

fn operand<'a>(op: &'a Operand, f: &mut impl FnMut(LocalId, Effect, Option<&'a Place>)) {
    match op {
        Operand::Read(place) => self::place(place, Effect::ReadInPlace, f),
        Operand::Move(local) => f(*local, Effect::Consume, None),
        Operand::Use(_) | Operand::Guarded(_) => {
            panic!("the ownership pass should leave no `use` and no guard mark")
        }
        Operand::Const(_) => {}
    }
}

/// The root of `place` with `root`'s effect, then each index operand.
fn place<'a>(
    place: &'a Place,
    root: Effect,
    f: &mut impl FnMut(LocalId, Effect, Option<&'a Place>),
) {
    f(place.local, root, Some(place));
    for projection in &place.projections {
        if let Projection::Index { index, .. } = projection {
            operand(index, f);
        }
    }
}

/// Each counted local's effects, in block and statement order, over the reachable blocks.
/// A statement that reads a local in place and then kills it is an error here.
fn mentions(function: &Function, tree: &Tree) -> Result<Vec<Vec<Event>>, Violation> {
    let mut events = vec![Vec::new(); function.locals.len()];
    let mut in_place = Vec::new();
    for (b, block) in function.blocks.iter().enumerate() {
        if !tree.reachable(b) {
            continue;
        }
        let mut found = None;
        let mut note = |at: usize, local: LocalId, effect: Effect, in_place: &mut Vec<LocalId>| {
            if !function.locals[local.0].counted {
                return;
            }
            if effect.kills() && in_place.contains(&local) && found.is_none() {
                found = Some(Violation {
                    block: BlockId(b),
                    local,
                    rule: Rule::Borrow,
                });
            }
            if effect == Effect::ReadInPlace {
                in_place.push(local);
            }
            events[local.0].push(Event {
                block: b,
                at,
                effect,
            });
        };
        for (at, stmt) in block.stmts.iter().enumerate() {
            in_place.clear();
            statement(stmt, &mut |local, effect, _| {
                note(at, local, effect, &mut in_place);
            });
        }
        in_place.clear();
        terminator(&block.term, &mut |local, effect, _| {
            note(block.stmts.len(), local, effect, &mut in_place);
        });
        if let Some(violation) = found {
            return Err(violation);
        }
    }
    Ok(events)
}

/// Rule 6, per reachable block: a `take` marks its place taken until the store to the same
/// place. A store of a counted value needs its place taken, and a read of a taken place, a
/// second `take` of it or the end of the block while it is taken is an error.
fn takes(function: &Function, tree: &Tree) -> Result<(), Violation> {
    for (b, block) in function.blocks.iter().enumerate() {
        if !tree.reachable(b) {
            continue;
        }
        let mut taken: Vec<&Place> = Vec::new();
        let mut found = None;
        let mut check = |taken: &[&Place], local: LocalId, at: Option<&Place>, own: bool| {
            if !own && found.is_none() && taken.iter().any(|t| overlaps(t, local, at)) {
                found = Some(local);
            }
        };
        for stmt in &block.stmts {
            // A store's own place is its target, not a read.
            let target = if let Statement::Store { place, .. } = stmt {
                Some(place)
            } else {
                None
            };
            statement(stmt, &mut |local, _, at| {
                let own = at
                    .zip(target)
                    .is_some_and(|(at, target)| std::ptr::eq(at, target));
                check(&taken, local, at, own);
            });
            if let Statement::Store { place, value } = stmt {
                if let Some(i) = taken.iter().position(|t| *t == place) {
                    taken.swap_remove(i);
                } else if counted_value(function, value) || overlapping(&taken, place) {
                    check(&[place], place.local, None, false);
                }
            } else if let Statement::Take { place, .. } = stmt {
                taken.push(place);
            }
        }
        terminator(&block.term, &mut |local, _, at| {
            check(&taken, local, at, false);
        });
        if let Some(local) = found.or(taken.first().map(|t| t.local)) {
            return Err(Violation {
                block: BlockId(b),
                local,
                rule: Rule::Take,
            });
        }
    }
    Ok(())
}

/// Whether a store of `value` moves a counted reference into its place.
fn counted_value(function: &Function, value: &Operand) -> bool {
    match value {
        Operand::Move(local) => function.locals[local.0].counted,
        Operand::Read(place) => {
            place.projections.is_empty() && function.locals[place.local.0].counted
        }
        Operand::Const(_) | Operand::Use(_) | Operand::Guarded(_) => false,
    }
}

fn overlapping(taken: &[&Place], place: &Place) -> bool {
    taken.iter().any(|t| overlaps(t, place.local, Some(place)))
}

/// Whether a mention of `local`, through `place` or whole, may touch the taken place `t`:
/// the same root, and one path a prefix of the other. Any two indexes may be equal.
fn overlaps(t: &Place, local: LocalId, place: Option<&Place>) -> bool {
    t.local == local
        && place.is_none_or(|place| {
            t.projections
                .iter()
                .zip(&place.projections)
                .all(|pair| match pair {
                    (Projection::Index { .. }, Projection::Index { .. }) => true,
                    (a, b) => a == b,
                })
        })
}

/// Per function, what each local's region queries: the dominance frontiers, and over the
/// dominator tree's pre-order the depth an edge's ends meet at and the returns.
struct Shape {
    frontier: Vec<Vec<usize>>,
    /// `meet[k][i]`: the least, over the blocks at pre-order `i..i + 2^k`, of the depth in the
    /// dominator tree where an edge from the block meets its target's dominators. An edge from
    /// a block of `S`'s subtree leaves it exactly when that depth is less than `S`'s.
    meet: Vec<Vec<usize>>,
    /// The `return` blocks before each pre-order number.
    returns: Vec<usize>,
}

impl Shape {
    fn new(function: &Function, graph: &Graph, tree: &Tree) -> Self {
        let n = graph.len();
        // Cooper, Harvey and Kennedy's runners; a runner stops at a block that already has the
        // join, since an earlier runner went on from there to the join's dominator.
        let mut frontier = vec![Vec::new(); n];
        for &join in &tree.at {
            for &pred in graph.preds(join) {
                let mut runner = if tree.reachable(pred) { pred } else { NONE };
                while runner != NONE
                    && runner != tree.idom[join]
                    && frontier[runner].last() != Some(&join)
                {
                    frontier[runner].push(join);
                    runner = tree.idom[runner];
                }
            }
        }
        // An edge to `to` meets `to` itself when `to` dominates its source, else `to`'s
        // immediate dominator, which dominates every predecessor of `to`.
        let first: Vec<usize> = tree
            .at
            .iter()
            .map(|&block| {
                graph
                    .succs(block)
                    .iter()
                    .map(|&to| {
                        if tree.dominates(to, block) {
                            tree.depth[to]
                        } else {
                            tree.depth[to] - 1
                        }
                    })
                    .min()
                    .unwrap_or(NONE)
            })
            .collect();
        let mut meet = vec![first];
        while 1 << meet.len() <= tree.at.len() {
            let half = 1 << (meet.len() - 1);
            let below = &meet[meet.len() - 1];
            let next = (0..below.len() - half)
                .map(|i| below[i].min(below[i + half]))
                .collect();
            meet.push(next);
        }
        let mut returns = Vec::with_capacity(tree.at.len() + 1);
        returns.push(0);
        for &block in &tree.at {
            let ends = matches!(function.blocks[block].term, Terminator::Return(_));
            returns.push(returns[returns.len() - 1] + usize::from(ends));
        }
        Shape {
            frontier,
            meet,
            returns,
        }
    }

    /// The least meeting depth over pre-order numbers `from..=to`.
    fn meet(&self, from: usize, to: usize) -> usize {
        let k = (to - from + 1).ilog2() as usize;
        self.meet[k][from].min(self.meet[k][to + 1 - (1 << k)])
    }

    fn returns(&self, from: usize, to: usize) -> bool {
        self.returns[to + 1] > self.returns[from]
    }
}

/// The buffers of one local's region, kept for the next local. Per block, the last region
/// stamped there and the block's node, so nothing is cleared between locals.
struct Scratch {
    stamp: Vec<usize>,
    slot: Vec<usize>,
    next: usize,
    /// Each node block's events, at `events[from[block]..to[block]]`.
    from: Vec<usize>,
    to: Vec<usize>,
    /// The node blocks, in pre-order: the scope block first.
    blocks: Vec<usize>,
    work: Vec<usize>,
    /// Each predecessor of a node inside the region: its pre-order number, the block and the
    /// node.
    queries: Vec<(usize, usize, usize)>,
    /// Each node's inputs, as the node whose set it reads and the predecessor block, grouped
    /// by node: node `i`'s are at `inputs[input_at[i]..input_at[i + 1]]`.
    links: Vec<(usize, (usize, usize))>,
    inputs: Vec<(usize, usize)>,
    input_at: Vec<usize>,
    /// The nodes that read each node's set, grouped as the inputs are.
    fed: Vec<(usize, usize)>,
    feeds: Vec<usize>,
    feed_at: Vec<usize>,
    /// One past the last node in each node's subtree.
    end: Vec<usize>,
    stack: Vec<usize>,
    out: Vec<States>,
    queued: Vec<bool>,
    queue: BinaryHeap<Reverse<usize>>,
}

impl Scratch {
    fn new(blocks: usize) -> Self {
        Scratch {
            stamp: vec![0; blocks],
            slot: vec![0; blocks],
            next: 0,
            from: vec![0; blocks],
            to: vec![0; blocks],
            blocks: Vec::new(),
            work: Vec::new(),
            queries: Vec::new(),
            links: Vec::new(),
            inputs: Vec::new(),
            input_at: Vec::new(),
            fed: Vec::new(),
            feeds: Vec::new(),
            feed_at: Vec::new(),
            end: Vec::new(),
            stack: Vec::new(),
            out: Vec::new(),
            queued: Vec::new(),
            queue: BinaryHeap::new(),
        }
    }

    /// Adds `block` to the region's nodes, unless it is one already.
    fn add(&mut self, block: usize) -> bool {
        let new = self.stamp[block] != self.next;
        if new {
            self.stamp[block] = self.next;
            self.from[block] = 0;
            self.to[block] = 0;
            self.blocks.push(block);
        }
        new
    }
}

/// Groups `pairs` by key: key `k`'s values go to `values[at[k]..at[k + 1]]`, in order.
fn group<T: Copy + Default>(
    pairs: &[(usize, T)],
    keys: usize,
    at: &mut Vec<usize>,
    values: &mut Vec<T>,
) {
    at.clear();
    at.resize(keys + 1, 0);
    for &(key, _) in pairs {
        at[key + 1] += 1;
    }
    for key in 0..keys {
        at[key + 1] += at[key];
    }
    values.clear();
    values.resize(pairs.len(), T::default());
    for &(key, value) in pairs {
        values[at[key]] = value;
        at[key] += 1;
    }
    // Each `at[k]` now ends key `k`, which is where key `k + 1` starts.
    at.rotate_right(1);
    at[0] = 0;
}

/// One local's region.
struct Region<'a> {
    function: &'a Function,
    graph: &'a Graph,
    tree: &'a Tree,
    shape: &'a Shape,
    local: LocalId,
    scope: usize,
    events: &'a [Event],
    /// Where the root of a `match` binding is killed.
    root: Option<&'a Kills>,
}

impl Region<'_> {
    fn violation(&self, block: usize, rule: Rule) -> Violation {
        Violation {
            block: BlockId(block),
            local: self.local,
            rule,
        }
    }

    fn check(&self, scratch: &mut Scratch) -> Result<(), Violation> {
        let param = self.local.0 < self.function.params;
        if self.events.is_empty() && !param {
            // Lowering never reached the local: dead code after a `return`.
            return Ok(());
        }
        for event in self.events {
            if !self.tree.reachable(self.scope) || !self.tree.dominates(self.scope, event.block) {
                return Err(self.violation(event.block, Rule::Scope));
            }
        }
        let outside = self.nodes(scratch);
        let s = scratch;
        let count = s.blocks.len();
        let init = if param { OWNED } else { EMPTY };
        // What reaches the scope block from outside the region: empty, or ended after a
        // borrow left it ended.
        let mut from_outside = EMPTY;
        s.out.clear();
        s.out.resize(count, 0);
        s.queued.clear();
        s.queued.resize(count, false);
        s.queue.clear();
        s.queue.push(Reverse(0));
        s.queued[0] = true;
        while let Some(Reverse(i)) = s.queue.pop() {
            s.queued[i] = false;
            let block = s.blocks[i];
            let mut states = if block == 0 { init } else { 0 };
            if i == 0 && outside {
                states |= from_outside;
            }
            for &(from, pred) in &s.inputs[s.input_at[i]..s.input_at[i + 1]] {
                states |= self.edge(s.blocks[from], s.out[from], pred, block);
            }
            let (out, leaves) = self.transfer(s, i, states)?;
            if leaves & ENDED != 0 && from_outside & ENDED == 0 && outside {
                from_outside |= ENDED;
                if !s.queued[0] {
                    s.queued[0] = true;
                    s.queue.push(Reverse(0));
                }
            }
            if out != s.out[i] {
                s.out[i] = out;
                for &next in &s.feeds[s.feed_at[i]..s.feed_at[i + 1]] {
                    if !s.queued[next] {
                        s.queued[next] = true;
                        s.queue.push(Reverse(next));
                    }
                }
            }
        }
        Ok(())
    }

    /// Fills `scratch` with the region's nodes in pre-order, the scope block first, their
    /// inputs and their subtrees. Whether the scope block has a predecessor outside the region.
    fn nodes(&self, s: &mut Scratch) -> bool {
        let tree = self.tree;
        let scope = self.scope;
        let inside = |block: usize| tree.reachable(block) && tree.dominates(scope, block);
        s.next += 1;
        s.blocks.clear();
        s.add(scope);
        for (at, event) in self.events.iter().enumerate() {
            if s.add(event.block) || s.to[event.block] == 0 {
                s.from[event.block] = at;
            }
            s.to[event.block] = at + 1;
            if let Terminator::IfPresent { optional, none, .. } =
                self.function.blocks[event.block].term
                && optional == self.local
                && inside(none.0)
            {
                s.add(none.0);
            }
        }
        // The iterated dominance frontier, within the region.
        s.work.clear();
        s.work.extend_from_slice(&s.blocks);
        while let Some(block) = s.work.pop() {
            for &join in &self.shape.frontier[block] {
                if inside(join) && s.add(join) {
                    s.work.push(join);
                }
            }
        }
        s.blocks.sort_unstable_by_key(|&block| tree.pre[block]);
        for (i, &block) in s.blocks.iter().enumerate() {
            s.slot[block] = i;
        }
        let mut outside = false;
        s.queries.clear();
        for (i, &block) in s.blocks.iter().enumerate() {
            for &pred in self.graph.preds(block) {
                if inside(pred) {
                    s.queries.push((tree.pre[pred], pred, i));
                } else if tree.reachable(pred) {
                    outside = true;
                }
            }
        }
        s.queries.sort_unstable();
        // Each predecessor's nearest dominating node, by one sweep in pre-order with the
        // nodes that dominate the current point on a stack.
        let count = s.blocks.len();
        s.end.clear();
        s.end.resize(count, count);
        s.stack.clear();
        s.links.clear();
        let mut next = 0;
        let mut query = 0;
        while next < count || query < s.queries.len() {
            let node_pre = s.blocks.get(next).map_or(NONE, |&block| tree.pre[block]);
            let query_pre = s.queries.get(query).map_or(NONE, |query| query.0);
            let pre = node_pre.min(query_pre);
            while let Some(&top) = s.stack.last()
                && tree.last[s.blocks[top]] < pre
            {
                s.end[top] = next;
                s.stack.pop();
            }
            if node_pre <= query_pre {
                s.stack.push(next);
                next += 1;
            } else {
                let (_, pred, to) = s.queries[query];
                let from = *s
                    .stack
                    .last()
                    .expect("the scope block should dominate every block of the region");
                s.links.push((to, (from, pred)));
                query += 1;
            }
        }
        group(&s.links, count, &mut s.input_at, &mut s.inputs);
        s.fed.clear();
        s.fed
            .extend(s.links.iter().map(|&(to, (from, _))| (from, to)));
        group(&s.fed, count, &mut s.feed_at, &mut s.feeds);
        outside
    }

    /// The set on the edge from `pred`, which holds the set `out` of node `from`, to `to`.
    fn edge(&self, from: usize, out: States, pred: usize, to: usize) -> States {
        if from == pred
            && let Terminator::IfPresent {
                optional,
                some,
                none,
            } = self.function.blocks[pred].term
            && optional == self.local
        {
            let mut states = 0;
            if none.0 == to {
                states |= EMPTY;
            }
            if some.0 == to {
                states |= out;
            }
            return states;
        }
        out
    }

    /// Node `i`'s set at its end, and the states that leave the region from it or from the
    /// blocks it covers: those of its subtree outside its node children's. Checks the rules
    /// on the way.
    fn transfer(
        &self,
        s: &Scratch,
        i: usize,
        mut states: States,
    ) -> Result<(States, States), Violation> {
        let block = s.blocks[i];
        let kills = self
            .root
            .map_or(&[][..], |root| by_block(&root.events, block));
        let mut kills = kills.iter().peekable();
        for event in &self.events[s.from[block]..s.to[block]] {
            // The root's kills in a statement come before the statement's own effects.
            while kills.next_if(|kill| kill.at <= event.at).is_some() {
                if states & BORROWED != 0 {
                    return Err(self.violation(block, Rule::Borrow));
                }
            }
            states = self.apply(block, event.effect, states)?;
        }
        if kills.next().is_some() && states & BORROWED != 0 {
            return Err(self.violation(block, Rule::Borrow));
        }
        if let Terminator::Return(_) = self.function.blocks[block].term {
            self.at_return(block, states)?;
        }
        let mut leaves = 0;
        for &to in self.graph.succs(block) {
            if !self.tree.dominates(self.scope, to) {
                let states = self.edge(block, states, block, to);
                self.leave(block, states)?;
                leaves |= states;
            }
        }
        let tree = self.tree;
        let mut from = tree.pre[block] + 1;
        let mut child = i + 1;
        while from <= tree.last[block] {
            let to = if child < s.end[i] {
                tree.pre[s.blocks[child]]
            } else {
                tree.last[block] + 1
            };
            if from < to {
                leaves |= self.cover(from, to - 1, states)?;
            }
            if child >= s.end[i] {
                break;
            }
            from = tree.last[s.blocks[child]] + 1;
            child = s.end[child];
        }
        Ok((states, leaves))
    }

    /// Checks the blocks at pre-order `from..=to`, which hold `states` throughout: the root's
    /// kills, the returns and the exits. The states that leave the region from them.
    fn cover(&self, from: usize, to: usize, states: States) -> Result<States, Violation> {
        if states & BORROWED != 0
            && let Some(Kills { pres: kills, .. }) = self.root
        {
            let at = kills.partition_point(|&pre| pre < from);
            if kills.get(at).is_some_and(|&pre| pre <= to) {
                return Err(self.violation(self.tree.at[kills[at]], Rule::Borrow));
            }
        }
        // Only a violation needs its block, which a search over the range finds.
        let owes = states & (OWNED | BORROWED) != 0;
        if owes && self.shape.returns(from, to) {
            let at = (from..=to)
                .find(|&pre| self.shape.returns(pre, pre))
                .expect("a range with a return should hold a return block");
            self.at_return(self.tree.at[at], states)?;
        }
        let depth = self.tree.depth[self.scope];
        if self.shape.meet(from, to) >= depth {
            return Ok(0);
        }
        if owes {
            let at = (from..=to)
                .find(|&pre| self.shape.meet(pre, pre) < depth)
                .expect("a range with an exit should hold an exiting block");
            self.leave(self.tree.at[at], states)?;
        }
        Ok(states)
    }

    /// One effect on the set, checked against rules 1, 2, 5 and the end of a borrow.
    fn apply(&self, block: usize, effect: Effect, states: States) -> Result<States, Violation> {
        let (bad, rule, after) = match effect {
            Effect::Read | Effect::ReadInPlace => (EMPTY | ENDED, Rule::Read, states),
            Effect::Consume => (EMPTY | BORROWED | ENDED, Rule::Consume, EMPTY),
            Effect::ScopeRelease => (BORROWED | ENDED, Rule::Consume, EMPTY),
            Effect::Assign => (OWNED, Rule::Assign, OWNED),
            Effect::AssignBorrowed => (OWNED, Rule::Assign, BORROWED),
            Effect::End => (OWNED, Rule::End, ENDED),
        };
        if states & bad != 0 {
            return Err(self.violation(block, rule));
        }
        Ok(after)
    }

    /// Rule 4, and a borrow that reaches a `return`.
    fn at_return(&self, block: usize, states: States) -> Result<(), Violation> {
        if states & OWNED != 0 {
            return Err(self.violation(block, Rule::Return));
        }
        if states & BORROWED != 0 {
            return Err(self.violation(block, Rule::Unended));
        }
        Ok(())
    }

    /// A set that leaves the region from `block`.
    fn leave(&self, block: usize, states: States) -> Result<(), Violation> {
        if states & OWNED != 0 {
            return Err(self.violation(block, Rule::Escape));
        }
        if states & BORROWED != 0 {
            return Err(self.violation(block, Rule::Unended));
        }
        Ok(())
    }
}

/// The events of `block` in `events`, which are in block order.
fn by_block(events: &[Event], block: usize) -> &[Event] {
    let from = events.partition_point(|event| event.block < block);
    let to = events.partition_point(|event| event.block <= block);
    &events[from..to]
}
