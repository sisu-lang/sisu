//! Borrowed-parameter inference (ADR 0029): a counted parameter is borrowed unless its function
//! consumes it, and a call passes a borrowed argument in place.
//!
//! The walk reads the mir as lowering left it, through the certifier's visitor with `lowered`,
//! where a `use` consumes its local as a move does. A local is consumed when it is moved into a
//! field, an aggregate, a return, a named local, a join's result or an operation that takes its
//! operand, such as `concat`. One graph covers the whole program. Its nodes are each function's
//! locals, and an edge `u → v` means that if `u` is consumed, so is `v`:
//!
//! - **alias:** `d = use l`, where `d` is a temporary that this statement alone assigns and that
//!   only its own block mentions, gives `d → l`. Lowering reads a value a statement only reads
//!   into such a temporary: `print(s)` is `_1 = use s; print(_1); release _1`. A named `d`, one
//!   assigned on several paths to a join, or one used or released in another block, such as a
//!   spilled operand of a call, consumes `l` instead: pairs (part 3) cannot drop a copy whose
//!   release is in another block.
//! - **owners:** a borrowed binding consumes the local it borrows from (the backward rule).
//! - **call:** an argument naming `l` at position `i` of a call to `g` gives `(g, i) → l`.
//! - **tail:** a call is in tail position when only scope releases and `end`s, through `goto`s,
//!   separate it from a `return` of its result, or from a `return` with no value for a call with
//!   none. For a tail call to `g` in the caller's own strongly connected component, an argument
//!   at position `i` that is a parameter `p`, or a borrowed binding rooted at `p`, gives
//!   `p → (g, i)`; any other argument, a fresh value or a local the caller owns, makes `(g, i)`
//!   owned outright. Otherwise the caller would release the argument after the call, and the
//!   call would not be a tail call. A temporary that only pins a local stands for that local.
//!
//! Every other consumption, each parameter of `main`, which the C entry point hands over owned,
//! and each parameter lowering marks in `reuses`, whose box a `match` that is its last mention
//! and in no loop would build a new box into (part 5), seeds one worklist search that visits each
//! edge at most once. A parameter is owned when the search reaches it. That is what solving the
//! call graph one component at a time, callees first, would give, without scanning a component
//! twice.
//!
//! The hoist (ADR 0021) stays sound. A copy of a borrowed parameter that outlives the call is
//! returned, stored, kept in a named local or passed to an owned parameter, and each of those
//! consumes the parameter. So every copy a callee makes of a borrowed parameter is a temporary it
//! releases before it returns, and the caller's count is what it was when the call began.

use std::mem;

use super::certify::{self, Effect};
use super::{Function, LocalId, Operand, Place, Rvalue, Statement, Terminator};
use crate::graph::components;
use crate::tir::{self, FuncId};

/// The flags lowering starts from: every counted parameter is borrowed, except those of `main`.
pub(super) fn assumed(program: &tir::Program) -> Vec<Vec<bool>> {
    program
        .functions
        .iter()
        .map(|f| {
            f.params
                .iter()
                .map(|param| f.name != "main" && f.locals[param.0].ty.is_counted(&program.enums))
                .collect()
        })
        .collect()
}

/// Per function and parameter, whether the parameter is borrowed: counted, and not consumed.
/// `functions` is lowered mir, before any pass, and `reuses` marks the parameters it seeds owned.
pub(super) fn infer(functions: &[Function], main: FuncId, reuses: &[Vec<bool>]) -> Vec<Vec<bool>> {
    let mut first = Vec::with_capacity(functions.len());
    let mut nodes = 0;
    for function in functions {
        first.push(nodes);
        nodes += function.locals.len();
    }
    let calls: Vec<Vec<usize>> = functions.iter().map(callees).collect();
    let component = components(&calls);
    let mut graph = Graph {
        edges: vec![Vec::new(); nodes],
        owned: vec![false; nodes],
        work: Vec::new(),
    };
    for param in 0..functions[main.0].params {
        graph.seed(first[main.0] + param);
    }
    for (id, marks) in reuses.iter().enumerate() {
        for (param, _) in marks.iter().enumerate().filter(|&(_, &marked)| marked) {
            graph.seed(first[id] + param);
        }
    }
    for (id, function) in functions.iter().enumerate() {
        Walk {
            function,
            id,
            first: &first,
            component: &component,
            graph: &mut graph,
        }
        .run();
    }
    graph.search();
    functions
        .iter()
        .zip(&first)
        .map(|(function, &at)| {
            (0..function.params)
                .map(|param| function.locals[param].counted && !graph.owned[at + param])
                .collect()
        })
        .collect()
}

