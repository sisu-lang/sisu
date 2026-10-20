//! The certifier: it checks every function after the ownership pass, in debug builds, against
//! the spec's six rules. Each counted local has a set of states at each point: owned, empty,
//! borrowed or ended. A forward dataflow over blocks joins the sets by union.
//!
//! The dataflow is sparse, per local. Dominators come from Lengauer and Tarjan's algorithm.
//! A local's region is the blocks its scope block dominates, and its nodes are the blocks
//! that mention it, its `None` edge targets, its scope block and their iterated dominance
//! frontier within the region. Every other block of the region holds the set of its nearest
//! dominating node, so a node answers for the returns, the exits, the root's kills and, for a
//! local that borrows through a field or an element, the clobbers of the blocks it covers, with
//! range queries over the dominator tree's pre-order. The work per local is its nodes and their
//! edges, not the blocks between them.
//!
//! An owned or borrowed value that leaves its region is an error, so a cycle that leaks is
//! caught whether or not `Function::loops` records it.
//!
//! Rule 3 ends a borrow through an unstable place at any call: one through a `mut` field or an
//! element (`Projection::stable`). Weak fields come in milestone 9, and will be unstable.
//!
//! A borrowed parameter starts borrowed, and stays borrowed to the end: its root is the caller.
//! A call's arguments follow its own flags: a borrowed one is read in place, an owned one is
//! consumed.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};
use std::fmt;

use rustc_hash::FxHashMap;

use self::graph::Graph;
use self::range::RangeMin;
use self::tree::Tree;
use self::wavelet::Wavelet;
use super::{
    BlockId, Const, Function, LocalId, Operand, Place, Projection, Rvalue, Statement, Terminator,
};
use crate::tir::TypeKind;

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
    /// Rule 1: a scope release of a local that may be borrowed or ended, another consume of
    /// one that may not be owned, or a consuming operand that reads a whole local in place and
    /// so hands on a reference the local keeps.
    Consume,
    /// Rule 2: a read of a local that may be empty or ended.
    Read,
    /// Rule 3: the root of the local's borrow is consumed, assigned, released or written while
    /// it is borrowed, or a call, a counted write or a `make_unique` happens while it borrows
    /// through a `mut` field or an element; or a statement reads a root in place and then
    /// consumes, assigns or releases it.
    Borrow,
    /// Rule 4: a `return` where the local may be owned.
    Return,
    /// Rule 5: an assignment to a local that may be owned.
    Assign,
    /// Rule 3: a `take`, a store or a `make_unique` writes through a local that may be
    /// borrowed and is not an object. An array or a string with a second holder is copied on
    /// write, but a borrow holds no count, so the write would go into its root's value.
    WriteBorrowed,
    /// Rule 6: a `take` and the store to its place are not one pair in one block.
    Take,
    /// The borrow of a local that may be owned ends.
    End,
    /// A borrow reaches a `return`, or leaves the local's region, still borrowed. The check is
    /// by region: a missing `End` inside the region shows later, as `Borrow` at the root's
    /// kill (the scrutinee's scope release), and not at all before a `panic`.
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
            Rule::Borrow => {
                "its borrow's root is consumed, assigned, released or written while it is \
                 borrowed, or a call, a counted write or a `make_unique` happens while it \
                 borrows through a `mut` field or an element"
            }
            Rule::Return => "may still own a reference at `return`",
            Rule::Assign => "assigned where it may own a reference",
            Rule::WriteBorrowed => {
                "a write goes into it while it is borrowed, and it is not an object"
            }
            Rule::Take => "a `take` without exactly one store to its place in its block",
            Rule::End => "its borrow ends where it may own a reference",
            Rule::Unended => "still borrowed outside its arm",
            Rule::Escape => "may still own a reference where its scope ends",
        })
    }
}

/// The states a local may be in at a point, as bits; no bit is a point not reached yet.
pub(super) type States = u8;
pub(super) const OWNED: States = 1;
pub(super) const EMPTY: States = 2;
const BORROWED: States = 4;
/// The slot holds a pointer the local does not own: a borrow that ended, or a value that
/// `release x` or `release? x` freed. Unlike an empty slot, which holds the zero value wherever a
/// `release?` can read it (a flagged local), a scope release would free it.
const ENDED: States = 8;

/// No block, or no number.
const NONE: usize = usize::MAX;

/// Checks `function` after the ownership pass: the first violation, if any. Rule 6 is checked
/// first, then the statements that read a root in place and then kill it, then each counted
/// local in order.
pub(crate) fn certify(function: &Function) -> Result<(), Violation> {
    regions(
        function,
        |_| true,
        true,
        |region, scratch| region.check(scratch, &mut |_, _| {}),
    )
}

/// The set that reaches each scope release of a `named` local (one a `release?` names), keyed by
/// its block and its statement's index there, for the elaboration. A local whose region breaks a
/// rule is left out, and so is every local when the function breaks rule 6 or a statement reads a
/// root in place and then kills it.
///
/// A region's sets only grow while its fixpoint runs, so the union of every set observed at a
/// scope release is the set the fixpoint ends with there.
pub(super) fn scope_release_states(
    function: &Function,
    named: &[bool],
) -> FxHashMap<(BlockId, usize), States> {
    let mut states = FxHashMap::default();
    if !named.contains(&true) {
        return states;
    }
    let mut seen = FxHashMap::default();
    // `takes` and `mentions` fail only on mir the certifier rejects; then no region runs, and
    // every `release?` would stay unelaborated without a sign.
    let built = regions(
        function,
        |local| named[local.0],
        false,
        |region, scratch| {
            seen.clear();
            let observed = region.check(scratch, &mut |event, set| {
                if event.effect == Effect::ScopeRelease {
                    *seen.entry((BlockId(event.block), event.at)).or_insert(0) |= set;
                }
            });
            if observed.is_ok() {
                states.extend(seen.drain());
            }
            Ok(())
        },
    );
    debug_assert!(
        built.is_ok(),
        "the elaboration should see mir that breaks no rule of `takes` or `mentions`: {built:?}"
    );
    states
}

