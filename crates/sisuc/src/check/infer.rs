//! The inference table of one top-level function. It holds the inference variables
//! (`TypeKind::Infer`) and the checks that wait for the type of one. `write_back` deals with each
//! variable that is left when the function is checked.
//!
//! A variable is a union-find node with a kind on its root. Two variables that unify share a root.
//! A root gets a type once, when it unifies with a type that is no variable. A snapshot marks the
//! undo log. A rollback removes each union, each bind, each queued check and each check made since
//! the mark. A check that waits for a variable sits at that variable, and a bind wakes only the
//! checks of its own set.
//! The ids start at 0 in each table, so a type that holds a variable means something only in the
//! table that made it: no check-wide cache keeps a key that holds one (`Type::holds_infer`).

use std::cell::Cell;

use rustc_hash::{FxHashMap, FxHashSet};

use super::{Checker, Poisoned};
use crate::conformance::ensure_stack;
use crate::diagnostic::{DiagnosticKind, Literal};
use crate::source::Span;
use crate::tir::{self, ExprKind, Type, TypeArgs, TypeKind};
use crate::types::InferId;

#[cfg(test)]
thread_local! {
    /// The obligations that a bind has looked at.
    static OBLIGATION_VISITS: Cell<usize> = const { Cell::new(0) };
    /// The steps to keep each root of a type once: one for each root met.
    static ROOT_STEPS: Cell<usize> = const { Cell::new(0) };
    /// The types that `rebuild_in` has rebuilt: the ones that its memo did not hold.
    static REBUILDS: Cell<usize> = const { Cell::new(0) };
}

/// What a variable stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// An integer literal: it settles to `i64`, `u8` or `f64`.
    Int,
    /// An unknown type: the payload of `None`, the element of `[]`, a type parameter of a call.
    Any,
}

/// The value and sign of an integer literal in an `Int` variable, which E0040, E0042 and E0104
/// check when the variable gets its type. `value` is the magnitude: a literal is negative only
/// as `-9223372036854775808`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LiteralCheck {
    pub(super) span: Span,
    pub(super) value: u64,
    pub(super) negative: bool,
}

/// A place in the list of obligations that `generic.rs` and `conform.rs` own. The table holds it
/// and does not read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ObligationId(pub(super) usize);

/// A check that was waiting for a type, and can run now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Waiting {
    /// The literal fits `ty`, the type of its variable, or it does not.
    Literal { check: LiteralCheck, ty: Type },
    /// The obligation can run on `ty`, the type that held an `Any` variable, which has no open
    /// variable now.
    Obligation { id: ObligationId, ty: Type },
}

/// An `Any` variable that no type reached, and that no failed use took: E0050 at `origin`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Unsettled {
    pub(super) origin: Span,
}

/// What is known of the set of variables that one root stands for.
#[derive(Clone, Copy)]
struct Root {
    /// How many variables are in the set: a union puts the smaller set under the larger.
    size: u32,
    kind: Kind,
    bound: Option<Type>,
    /// A failed use reached the set: it reports nothing more.
    poisoned: bool,
    /// `write_back` has dealt with the set.
    reported: bool,
}

struct Slot {
    /// This variable's own id for a root. A cell, so that `find` halves a path through `&self`.
    parent: Cell<u32>,
    /// The next variable of the set, in a circle: two sets join by swapping one `next` of each.
    next: u32,
    origin: Span,
    /// The literals that were made for this variable, not for its set.
    literals: Vec<LiteralCheck>,
    /// The obligations that wait for this variable: each one waits at the one variable that it
    /// needs next. An entry that the obligation has left is stale, and `bind` skips it.
    waiting: Vec<usize>,
    /// Read for a root only.
    root: Root,
}

struct Obligation {
    ty: Type,
    id: ObligationId,
    done: bool,
    /// The variable whose `waiting` list holds this obligation now.
    at: u32,
}

/// What a rollback undoes, most recent last.
enum Undo {
    Union {
        child: u32,
        root: u32,
        was: Root,
    },
    Bind {
        root: u32,
    },
    Poison {
        root: u32,
    },
    Done {
        obligation: usize,
    },
    /// A literal check was made for the variable `at`.
    Literal {
        at: u32,
    },
    /// An obligation was made, waiting at the variable `at`.
    Obligation {
        at: u32,
    },
    /// The obligation moved from the variable `from` to the variable `to`.
    Move {
        obligation: usize,
        from: u32,
        to: u32,
    },
}

/// A mark in the undo log and in the queue of ready checks.
#[derive(Clone, Copy, Debug)]
pub(super) struct Snapshot {
    log: usize,
    ready: usize,
}

#[derive(Default)]
pub(super) struct Table {
    slots: Vec<Slot>,
    obligations: Vec<Obligation>,
    log: Vec<Undo>,
    /// The checks that became ready inside a snapshot, until the snapshot is kept.
    ready: Vec<Waiting>,
    /// How many snapshots are open: a change is logged, and a check is queued, only while one is.
    open: Cell<usize>,
}