/// Sets each function's `quiet` and each call's, over the call graph's components, callees
/// first. `functions` are placed, hoisted and elaborated: those steps add clobbers.
///
/// A prelude function is quiet when no statement of its body is a clobber (`clobbers`), a call
/// of a quiet function aside. A component is quiet when all its members are prelude functions
/// and none has any other clobber, so a call inside the component is no clobber. A user
/// function is never quiet, so no program that calls no prelude function changes. Each
/// statement of each prelude function is visited once.
pub(super) fn quiet(functions: &mut [Function]) {
    let calls: Vec<Vec<usize>> = functions.iter().map(callees).collect();
    let component = components(&calls);
    let mut members = vec![Vec::new(); component.iter().max().map_or(0, |&last| last + 1)];
    for (id, &at) in component.iter().enumerate() {
        members[at].push(id);
    }
    let mut quiet = vec![false; functions.len()];
    for group in &members {
        let whole = group.iter().all(|&id| {
            let function = &functions[id];
            function.prelude
                && function.blocks.iter().flat_map(|b| &b.stmts).all(|stmt| {
                    #[cfg(test)]
                    VISITED.set(VISITED.get() + 1);
                    match stmt {
                        Statement::Assign {
                            value: Rvalue::Call { func, .. },
                            ..
                        } => component[func.0] == component[id] || quiet[func.0],
                        _ => !clobbers(function, stmt),
                    }
                })
        });
        for &id in group {
            quiet[id] = whole;
        }
    }
    for (function, &flag) in functions.iter_mut().zip(&quiet) {
        function.quiet = flag;
        for stmt in function.blocks.iter_mut().flat_map(|b| &mut b.stmts) {
            if let Statement::Assign {
                value: Rvalue::Call {
                    func, quiet: at, ..
                },
                ..
            } = stmt
            {
                *at = quiet[func.0];
            }
        }
    }
}

/// Whether `stmt` may free or change what a borrow through a `mut` field or an element reads:
/// a call that is not quiet, a statement that runs a written method, a `take` into a counted
/// local, a store of a counted value, and a `make_unique`. These are the clobbers of pair
/// removal and of the certifier.
pub(super) fn clobbers(function: &Function, stmt: &Statement) -> bool {
    match stmt {
        Statement::Assign { value, .. } => value.may_call_written(),
        Statement::Take { dest, .. } => function.locals[dest.0].counted,
        Statement::Store { value, .. } => certify::counted_value(function, value),
        Statement::MakeUnique(_) => true,
        Statement::Release(_)
        | Statement::ScopeRelease(_)
        | Statement::Check(_)
        | Statement::End(_)
        | Statement::TakeApart { .. }
        | Statement::Free(_) => false,
    }
}

/// Sets each function's `borrowed` and each call's flags from `modes`, and drops the scope
/// releases of borrowed parameters. At a borrowed position, a `use` becomes a borrow site, and a
/// moved temporary is read in place and released after the call.
pub(super) fn rewrite(functions: &mut [Function], modes: &[Vec<bool>]) {
    let mut after = Vec::new();
    for (function, borrowed) in functions.iter_mut().zip(modes) {
        function.borrowed.clone_from(borrowed);
        for block in &mut function.blocks {
            let stmts = mem::take(&mut block.stmts);
            block.stmts.reserve(stmts.len());
            for mut stmt in stmts {
                if let Statement::ScopeRelease(local) = stmt
                    && borrowed.get(local.0) == Some(&true)
                {
                    continue;
                }
                if let Statement::Assign {
                    value:
                        Rvalue::Call {
                            func,
                            args,
                            borrowed: flags,
                            ..
                        },
                    ..
                } = &mut stmt
                {
                    flags.clone_from(&modes[func.0]);
                    for (arg, _) in args.iter_mut().zip(flags.iter()).filter(|(_, b)| **b) {
                        *arg = match *arg {
                            Operand::Use(local) => read(local, true, false),
                            Operand::Guarded(local) => read(local, true, true),
                            Operand::Move(temp) => {
                                after.push(Statement::Release(temp));
                                read(temp, false, false)
                            }
                            Operand::Read(_) | Operand::Const(_) => continue,
                        };
                    }
                }
                block.stmts.push(stmt);
                block.stmts.append(&mut after);
            }
        }
    }
}