/// Builds what the regions share, then runs `each` on the region of each counted local that
/// `wanted` takes, in order, up to its first error. Only with `clobbers` does a region check
/// rule 3's clobbers. Only `certify` builds them, so the elaboration, which runs in release
/// builds too, skips their cost.
fn regions(
    function: &Function,
    wanted: impl Fn(LocalId) -> bool,
    clobbers: bool,
    mut each: impl FnMut(&Region<'_>, &mut Scratch) -> Result<(), Violation>,
) -> Result<(), Violation> {
    let graph = Graph::new(function);
    let tree = Tree::new(&graph);
    takes(function, &tree)?;
    let mentions = mentions(function, &tree)?;
    let shape = Shape::new(function, &graph, &tree);
    let clobbers = clobbers.then(|| Clobbers::new(function, &tree));
    let mut roots = FxHashMap::default();
    let mut scratch = Scratch::new(function.blocks.len());
    for (i, local) in function.locals.iter().enumerate() {
        if local.counted && wanted(LocalId(i)) {
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
                clobbers: clobbers.as_ref().filter(|clobbers| clobbers.unstable[i]),
            };
            each(&region, &mut scratch)?;
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

/// Where a binding's root is consumed, assigned, released or written.
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
            .filter(|event| event.effect.kills() || event.effect == Effect::Write)
            .copied()
            .collect();
        let mut pres: Vec<usize> = events.iter().map(|event| tree.pre(event.block)).collect();
        pres.sort_unstable();
        pres.dedup();
        Kills { events, pres }
    }
}

/// Rule 3's clobbers: the statements that may free or change what a borrow through an unstable
/// place reads. A call can replace any field or element; a `take` into a counted local and
/// a store of a counted value can free one; and a `make_unique` decides on a count that a
/// borrow does not hold.
struct Clobbers {
    /// Whether each local borrows through an unstable place, or from a local that does.
    unstable: Vec<bool>,
    /// The pre-order numbers of the clobbers' blocks, sorted.
    pres: Vec<usize>,
    /// Per block, the clobbers before each statement: `prefix[block][at]` for statement `at`,
    /// and the terminator's at the block's length.
    prefix: Vec<Vec<usize>>,
}

impl Clobbers {
    fn new(function: &Function, tree: &Tree) -> Self {
        let locals = function.locals.len();
        // Each local is marked as it is queued, so it is queued, and its edges walked, once.
        let mut unstable = vec![false; locals];
        let mut work = Vec::new();
        // Each borrow, as the local it reads to the local it assigns.
        let mut edges = Vec::new();
        let mut pres = Vec::new();
        let mut prefix = Vec::with_capacity(function.blocks.len());
        for (b, block) in function.blocks.iter().enumerate() {
            if !tree.reachable(b) {
                prefix.push(Vec::new());
                continue;
            }
            let mut before = Vec::with_capacity(block.stmts.len() + 1);
            let mut so_far = 0;
            for stmt in &block.stmts {
                before.push(so_far);
                let clobbers = match stmt {
                    Statement::Assign { dest, value } => match value {
                        Rvalue::Call { .. } => true,
                        Rvalue::Borrow(place) => {
                            if let Some(dest) = *dest {
                                if !place.projections.iter().all(Projection::stable)
                                    && !unstable[dest.0]
                                {
                                    unstable[dest.0] = true;
                                    work.push(dest.0);
                                }
                                edges.push((place.local.0, dest.0));
                            }
                            false
                        }
                        Rvalue::Use(_)
                        | Rvalue::Copy(_)
                        | Rvalue::CopyStr(_)
                        | Rvalue::Unwrap(_)
                        | Rvalue::New { .. }
                        | Rvalue::Variant { .. }
                        | Rvalue::ArrayLit(_)
                        | Rvalue::ArrayFill { .. }
                        | Rvalue::Wrap(_)
                        | Rvalue::Concat { .. }
                        | Rvalue::Interpolate { .. }
                        | Rvalue::ReadLine
                        | Rvalue::Chars(_)
                        | Rvalue::Bytes(_)
                        | Rvalue::Words(_)
                        | Rvalue::FromChars(_)
                        | Rvalue::Len(_)
                        | Rvalue::LenWords(_)
                        | Rvalue::Sum { .. }
                        | Rvalue::Contains { .. }
                        | Rvalue::Convert(_)
                        | Rvalue::Math { .. }
                        | Rvalue::Unary { .. }
                        | Rvalue::Binary { .. }
                        | Rvalue::Compare { .. }
                        | Rvalue::Equal { .. }
                        | Rvalue::Is { .. }
                        | Rvalue::Print { .. } => false,
                    },
                    Statement::Take { dest, .. } => function.locals[dest.0].counted,
                    Statement::Store { value, .. } => counted_value(function, value),
                    Statement::MakeUnique(_) => true,
                    Statement::Release(_)
                    | Statement::ScopeRelease(_)
                    | Statement::Check(_)
                    | Statement::End(_) => false,
                };
                so_far += usize::from(clobbers);
            }
            before.push(so_far);
            if so_far > 0 {
                pres.push(tree.pre(b));
            }
            prefix.push(before);
        }
        pres.sort_unstable();
        // Instability flows along borrows in any block order, so from the seeds over the edges.
        let (mut at, mut next) = (Vec::new(), Vec::new());
        group(&edges, locals, &mut at, &mut next);
        while let Some(local) = work.pop() {
            for &to in &next[at[local]..at[local + 1]] {
                count(1);
                if !unstable[to] {
                    unstable[to] = true;
                    work.push(to);
                }
            }
        }
        Clobbers {
            unstable,
            pres,
            prefix,
        }
    }
}

#[cfg(test)]
thread_local! {
    /// How much of the function's structure the certifier has touched on this thread, counted
    /// where the data is read or written: each control-flow edge handed out, each read of the
    /// dominator tree, each entry of a range table or wavelet matrix built or read, each step
    /// of a wavelet query or of a dominator search's path, each kill of a root walked, and
    /// each read of the `return` counts. Reads of a `Shape` vector that go with a counted edge
    /// or query (`component`, `join_at`) are not counted again.
    pub(super) static READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Counts `n` reads of the function's structure.
#[cfg_attr(
    not(test),
    expect(clippy::missing_const_for_fn, reason = "counts only in tests")
)]
fn count(n: usize) {
    #[cfg(test)]
    READS.set(READS.get() + n);
    #[cfg(not(test))]
    let _ = n;
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
        super::count(edges.len());
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
/// dominates `b` when `pre(a) <= pre(b) <= last(a)`. An unreachable block has `pre` `NONE`.
/// Its fields are private, so every read is counted.
mod tree {
    use super::{Graph, NONE, count, dominators};

    pub(super) struct Tree {
        idom: Vec<usize>,
        pre: Vec<usize>,
        /// The largest pre-order number in each block's subtree.
        last: Vec<usize>,
        depth: Vec<usize>,
        /// The block at each pre-order number.
        at: Vec<usize>,
    }

    impl Tree {
        pub(super) fn new(graph: &Graph) -> Self {
            let n = graph.len();
            let idom = dominators(graph);
            let mut children = vec![Vec::new(); n];
            for (block, &parent) in idom.iter().enumerate() {
                if parent != NONE {
                    children[parent].push(block);
                }
            }
            count(n);
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
                count(1);
                if let Some(&child) = children[block].get(next) {
                    stack.push((block, next + 1));
                    tree.pre[child] = tree.len();
                    tree.depth[child] = tree.depth[block] + 1;
                    tree.at.push(child);
                    stack.push((child, 0));
                } else {
                    tree.last[block] = tree.len() - 1;
                }
            }
            tree
        }

        /// How many blocks are reachable.
        pub(super) fn len(&self) -> usize {
            self.at.len()
        }

        /// `block`'s immediate dominator.
        pub(super) fn parent(&self, block: usize) -> usize {
            count(1);
            self.idom[block]
        }

        pub(super) fn pre(&self, block: usize) -> usize {
            count(1);
            self.pre[block]
        }

        /// The largest pre-order number in `block`'s subtree.
        pub(super) fn last(&self, block: usize) -> usize {
            count(1);
            self.last[block]
        }

        pub(super) fn depth(&self, block: usize) -> usize {
            count(1);
            self.depth[block]
        }

        /// The block at pre-order number `pre`.
        pub(super) fn at(&self, pre: usize) -> usize {
            count(1);
            self.at[pre]
        }

        pub(super) fn reachable(&self, block: usize) -> bool {
            self.pre(block) != NONE
        }

        /// Whether `a` dominates `b`, which must be reachable.
        pub(super) fn dominates(&self, a: usize, b: usize) -> bool {
            let pre = self.pre(b);
            self.pre(a) <= pre && pre <= self.last(a)
        }
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
    // Each block's successors are fetched once, so a switch of many arms costs its arms once.
    number[0] = 0;
    vertex.push(0);
    parent.push(NONE);
    let mut stack = vec![(0, graph.succs(0), 0)];
    while let Some(&mut (block, succs, ref mut next)) = stack.last_mut() {
        if let Some(&to) = succs.get(*next) {
            *next += 1;
            if number[to] == NONE {
                number[to] = vertex.len();
                vertex.push(to);
                parent.push(number[block]);
                stack.push((to, graph.succs(to), 0));
            }
        } else {
            stack.pop();
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
        count(1);
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
pub(super) enum Effect {
    /// A read that leaves the value in place for a copy or a scalar result.
    Read,
    /// A read of the root of a place that the statement uses in place.
    ReadInPlace,
    /// A whole local read in place where the operand is consumed: an error for a counted one.
    ReadConsumed,
    /// A move.
    Consume,
    Release,
    ScopeRelease,
    Assign,
    /// `b = borrow p`.
    AssignBorrowed,
    End,
    /// The root of a place that a `take`, a store or a `make_unique` writes through. It reads
    /// the local in place, as `ReadInPlace` does, and also ends a borrow of it.
    Write,
}

impl Effect {
    /// Whether the effect consumes, assigns or releases the local.
    pub(super) fn kills(self) -> bool {
        match self {
            Effect::Consume
            | Effect::Release
            | Effect::ScopeRelease
            | Effect::Assign
            | Effect::AssignBorrowed => true,
            Effect::Read
            | Effect::ReadInPlace
            | Effect::ReadConsumed
            | Effect::End
            | Effect::Write => false,
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

/// Calls `f` with each local `stmt` mentions, its effect and the place it is the root of. With
/// `lowered`, the mir is as lowering left it, and a `use` or a guard mark consumes its local as a
/// move does.
pub(super) fn statement<'a>(
    stmt: &'a Statement,
    lowered: bool,
    f: &mut impl FnMut(LocalId, Effect, Option<&'a Place>),
) {
    match stmt {
        Statement::Assign { dest, value } => {
            rvalue(value, lowered, f);
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
            self::place(place, Effect::Write, lowered, f);
            f(*dest, Effect::Assign, None);
        }
        Statement::Store { place, value } => {
            self::place(place, Effect::Write, lowered, f);
            consumed(value, lowered, f);
        }
        Statement::Release(local) => f(*local, Effect::Release, None),
        Statement::ScopeRelease(local) => f(*local, Effect::ScopeRelease, None),
        Statement::MakeUnique(place) => self::place(place, Effect::Write, lowered, f),
        Statement::Check(place) => self::place(place, Effect::ReadInPlace, lowered, f),
        Statement::End(local) => f(*local, Effect::End, None),
    }
}

/// The spec's table of operands: a consuming one hands its reference to the receiver.
fn rvalue<'a>(
    value: &'a Rvalue,
    lowered: bool,
    f: &mut impl FnMut(LocalId, Effect, Option<&'a Place>),
) {
    match value {
        // `l = use op` makes `l` own what `op` hands on.
        Rvalue::Use(op) | Rvalue::Unwrap(op) | Rvalue::Wrap(op) | Rvalue::Sum { base: op, .. } => {
            consumed(op, lowered, f);
        }
        Rvalue::Chars(op)
        | Rvalue::Bytes(op)
        | Rvalue::Words(op)
        | Rvalue::FromChars(op)
        | Rvalue::Len(op)
        | Rvalue::LenWords(op)
        | Rvalue::Convert(op)
        | Rvalue::Unary { operand: op, .. }
        | Rvalue::Print { operand: op, .. } => operand(op, lowered, f),
        Rvalue::Copy(place) => self::place(place, Effect::Read, lowered, f),
        Rvalue::Borrow(place) => self::place(place, Effect::ReadInPlace, lowered, f),
        Rvalue::CopyStr(_) | Rvalue::ReadLine => {}
        // A borrowed argument is read in place, and a move into one hands on nothing: the
        // caller still owns the value after the call.
        Rvalue::Call { args, borrowed, .. } => {
            for (op, &borrowed) in args.iter().zip(borrowed) {
                match op {
                    Operand::Move(local) if borrowed => f(*local, Effect::ReadInPlace, None),
                    _ if borrowed => operand(op, lowered, f),
                    _ => consumed(op, lowered, f),
                }
            }
        }
        Rvalue::New { args, .. }
        | Rvalue::Variant { args, .. }
        | Rvalue::ArrayLit(args)
        | Rvalue::Interpolate { parts: args, .. } => {
            for op in args {
                consumed(op, lowered, f);
            }
        }
        Rvalue::Math { operands, .. } => {
            for op in operands {
                operand(op, lowered, f);
            }
        }
        Rvalue::ArrayFill { value, len, .. } => {
            consumed(value, lowered, f);
            operand(len, lowered, f);
        }
        Rvalue::Concat { lhs, rhs }
        | Rvalue::Contains {
            base: lhs,
            value: rhs,
        } => {
            consumed(lhs, lowered, f);
            consumed(rhs, lowered, f);
        }
        Rvalue::Binary { lhs, rhs, .. }
        | Rvalue::Compare { lhs, rhs, .. }
        | Rvalue::Equal { lhs, rhs, .. }
        | Rvalue::Is { lhs, rhs } => {
            operand(lhs, lowered, f);
            operand(rhs, lowered, f);
        }
    }
}

/// `statement`'s visitor for a terminator.
pub(super) fn terminator<'a>(
    term: &'a Terminator,
    lowered: bool,
    f: &mut impl FnMut(LocalId, Effect, Option<&'a Place>),
) {
    match term {
        Terminator::Branch { cond: op, .. } | Terminator::SwitchValue { operand: op, .. } => {
            operand(op, lowered, f);
        }
        Terminator::Return(Some(op)) | Terminator::Panic { message: op, .. } => {
            consumed(op, lowered, f);
        }
        Terminator::IfPresent { optional, .. } => f(*optional, Effect::Read, None),
        Terminator::SwitchTag { place, .. } => self::place(place, Effect::ReadInPlace, lowered, f),
        Terminator::Goto(_) | Terminator::Return(None) | Terminator::Unreachable => {}
    }
}

fn operand<'a>(
    op: &'a Operand,
    lowered: bool,
    f: &mut impl FnMut(LocalId, Effect, Option<&'a Place>),
) {
    match op {
        Operand::Read(place) => self::place(place, Effect::ReadInPlace, lowered, f),
        Operand::Move(local) => f(*local, Effect::Consume, None),
        Operand::Use(local) | Operand::Guarded(local) if lowered => {
            f(*local, Effect::Consume, None);
        }
        Operand::Use(_) | Operand::Guarded(_) => {
            panic!("the ownership pass should leave no `use` and no guard mark")
        }
        Operand::Const(_) => {}
    }
}

/// An operand in a consuming position. Only a move or a copy hands on a reference, so a whole
/// local read in place there hands on one it keeps; a value read through a projection is
/// uncounted, or a copy would have made it owned.
fn consumed<'a>(
    op: &'a Operand,
    lowered: bool,
    f: &mut impl FnMut(LocalId, Effect, Option<&'a Place>),
) {
    match op {
        Operand::Read(place) if place.projections.is_empty() => {
            f(place.local, Effect::ReadConsumed, Some(place));
        }
        Operand::Read(_)
        | Operand::Move(_)
        | Operand::Use(_)
        | Operand::Guarded(_)
        | Operand::Const(_) => operand(op, lowered, f),
    }
}

/// The root of `place` with `root`'s effect, then each index operand.
fn place<'a>(
    place: &'a Place,
    root: Effect,
    lowered: bool,
    f: &mut impl FnMut(LocalId, Effect, Option<&'a Place>),
) {
    f(place.local, root, Some(place));
    for projection in &place.projections {
        if let Projection::Index { index, .. } = projection {
            operand(index, lowered, f);
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
            let rule = if effect == Effect::ReadConsumed {
                Some(Rule::Consume)
            } else if effect.kills() && in_place.contains(&local) {
                Some(Rule::Borrow)
            } else {
                None
            };
            if let Some(rule) = rule
                && found.is_none()
            {
                found = Some(Violation {
                    block: BlockId(b),
                    local,
                    rule,
                });
            }
            if matches!(effect, Effect::ReadInPlace | Effect::Write) {
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
            statement(stmt, false, &mut |local, effect, _| {
                note(at, local, effect, &mut in_place);
            });
        }
        in_place.clear();
        terminator(&block.term, false, &mut |local, effect, _| {
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
/// second `take` of it, a change to a local its indexes read, or the end of the block while
/// it is taken is an error. The last keeps the store's place the one the `take` addressed.
fn takes(function: &Function, tree: &Tree) -> Result<(), Violation> {
    for (b, block) in function.blocks.iter().enumerate() {
        if !tree.reachable(b) {
            continue;
        }
        let mut taken: Vec<&Place> = Vec::new();
        let mut found = None;
        let mut check =
            |taken: &[&Place], local: LocalId, at: Option<&Place>, own: bool, changes| {
                if found.is_none()
                    && taken.iter().any(|t| {
                        (!own && overlaps(t, local, at)) || (changes && indexed_by(t, local))
                    })
                {
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
            statement(stmt, false, &mut |local, effect, at| {
                let own = at
                    .zip(target)
                    .is_some_and(|(at, target)| std::ptr::eq(at, target));
                check(&taken, local, at, own, own || effect.kills());
            });
            if let Statement::Store { place, value } = stmt {
                if let Some(i) = taken.iter().position(|t| *t == place) {
                    taken.swap_remove(i);
                } else if counted_value(function, value) || overlapping(&taken, place) {
                    check(&[place], place.local, None, false, false);
                }
            } else if let Statement::Take { place, .. } = stmt {
                taken.push(place);
            }
        }
        terminator(&block.term, false, &mut |local, _, at| {
            check(&taken, local, at, false, false);
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
pub(super) fn counted_value(function: &Function, value: &Operand) -> bool {
    match value {
        Operand::Move(local) => function.locals[local.0].counted,
        Operand::Read(place) => {
            place.projections.is_empty() && function.locals[place.local.0].counted
        }
        // shortcut: `Type::is_counted`'s kinds without its enum case, which needs the enum
        // table the certifier does not have, so a `None` of an enum optional counts as
        // uncounted; pass the table in if lowering ever stores one without a take.
        Operand::Const(Const::None(ty)) => matches!(
            ty.kind(),
            TypeKind::Optional(payload) if matches!(
                payload.kind(),
                TypeKind::String | TypeKind::Class(_) | TypeKind::Array(_)
            )
        ),
        Operand::Const(_) | Operand::Use(_) | Operand::Guarded(_) => false,
    }
}

/// Whether an index of `t` reads `local`.
fn indexed_by(t: &Place, local: LocalId) -> bool {
    let mut found = false;
    for projection in &t.projections {
        match projection {
            Projection::Index { index, .. } => operand(index, false, &mut |read, _, _| {
                found |= read == local;
            }),
            Projection::Field { .. } | Projection::Payload { .. } | Projection::Some => {}
        }
    }
    found
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

/// Per function, what each local's region queries, over the dominator tree's pre-order.
struct Shape {
    /// Per block, the least depth in the dominator tree where an edge from it meets its
    /// target's dominators: the target's when it dominates the block, else its immediate
    /// dominator's. An edge from `S`'s subtree leaves it exactly when that depth is less than
    /// `S`'s.
    meet: RangeMin,
    /// The same over the edges whose ends share a strongly connected component. A path from
    /// `S` that leaves its subtree comes back to `S` exactly along such an edge.
    back: RangeMin,
    /// Each block's strongly connected component.
    component: Vec<usize>,
    /// The target of each edge `y -> z` where `y` is not `z`'s immediate dominator, by `y`'s
    /// pre-order number, with `z`'s depth as its value. `z` is then in the dominance frontier
    /// of each block from `y` up to the one below `z`'s immediate dominator: of `x` when `y`
    /// is in `x`'s subtree and `z` is no deeper than `x`.
    joins: Wavelet,
    /// The edges from the block at pre-order `i`, at `join_at[i]..join_at[i + 1]`.
    join_at: Vec<usize>,
    /// The `return` blocks before each pre-order number.
    returns: Vec<usize>,
}

impl Shape {
    fn new(function: &Function, graph: &Graph, tree: &Tree) -> Self {
        let component = components(graph, tree);
        let depth = |block: usize| u32::try_from(tree.depth(block)).unwrap_or(u32::MAX);
        let mut meet = Vec::with_capacity(tree.len());
        let mut back = Vec::with_capacity(tree.len());
        let mut joins = Vec::new();
        let mut join_to = Vec::new();
        let mut join_at = Vec::with_capacity(tree.len() + 1);
        let mut returns = Vec::with_capacity(tree.len() + 1);
        returns.push(0);
        for pre in 0..tree.len() {
            let block = tree.at(pre);
            let (mut least, mut least_back) = (u32::MAX, u32::MAX);
            join_at.push(joins.len());
            for &to in graph.succs(block) {
                let meets = if tree.dominates(to, block) {
                    depth(to)
                } else {
                    depth(to) - 1
                };
                least = least.min(meets);
                if component[to] == component[block] {
                    least_back = least_back.min(meets);
                }
                if tree.parent(to) != block {
                    joins.push(depth(to));
                    join_to.push(to);
                }
            }
            meet.push(least);
            back.push(least_back);
            let ends = matches!(function.blocks[block].term, Terminator::Return(_));
            returns.push(returns[returns.len() - 1] + usize::from(ends));
        }
        join_at.push(joins.len());
        Shape {
            meet: RangeMin::new(meet),
            back: RangeMin::new(back),
            component,
            joins: Wavelet::new(&joins, &join_to),
            join_at,
            returns,
        }
    }

    fn returns(&self, from: usize, to: usize) -> bool {
        count(2);
        self.returns[to + 1] > self.returns[from]
    }
}

/// Each reachable block's strongly connected component, by Kosaraju's two walks: a block's
/// post-order along the edges from `bb0`, then backwards along them from each block in reverse
/// post-order. An unreachable block's is `NONE`.
fn components(graph: &Graph, tree: &Tree) -> Vec<usize> {
    let n = graph.len();
    let mut order = Vec::with_capacity(tree.len());
    let mut seen = vec![false; n];
    seen[0] = true;
    let mut stack = vec![(0, graph.succs(0), 0)];
    while let Some(&mut (block, succs, ref mut next)) = stack.last_mut() {
        if let Some(&to) = succs.get(*next) {
            *next += 1;
            if !seen[to] {
                seen[to] = true;
                stack.push((to, graph.succs(to), 0));
            }
        } else {
            order.push(block);
            stack.pop();
        }
    }
    let mut component = vec![NONE; n];
    let mut flood = Vec::new();
    for &root in order.iter().rev() {
        if component[root] != NONE {
            continue;
        }
        component[root] = root;
        flood.push(root);
        while let Some(block) = flood.pop() {
            for &pred in graph.preds(block) {
                if tree.reachable(pred) && component[pred] == NONE {
                    component[pred] = root;
                    flood.push(pred);
                }
            }
        }
    }
    component
}

/// Values at positions, as a wavelet matrix: one bit level per value bit, high to low, each
/// level's bits in the order the levels above sorted the positions by. It reports the items at
/// a range of positions whose values lie in a range, each query in time proportional to its
/// answers and the bits of a value, and the whole in a bit and an item per position.
pub(in crate::mir) mod wavelet {
    use self::matrix::{Level, Matrix};

    /// The levels and items. Their fields are private, so every read, `report`'s too, is
    /// counted.
    mod matrix {
        pub(super) struct Matrix {
            levels: Vec<Level>,
            /// The items in the order the last level sorted them.
            items: Vec<usize>,
        }

        pub(super) struct Level {
            words: Vec<u64>,
            /// The ones before each word.
            ones: Vec<usize>,
            zeros: usize,
        }

        impl Level {
            pub(super) fn new(words: Vec<u64>, zeros: usize) -> Self {
                let mut ones = Vec::with_capacity(words.len() + 1);
                ones.push(0);
                for &word in &words {
                    ones.push(ones[ones.len() - 1] + word.count_ones() as usize);
                }
                Level { words, ones, zeros }
            }
        }

        impl Matrix {
            pub(super) fn new(levels: Vec<Level>, items: Vec<usize>) -> Self {
                super::super::count((levels.len() + 1) * items.len());
                Matrix { levels, items }
            }

            /// How many bit levels there are.
            pub(super) fn levels(&self) -> usize {
                self.levels.len()
            }

            /// The zeros of level `level`.
            pub(super) fn zeros(&self, level: usize) -> usize {
                super::super::count(1);
                self.levels[level].zeros
            }

            /// The zeros of level `level` before position `at`.
            pub(super) fn zeros_before(&self, level: usize, at: usize) -> usize {
                super::super::count(1);
                let this = &self.levels[level];
                let (word, bit) = (at / 64, at % 64);
                let below = this
                    .words
                    .get(word)
                    .map_or(0, |&w| (w & ((1 << bit) - 1)).count_ones() as usize);
                at - this.ones[word] - below
            }

            pub(super) fn item(&self, at: usize) -> usize {
                super::super::count(1);
                self.items[at]
            }
        }
    }

    pub(in crate::mir) struct Wavelet {
        matrix: Matrix,
    }

    impl Wavelet {
        pub(in crate::mir) fn new(values: &[u32], items: &[usize]) -> Self {
            let bits = 32 - values.iter().max().map_or(0, |max| max.leading_zeros());
            let mut order: Vec<usize> = (0..values.len()).collect();
            let mut levels = Vec::with_capacity(bits as usize);
            for bit in (0..bits).rev() {
                let one = |at: usize| values[at] >> bit & 1 == 1;
                let mut words = vec![0u64; values.len().div_ceil(64)];
                for (i, &at) in order.iter().enumerate() {
                    if one(at) {
                        words[i / 64] |= 1 << (i % 64);
                    }
                }
                let (mut low, high): (Vec<usize>, Vec<usize>) =
                    order.iter().partition(|&&at| !one(at));
                let zeros = low.len();
                low.extend(high);
                order = low;
                levels.push(Level::new(words, zeros));
            }
            let items = order.iter().map(|&at| items[at]).collect();
            Wavelet {
                matrix: Matrix::new(levels, items),
            }
        }

        /// Calls `f` with the item at each position of `from..to` whose value lies in
        /// `low..=high`.
        pub(in crate::mir) fn report(
            &self,
            from: usize,
            to: usize,
            low: u32,
            high: u32,
            stack: &mut Vec<(usize, usize, usize, u64)>,
            mut f: impl FnMut(usize),
        ) {
            let bits = self.matrix.levels();
            stack.clear();
            stack.push((0, from, to, 0));
            while let Some((level, from, to, base)) = stack.pop() {
                super::count(1);
                let width = 1u64 << (bits - level);
                if from >= to || base > u64::from(high) || base + width <= u64::from(low) {
                    continue;
                }
                if level == bits {
                    for at in from..to {
                        f(self.matrix.item(at));
                    }
                    continue;
                }
                let (from0, to0) = (
                    self.matrix.zeros_before(level, from),
                    self.matrix.zeros_before(level, to),
                );
                let zeros = self.matrix.zeros(level);
                stack.push((level + 1, from0, to0, base));
                stack.push((
                    level + 1,
                    zeros + from - from0,
                    zeros + to - to0,
                    base + width / 2,
                ));
            }
        }
    }
}

/// The least of fixed values over a range of positions, each query in constant time and the
/// whole in linear memory: per block of 64 positions, each position's mask of the minima of
/// its block so far, and over the blocks a sparse table of their least positions.
pub(in crate::mir) mod range {
    use self::tables::Tables;

    /// The tables. Their fields are private, so every read, `argmin`'s too, is counted.
    mod tables {
        pub(super) struct Tables {
            values: Vec<u32>,
            /// Bit `j` of `masks[i]`: the position `j` of `i`'s block holds a value less than
            /// every one after it up to `i`.
            masks: Vec<u64>,
            /// `table[k][b]`: the position of the least value in blocks `b..b + 2^k`.
            table: Vec<Vec<usize>>,
        }

        impl Tables {
            pub(super) fn new(values: Vec<u32>, masks: Vec<u64>, table: Vec<Vec<usize>>) -> Self {
                super::super::count(
                    values.len() + masks.len() + table.iter().map(Vec::len).sum::<usize>(),
                );
                Tables {
                    values,
                    masks,
                    table,
                }
            }

            pub(super) fn value(&self, at: usize) -> u32 {
                super::super::count(1);
                self.values[at]
            }

            pub(super) fn mask(&self, at: usize) -> u64 {
                super::super::count(1);
                self.masks[at]
            }

            pub(super) fn table(&self, k: usize, block: usize) -> usize {
                super::super::count(1);
                self.table[k][block]
            }
        }
    }

    pub(in crate::mir) struct RangeMin {
        tables: Tables,
    }

    impl RangeMin {
        pub(in crate::mir) fn new(values: Vec<u32>) -> Self {
            let mut masks = vec![0; values.len()];
            let mut least = Vec::with_capacity(values.len().div_ceil(64));
            for (b, chunk) in values.chunks(64).enumerate() {
                let mut mask: u64 = 0;
                for (j, &value) in chunk.iter().enumerate() {
                    while mask != 0 {
                        let top = 63 - mask.leading_zeros() as usize;
                        if chunk[top] < value {
                            break;
                        }
                        mask &= !(1 << top);
                    }
                    mask |= 1 << j;
                    masks[b * 64 + j] = mask;
                }
                least.push(b * 64 + mask.trailing_zeros() as usize);
            }
            let less = |a: usize, b: usize| if values[b] < values[a] { b } else { a };
            let mut table = vec![least];
            while 1 << table.len() <= table[0].len() {
                let half = 1 << (table.len() - 1);
                let below = &table[table.len() - 1];
                let next = (0..below.len() - half)
                    .map(|b| less(below[b], below[b + half]))
                    .collect();
                table.push(next);
            }
            RangeMin {
                tables: Tables::new(values, masks, table),
            }
        }

        pub(in crate::mir) fn get(&self, at: usize) -> u32 {
            self.tables.value(at)
        }

        /// The position of a least value at `from..=to`.
        pub(in crate::mir) fn argmin(&self, from: usize, to: usize) -> usize {
            let (first, last) = (from / 64, to / 64);
            if first == last {
                return self.within(from, to);
            }
            let mut best = self.less(
                self.within(from, first * 64 + 63),
                self.within(last * 64, to),
            );
            if first + 1 < last {
                let (a, b) = (first + 1, last - 1);
                let k = (b - a + 1).ilog2() as usize;
                let blocks = self.less(
                    self.tables.table(k, a),
                    self.tables.table(k, b + 1 - (1 << k)),
                );
                best = self.less(best, blocks);
            }
            best
        }

        pub(in crate::mir) fn min(&self, from: usize, to: usize) -> u32 {
            self.get(self.argmin(from, to))
        }

        /// The least at `from..=to`, both in one block: the first minimum at or after `from`
        /// that is still one at `to`.
        fn within(&self, from: usize, to: usize) -> usize {
            let mask = self.tables.mask(to) & (u64::MAX << (from % 64));
            (to & !63) + mask.trailing_zeros() as usize
        }

        fn less(&self, a: usize, b: usize) -> usize {
            if self.get(b) < self.get(a) { b } else { a }
        }
    }
}

/// The buffers of one local's region, kept for the next local. Per block, the last region
/// stamped there, so nothing is cleared between locals.
struct Scratch {
    stamp: Vec<usize>,
    next: usize,
    /// Each node block's events, at `events[from[block]..to[block]]`.
    from: Vec<usize>,
    to: Vec<usize>,
    /// The node blocks, in pre-order: the scope block first.
    blocks: Vec<usize>,
    /// The nodes whose frontier is still to search, deepest first.
    work: BinaryHeap<(usize, usize)>,
    /// The subtrees searched, as pre-order ranges keyed by their first number.
    searched: BTreeMap<usize, usize>,
    /// Ranges of positions in `Shape::joins` to search, and the search's own stack.
    ranges: Vec<(usize, usize)>,
    found: Vec<usize>,
    levels: Vec<(usize, usize, usize, u64)>,
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
            next: 0,
            from: vec![0; blocks],
            to: vec![0; blocks],
            blocks: Vec::new(),
            work: BinaryHeap::new(),
            searched: BTreeMap::new(),
            ranges: Vec::new(),
            found: Vec::new(),
            levels: Vec::new(),
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
    /// The function's clobbers, when the local is unstable.
    clobbers: Option<&'a Clobbers>,
}

impl Region<'_> {
    fn violation(&self, block: usize, rule: Rule) -> Violation {
        Violation {
            block: BlockId(block),
            local: self.local,
            rule,
        }
    }

    /// Runs the region's fixpoint. `observe` sees each event with the set it is applied to.
    fn check(
        &self,
        scratch: &mut Scratch,
        observe: &mut impl FnMut(&Event, States),
    ) -> Result<(), Violation> {
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
        let init = if self.borrowed_param() {
            BORROWED
        } else if param {
            OWNED
        } else {
            EMPTY
        };
        // What reaches the scope block from outside the region: empty, and ended once a
        // borrow's end or a release leaves it ended along a path that comes back.
        let mut from_outside = if outside { EMPTY } else { 0 };
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
            if i == 0 {
                states |= from_outside;
            }
            for &(from, pred) in &s.inputs[s.input_at[i]..s.input_at[i + 1]] {
                states |= self.edge(s.blocks[from], s.out[from], pred, block);
            }
            let (out, back) = self.transfer(s, i, states, observe)?;
            if back & ENDED != 0 && from_outside & ENDED == 0 {
                from_outside |= ENDED;
                // The queue takes the least node first, so the scope block is never queued here.
                s.queued[0] = true;
                s.queue.push(Reverse(0));
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
        self.frontier(s);
        s.blocks.sort_unstable_by_key(|&block| tree.pre(block));
        let mut outside = false;
        s.queries.clear();
        for (i, &block) in s.blocks.iter().enumerate() {
            for &pred in self.graph.preds(block) {
                if inside(pred) {
                    s.queries.push((tree.pre(pred), pred, i));
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
            let node_pre = s.blocks.get(next).map_or(NONE, |&block| tree.pre(block));
            let query_pre = s.queries.get(query).map_or(NONE, |query| query.0);
            let pre = node_pre.min(query_pre);
            while let Some(&top) = s.stack.last()
                && tree.last(s.blocks[top]) < pre
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

    /// Adds the iterated dominance frontier of the nodes, within the region, deepest node
    /// first. A node's frontier within the region is the targets of the join edges from its
    /// subtree that are deeper than the scope block and no deeper than the node. A subtree
    /// searched already holds no other such edge, since a node below the one searching is
    /// deeper, so each edge is found at most once. The scope block's frontier within the
    /// region is at most itself.
    ///
    /// The query is two-sided because a one-sided one, all edges no deeper than the node,
    /// also finds the edges that leave the region. With `n` locals each mentioned in an `if`
    /// whose body holds many `break`s, every local would find every `break`: `O(n * breaks)`.
    fn frontier(&self, s: &mut Scratch) {
        let (tree, shape) = (self.tree, self.shape);
        let low = u32::try_from(tree.depth(self.scope) + 1).unwrap_or(u32::MAX);
        s.work.clear();
        s.searched.clear();
        for &block in &s.blocks[1..] {
            s.work.push((tree.depth(block), block));
        }
        while let Some((depth, block)) = s.work.pop() {
            let (first, last) = (tree.pre(block), tree.last(block));
            let mut from = first;
            while let Some((&inner, &end)) = s.searched.range(first..=last).next() {
                s.ranges.push((shape.join_at[from], shape.join_at[inner]));
                s.searched.remove(&inner);
                from = end + 1;
            }
            s.ranges
                .push((shape.join_at[from], shape.join_at[last + 1]));
            s.searched.insert(first, last);
            let high = u32::try_from(depth).unwrap_or(u32::MAX);
            while let Some((a, b)) = s.ranges.pop() {
                let found = &mut s.found;
                shape
                    .joins
                    .report(a, b, low, high, &mut s.levels, |join| found.push(join));
            }
            for at in 0..s.found.len() {
                let join = s.found[at];
                if s.add(join) {
                    s.work.push((tree.depth(join), join));
                }
            }
            s.found.clear();
        }
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
    /// blocks it covers (those of its subtree outside its node children's) along a path back
    /// to the scope block. Checks the rules on the way.
    fn transfer(
        &self,
        s: &Scratch,
        i: usize,
        mut states: States,
        observe: &mut impl FnMut(&Event, States),
    ) -> Result<(States, States), Violation> {
        let block = s.blocks[i];
        let kills = self
            .root
            .map_or(&[][..], |root| by_block(&root.events, block));
        let events = &self.events[s.from[block]..s.to[block]];
        // The kills up to the first event's statement all meet the entry set, so one test
        // stands for them, and a region walks only the kills inside its own events.
        let skip = events
            .first()
            .map_or(0, |first| kills.partition_point(|kill| kill.at <= first.at));
        if skip > 0 && states & BORROWED != 0 {
            return Err(self.violation(block, Rule::Borrow));
        }
        let mut kills = kills[skip..].iter().peekable();
        // Whether a clobber lies in statements `from..to`, one test per gap between events.
        let prefix = self.clobbers.map(|clobbers| &clobbers.prefix[block]);
        let clobbered = |from: usize, to: usize| {
            prefix.is_some_and(|prefix| {
                count(1);
                prefix[to] != prefix[from]
            })
        };
        let mut previous = 0;
        for event in events {
            // The root's kills in a statement come before the statement's own effects, and its
            // clobber after them: a call reads its arguments first.
            while kills.next_if(|kill| kill.at <= event.at).is_some() {
                count(1);
                if states & BORROWED != 0 {
                    return Err(self.violation(block, Rule::Borrow));
                }
            }
            if clobbered(previous, event.at) && states & BORROWED != 0 {
                return Err(self.violation(block, Rule::Borrow));
            }
            previous = event.at;
            observe(event, states);
            states = self.apply(block, event.effect, states)?;
        }
        let end = self.function.blocks[block].stmts.len();
        if (kills.next().is_some() || clobbered(previous, end)) && states & BORROWED != 0 {
            return Err(self.violation(block, Rule::Borrow));
        }
        if let Terminator::Return(_) = self.function.blocks[block].term {
            self.at_return(block, states)?;
        }
        let mut back = 0;
        for &to in self.graph.succs(block) {
            if !self.tree.dominates(self.scope, to) {
                let states = self.edge(block, states, block, to);
                self.leave(block, states)?;
                if self.shape.component[to] == self.shape.component[block] {
                    back |= states;
                }
            }
        }
        let tree = self.tree;
        let mut from = tree.pre(block) + 1;
        let mut child = i + 1;
        while from <= tree.last(block) {
            let to = if child < s.end[i] {
                tree.pre(s.blocks[child])
            } else {
                tree.last(block) + 1
            };
            if from < to {
                back |= self.cover(from, to - 1, states)?;
            }
            if child >= s.end[i] {
                break;
            }
            from = tree.last(s.blocks[child]) + 1;
            child = s.end[child];
        }
        Ok((states, back))
    }

    /// Checks the blocks at pre-order `from..=to`, which hold `states` throughout: the root's
    /// kills, the returns and the exits. The states that leave the region from them along a
    /// path back to the scope block.
    fn cover(&self, from: usize, to: usize, states: States) -> Result<States, Violation> {
        if states & BORROWED != 0 {
            let root = self.root.map(|root| &root.pres);
            let clobbers = self.clobbers.map(|clobbers| &clobbers.pres);
            for kills in root.into_iter().chain(clobbers) {
                let at = kills.partition_point(|&pre| pre < from);
                if kills.get(at).is_some_and(|&pre| pre <= to) {
                    return Err(self.violation(self.tree.at(kills[at]), Rule::Borrow));
                }
            }
        }
        // Only a violation needs its block, which a search over the range finds.
        let owes = states & (OWNED | BORROWED) != 0;
        if owes && self.shape.returns(from, to) {
            let at = (from..=to)
                .find(|&pre| self.shape.returns(pre, pre))
                .expect("a range with a return should hold a return block");
            self.at_return(self.tree.at(at), states)?;
        }
        let depth = self.tree.depth(self.scope);
        let at = self.shape.meet.argmin(from, to);
        if self.shape.meet.get(at) as usize >= depth {
            return Ok(0);
        }
        self.leave(self.tree.at(at), states)?;
        if (self.shape.back.min(from, to) as usize) < depth {
            Ok(states)
        } else {
            Ok(0)
        }
    }

    /// One effect on the set, checked against rules 1, 2, 5, a write while borrowed and the end
    /// of a borrow.
    fn apply(&self, block: usize, effect: Effect, states: States) -> Result<States, Violation> {
        let (bad, rule, after) = match effect {
            // `mentions` rejects a counted `ReadConsumed` before any region is checked.
            Effect::Read | Effect::ReadInPlace | Effect::ReadConsumed | Effect::Write => {
                (EMPTY | ENDED, Rule::Read, states)
            }
            Effect::Consume => (EMPTY | BORROWED | ENDED, Rule::Consume, EMPTY),
            Effect::Release => (EMPTY | BORROWED | ENDED, Rule::Consume, ENDED),
            // An empty slot holds the zero value where a `release?` can read it; only an owned
            // value is freed and left in the slot.
            Effect::ScopeRelease => (
                BORROWED | ENDED,
                Rule::Consume,
                states & EMPTY | if states & OWNED == 0 { 0 } else { ENDED },
            ),
            Effect::Assign => (OWNED, Rule::Assign, OWNED),
            Effect::AssignBorrowed => (OWNED, Rule::Assign, BORROWED),
            Effect::End => (OWNED, Rule::End, ENDED),
        };
        if states & bad != 0 {
            return Err(self.violation(block, rule));
        }
        if effect == Effect::Write
            && states & BORROWED != 0
            && !matches!(
                self.function.locals[self.local.0].ty.kind(),
                TypeKind::Class(_)
            )
        {
            return Err(self.violation(block, Rule::WriteBorrowed));
        }
        Ok(after)
    }

    /// Whether the local is a borrowed parameter, whose borrow lasts the whole body.
    fn borrowed_param(&self) -> bool {
        self.function.borrowed.get(self.local.0) == Some(&true)
    }

    /// Rule 4, and a borrow that reaches a `return`.
    fn at_return(&self, block: usize, states: States) -> Result<(), Violation> {
        if states & OWNED != 0 {
            return Err(self.violation(block, Rule::Return));
        }
        if states & BORROWED != 0 && !self.borrowed_param() {
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