impl Table {
    pub(super) fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// A new variable of `kind`, made at `origin`. Its type is `Infer(id)`.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the first caller comes in Task 4")
    )]
    pub(super) fn fresh(&mut self, kind: Kind, origin: Span) -> Type {
        let id = u32::try_from(self.slots.len())
            .expect("a function should make fewer than 2^32 variables");
        self.slots.push(Slot {
            parent: Cell::new(id),
            next: id,
            origin,
            literals: Vec::new(),
            waiting: Vec::new(),
            root: Root {
                size: 1,
                kind,
                bound: None,
                poisoned: false,
                reported: false,
            },
        });
        Type::intern(TypeKind::Infer(InferId(id)))
    }

    /// Keeps `check` for the `Int` variable `var`, which has no type yet: it reports when the
    /// variable gets one, and once however many variables unify with it.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the first caller comes in Task 4")
    )]
    pub(super) fn literal(&mut self, var: Type, check: LiteralCheck) {
        let TypeKind::Infer(id) = var.kind() else {
            debug_assert!(false, "a literal check should wait for a variable");
            return;
        };
        debug_assert!(
            self.slots[self.find(id.0) as usize].root.bound.is_none(),
            "a literal check should wait for a variable with no type"
        );
        self.slots[id.0 as usize].literals.push(check);
        self.record(Undo::Literal { at: id.0 });
    }

    /// Keeps the obligation `id`, which needs `ty` to hold no open variable: it is ready when the
    /// last one gets its type. `ty` holds an open variable now.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the first caller comes in Task 5")
    )]
    pub(super) fn obligation(&mut self, ty: Type, id: ObligationId) {
        let Some(&at) = self.open_roots(ty).first() else {
            debug_assert!(false, "an obligation should wait for an open variable");
            return;
        };
        self.slots[at as usize].waiting.push(self.obligations.len());
        self.obligations.push(Obligation {
            ty,
            id,
            done: false,
            at,
        });
        self.record(Undo::Obligation { at });
    }

    /// `ty` with each variable that has a type replaced by it. An open variable stays, as the
    /// variable of its root, so two types that name unified variables are the same type.
    pub(super) fn resolve(&self, ty: Type) -> Type {
        self.rebuild(ty, false)
    }

    /// `resolve`, with each open `Int` variable as `i64`: the type as a diagnostic shows it. An
    /// open `Any` stays open. It binds nothing, so a later use can still settle the variable.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the first caller comes in Task 4")
    )]
    pub(super) fn default_open(&self, ty: Type) -> Type {
        self.rebuild(ty, true)
    }

    /// Makes `a` and `b` one type, or changes nothing and gives `None`. An `Int` variable unifies
    /// only with `i64`, `u8`, `f64` or another variable; an `Any` variable with any type that does
    /// not hold it (the occurs check). The checks whose variable got a type are returned; inside a
    /// snapshot they stay queued until `keep` returns them.
    #[must_use = "a check that was waiting for a type must be reported"]
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the first caller comes in Task 4")
    )]
    fn unify(&mut self, a: Type, b: Type) -> Option<Vec<Waiting>> {
        let snapshot = self.snapshot();
        if self.unify_pairs(a, b) {
            Some(self.keep(snapshot))
        } else {
            self.rollback(snapshot);
            None
        }
    }

    /// A mark to `rollback` to, or to `keep`. Each snapshot ends with one of the two.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the first caller comes in Task 4")
    )]
    pub(super) fn snapshot(&self) -> Snapshot {
        self.open.set(self.open.get() + 1);
        Snapshot {
            log: self.log.len(),
            ready: self.ready.len(),
        }
    }

    /// Removes each union, bind and queued check since `to`.
    pub(super) fn rollback(&mut self, to: Snapshot) {
        while self.log.len() > to.log {
            match self
                .log
                .pop()
                .expect("the log should hold an entry above the mark")
            {
                Undo::Union { child, root, was } => {
                    self.slots[root as usize].root = was;
                    self.slots[child as usize].parent.set(child);
                    self.swap_next(root, child);
                }
                Undo::Bind { root } => self.slots[root as usize].root.bound = None,
                Undo::Poison { root } => self.slots[root as usize].root.poisoned = false,
                Undo::Done { obligation } => self.obligations[obligation].done = false,
                Undo::Literal { at } => {
                    self.slots[at as usize].literals.pop();
                }
                Undo::Obligation { at } => {
                    self.slots[at as usize].waiting.pop();
                    self.obligations.pop();
                }
                Undo::Move {
                    obligation,
                    from,
                    to,
                } => {
                    self.slots[to as usize].waiting.pop();
                    self.obligations[obligation].at = from;
                }
            }
        }
        self.ready.truncate(to.ready);
        self.close();
    }

    /// Ends the snapshot `to` and keeps what happened since. The checks that became ready are
    /// returned when no outer snapshot is open, and stay queued for the outer one otherwise.
    #[must_use = "a check that was waiting for a type must be reported"]
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the first caller comes in Task 4")
    )]
    fn keep(&mut self, to: Snapshot) -> Vec<Waiting> {
        debug_assert!(to.log <= self.log.len(), "a snapshot should end once");
        self.close();
        if self.open.get() == 0 {
            self.log.clear();
            std::mem::take(&mut self.ready)
        } else {
            Vec::new()
        }
    }

    fn close(&self) {
        let open = self.open.get();
        debug_assert!(open > 0, "a snapshot should be open to end it");
        self.open.set(open.saturating_sub(1));
    }

    /// Each variable in `ty` reports nothing more: a failed use reached it.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the first caller comes in Task 4")
    )]
    pub(super) fn poison(&mut self, ty: Type) {
        for root in self.open_roots(ty) {
            let slot = &mut self.slots[root as usize].root;
            if !slot.poisoned {
                slot.poisoned = true;
                if self.open.get() > 0 {
                    self.log.push(Undo::Poison { root });
                }
            }
        }
    }

    /// The root of `id`'s set. Outside a snapshot it halves the path as it goes; inside one, a
    /// halved path could outlive the union it went through, so it only reads: the union by size
    /// keeps the paths short.
    fn find(&self, mut id: u32) -> u32 {
        let halving = self.open.get() == 0;
        loop {
            let parent = self.slots[id as usize].parent.get();
            if parent == id {
                return id;
            }
            if halving {
                let grand = self.slots[parent as usize].parent.get();
                self.slots[id as usize].parent.set(grand);
                id = grand;
            } else {
                id = parent;
            }
        }
    }

    fn swap_next(&mut self, a: u32, b: u32) {
        let (next_a, next_b) = (self.slots[a as usize].next, self.slots[b as usize].next);
        self.slots[a as usize].next = next_b;
        self.slots[b as usize].next = next_a;
    }

    fn record(&mut self, undo: Undo) {
        if self.open.get() > 0 {
            self.log.push(undo);
        }
    }

    /// The type of `ty` when it is a variable with a type. Else `ty` as it stands, where an open
    /// variable is the variable of its root.
    fn shallow(&self, mut ty: Type) -> Type {
        while let TypeKind::Infer(id) = ty.kind() {
            let root = self.find(id.0);
            match self.slots[root as usize].root.bound {
                Some(bound) => ty = bound,
                None => return Type::intern(TypeKind::Infer(InferId(root))),
            }
        }
        ty
    }

    /// The roots of the open variables in `ty`, each once, in the order that the walk meets them.
    fn open_roots(&self, ty: Type) -> Vec<u32> {
        self.open_roots_in(ty, &mut FxHashSet::default())
    }

    /// `open_roots`, skipping each type in `seen` and adding each type it walks to it: a walk that
    /// shares `seen` meets each distinct type once, and finds each root once.
    fn open_roots_in(&self, ty: Type, seen: &mut FxHashSet<Type>) -> Vec<u32> {
        let (mut roots, mut kept) = (Vec::new(), FxHashSet::default());
        let mut stack = vec![ty];
        while let Some(ty) = stack.pop() {
            if !ty.holds_infer() || !seen.insert(ty) {
                continue;
            }
            match ty.kind() {
                TypeKind::Infer(id) => {
                    let root = self.find(id.0);
                    if let Some(bound) = self.slots[root as usize].root.bound {
                        stack.push(bound);
                    } else {
                        #[cfg(test)]
                        ROOT_STEPS.set(ROOT_STEPS.get() + 1);
                        if kept.insert(root) {
                            roots.push(root);
                        }
                    }
                }
                TypeKind::Array(inner) | TypeKind::Optional(inner) => stack.push(inner),
                TypeKind::Class(_, args)
                | TypeKind::Struct(_, args)
                | TypeKind::Enum(_, args)
                | TypeKind::Any(_, args) => stack.extend(args.get()),
                TypeKind::Assoc(base, _, args, _) => {
                    stack.push(base);
                    stack.extend(args.get());
                }
                TypeKind::I64
                | TypeKind::U8
                | TypeKind::F64
                | TypeKind::Bool
                | TypeKind::Char
                | TypeKind::String
                | TypeKind::Unit
                | TypeKind::Never
                | TypeKind::Param(..) => {}
            }
        }
        roots
    }

    fn rebuild(&self, ty: Type, defaults: bool) -> Type {
        if !ty.holds_infer() {
            return ty;
        }
        self.rebuild_in(ty, defaults, &mut FxHashMap::default())
    }

    /// `rebuild`, with the types it has met, so that each distinct type is rebuilt once.
    fn rebuild_in(&self, ty: Type, defaults: bool, memo: &mut FxHashMap<Type, Type>) -> Type {
        if !ty.holds_infer() {
            return ty;
        }
        if let Some(&done) = memo.get(&ty) {
            return done;
        }
        #[cfg(test)]
        REBUILDS.set(REBUILDS.get() + 1);
        let done = ensure_stack(|| match ty.kind() {
            TypeKind::Infer(id) => {
                let root = self.find(id.0);
                let known = self.slots[root as usize].root;
                match known.bound {
                    Some(bound) => self.rebuild_in(bound, defaults, memo),
                    None if defaults && known.kind == Kind::Int => Type::I64,
                    None => Type::intern(TypeKind::Infer(InferId(root))),
                }
            }
            TypeKind::Array(inner) => Type::array(self.rebuild_in(inner, defaults, memo)),
            TypeKind::Optional(inner) => Type::optional(self.rebuild_in(inner, defaults, memo)),
            TypeKind::Class(id, args) => {
                Type::intern(TypeKind::Class(id, self.rebuild_args(args, defaults, memo)))
            }
            TypeKind::Struct(id, args) => Type::intern(TypeKind::Struct(
                id,
                self.rebuild_args(args, defaults, memo),
            )),
            TypeKind::Enum(id, args) => {
                Type::intern(TypeKind::Enum(id, self.rebuild_args(args, defaults, memo)))
            }
            TypeKind::Any(id, args) => {
                Type::intern(TypeKind::Any(id, self.rebuild_args(args, defaults, memo)))
            }
            TypeKind::Assoc(base, interface, args, k) => Type::intern(TypeKind::Assoc(
                self.rebuild_in(base, defaults, memo),
                interface,
                self.rebuild_args(args, defaults, memo),
                k,
            )),
            TypeKind::I64
            | TypeKind::U8
            | TypeKind::F64
            | TypeKind::Bool
            | TypeKind::Char
            | TypeKind::String
            | TypeKind::Unit
            | TypeKind::Never
            | TypeKind::Param(..) => ty,
        });
        memo.insert(ty, done);
        done
    }

    fn rebuild_args(
        &self,
        args: TypeArgs,
        defaults: bool,
        memo: &mut FxHashMap<Type, Type>,
    ) -> TypeArgs {
        if !args.holds_infer() {
            return args;
        }
        let list: Vec<Type> = (args.get().iter())
            .map(|&arg| self.rebuild_in(arg, defaults, memo))
            .collect();
        TypeArgs::intern(&list)
    }

    /// Whether `left` and `right` unify, by the pairs of their parts. A pair is met once, and a
    /// failure leaves what it did to the caller's rollback.
    fn unify_pairs(&mut self, left: Type, right: Type) -> bool {
        let (mut work, mut met) = (vec![(left, right)], FxHashSet::default());
        while let Some((left, right)) = work.pop() {
            let (left, right) = (self.shallow(left), self.shallow(right));
            if left == right {
                continue;
            }
            // A type with no variable is equal to another only when it is the same type.
            if !left.holds_infer() && !right.holds_infer() {
                return false;
            }
            match (left.kind(), right.kind()) {
                (TypeKind::Infer(x), TypeKind::Infer(y)) => self.union(x.0, y.0),
                (TypeKind::Infer(var), _) => {
                    if !self.bind(var.0, right) {
                        return false;
                    }
                }
                (_, TypeKind::Infer(var)) => {
                    if !self.bind(var.0, left) {
                        return false;
                    }
                }
                _ if !met.insert((left, right)) => {}
                (TypeKind::Array(ours), TypeKind::Array(theirs))
                | (TypeKind::Optional(ours), TypeKind::Optional(theirs)) => {
                    work.push((ours, theirs));
                }
                (TypeKind::Class(from, ours), TypeKind::Class(to, theirs)) if from == to => {
                    if !pair_args(&mut work, ours, theirs) {
                        return false;
                    }
                }
                (TypeKind::Struct(from, ours), TypeKind::Struct(to, theirs)) if from == to => {
                    if !pair_args(&mut work, ours, theirs) {
                        return false;
                    }
                }
                (TypeKind::Enum(from, ours), TypeKind::Enum(to, theirs)) if from == to => {
                    if !pair_args(&mut work, ours, theirs) {
                        return false;
                    }
                }
                (TypeKind::Any(from, ours), TypeKind::Any(to, theirs)) if from == to => {
                    if !pair_args(&mut work, ours, theirs) {
                        return false;
                    }
                }
                (
                    TypeKind::Assoc(ours, from, our_args, place),
                    TypeKind::Assoc(theirs, to, their_args, other),
                ) if from == to && place == other => {
                    work.push((ours, theirs));
                    if !pair_args(&mut work, our_args, their_args) {
                        return false;
                    }
                }
                _ => return false,
            }
        }
        true
    }

    /// Puts the open roots `x` and `y` in one set. A set with an `Int` variable is an `Int`.
    fn union(&mut self, x: u32, y: u32) {
        let (big, small) = if self.slots[x as usize].root.size >= self.slots[y as usize].root.size {
            (x, y)
        } else {
            (y, x)
        };
        let (was, other) = (
            self.slots[big as usize].root,
            self.slots[small as usize].root,
        );
        self.slots[small as usize].parent.set(big);
        self.slots[big as usize].root = Root {
            size: was.size + other.size,
            kind: if was.kind == Kind::Int || other.kind == Kind::Int {
                Kind::Int
            } else {
                Kind::Any
            },
            bound: None,
            poisoned: was.poisoned || other.poisoned,
            reported: was.reported || other.reported,
        };
        self.swap_next(big, small);
        self.record(Undo::Union {
            child: small,
            root: big,
            was,
        });
    }

    /// Gives the open root `root` the type `ty`, which is no open variable. It fails when the kind
    /// of the root does not take `ty`, or `ty` holds the root. The checks that wait are ready then.
    fn bind(&mut self, root: u32, ty: Type) -> bool {
        let known = self.slots[root as usize].root;
        let fits = match known.kind {
            Kind::Int => matches!(ty.kind(), TypeKind::I64 | TypeKind::U8 | TypeKind::F64),
            Kind::Any => !self.open_roots(ty).contains(&root),
        };
        if !fits {
            return false;
        }
        self.bind_raw(root, ty);
        let mut at = root;
        loop {
            if !known.poisoned {
                let literals = &self.slots[at as usize].literals;
                self.ready
                    .extend(literals.iter().map(|&check| Waiting::Literal { check, ty }));
            }
            self.wake(at);
            at = self.slots[at as usize].next;
            if at == root {
                break;
            }
        }
        true
    }

    /// Looks at each obligation that waits at the variable `at`, whose root just got a type: it is
    /// ready when its type holds no open variable, and else it waits at the next one.
    fn wake(&mut self, at: u32) {
        for entry in 0..self.slots[at as usize].waiting.len() {
            let obligation = self.slots[at as usize].waiting[entry];
            if self.obligations[obligation].done || self.obligations[obligation].at != at {
                continue;
            }
            #[cfg(test)]
            OBLIGATION_VISITS.set(OBLIGATION_VISITS.get() + 1);
            let resolved = self.resolve(self.obligations[obligation].ty);
            if let Some(&next) = self.open_roots(resolved).first() {
                self.obligations[obligation].at = next;
                self.slots[next as usize].waiting.push(obligation);
                self.record(Undo::Move {
                    obligation,
                    from: at,
                    to: next,
                });
            } else {
                self.obligations[obligation].done = true;
                self.record(Undo::Done { obligation });
                self.ready.push(Waiting::Obligation {
                    id: self.obligations[obligation].id,
                    ty: resolved,
                });
            }
        }
    }

    fn bind_raw(&mut self, root: u32, ty: Type) {
        self.slots[root as usize].root.bound = Some(ty);
        self.record(Undo::Bind { root });
    }

    /// The first origin in the source of the variables in the set of `root`.
    fn first_origin(&self, root: u32) -> Span {
        let mut at = root;
        let mut first = self.slots[root as usize].origin;
        loop {
            let slot = &self.slots[at as usize];
            if slot.origin.start() < first.start() {
                first = slot.origin;
            }
            at = slot.next;
            if at == root {
                return first;
            }
        }
    }
}