/// A read of `local` whole: a borrow site when `borrow`, marked when `guard`.
const fn read(local: LocalId, borrow: bool, guard: bool) -> Operand {
    Operand::Read(Place {
        local,
        projections: Vec::new(),
        borrow,
        guard,
    })
}

/// The functions `function` calls.
fn callees(function: &Function) -> Vec<usize> {
    let mut called = Vec::new();
    for block in &function.blocks {
        for stmt in &block.stmts {
            if let Statement::Assign {
                value: Rvalue::Call { func, .. },
                ..
            } = stmt
            {
                called.push(func.0);
            }
        }
    }
    called
}

/// The local an operand reads whole and consumes.
const fn named(op: &Operand) -> Option<LocalId> {
    match *op {
        Operand::Use(local) | Operand::Move(local) | Operand::Guarded(local) => Some(local),
        Operand::Read(_) | Operand::Const(_) => None,
    }
}

#[cfg(test)]
thread_local! {
    /// How many statements `quiet` has visited on this thread.
    pub(super) static VISITED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
thread_local! {
    /// How many edges `infer`'s search has followed on this thread.
    pub(super) static FOLLOWED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The program's graph of consuming uses, and the search's state.
struct Graph {
    edges: Vec<Vec<usize>>,
    owned: Vec<bool>,
    work: Vec<usize>,
}

impl Graph {
    fn edge(&mut self, from: usize, to: usize) {
        self.edges[from].push(to);
    }

    /// Marks `node` consumed.
    fn seed(&mut self, node: usize) {
        if !mem::replace(&mut self.owned[node], true) {
            self.work.push(node);
        }
    }

    /// Marks every node an edge reaches from a marked one, following each edge once.
    fn search(&mut self) {
        while let Some(node) = self.work.pop() {
            for i in 0..self.edges[node].len() {
                #[cfg(test)]
                FOLLOWED.set(FOLLOWED.get() + 1);
                self.seed(self.edges[node][i]);
            }
        }
    }
}

/// Where a block's terminator leads through blocks of only scope releases and `end`s.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Exit {
    /// Anywhere else.
    Other,
    /// A `return` of this local, or with no value.
    Return(Option<LocalId>),
}

/// The edges and seeds of one function.
struct Walk<'a> {
    function: &'a Function,
    id: usize,
    first: &'a [usize],
    component: &'a [usize],
    graph: &'a mut Graph,
}

impl Walk<'_> {
    fn node(&self, local: LocalId) -> usize {
        self.first[self.id] + local.0
    }

    fn run(&mut self) {
        let function = self.function;
        // How many statements assign each local, whether only by `borrow`, the local a
        // `d = use l` reads, and whether the local is mentioned in more than one block.
        let mut assigned = vec![0_usize; function.locals.len()];
        let mut borrows = vec![true; function.locals.len()];
        let mut reads = vec![None; function.locals.len()];
        let mut home = vec![None; function.locals.len()];
        let mut spread = vec![false; function.locals.len()];
        for (b, block) in function.blocks.iter().enumerate() {
            let mut mention = |local: LocalId| {
                spread[local.0] |= *home[local.0].get_or_insert(b) != b;
            };
            certify::terminator(&block.term, true, &mut |local, _, _| mention(local));
            for stmt in &block.stmts {
                certify::statement(stmt, true, &mut |local, effect, _| {
                    mention(local);
                    if matches!(effect, Effect::Assign | Effect::AssignBorrowed) {
                        assigned[local.0] += 1;
                        borrows[local.0] &= effect == Effect::AssignBorrowed;
                    }
                });
                if let Statement::Assign {
                    dest: Some(dest),
                    value: Rvalue::Use(op),
                } = stmt
                {
                    reads[dest.0] = named(op);
                }
            }
        }
        // The local a temporary that only pins it stands for: pairs can drop the temporary's
        // copy only when its release is in the same block.
        let alias = |local: LocalId| {
            reads[local.0].filter(|_| {
                function.locals[local.0].name.is_none()
                    && assigned[local.0] == 1
                    && !spread[local.0]
            })
        };
        let mut exits = None;
        for (b, block) in function.blocks.iter().enumerate() {
            for (at, stmt) in block.stmts.iter().enumerate() {
                if let Statement::Assign {
                    dest: Some(dest),
                    value: Rvalue::Use(_),
                } = stmt
                    && let Some(local) = alias(*dest)
                {
                    self.graph.edge(self.node(*dest), self.node(local));
                } else if let Statement::Assign {
                    dest,
                    value: Rvalue::Call { func, args, .. },
                } = stmt
                {
                    let tail = self.component[func.0] == self.component[self.id]
                        && block.stmts[at + 1..].iter().all(cleanup)
                        && exits.get_or_insert_with(|| exits_of(function))[b]
                            == Exit::Return(*dest);
                    for (i, arg) in args.iter().enumerate() {
                        let Some(local) = named(arg) else { continue };
                        let param = self.first[func.0] + i;
                        self.graph.edge(param, self.node(local));
                        if tail {
                            self.tail_argument(alias(local).unwrap_or(local), param, &borrows);
                        }
                    }
                } else {
                    certify::statement(stmt, true, &mut |local, effect, _| {
                        if effect == Effect::Consume {
                            self.graph.seed(self.node(local));
                        }
                    });
                }
            }
            certify::terminator(&block.term, true, &mut |local, effect, _| {
                if effect == Effect::Consume {
                    self.graph.seed(self.node(local));
                }
            });
        }
        for (&binding, &owner) in &function.owners {
            self.graph.edge(self.node(binding), self.node(owner));
        }
    }

    /// The tail rule for `local`, passed at the callee's node `param`.
    fn tail_argument(&mut self, local: LocalId, param: usize, borrows: &[bool]) {
        let function = self.function;
        if local.0 < function.params {
            self.graph.edge(self.node(local), param);
        } else if function.owners.contains_key(&local) && borrows[local.0] {
            if let Some(root) = certify::root(function, local)
                && root.0 < function.params
            {
                self.graph.edge(self.node(root), param);
            }
        } else {
            self.graph.seed(param);
        }
    }
}

/// Whether a statement may sit between a tail call and its `return`.
const fn cleanup(stmt: &Statement) -> bool {
    matches!(
        stmt,
        Statement::ScopeRelease(_) | Statement::End(_) | Statement::Free(_)
    )
}

/// Each block's `Exit`, each chain of blocks followed once.
fn exits_of(function: &Function) -> Vec<Exit> {
    let blocks = &function.blocks;
    let mut exits: Vec<Option<Exit>> = vec![None; blocks.len()];
    let mut on_path = vec![false; blocks.len()];
    let mut path = Vec::new();
    for start in 0..blocks.len() {
        let mut block = start;
        let exit = loop {
            if let Some(exit) = exits[block] {
                break exit;
            }
            path.push(block);
            on_path[block] = true;
            match &blocks[block].term {
                Terminator::Return(None) => break Exit::Return(None),
                Terminator::Return(Some(Operand::Move(local))) => break Exit::Return(Some(*local)),
                Terminator::Return(Some(Operand::Read(place))) if place.projections.is_empty() => {
                    break Exit::Return(Some(place.local));
                }
                Terminator::Goto(to)
                    if !on_path[to.0] && blocks[to.0].stmts.iter().all(cleanup) =>
                {
                    block = to.0;
                }
                Terminator::Goto(_)
                | Terminator::Return(Some(_))
                | Terminator::Branch { .. }
                | Terminator::IfPresent { .. }
                | Terminator::SwitchTag { .. }
                | Terminator::TestPresent { .. }
                | Terminator::SwitchValue { .. }
                | Terminator::Panic { .. }
                | Terminator::Unreachable => break Exit::Other,
            }
        };
        for block in path.drain(..) {
            exits[block] = Some(exit);
            on_path[block] = false;
        }
    }
    exits
        .into_iter()
        .map(|exit| exit.expect("every block should have its exit"))
        .collect()
}