/// Queues the pairs of the arguments `ours` and `theirs`; `false` when they differ in length.
fn pair_args(work: &mut Vec<(Type, Type)>, ours: TypeArgs, theirs: TypeArgs) -> bool {
    let (ours, theirs) = (ours.get(), theirs.get());
    work.extend(ours.iter().copied().zip(theirs.iter().copied()));
    ours.len() == theirs.len()
}

/// What a walk over types calls for each type that it visits.
type Each<'a> = &'a mut dyn FnMut(&mut Type);

/// What the write-back of a function or a constant found, and the state that its walk shares. The
/// walk has two passes over the types. `settle` meets each distinct type once and deals with each
/// open variable in it. Then `put` rebuilds each type, with one memo, so a chain of types that
/// name each other costs a step for each link.
#[derive(Default)]
pub(super) struct Written {
    /// The `Any` variables that no type reached, once for each set, by origin in the source.
    pub(super) unsettled: Vec<Unsettled>,
    /// A type still holds an inference variable.
    pub(super) open: bool,
    /// The types that `settle` has walked.
    seen: FxHashSet<Type>,
    /// The types that `put` has rebuilt.
    memo: FxHashMap<Type, Type>,
}

impl Written {
    /// Gives each open `Int` in `ty` the type `i64`. It notes the first origin in the source of
    /// each open `Any` set that no earlier type has reported: E0050. A set that a poisoned value
    /// or a failed use reached reports nothing.
    fn settle(&mut self, table: &mut Table, ty: Type) {
        if !ty.holds_infer() {
            return;
        }
        for root in table.open_roots_in(ty, &mut self.seen) {
            let known = table.slots[root as usize].root;
            match known.kind {
                Kind::Int => table.bind_raw(root, Type::I64),
                Kind::Any => {
                    table.slots[root as usize].root.reported = true;
                    if !known.poisoned && !known.reported {
                        self.unsettled.push(Unsettled {
                            origin: table.first_origin(root),
                        });
                    }
                }
            }
        }
    }

    /// Puts the type of each variable in `ty` in its place: the pass after `settle`.
    fn put(&mut self, table: &Table, ty: &mut Type) {
        if !ty.holds_infer() {
            return;
        }
        *ty = table.rebuild_in(*ty, false, &mut self.memo);
        self.open |= ty.holds_infer();
    }

    /// The two passes over what `walk` visits, each type once for each pass.
    fn walk_types(table: &mut Table, walk: &mut dyn FnMut(Each<'_>)) -> Written {
        let mut written = Written::default();
        walk(&mut |ty| written.settle(table, *ty));
        walk(&mut |ty| written.put(table, ty));
        written
            .unsettled
            .sort_by_key(|unsettled| unsettled.origin.start());
        written
    }
}

/// A checked result whose types the write-back puts in place: each open `Int` is `i64`, and each
/// `Any` set that no type reached is E0050.
pub(super) trait WriteBack {
    fn write_back_all(&mut self, table: &mut Table) -> Written;
}

/// Each `Int` node whose type is `f64` becomes a `Float`, as the checker makes it for a literal
/// that the context types `f64`.
fn float_literals(e: &mut tir::Expr) {
    if let ExprKind::Int(value) = e.kind
        && e.ty == Type::F64
    {
        #[expect(
            clippy::cast_precision_loss,
            reason = "E0104 has checked that the literal is exact when its variable settled"
        )]
        let x = value as f64;
        e.kind = ExprKind::Float(x);
    }
    e.each_child_mut(&mut float_literals);
}

impl WriteBack for tir::Expr {
    fn write_back_all(&mut self, table: &mut Table) -> Written {
        let written =
            Written::walk_types(table, &mut |each| self.each_type_mut(&mut |ty| each(ty)));
        float_literals(self);
        written
    }
}

impl WriteBack for tir::Function {
    fn write_back_all(&mut self, table: &mut Table) -> Written {
        let written =
            Written::walk_types(table, &mut |each| self.each_type_mut(&mut |ty| each(ty)));
        self.body.each_expr_mut(&mut float_literals);
        written
    }
}

impl<T: WriteBack> WriteBack for Option<T> {
    fn write_back_all(&mut self, table: &mut Table) -> Written {
        self.as_mut()
            .map_or_else(Written::default, |inner| inner.write_back_all(table))
    }
}

/// A failed value holds the types it kept, which can hold a variable too.
impl WriteBack for Result<tir::Expr, Poisoned> {
    fn write_back_all(&mut self, table: &mut Table) -> Written {
        match self {
            Ok(expr) => expr.write_back_all(table),
            Err(poisoned) => Written::walk_types(table, &mut |each| {
                if let Some(ty) = &mut poisoned.ty {
                    each(ty);
                }
                if let Some(misfit) = &mut poisoned.found {
                    each(&mut misfit.ty);
                }
            }),
        }
    }
}

impl Checker<'_> {
    /// Reports the check that was waiting, which `ty` has answered: a literal that does not fit
    /// (E0040, E0042 or E0104, as `integer` reports it for a literal whose type is known).
    pub(super) fn report_waiting(&mut self, waiting: Waiting) {
        let Waiting::Literal { check, ty } = waiting else {
            panic!("the obligations of `generic.rs` and `conform.rs` land in Task 5");
        };
        let LiteralCheck {
            span,
            value,
            negative,
        } = check;
        if ty == Type::U8 {
            if negative {
                self.error(DiagnosticKind::NegatedU8 {
                    at: span,
                    convert: None,
                });
            } else if value > u64::from(u8::MAX) {
                self.error(DiagnosticKind::IntegerDoesNotFit {
                    text: self.source.text(span).to_string(),
                    at: span,
                });
            }
        } else if ty == Type::F64 {
            // The literal is exact when it survives the trip through `f64`, as `integer` checks.
            #[expect(
                clippy::cast_precision_loss,
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "the round trip is the check that the `f64` is exact"
            )]
            let (x, exact) = (value as f64, (value as f64) as u128 == u128::from(value));
            if !exact {
                self.error(DiagnosticKind::NotExactlyF64 {
                    text: self.source.text(span).to_string(),
                    nearest: format!("{:?}", if negative { -x } else { x }),
                    at: span,
                });
            }
        }
    }

    /// Makes `a` and `b` one type, if the table lets it, and reports each check that was waiting
    /// for a type that this gave. With `keep`, the only place that reads the checks that the
    /// table gives back.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the first caller comes in Task 4")
    )]
    pub(super) fn unify(&mut self, a: Type, b: Type) -> bool {
        let Some(ready) = self.table.unify(a, b) else {
            return false;
        };
        for waiting in ready {
            self.report_waiting(waiting);
        }
        true
    }

    /// Ends the snapshot `to`, keeps what happened since, and reports each check that became ready
    /// if no outer snapshot is open.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the first caller comes in Task 4")
    )]
    pub(super) fn keep_snapshot(&mut self, to: Snapshot) {
        for waiting in self.table.keep(to) {
            self.report_waiting(waiting);
        }
    }

    /// Reports E0050 at the origin of an `Any` variable that no type reached.
    pub(super) fn report_unsettled(&mut self, unsettled: Unsettled) {
        let literal = if self.source.text(unsettled.origin).starts_with('[') {
            Literal::EmptyArray
        } else {
            Literal::None
        };
        let _reported = self.report_uninferred(literal, unsettled.origin);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::tests::joins::with_checker;
    use crate::diagnostic::{Code, Diagnostic};

    fn at(n: usize) -> Span {
        Span::new(n, n + 1)
    }

    /// The span of the `n`th `text` in `src`.
    fn nth(src: &str, text: &str, n: usize) -> Span {
        let (start, _) = src
            .match_indices(text)
            .nth(n)
            .expect("the text should be in the source");
        Span::new(start, start + text.len())
    }

    fn literal(span: Span, value: u64) -> LiteralCheck {
        LiteralCheck {
            span,
            value,
            negative: false,
        }
    }

    #[test]
    fn int_should_unify_with_each_number_type_only() {
        let mut table = Table::default();
        for ty in [Type::I64, Type::U8, Type::F64] {
            let var = table.fresh(Kind::Int, at(0));
            assert!(table.unify(var, ty).is_some(), "{ty:?}");
            assert_eq!(table.resolve(var), ty);
        }
        let other = table.fresh(Kind::Int, at(1));
        let var = table.fresh(Kind::Int, at(2));
        assert!(table.unify(var, other).is_some(), "an Int and an Int");
        for ty in [
            Type::STRING,
            Type::BOOL,
            Type::optional(Type::I64),
            Type::array(Type::I64),
        ] {
            let var = table.fresh(Kind::Int, at(3));
            assert!(table.unify(var, ty).is_none(), "{ty:?}");
            assert_eq!(
                table.resolve(var),
                var,
                "a refused unify leaves the variable open"
            );
        }
    }

    #[test]
    fn any_should_refuse_a_type_that_holds_it() {
        let mut table = Table::default();
        let a = table.fresh(Kind::Any, at(0));
        assert!(table.unify(a, Type::array(a)).is_none());
        assert_eq!(table.resolve(a), a);
        // The check reads through a bound variable: `B = [A]` then `A = B` is `A = [A]`.
        let b = table.fresh(Kind::Any, at(1));
        assert!(table.unify(b, Type::array(a)).is_some());
        assert!(table.unify(a, b).is_none());
        assert_eq!(table.resolve(a), a);
        assert!(table.unify(a, Type::array(Type::I64)).is_some());
        assert_eq!(table.resolve(b), Type::array(Type::array(Type::I64)));
    }

    #[test]
    fn rollback_should_remove_each_bind_since_the_snapshot() {
        let mut table = Table::default();
        let a = table.fresh(Kind::Any, at(0));
        let b = table.fresh(Kind::Any, at(1));
        let snapshot = table.snapshot();
        assert!(table.unify(a, Type::I64).is_some());
        assert!(table.unify(b, a).is_some());
        assert_eq!(table.resolve(b), Type::I64);
        table.rollback(snapshot);
        assert_eq!(table.resolve(a), a);
        assert_eq!(table.resolve(b), b);
        // `b` is free again: it can take another type and leave `a` open.
        assert!(table.unify(b, Type::STRING).is_some());
        assert_eq!(table.resolve(a), a);
    }

    #[test]
    fn write_back_should_make_an_open_int_i64() {
        let mut table = Table::default();
        let var = table.fresh(Kind::Int, at(0));
        let mut ty = Type::array(var);
        assert_eq!(write_back(&mut table, &mut ty), []);
        assert_eq!(ty, Type::array(Type::I64));
    }

    #[test]
    fn write_back_should_make_an_int_literal_that_settles_to_f64_a_float() {
        let mut table = Table::default();
        let var = table.fresh(Kind::Int, at(0));
        let mut expr = tir::Expr::new(ExprKind::Int(3), var, at(0));
        assert!(table.unify(var, Type::F64).is_some());
        let written = expr.write_back_all(&mut table);
        assert!(written.unsettled.is_empty() && !written.open);
        assert_eq!(expr.ty, Type::F64);
        assert!(matches!(expr.kind, ExprKind::Float(x) if x.to_bits() == 3.0_f64.to_bits()));
    }

    #[test]
    fn write_back_should_return_one_unsettled_per_open_any() {
        let mut table = Table::default();
        let later = table.fresh(Kind::Any, at(10));
        let earlier = table.fresh(Kind::Any, at(5));
        let apart = table.fresh(Kind::Any, at(20));
        assert!(table.unify(later, earlier).is_some());
        let mut types = [Type::array(later), earlier, Type::optional(apart), later];
        let unsettled: Vec<Vec<Unsettled>> = types
            .iter_mut()
            .map(|ty| write_back(&mut table, ty))
            .collect();
        // The two that unified are one entry, at the first origin in the source, once.
        assert_eq!(
            unsettled,
            [
                vec![Unsettled { origin: at(5) }],
                vec![],
                vec![Unsettled { origin: at(20) }],
                vec![]
            ]
        );
        assert!(types.iter().all(|ty| ty.holds_infer()));
    }

    #[test]
    fn poisoned_variable_should_report_nothing() {
        let mut table = Table::default();
        let any = table.fresh(Kind::Any, at(0));
        let int = table.fresh(Kind::Int, at(1));
        table.literal(int, literal(at(1), 300));
        table.poison(Type::array(any));
        table.poison(int);
        let mut ty = any;
        assert_eq!(write_back(&mut table, &mut ty), []);
        // A literal that a failed use reached waits for nothing.
        assert_eq!(table.unify(int, Type::U8), Some(Vec::new()));
    }

    #[test]
    fn waiting_literal_check_should_report_once_after_a_union() {
        let src = "fn main() {\n    let a = 300\n    let b = 300\n}\n";
        let (first, second) = (nth(src, "300", 0), nth(src, "300", 1));
        with_checker(src, |c| {
            let (a, b) = (
                c.table.fresh(Kind::Int, first),
                c.table.fresh(Kind::Int, second),
            );
            c.table.literal(a, literal(first, 300));
            c.table.literal(b, literal(second, 300));
            assert!(c.unify(a, b));
            assert!(c.diagnostics.is_empty(), "no type yet, so no report");
            assert!(c.unify(a, Type::U8));
            let reported: Vec<(Code, Span)> =
                c.diagnostics.iter().map(|d| (d.code(), d.span())).collect();
            assert_eq!(
                reported,
                [
                    (Code::IntegerDoesNotFit, first),
                    (Code::IntegerDoesNotFit, second)
                ]
            );
            assert!(c.unify(b, Type::U8));
            assert_eq!(
                c.diagnostics.len(),
                2,
                "a second unify reports nothing more"
            );
        });
    }

    #[test]
    fn negative_literal_should_report_e0042_when_it_settles_to_u8() {
        let src = "fn main() {\n    let a = -9223372036854775808\n}\n";
        let span = nth(src, "-9223372036854775808", 0);
        with_checker(src, |c| {
            let var = c.table.fresh(Kind::Int, span);
            c.table.literal(
                var,
                LiteralCheck {
                    span,
                    value: 1 << 63,
                    negative: true,
                },
            );
            assert!(c.unify(var, Type::U8));
            let reported: Vec<(Code, Span)> =
                c.diagnostics.iter().map(|d| (d.code(), d.span())).collect();
            assert_eq!(reported, [(Code::NegatedU8, span)]);
        });
    }

    #[test]
    fn wide_literal_should_report_e0104_when_it_settles_to_f64() {
        let src = "fn main() {\n    let a = 9007199254740993\n}\n";
        let span = nth(src, "9007199254740993", 0);
        with_checker(src, |c| {
            let var = c.table.fresh(Kind::Int, span);
            c.table.literal(var, literal(span, 9_007_199_254_740_993));
            assert!(c.unify(var, Type::F64));
            let reported: Vec<(Code, Span)> =
                c.diagnostics.iter().map(|d| (d.code(), d.span())).collect();
            assert_eq!(reported, [(Code::NotExactlyF64, span)]);
        });
    }

    #[test]
    fn waiting_check_inside_a_rolled_back_snapshot_should_report_nothing() {
        let mut table = Table::default();
        let var = table.fresh(Kind::Int, at(0));
        table.literal(var, literal(at(0), 300));
        let snapshot = table.snapshot();
        assert_eq!(table.unify(var, Type::U8), Some(Vec::new()));
        table.rollback(snapshot);
        let snapshot = table.snapshot();
        assert_eq!(
            table.keep(snapshot),
            Vec::new(),
            "the rollback took the queued check"
        );
        // Kept, the check is returned once, and the variable has its type.
        let snapshot = table.snapshot();
        assert_eq!(table.unify(var, Type::U8), Some(Vec::new()));
        let ready = table.keep(snapshot);
        assert_eq!(
            ready,
            [Waiting::Literal {
                check: literal(at(0), 300),
                ty: Type::U8
            }]
        );
        assert_eq!(table.unify(var, Type::U8), Some(Vec::new()));
    }

    #[test]
    fn obligation_should_wait_for_the_last_variable() {
        let mut table = Table::default();
        let (a, b) = (table.fresh(Kind::Any, at(0)), table.fresh(Kind::Any, at(1)));
        table.obligation(Type::array(a), ObligationId(7));
        table.obligation(Type::optional(b), ObligationId(8));
        table.obligation(
            Type::intern(TypeKind::Class(tir::ClassId(0), TypeArgs::intern(&[a, b]))),
            ObligationId(9),
        );
        let ready = table.unify(a, Type::I64).expect("A should unify");
        assert_eq!(
            ready,
            [Waiting::Obligation {
                id: ObligationId(7),
                ty: Type::array(Type::I64)
            }]
        );
        let ready = table.unify(b, Type::STRING).expect("B should unify");
        assert_eq!(ready.len(), 2);
    }

    #[test]
    fn default_open_should_show_an_open_int_as_i64() {
        let mut table = Table::default();
        let int = table.fresh(Kind::Int, at(0));
        let any = table.fresh(Kind::Any, at(1));
        let shown = table.default_open(Type::array(Type::optional(int)));
        assert_eq!(shown, Type::array(Type::optional(Type::I64)));
        assert_eq!(table.default_open(Type::array(any)), Type::array(any));
        // It binds nothing: the literal can still settle to `u8`.
        assert!(table.unify(int, Type::U8).is_some());
    }

    /// The write-back of the one type `ty`: the walk of a function, over that type alone.
    fn write_back(table: &mut Table, ty: &mut Type) -> Vec<Unsettled> {
        let mut written = Written::default();
        written.settle(table, *ty);
        written.put(table, ty);
        written.unsettled
    }

    /// A type of `Struct 0` that holds each of `args`.
    fn holding(args: &[Type]) -> Type {
        Type::intern(TypeKind::Struct(tir::StructId(0), TypeArgs::intern(args)))
    }

    #[test]
    fn write_back_should_report_each_independent_any_set_of_a_type() {
        let mut table = Table::default();
        let (a, b) = (table.fresh(Kind::Any, at(3)), table.fresh(Kind::Any, at(8)));
        let mut expr = tir::Expr::new(ExprKind::Unit, holding(&[a, b]), at(0));
        let written = expr.write_back_all(&mut table);
        assert_eq!(
            written.unsettled,
            [Unsettled { origin: at(3) }, Unsettled { origin: at(8) }]
        );
    }

    /// The obligation visits that settling `n` independent obligations costs.
    fn obligation_visits(n: usize) -> usize {
        let mut table = Table::default();
        let vars: Vec<Type> = (0..n).map(|i| table.fresh(Kind::Any, at(i))).collect();
        for (i, &var) in vars.iter().enumerate() {
            table.obligation(Type::array(var), ObligationId(i));
        }
        OBLIGATION_VISITS.set(0);
        for &var in &vars {
            let ready = table.unify(var, Type::I64).expect("A should unify");
            assert_eq!(ready.len(), 1, "a bind readies its own obligation only");
        }
        OBLIGATION_VISITS.get()
    }

    #[test]
    fn bind_should_visit_only_its_own_obligations() {
        // Each bind looks at its own obligation: the work stays linear in the obligations.
        assert!(obligation_visits(100) <= 200);
        assert!(obligation_visits(200) <= 400);
    }

    #[test]
    fn open_roots_should_keep_each_root_in_linear_steps() {
        let mut table = Table::default();
        let vars: Vec<Type> = (0..400).map(|i| table.fresh(Kind::Any, at(i))).collect();
        ROOT_STEPS.set(0);
        table.poison(holding(&vars));
        assert!(ROOT_STEPS.get() <= 4 * vars.len(), "{}", ROOT_STEPS.get());
    }

    /// The types that writing back `n` nested inferred types rebuilds: `A_0` is `i64` and each
    /// `A_i` is `[A_(i-1)]`, and each of them is the type of a node.
    fn rebuilds(n: usize) -> usize {
        let mut table = Table::default();
        let vars: Vec<Type> = (0..n).map(|i| table.fresh(Kind::Any, at(i))).collect();
        assert!(table.unify(vars[0], Type::I64).is_some());
        for pair in vars.windows(2) {
            assert!(table.unify(pair[1], Type::array(pair[0])).is_some());
        }
        let nodes = vars
            .iter()
            .map(|&var| tir::Expr::new(ExprKind::Unit, var, at(0)))
            .collect();
        let mut expr = tir::Expr::new(ExprKind::ArrayLit(nodes), Type::UNIT, at(0));
        REBUILDS.set(0);
        let written = expr.write_back_all(&mut table);
        assert!(!written.open);
        REBUILDS.get()
    }

    #[test]
    fn write_back_should_rebuild_each_type_once_for_a_function() {
        assert!(rebuilds(100) <= 400, "{}", rebuilds(100));
        assert!(rebuilds(200) <= 800, "{}", rebuilds(200));
    }

    #[test]
    fn rollback_should_restore_a_union() {
        let mut table = Table::default();
        let (a, b) = (table.fresh(Kind::Any, at(0)), table.fresh(Kind::Any, at(1)));
        let snapshot = table.snapshot();
        assert!(table.unify(a, b).is_some());
        assert!(table.unify(a, Type::I64).is_some());
        table.rollback(snapshot);
        assert!(table.unify(a, Type::STRING).is_some());
        assert!(table.unify(b, Type::I64).is_some());
        assert_eq!(table.resolve(a), Type::STRING);
        assert_eq!(table.resolve(b), Type::I64);
    }

    #[test]
    fn rollback_should_undo_a_union_under_a_longer_path() {
        // A walk inside the snapshot must not halve a path through the union that is undone.
        let mut table = Table::default();
        let [one, two, three, four, five] = [0, 1, 2, 3, 4].map(|i| table.fresh(Kind::Any, at(i)));
        assert!(table.unify(one, two).is_some());
        assert!(table.unify(three, four).is_some());
        assert!(table.unify(four, five).is_some());
        let snapshot = table.snapshot();
        assert!(table.unify(two, three).is_some());
        assert_eq!(table.resolve(two), table.resolve(five));
        table.rollback(snapshot);
        assert!(table.unify(three, Type::I64).is_some());
        assert!(
            table.resolve(two).holds_infer(),
            "`two` is not in the set of `three` any more"
        );
        assert_eq!(table.resolve(two), table.resolve(one));
        assert_eq!(table.resolve(five), Type::I64);
    }

    #[test]
    fn failed_unify_should_leave_no_partial_bind() {
        let mut table = Table::default();
        let a = table.fresh(Kind::Any, at(0));
        let class =
            |args: &[Type]| Type::intern(TypeKind::Class(tir::ClassId(0), TypeArgs::intern(args)));
        // `A` binds to `i64` before the second pair, `i64` and `string`, fails.
        assert!(
            table
                .unify(class(&[Type::I64, a]), class(&[Type::STRING, Type::I64]))
                .is_none()
        );
        assert_eq!(table.resolve(a), a);
    }

    #[test]
    fn rollback_should_undo_a_poison() {
        let mut table = Table::default();
        let int = table.fresh(Kind::Int, at(0));
        table.literal(int, literal(at(0), 300));
        let snapshot = table.snapshot();
        table.poison(int);
        table.rollback(snapshot);
        assert_eq!(table.unify(int, Type::U8).map(|ready| ready.len()), Some(1));
    }

    #[test]
    fn union_should_keep_the_poison_of_either_set() {
        let mut table = Table::default();
        let (a, b) = (table.fresh(Kind::Int, at(0)), table.fresh(Kind::Int, at(1)));
        table.literal(a, literal(at(0), 300));
        table.literal(b, literal(at(1), 300));
        table.poison(a);
        assert!(table.unify(b, a).is_some());
        assert_eq!(table.unify(a, Type::U8), Some(Vec::new()));
    }

    #[test]
    fn rollback_should_make_a_done_obligation_wait_again() {
        let mut table = Table::default();
        let a = table.fresh(Kind::Any, at(0));
        table.obligation(Type::array(a), ObligationId(1));
        let snapshot = table.snapshot();
        assert_eq!(table.unify(a, Type::I64), Some(Vec::new()));
        table.rollback(snapshot);
        let ready = table.unify(a, Type::I64).expect("A should unify");
        assert_eq!(ready.len(), 1);
    }

    #[test]
    fn rollback_should_remove_a_check_made_in_the_snapshot() {
        let mut table = Table::default();
        let (int, any) = (table.fresh(Kind::Int, at(0)), table.fresh(Kind::Any, at(1)));
        let snapshot = table.snapshot();
        table.literal(int, literal(at(0), 300));
        table.obligation(Type::array(any), ObligationId(1));
        table.rollback(snapshot);
        assert_eq!(table.unify(int, Type::U8), Some(Vec::new()));
        assert_eq!(table.unify(any, Type::I64), Some(Vec::new()));
    }

    #[test]
    fn type_that_holds_a_variable_should_count_as_maybe_counted() {
        // The checker asks before `write_back`: no answer here may panic.
        let mut table = Table::default();
        let var = table.fresh(Kind::Int, at(0));
        assert!(var.is_counted(&[], &[]));
    }

    #[test]
    fn checker_keep_should_report_the_checks_that_the_table_gives_back() {
        let src = "fn main() {\n    let a = 300\n}\n";
        let span = nth(src, "300", 0);
        with_checker(src, |c| {
            let var = c.table.fresh(Kind::Int, span);
            c.table.literal(var, literal(span, 300));
            let snapshot = c.table.snapshot();
            assert!(c.unify(var, Type::U8));
            assert!(c.diagnostics.is_empty(), "the check waits for the snapshot");
            c.keep_snapshot(snapshot);
            let reported: Vec<Code> = c.diagnostics.iter().map(Diagnostic::code).collect();
            assert_eq!(reported, [Code::IntegerDoesNotFit]);
        });
    }
}
