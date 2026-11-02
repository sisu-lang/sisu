//! Where conformances meet types: which conformance covers a type, and whether two conformances
//! cover a type together. A conformance's target is a class or an enum, with the type parameters
//! of its `extend` block in it; they are the `Param`s numbered from 0.

use std::cmp::Ordering;
use std::rc::Rc;

use rustc_hash::FxHashMap;

use crate::graph::components;
use crate::symbol::Symbol;
use crate::tir::{
    ChoicePlace, Class, ClassId, Conformance, Conformances, Enum, EnumId, Interface, InterfaceId,
    LangInterfaces, Type, TypeArgs, TypeKind, head_of,
};

#[cfg(test)]
thread_local! {
    /// How many conformances `find` and `clash` have read on this thread.
    static CANDIDATES_READ: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many types `Unifier::resolve` has read on this thread.
    static RESOLVE_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many types `Unifier::unify` has read on this thread.
    static UNIFY_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many parameters `Unifier::walk` has stepped over on this thread.
    static WALK_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many types `normalize` has walked on this thread.
    pub(crate) static NORMALIZED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many class or enum keys `holds` has decided (each a miss of its memo) on this thread.
    pub(crate) static HOLDS_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many keys `calls_written` has listed the parts of on this thread.
    pub(crate) static CALLS_KEYS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The free stack that a walk over the depth of a type keeps. rustc's `ensure_sufficient_stack`
/// uses the same value.
const STACK_RED_ZONE: usize = 100 * 1024;

/// The size of each new piece of stack. rustc's `ensure_sufficient_stack` uses the same value.
const STACK_SEGMENT: usize = 1024 * 1024;

/// Runs `f` on a new piece of stack when the stack that is left is less than `STACK_RED_ZONE`. A
/// type can nest deeper than the call stack allows, and the walks that follow its depth (the
/// queries for a conformance, here and in the checker) call themselves once for each level.
pub(crate) fn ensure_stack<R>(f: impl FnOnce() -> R) -> R {
    stacker::maybe_grow(STACK_RED_ZONE, STACK_SEGMENT, f)
}

/// The conformance of `interface` to `ty` with the interface arguments `args`, as its index
/// among those of `interface` and the types that its block's parameters stand for. The bounds of
/// the block are not checked. A target without parameters is found by its exact type and the
/// interface arguments. The scan among the targets with parameters reads only those of `ty`'s
/// class or enum.
///
/// The second recorded exception to "A pass is linear in the program" (user, 2026-10-25): `N`
/// blocks `extend<T> Pair<TagK, T>: Run` make each lookup at `Pair` inspect up to `N`
/// candidates, so `N` lookups cost `N²`. Rust's impl selection scans the impls of one
/// simplified self type the same way. An index on the whole target would remove it, and waits
/// until a program needs it.
pub(crate) fn find(
    conformances: &Conformances,
    interface: InterfaceId,
    ty: Type,
    args: TypeArgs,
) -> Option<(usize, TypeArgs)> {
    find_with(conformances, interface, ty, args, |_| true)
}

/// `find`, among the conformances that `accept` takes.
pub(crate) fn find_with(
    conformances: &Conformances,
    interface: InterfaceId,
    ty: Type,
    args: TypeArgs,
    accept: impl Fn(&Conformance) -> bool,
) -> Option<(usize, TypeArgs)> {
    let exact = conformances.exact(interface, ty, args);
    if let Some(&k) = exact.iter().find(|&&k| {
        #[cfg(test)]
        CANDIDATES_READ.set(CANDIDATES_READ.get() + 1);
        let c = conformances.get(interface, k);
        c.args == args && accept(c)
    }) {
        return Some((k, TypeArgs::NONE));
    }
    let head = head_of(ty)?;
    conformances.generic(interface, head).iter().find_map(|&k| {
        #[cfg(test)]
        CANDIDATES_READ.set(CANDIDATES_READ.get() + 1);
        let c = conformances.get(interface, k);
        let bound = bind(c.target, c.type_params, ty)?;
        (c.args.subst(bound.get()) == args && accept(c)).then_some((k, bound))
    })
}

/// The most interfaces that the closure of one bound or one `any` type holds. An interface that
/// inherits another twice, with different arguments, doubles the number at each level of
/// inheritance, so a walk stops here (E0157).
pub(crate) const BOUND_CLOSURE_LIMIT: usize = 256;

/// The interface `root` with the arguments `args`, and every interface it inherits, each
/// interface with its arguments once, depth first in the order of the lists. `inherits` gives the
/// interfaces that one inherits, with arguments over its own parameters, `Self` first, and
/// `receiver` stands for `Self`. `tick` is called for each entry that the walk adds, so that a
/// test counts them. The walk stops at `limit` entries, which a bound's closure may reach when
/// an interface inherits another with two arguments (E0157).
pub(crate) fn interface_closure<I: IntoIterator<Item = (InterfaceId, TypeArgs)>>(
    inherits: impl Fn(InterfaceId) -> I,
    root: (InterfaceId, TypeArgs),
    receiver: Type,
    limit: usize,
    mut tick: impl FnMut(),
) -> Vec<(InterfaceId, TypeArgs)> {
    let mut seen = rustc_hash::FxHashSet::default();
    let mut out = Vec::new();
    let mut stack = vec![root];
    let mut with = Vec::new();
    while let Some(entry) = stack.pop() {
        if !seen.insert(entry) {
            continue;
        }
        tick();
        out.push(entry);
        if out.len() >= limit {
            break;
        }
        with.clear();
        with.push(receiver);
        with.extend_from_slice(entry.1.get());
        let first = stack.len();
        stack.extend(
            inherits(entry.0)
                .into_iter()
                .map(|(interface, args)| (interface, args.subst(&with))),
        );
        // The first interface of the list is the next one to pop.
        stack[first..].reverse();
    }
    out
}

/// The interfaces that the `any` type `any_ty` conforms to: its own interface and every interface
/// that one inherits, each with its arguments once (see `interface_closure`). The checker and
/// `mono` both answer "does `any I` conform to `J`" with this list.
pub(crate) fn any_closure<I: IntoIterator<Item = (InterfaceId, TypeArgs)>>(
    inherits: impl Fn(InterfaceId) -> I,
    any_ty: Type,
    tick: impl FnMut(),
) -> Vec<(InterfaceId, TypeArgs)> {
    let TypeKind::Any(interface, args) = any_ty.kind() else {
        unreachable!("only an `any` type has an interface closure");
    };
    interface_closure(
        inherits,
        (interface, args),
        any_ty,
        BOUND_CLOSURE_LIMIT,
        tick,
    )
}

/// What the generated rule of an interface asks of a type, for the caller to settle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Generated {
    /// The type has the interface, with no part query.
    Holds,
    /// The generated rule gives the type no such interface.
    Fails,
    /// An array or an optional has `Eq` when this part, its element or its contents, has.
    Element(Type),
    /// An enum has `Eq` when each payload type of each variant, with the arguments in place of
    /// the enum's parameters, has.
    Payloads(EnumId, TypeArgs),
}

/// The generated rule (spec, "Generated conformances"): what the compiler writes for `ty` and the
/// language interface `interface`, when no written conformance covers `ty`. `Display` holds for
/// every type but `unit` and `never`, a type parameter and a class included, and asks nothing.
/// `Eq` holds for scalars and `string`, for an array or an optional of a type that has `Eq`, and
/// for an enum whose payload types have it; never for a class. `Ord` holds for numbers, `char`
/// and `string`, never for `bool`, and nothing else has it. `AnyObject` holds for a class and for
/// nothing else. An interface of the program has no generated rule. The caller asks the parts, so
/// that the checker and `mono` each keep their own answers.
pub(crate) fn generated(lang: LangInterfaces, ty: Type, interface: InterfaceId) -> Generated {
    let holds = |yes: bool| {
        if yes {
            Generated::Holds
        } else {
            Generated::Fails
        }
    };
    let kind = ty.kind();
    if interface == lang.display {
        return holds(!matches!(kind, TypeKind::Unit | TypeKind::Never));
    }
    if interface == lang.any_object {
        return holds(matches!(kind, TypeKind::Class(..)));
    }
    let number = matches!(
        kind,
        TypeKind::I64 | TypeKind::U8 | TypeKind::F64 | TypeKind::Char | TypeKind::String
    );
    if interface == lang.ord {
        return holds(number);
    }
    if interface != lang.eq {
        return Generated::Fails;
    }
    match kind {
        _ if number || kind == TypeKind::Bool => Generated::Holds,
        TypeKind::Array(part) | TypeKind::Optional(part) => Generated::Element(part),
        TypeKind::Enum(id, args) => Generated::Payloads(id, args),
        _ => Generated::Fails,
    }
}

/// A class or an enum type asked for an interface with arguments: what a query remembers.
pub(crate) type Key = (Type, InterfaceId, TypeArgs);

/// What `Open::enter` found for a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Entered {
    /// The key is on the stack, with this number: its query is open.
    Open(usize),
    /// The key has a pending answer, which holds if the keys below it hold, with this number.
    Pending(usize),
    /// The key is new, and is now on the stack.
    New,
}

/// An entry of the stack of an `Open`.
#[derive(Clone, Copy, Debug)]
struct Frame {
    key: Key,
    number: usize,
    /// The lowest number among the keys on the stack that the key's query rests on.
    low: usize,
}

/// The state of one root query for a generated conformance. The query for an enum can reach
/// itself (`enum Expr { Num(i64)\n Add(Expr, Expr) }`). So the query is decided as if it holds,
/// and then each part is checked. This is a coinductive check, as Rust's is. The state follows
/// Tarjan's algorithm for strongly connected components. It holds these parts:
///
/// - the stack of the keys in progress, each with its entry number and its `low`;
/// - the pending answers, each with its key and its entry number;
/// - the counter that gives the numbers.
///
/// The counter starts again only when the stack is empty. So one number never names two keys in
/// one root query.
///
/// Every query nested in a query is a conjunction, so the rule is exact. A written conformance
/// decides, and a bound on `Display` always holds. So the key of a `Display` is never on the
/// stack. Its choice of a written conformance or the generated one is a disjunction.
///
/// A key that leaves with `false` holds under assumptions that are all `true`. So its answer is
/// never wrong, and every caller then fails too. A key that leaves with `true` and a `low` equal
/// to its own number is a head. It and every pending answer entered after it are final. A `true`
/// with a lower `low` rests on a key that is still open. So it waits as a pending answer, which
/// a later part query finds by its key. No key is expanded twice in one root query. When the root
/// query returns, the pending answers that are left rest on a key that failed, through
/// conjunctions. So each of them is `false`.
///
/// This is the fourth recorded exception to "A pass is linear in the program" (user,
/// 2026-10-27; ruling F9). A query costs one step for each class or enum type that it forms (see
/// `CODING_STANDARDS.md`, Performance). The types that a query forms from a concrete type are the
/// types that `mono` instantiates when the program uses them.
#[derive(Debug, Default)]
pub(crate) struct Open {
    stack: Vec<Frame>,
    /// The number of each key on the stack.
    on_stack: FxHashMap<Key, usize>,
    /// The pending answers in the order they left, so those entered after a head are a suffix.
    pending: Vec<(Key, usize)>,
    /// The number of each key that has a pending answer.
    pending_at: FxHashMap<Key, usize>,
    /// The number the last key got.
    counter: usize,
}

impl Open {
    /// Starts the query for `key`, if it is new: the key goes on the stack with the next number.
    pub(crate) fn enter(&mut self, key: Key) -> Entered {
        if let Some(&number) = self.on_stack.get(&key) {
            return Entered::Open(number);
        }
        if let Some(&number) = self.pending_at.get(&key) {
            return Entered::Pending(number);
        }
        if self.stack.is_empty() {
            self.counter = 0;
        }
        self.counter += 1;
        self.stack.push(Frame {
            key,
            number: self.counter,
            low: self.counter,
        });
        self.on_stack.insert(key, self.counter);
        Entered::New
    }

    /// Records that the key on top of the stack, the caller, rests on the key with the number
    /// `number`, which `enter` found open or pending: the part query answers `true` with no
    /// expansion.
    pub(crate) fn hit(&mut self, number: usize) {
        let caller = (self.stack.last_mut()).expect("a hit should have a caller on the stack");
        caller.low = caller.low.min(number);
    }

    /// Ends the query for `key`, the key on top of the stack, with `answer`. Gives the answers
    /// that are final, for the memo. A `false` is final at once. A `true` is final when it is a
    /// head, with the pending answers entered after it; otherwise it waits.
    pub(crate) fn leave(&mut self, key: Key, answer: bool) -> Vec<(Key, bool)> {
        let frame = (self.stack.pop()).expect("a query that ends should be on the stack");
        debug_assert_eq!(frame.key, key, "the query that ends should be the top one");
        self.on_stack.remove(&key);
        if !answer {
            return vec![(key, false)];
        }
        if frame.low == frame.number {
            let mut done = vec![(key, true)];
            while let Some(&(pending, number)) = self.pending.last() {
                if number <= frame.number {
                    break;
                }
                self.pending.pop();
                self.pending_at.remove(&pending);
                done.push((pending, true));
            }
            return done;
        }
        self.pending.push((key, frame.number));
        self.pending_at.insert(key, frame.number);
        // The root has the lowest number, so it is a head: a key that waits has a caller.
        let caller = (self.stack.last_mut()).expect("a key that waits should have a caller");
        caller.low = caller.low.min(frame.low);
        Vec::new()
    }

    /// How many keys are open, one inside the other.
    pub(crate) fn depth(&self) -> usize {
        self.stack.len()
    }

    /// The key of the root query, if one is open.
    pub(crate) fn root(&self) -> Option<Key> {
        self.stack.first().map(|frame| frame.key)
    }

    /// Whether no query is open: the root query has returned.
    pub(crate) fn is_idle(&self) -> bool {
        self.stack.is_empty()
    }

    /// Ends the root query: gives the pending answers that are left as `false` for the memo. A
    /// root that holds is a head, since no key has a lower number, so answers are left only when
    /// the root fails.
    pub(crate) fn settle(&mut self) -> Vec<(Key, bool)> {
        self.pending_at.clear();
        (self.pending.drain(..))
            .map(|(key, _)| (key, false))
            .collect()
    }
}

/// The payload types of each enum, declared: what [`parts`] and [`holds`] read of an enum. The
/// output of `mono` has `Enum`s; `mono` also keeps the payload types of the input.
pub(crate) trait Payloads {
    /// Calls `each` with the type of each field of each variant of the enum `id`.
    fn each(&self, id: EnumId, each: &mut dyn FnMut(Type));
}

impl Payloads for [Enum] {
    fn each(&self, id: EnumId, each: &mut dyn FnMut(Type)) {
        for payload in self[id.0].variants.iter().flat_map(|v| &v.fields) {
            each(payload.ty);
        }
    }
}

impl Payloads for [Vec<Type>] {
    fn each(&self, id: EnumId, each: &mut dyn FnMut(Type)) {
        for &ty in &self[id.0] {
            each(ty);
        }
    }
}

/// The parts of `ty`, whatever the interface: an array's element, an optional's contents and each
/// payload type of an enum, with the type's arguments in place of its parameters. Nothing for a
/// scalar, `string`, a class (it compares by identity, or through a written `Eq`), a type
/// parameter or an `any` value. A part whose associated type has no choice is poison, and is
/// left out. A generated `Display`, which asks no part, still lists them.
pub(crate) fn parts<P: Payloads + ?Sized>(
    conformances: &Conformances,
    enums: &P,
    ty: Type,
    mut each: impl FnMut(Type),
) {
    match ty.kind() {
        TypeKind::Array(part) | TypeKind::Optional(part) => each(part),
        TypeKind::Enum(id, args) => enums.each(id, &mut |payload| {
            if let Some(part) = normalize(conformances, payload.subst(args.get())) {
                each(part);
            }
        }),
        TypeKind::I64
        | TypeKind::U8
        | TypeKind::F64
        | TypeKind::Bool
        | TypeKind::Char
        | TypeKind::String
        | TypeKind::Unit
        | TypeKind::Never
        | TypeKind::Class(..)
        | TypeKind::Param(..)
        | TypeKind::Assoc(..)
        | TypeKind::Any(..) => {}
    }
}

/// Whether `print` of a value of type `ty` writes it through a helper that may reach a written
/// `to_string`: an array, an optional, a class, an enum or an `any` value, whose table holds the
/// helper of the type inside. A scalar and a `string` print as they always did and reach none.
pub(crate) fn prints_by_helper(ty: Type) -> bool {
    matches!(
        ty.kind(),
        TypeKind::Array(_)
            | TypeKind::Optional(_)
            | TypeKind::Class(..)
            | TypeKind::Enum(..)
            | TypeKind::Any(..)
    )
}

/// Whether the text of a value of type `ty` can hold a group, which `{x:#}` may break.
/// An array, an enum and an optional of an optional (the `Some(` of the `Some` rule) can.
/// So can an optional of an array or of an enum.
/// A type parameter or an associated type can, because its argument may be one of these, and so
/// can an `any` value, whose type inside may be one.
/// A scalar, a `string` and a class write one piece.
/// An optional of one of those is transparent.
pub(crate) fn makes_group(mut ty: Type) -> bool {
    while let TypeKind::Optional(inner) = ty.kind() {
        if matches!(inner.kind(), TypeKind::Optional(_)) {
            return true;
        }
        ty = inner;
    }
    matches!(
        ty.kind(),
        TypeKind::Array(_)
            | TypeKind::Enum(..)
            | TypeKind::Param(..)
            | TypeKind::Assoc(..)
            | TypeKind::Any(..)
    )
}

/// How an interface holds for a concrete type, in the ids of the program that `mono` reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Holds {
    /// A written conformance, the one at this place among the interface's, with the types that
    /// stand for its block's parameters.
    Written(usize, TypeArgs),
    /// The compiler writes it.
    Generated,
    /// The type has no such conformance.
    No,
}

impl Holds {
    /// Whether the interface holds, written or generated.
    pub(crate) fn is_yes(self) -> bool {
        self != Holds::No
    }
}

/// What [`holds`] reads: the conformances of the program and the payload types of its enums.
pub(crate) struct Source<'a, P: Payloads + ?Sized> {
    pub(crate) conformances: &'a Conformances,
    pub(crate) enums: &'a P,
    pub(crate) lang: LangInterfaces,
    /// The interfaces of the program, for those that an `any` type inherits.
    pub(crate) interfaces: &'a [Interface],
}

/// The state of `holds` for a program: the answer for each class or enum key, and the `Open`
/// state of the root query that is running. `mono` keeps one, so each key is decided once.
#[derive(Debug, Default)]
pub(crate) struct Decide {
    memo: FxHashMap<Key, Holds>,
    open: Open,
    /// The answer of each key that held and left, until it is final.
    held: FxHashMap<Key, Holds>,
    /// The interfaces that each `any` type conforms to, read once (see `any_closure`).
    any_closures: FxHashMap<Type, Rc<[(InterfaceId, TypeArgs)]>>,
}

/// How `interface` holds for the concrete type `ty` with the arguments `args`: the rule of the
/// checker (`Checker::conforms`) over the input's ids. A written `Eq` or `Ord` that covers `ty`
/// decides, with its bounds, and when they fail the type has none. A key of a class or an enum is
/// decided once (`cx.memo`), and a key whose query is open or pending holds (`Open`).
///
/// `Display` is different (see [`display_holds`]): a bound on it always holds, and the choice of
/// a written `to_string` is a root query of its own. Ask it only when no query is open.
pub(crate) fn holds<P: Payloads + ?Sized>(
    cx: &mut Decide,
    src: &Source<'_, P>,
    ty: Type,
    interface: InterfaceId,
    args: TypeArgs,
) -> Holds {
    if interface == src.lang.display {
        return display_holds(cx, src, ty, args);
    }
    ensure_stack(|| match ty.kind() {
        TypeKind::Class(..) | TypeKind::Enum(..) => holder_holds(cx, src, ty, interface, args),
        // An `any` value has the interface that it is made from, and those that it inherits.
        TypeKind::Any(..) => {
            let closure = cx.any_closures.entry(ty).or_insert_with(|| {
                let inherits =
                    |inherited: InterfaceId| src.interfaces[inherited.0].inherits.iter().copied();
                any_closure(inherits, ty, || {}).into()
            });
            if closure.contains(&(interface, args)) {
                Holds::Generated
            } else {
                Holds::No
            }
        }
        _ => match generated(src.lang, ty, interface) {
            Generated::Holds => Holds::Generated,
            Generated::Element(part) => {
                if holds(cx, src, part, interface, args).is_yes() {
                    Holds::Generated
                } else {
                    Holds::No
                }
            }
            Generated::Fails | Generated::Payloads(..) => Holds::No,
        },
    })
}

/// How `Display` holds for `ty`: a written conformance whose bounds hold, or else the generated
/// one, which every type but `unit` and `never` has. This is a choice between two ways, so a
/// failed bound is no failure of the query: the key is not on the stack of `cx.open`, and the
/// bounds of the record are root queries that end before the choice does. A key on that stack
/// would turn a pending answer of a query below it to `true` when its own fallback holds.
fn display_holds<P: Payloads + ?Sized>(
    cx: &mut Decide,
    src: &Source<'_, P>,
    ty: Type,
    args: TypeArgs,
) -> Holds {
    let display = src.lang.display;
    let generated = if generated(src.lang, ty, display) == Generated::Holds {
        Holds::Generated
    } else {
        Holds::No
    };
    if !matches!(ty.kind(), TypeKind::Class(..) | TypeKind::Enum(..)) {
        return generated;
    }
    let key = (ty, display, args);
    if let Some(&answer) = cx.memo.get(&key) {
        return answer;
    }
    debug_assert!(
        cx.open.is_idle(),
        "the choice of a Display should be made when no query is open"
    );
    #[cfg(test)]
    HOLDS_STEPS.set(HOLDS_STEPS.get() + 1);
    let written = find(src.conformances, display, ty, args)
        .filter(|&(k, stands)| record_holds(cx, src, display, k, stands));
    let answer = written.map_or(generated, |(k, stands)| Holds::Written(k, stands));
    cx.memo.insert(key, answer);
    answer
}

fn holder_holds<P: Payloads + ?Sized>(
    cx: &mut Decide,
    src: &Source<'_, P>,
    ty: Type,
    interface: InterfaceId,
    args: TypeArgs,
) -> Holds {
    let key = (ty, interface, args);
    if let Some(&answer) = cx.memo.get(&key) {
        return answer;
    }
    match cx.open.enter(key) {
        Entered::Open(number) | Entered::Pending(number) => {
            cx.open.hit(number);
            return Holds::Generated;
        }
        Entered::New => {}
    }
    #[cfg(test)]
    HOLDS_STEPS.set(HOLDS_STEPS.get() + 1);
    let answer = expanded(cx, src, ty, interface, args);
    if answer.is_yes() {
        cx.held.insert(key, answer);
    }
    let mut done = cx.open.leave(key, answer.is_yes());
    if cx.open.is_idle() {
        done.extend(cx.open.settle());
    }
    for (key, yes) in done {
        let value = cx.held.remove(&key).filter(|_| yes).unwrap_or(Holds::No);
        cx.memo.insert(key, value);
    }
    answer
}

/// The answer for a key that `holder_holds` has opened.
fn expanded<P: Payloads + ?Sized>(
    cx: &mut Decide,
    src: &Source<'_, P>,
    ty: Type,
    interface: InterfaceId,
    args: TypeArgs,
) -> Holds {
    if let Some((k, stands)) = find(src.conformances, interface, ty, args) {
        return if record_holds(cx, src, interface, k, stands) {
            Holds::Written(k, stands)
        } else {
            Holds::No
        };
    }
    match generated(src.lang, ty, interface) {
        Generated::Holds => Holds::Generated,
        Generated::Fails | Generated::Element(_) => Holds::No,
        Generated::Payloads(id, own) => {
            let mut parts = Vec::new();
            src.enums.each(id, &mut |payload| parts.push(payload));
            for payload in parts {
                let Some(part) = normalize(src.conformances, payload.subst(own.get())) else {
                    continue;
                };
                if !holds(cx, src, part, interface, TypeArgs::NONE).is_yes() {
                    return Holds::No;
                }
            }
            Holds::Generated
        }
    }
}

/// Whether the conformance `k` of `interface` holds when its block's parameters stand for
/// `stands`: each bound of each parameter holds for the type that stands for it. A bound whose
/// arguments have no choice for an associated type is poison, and holds.
fn record_holds<P: Payloads + ?Sized>(
    cx: &mut Decide,
    src: &Source<'_, P>,
    interface: InterfaceId,
    k: usize,
    stands: TypeArgs,
) -> bool {
    let record = src.conformances.get(interface, k);
    for (i, bounds) in record.bounds.iter().enumerate() {
        for bound in bounds {
            let args: Option<Vec<Type>> = (bound.args.get().iter())
                .map(|&arg| normalize(src.conformances, arg.subst(stands.get())))
                .collect();
            let Some(args) = args else { continue };
            let args = TypeArgs::intern(&args);
            let part = stands.get()[i];
            // A bound on `Display` holds for every type that can stand for a parameter.
            let yes = if bound.interface == src.lang.display {
                generated(src.lang, part, bound.interface) == Generated::Holds
            } else {
                holds(cx, src, part, bound.interface, args).is_yes()
            };
            if !yes {
                return false;
            }
        }
    }
    true
}

/// The types whose `Eq` or `Display` reaches a written method, settled for a program: the keys
/// `calls_written` has decided. The flag of a key is whether a `dbg` helper writes the type.
#[derive(Debug, Default)]
pub(crate) struct Written {
    settled: FxHashMap<(Type, InterfaceId, bool), bool>,
}

/// The field types of each class, declared: what [`calls_written`] and [`holds_class`] read of a
/// class to find what `dbg` shows of it. The output of `mono` has `Class`es; `mono` also keeps the
/// field types of the input.
pub(crate) trait Fields {
    /// Calls `each` with the type of each field of the class `id`.
    fn each(&self, id: ClassId, each: &mut dyn FnMut(Type));
}

impl Fields for [Class] {
    fn each(&self, id: ClassId, each: &mut dyn FnMut(Type)) {
        for field in &self[id.0].fields {
            each(field.ty);
        }
    }
}

impl Fields for [Vec<Type>] {
    fn each(&self, id: ClassId, each: &mut dyn FnMut(Type)) {
        for &ty in &self[id.0] {
            each(ty);
        }
    }
}

/// The field types of the class `ty`, with its arguments in place of its parameters, as `dbg`
/// shows them: in `print` mode. Nothing for any other type.
pub(crate) fn fields_of<F: Fields + ?Sized>(
    conformances: &Conformances,
    classes: &F,
    ty: Type,
    mut each: impl FnMut(Type),
) {
    if let TypeKind::Class(id, args) = ty.kind() {
        classes.each(id, &mut |field| {
            if let Some(part) = normalize(conformances, field.subst(args.get())) {
                each(part);
            }
        });
    }
}

/// Whether the helper for `interface` on the concrete type `ty` may call a written method:
/// `ty`, or a part of it through [`parts`], is a class or an enum with a written conformance. The
/// keys are the pairs of a type and `interface`; the key of a type with a conformance in
/// `conformances` is a leaf, and the key of any other type reaches the keys of its parts. A key
/// is true exactly when it reaches a leaf. The keys that are not settled yet are listed once, with
/// their parts, and settled by their strongly connected components in completion order, so each
/// key of a program is decided once (see [`reaches`]).
///
/// With `dbg`, the helper is the one that `dbg` calls. A class with no written conformance also
/// writes its fields, each in `print` mode, so each field type is a part of the class.
pub(crate) fn calls_written<P: Payloads + ?Sized, F: Fields + ?Sized>(
    conformances: &Conformances,
    (enums, classes): (&P, &F),
    ty: Type,
    interface: InterfaceId,
    dbg: bool,
    written: &mut Written,
) -> bool {
    reaches(
        &mut written.settled,
        (ty, interface, dbg),
        |(key, _, dbg), parts_of| {
            // A written `eq` is the whole comparison: the helper compares no part of the type.
            // The table of an `any` value may hold the written method of any conforming type.
            if has_written(conformances, interface, key) || matches!(key.kind(), TypeKind::Any(..))
            {
                return true;
            }
            if dbg && matches!(key.kind(), TypeKind::Class(..)) {
                fields_of(conformances, classes, key, |part| {
                    parts_of((part, interface, false));
                });
            } else {
                parts(conformances, enums, key, |part| {
                    parts_of((part, interface, dbg));
                });
            }
            false
        },
    )
}

/// Whether the concrete type `ty` holds a class with no written `Display`. It does when it is
/// one or an `any` value, whose table may hold one, or when an array, an optional or an enum
/// payload (any depth) is. Only such a class is written with its fields by `dbg`. A class or an
/// enum with a written `Display` shows that text, so the walk stops at it. A `dbg` helper of a
/// type that holds none is the `print` helper. The answers of `held` are the types decided so
/// far, which later queries read.
pub(crate) fn holds_class<P: Payloads + ?Sized>(
    (conformances, display): (&Conformances, InterfaceId),
    enums: &P,
    ty: Type,
    held: &mut FxHashMap<Type, bool>,
) -> bool {
    reaches(held, ty, |key, parts_of| {
        if has_written(conformances, display, key) {
            return false;
        }
        // The table of an `any` value may hold a class.
        if matches!(key.kind(), TypeKind::Class(..) | TypeKind::Any(..)) {
            return true;
        }
        parts(conformances, enums, key, parts_of);
        false
    })
}

/// Whether `conformances` holds a written conformance to `interface` for the concrete type `ty`.
pub(crate) fn has_written(conformances: &Conformances, interface: InterfaceId, ty: Type) -> bool {
    head_of(ty).is_some() && find(conformances, interface, ty, TypeArgs::NONE).is_some()
}

/// Whether `ty`, a type of a body that may name type parameters, has a written `Display` that
/// always applies. It does when each bound of the conformance is on `Display`, which always
/// holds. A bound on another interface may fail for the types that stand for the parameters.
pub(crate) fn always_written(conformances: &Conformances, display: InterfaceId, ty: Type) -> bool {
    head_of(ty).is_some()
        && find_with(conformances, display, ty, TypeArgs::NONE, |c| {
            (c.bounds.iter().flatten()).all(|bound| bound.interface == display)
        })
        .is_some()
}

/// Whether `start` reaches a leaf through the graph that `step` gives, a key at a time: `step`
/// answers whether the key is a leaf, and otherwise calls its argument with each key that it
/// reaches. `settled` holds the answers decided so far. The keys not settled that `start` reaches
/// are listed once, with their edges, and settled by their strongly connected components in
/// completion order. A component is true when one of its keys is a leaf or reaches a settled
/// true. So each key of a program is decided once, whichever order the queries come in. A
/// `false` met under an open query is never kept.
fn reaches<K: Copy + Eq + std::hash::Hash>(
    settled: &mut FxHashMap<K, bool>,
    start: K,
    mut step: impl FnMut(K, &mut dyn FnMut(K)) -> bool,
) -> bool {
    if let Some(&answer) = settled.get(&start) {
        return answer;
    }
    // The new keys, with the edges to the new keys they reach and whether they are a leaf or
    // reach a settled `true`.
    let mut keys: Vec<K> = vec![start];
    let mut at: FxHashMap<K, usize> = FxHashMap::default();
    at.insert(start, 0);
    let mut edges: Vec<Vec<usize>> = Vec::new();
    let mut yes: Vec<bool> = Vec::new();
    let mut next = 0;
    while next < keys.len() {
        let key = keys[next];
        next += 1;
        #[cfg(test)]
        CALLS_KEYS.set(CALLS_KEYS.get() + 1);
        let mut reach = Vec::new();
        let mut settled_true = false;
        let leaf = step(key, &mut |part| {
            if let Some(&answer) = settled.get(&part) {
                settled_true |= answer;
                return;
            }
            let index = *at.entry(part).or_insert_with(|| {
                keys.push(part);
                keys.len() - 1
            });
            reach.push(index);
        });
        settled_true |= leaf;
        edges.push(reach);
        yes.push(settled_true);
    }
    let component = components(&edges);
    let count = component.iter().max().map_or(0, |&c| c + 1);
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (key, &c) in component.iter().enumerate() {
        members[c].push(key);
    }
    // A component reaches only components with a lower number, which are settled by then.
    for members in &members {
        let answer = members.iter().any(|&m| yes[m])
            || members.iter().any(|&m| {
                edges[m]
                    .iter()
                    .any(|&n| component[n] != component[m] && settled[&keys[n]])
            });
        for &m in members {
            settled.insert(keys[m], answer);
        }
    }
    settled[&start]
}

/// `ty` with every associated type replaced whose base is a class or an enum: by the type that
/// the base's conformance chooses, with the types that stand for the conformance's parameters in
/// it, innermost first. An associated type whose base is a type parameter, or another associated
/// type of one, stays: a body is checked once for every type argument, and `mono` normalizes the
/// instance. `None` when a base has no conformance to the interface, or the conformance chooses no
/// type: the type does not exist, and the checker reported why. Also `None` when the choice of an
/// associated type needs the associated type itself, or more than `CHOICE_DEPTH_LIMIT` choices
/// are in progress (see `Stuck`).
///
/// A choice names an associated type only through a parameter of its own conformance, so a
/// choice that grows its types does not end the walk by itself: the walk keeps the associated
/// types it is in the middle of, and meeting one again ends it, as does the limit on how many are
/// in progress. Each distinct type is normalized once, whatever number of paths lead to it.
pub(crate) fn normalize(conformances: &Conformances, ty: Type) -> Option<Type> {
    normalize_noting(conformances, ty, &mut Vec::new())
}

/// How many associated types a walk may be in the middle of at once, each with a choice that names
/// one more. A choice that grows its types never meets the same associated type again, so this
/// limit, and not a loop, ends such a walk. It is the recursion limit of Rust, and it bounds the
/// depth of the walk, and so its stack.
///
/// The count is of the choices in progress, not of the steps that a type needs in all: a type that
/// the walk has done already (`memo`) ends a chain at once, so the same type can be within the
/// limit after another type of the same walk and past it alone.
pub(crate) const CHOICE_DEPTH_LIMIT: usize = 128;

/// The first type found for each choice that a walk stopped at, with why.
pub(crate) type StuckChoices = FxHashMap<ChoicePlace, (Type, Cause)>;

/// Why a walk stopped at an associated type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cause {
    /// The choice of the associated type needs the associated type itself.
    Loops,
    /// The associated type is more than `CHOICE_DEPTH_LIMIT` choices deep.
    TooDeep,
}

/// An associated type that the walk did not follow: the type, the place of the choice that it
/// stopped at, and why. The type is the one the walk was asked for: for a loop, the type that met
/// itself, and for a chain that is too deep, the first of the chain, which needs the steps. The
/// place is the interface, the index of the conformance among those of the interface and the
/// index of the associated type among the interface's: the choice where the walk stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stuck {
    pub(crate) assoc: Type,
    pub(crate) interface: InterfaceId,
    pub(crate) index: usize,
    pub(crate) k: usize,
    pub(crate) cause: Cause,
}

/// `normalize`, adding to `stuck` each loop and each chain that is too deep that it meets, which
/// its caller reports.
pub(crate) fn normalize_noting(
    conformances: &Conformances,
    ty: Type,
    stuck: &mut Vec<Stuck>,
) -> Option<Type> {
    if !ty.has_assoc() {
        return Some(ty);
    }
    Normalizer {
        conformances,
        memo: FxHashMap::default(),
        active: FxHashMap::default(),
        start: None,
        stuck,
    }
    .walk(ty)
}

/// The state of one `normalize`.
struct Normalizer<'a> {
    conformances: &'a Conformances,
    /// The result for each type done.
    memo: FxHashMap<Type, Type>,
    /// The associated types whose choice the walk is in, each with the place of that choice.
    active: FxHashMap<Type, (InterfaceId, usize, usize)>,
    /// The first of the associated types in `active`: the one that the chain in progress began at.
    start: Option<Type>,
    stuck: &'a mut Vec<Stuck>,
}

impl Normalizer<'_> {
    fn walk(&mut self, ty: Type) -> Option<Type> {
        if !ty.has_assoc() {
            return Some(ty);
        }
        if let Some(&done) = self.memo.get(&ty) {
            return Some(done);
        }
        #[cfg(test)]
        NORMALIZED.set(NORMALIZED.get() + 1);
        let done = match ty.kind() {
            TypeKind::Array(element) => Type::array(self.walk(element)?),
            TypeKind::Optional(payload) => Type::optional(self.walk(payload)?),
            TypeKind::Class(id, args) => Type::intern(TypeKind::Class(id, self.all(args)?)),
            TypeKind::Enum(id, args) => Type::intern(TypeKind::Enum(id, self.all(args)?)),
            TypeKind::Any(id, args) => Type::intern(TypeKind::Any(id, self.all(args)?)),
            TypeKind::Assoc(base, interface, args, k) => {
                let base = self.walk(base)?;
                let args = self.all(args)?;
                match base.kind() {
                    TypeKind::Class(..) | TypeKind::Enum(..) => {
                        self.choice(Type::intern(TypeKind::Assoc(base, interface, args, k)))?
                    }
                    TypeKind::Param(..) | TypeKind::Assoc(..) => {
                        Type::intern(TypeKind::Assoc(base, interface, args, k))
                    }
                    _ => return None,
                }
            }
            _ => unreachable!("only a kind with a child can hold an associated type"),
        };
        self.memo.insert(ty, done);
        Some(done)
    }

    fn all(&mut self, args: TypeArgs) -> Option<TypeArgs> {
        let list = (args.get().iter())
            .map(|&ty| self.walk(ty))
            .collect::<Option<Vec<Type>>>()?;
        Some(TypeArgs::intern(&list))
    }

    /// The type that the conformance of `assoc`'s base chooses for it, normalized. `assoc` is in
    /// progress while its choice is walked: meeting it there is a loop, and one that would be the
    /// `CHOICE_DEPTH_LIMIT + 1`th is too deep.
    fn choice(&mut self, assoc: Type) -> Option<Type> {
        let TypeKind::Assoc(base, interface, args, k) = assoc.kind() else {
            unreachable!("only an associated type should have a choice");
        };
        if let Some(&(interface, index, k)) = self.active.get(&assoc) {
            let cause = Cause::Loops;
            self.stuck.push(Stuck {
                assoc,
                interface,
                index,
                k,
                cause,
            });
            return None;
        }
        let (index, stands) = find(self.conformances, interface, base, args)?;
        let chosen = (*self
            .conformances
            .get(interface, index)
            .assoc
            .get(k as usize)?)?;
        let chosen = chosen.subst(stands.get());
        let k = k as usize;
        if chosen.has_assoc() && self.active.len() >= CHOICE_DEPTH_LIMIT {
            let cause = Cause::TooDeep;
            // The chain began at `start`, and needs the steps; the choice here is where it stops.
            // `active` holds `CHOICE_DEPTH_LIMIT` choices, so `start` is set: it was set when
            // `active` was empty, and each top-level choice sets it again before any push. A type
            // parameter in its name belongs to the block of the start choice, which can be another
            // block than the one of the choice that gives the span.
            let assoc = self.start.unwrap_or(assoc);
            self.stuck.push(Stuck {
                assoc,
                interface,
                index,
                k,
                cause,
            });
            return None;
        }
        if self.active.is_empty() {
            self.start = Some(assoc);
        }
        self.active.insert(assoc, (interface, index, k));
        let done = self.walk(chosen);
        self.active.remove(&assoc);
        done
    }
}

/// The first conformance of `interface` that covers a type together with a new one: `target`,
/// which has `params` parameters, with the interface arguments `args`; and the types they both
/// cover. Only the conformances that the index names are read: those of the same exact target,
/// and those of the same class or enum with parameters.
///
/// The second recorded exception to "A pass is linear in the program" (user, 2026-10-25; ruling
/// F9), as for `find`: a target with parameters meets every exact target of its class or enum,
/// and every target with parameters of it, so `N` blocks of one class cost `N²` overlap tests.
pub(crate) fn clash(
    conformances: &Conformances,
    interface: InterfaceId,
    target: Type,
    params: usize,
    args: TypeArgs,
) -> Option<(usize, Vec<Type>)> {
    let mut mine = vec![target];
    mine.extend_from_slice(args.get());
    // A target without parameters meets the one of its exact type; one with parameters meets the
    // exact targets of its class or enum (the recorded exception of the doc comment).
    let head = head_of(target).expect("the target of a conformance should be a class or an enum");
    let exact = if params == 0 {
        conformances.exact(interface, target, args)
    } else {
        conformances.exact_of(interface, head)
    };
    let generic = conformances.generic(interface, head);
    exact.iter().chain(generic).find_map(|&k| {
        #[cfg(test)]
        CANDIDATES_READ.set(CANDIDATES_READ.get() + 1);
        let c = conformances.get(interface, k);
        let mut theirs = vec![c.target];
        theirs.extend_from_slice(c.args.get());
        let common = overlap(&mine, params, &theirs, c.type_params)?;
        Some((k, common))
    })
}

/// What the `params` parameters of `target` stand for when it is `ty`: the one-sided match of
/// the structure. A parameter takes whatever stands in its place, and the same parameter must
/// meet the same type. `T?` meets only an optional: `Bindings::unify` (`check/generic.rs`)
/// would bind it to a plain `i64`, as an argument converts, and `extend<T> Box<T?>` must not
/// cover `Box<i64>`. `None` when the structures differ, or a parameter stands for nothing.
pub(crate) fn bind(target: Type, params: usize, ty: Type) -> Option<TypeArgs> {
    let mut slots = vec![None; params];
    matches(target, ty, &mut slots)?;
    let list = slots.into_iter().collect::<Option<Vec<Type>>>()?;
    Some(TypeArgs::intern(&list))
}

/// Records in `slots` what each parameter of `pattern` stands for in `ty`.
fn matches(pattern: Type, ty: Type, slots: &mut [Option<Type>]) -> Option<()> {
    match (pattern.kind(), ty.kind()) {
        (TypeKind::Param(_, i), _) => {
            let slot = &mut slots[i as usize];
            if let Some(bound) = *slot {
                return (bound == ty).then_some(());
            }
            *slot = Some(ty);
            Some(())
        }
        (TypeKind::Array(p), TypeKind::Array(t))
        | (TypeKind::Optional(p), TypeKind::Optional(t)) => matches(p, t, slots),
        (TypeKind::Class(a, p), TypeKind::Class(b, t))
            if a == b && p.get().len() == t.get().len() =>
        {
            matches_all(p, t, slots)
        }
        (TypeKind::Enum(a, p), TypeKind::Enum(b, t))
            if a == b && p.get().len() == t.get().len() =>
        {
            matches_all(p, t, slots)
        }
        (TypeKind::Any(a, p), TypeKind::Any(b, t)) if a == b && p.get().len() == t.get().len() => {
            matches_all(p, t, slots)
        }
        (
            TypeKind::I64
            | TypeKind::U8
            | TypeKind::F64
            | TypeKind::Bool
            | TypeKind::Char
            | TypeKind::String
            | TypeKind::Unit
            | TypeKind::Never
            | TypeKind::Array(_)
            | TypeKind::Optional(_)
            | TypeKind::Class(..)
            | TypeKind::Enum(..)
            | TypeKind::Assoc(..)
            | TypeKind::Any(..),
            _,
        ) => (pattern == ty).then_some(()),
    }
}

fn matches_all(patterns: TypeArgs, types: TypeArgs, slots: &mut [Option<Type>]) -> Option<()> {
    (patterns.get().iter().zip(types.get()))
        .try_for_each(|(&pattern, &ty)| matches(pattern, ty, slots))
}

/// The types that two conformances cover together, if they do: `a` is the target and then the
/// interface arguments of the first, with `a_params` parameters, and `b` the same for the
/// second. The second's parameters follow the first's, so the two never share a number. The
/// types come back as the most general ones both sides cover, with the parameters that are still
/// free in them. A parameter never stands for a type that holds it, so `extend<U> Box<U>:
/// Convert<[U]>` never meets `extend<T> Box<T>: Convert<T>`. The check for that waits until
/// every pair is unified, and is one pass over the bindings.
pub(crate) fn overlap(
    a: &[Type],
    a_params: usize,
    b: &[Type],
    b_params: usize,
) -> Option<Vec<Type>> {
    if a.len() != b.len() {
        return None;
    }
    // The names carry no meaning: a parameter is its number.
    let name = Symbol::intern("_");
    let shifted: Vec<Type> = (a_params..a_params + b_params)
        .map(|i| {
            let i = u32::try_from(i).expect("a declaration should have fewer than 2^32 parameters");
            Type::intern(TypeKind::Param(name, i))
        })
        .collect();
    let mut unifier = Unifier::new(a_params + b_params);
    for (&x, &y) in a.iter().zip(b) {
        if !unifier.unify(x, y.subst(&shifted)) {
            return None;
        }
    }
    a.iter().map(|&ty| unifier.resolve(ty)).collect()
}

/// Why a chain of replacements cannot be longer than the parameters.
const CHAIN: &str =
    "a chain of parameters that stand for each other should be shorter than the parameters";

/// The two-sided unifier of `overlap`: what each parameter stands for so far. A parameter may
/// stand for a type that holds it; `resolve` finds that out when the unifying is done.
struct Unifier {
    stood: Vec<Option<Type>>,
    /// The answer of `resolve` for each parameter that stands for something.
    memo: Vec<Memo>,
    /// The types that `unify` has made one, as a union-find forest: a type in a class has
    /// unified with the others of it, so a pair of one class is not unified again, and a cyclic
    /// binding cannot make `unify` run for ever.
    classes: FxHashMap<Type, usize>,
    parents: Vec<usize>,
}

/// What `resolve` knows of a parameter.
#[derive(Clone, Copy)]
enum Memo {
    Unknown,
    /// Being resolved: the parameter was met again on the way, so it stands for a type that holds
    /// it, and no type can stand for it.
    Doing,
    Done(Type),
}

impl Unifier {
    fn new(params: usize) -> Self {
        Self {
            stood: vec![None; params],
            memo: vec![Memo::Unknown; params],
            classes: FxHashMap::default(),
            parents: Vec::new(),
        }
    }

    /// The class of `ty` among the types that `unify` has made one.
    fn class(&mut self, ty: Type) -> usize {
        let next = self.parents.len();
        let at = *self.classes.entry(ty).or_insert(next);
        if at == next {
            self.parents.push(next);
        }
        let mut root = at;
        while self.parents[root] != root {
            root = self.parents[root];
        }
        // Path compression: every type on the way points at the root.
        let mut at = at;
        while self.parents[at] != root {
            at = std::mem::replace(&mut self.parents[at], root);
        }
        root
    }

    /// Makes `x` and `y` one class, and says whether they were one already.
    fn join(&mut self, x: Type, y: Type) -> bool {
        let (x, y) = (self.class(x), self.class(y));
        self.parents[x] = y;
        x == y
    }

    /// `ty`, with the parameter at its root replaced while one stands for something. A chain of
    /// parameters that stand for each other is no longer than the parameters, so a longer one is
    /// a parameter that stands for a type that holds it. Each parameter on the way then stands for
    /// the end, so that a later walk over the chain takes one step.
    fn walk(&mut self, start: Type) -> Type {
        let (mut ty, mut steps) = (start, 0);
        while let TypeKind::Param(_, i) = ty.kind()
            && let Some(Some(next)) = self.stood.get(i as usize)
        {
            #[cfg(test)]
            WALK_STEPS.set(WALK_STEPS.get() + 1);
            ty = *next;
            steps += 1;
            assert!(steps <= self.stood.len(), "{CHAIN}");
        }
        let mut at = start;
        while at != ty
            && let TypeKind::Param(_, i) = at.kind()
            && let Some(next) = self.stood[i as usize].replace(ty)
        {
            at = next;
        }
        ty
    }

    /// `ty` with every parameter replaced that stands for something, or `None` when a parameter
    /// stands for a type that holds it. A parameter is resolved once, whatever number of places
    /// hold it, so types that share a binding cost the sum of their parts, not the size of the
    /// type they unfold to; and the parameters that are met again while one is resolved are the
    /// cycles, so this is the one pass that finds them.
    fn resolve(&mut self, ty: Type) -> Option<Type> {
        #[cfg(test)]
        RESOLVE_STEPS.set(RESOLVE_STEPS.get() + 1);
        Some(match ty.kind() {
            TypeKind::Param(_, i) => {
                let Some(&Some(next)) = self.stood.get(i as usize) else {
                    return Some(ty);
                };
                match self.memo[i as usize] {
                    Memo::Done(done) => return Some(done),
                    Memo::Doing => return None,
                    Memo::Unknown => {}
                }
                self.memo[i as usize] = Memo::Doing;
                let done = self.resolve(next)?;
                self.memo[i as usize] = Memo::Done(done);
                done
            }
            TypeKind::Array(element) => Type::array(self.resolve(element)?),
            TypeKind::Optional(payload) => Type::optional(self.resolve(payload)?),
            TypeKind::Class(id, args) => Type::intern(TypeKind::Class(id, self.resolve_all(args)?)),
            TypeKind::Enum(id, args) => Type::intern(TypeKind::Enum(id, self.resolve_all(args)?)),
            TypeKind::Any(id, args) => Type::intern(TypeKind::Any(id, self.resolve_all(args)?)),
            // An associated type is opaque here, as it is to `unify`.
            TypeKind::I64
            | TypeKind::U8
            | TypeKind::F64
            | TypeKind::Bool
            | TypeKind::Char
            | TypeKind::String
            | TypeKind::Unit
            | TypeKind::Never
            | TypeKind::Assoc(..) => ty,
        })
    }

    fn resolve_all(&mut self, args: TypeArgs) -> Option<TypeArgs> {
        let list = (args.get().iter())
            .map(|&ty| self.resolve(ty))
            .collect::<Option<Vec<Type>>>()?;
        Some(TypeArgs::intern(&list))
    }

    /// Makes `x` and `y` the same type, if some types can stand for their parameters. A
    /// parameter may come to stand for a type that holds it: `resolve` rejects that.
    fn unify(&mut self, left: Type, right: Type) -> bool {
        #[cfg(test)]
        UNIFY_STEPS.set(UNIFY_STEPS.get() + 1);
        let (left, right) = (self.walk(left), self.walk(right));
        if left == right {
            return true;
        }
        match (left.kind(), right.kind()) {
            // The later parameter stands for the earlier, so the first side's names stay.
            (TypeKind::Param(_, i), TypeKind::Param(_, j)) => {
                match i.cmp(&j) {
                    Ordering::Less => self.stood[j as usize] = Some(left),
                    _ => self.stood[i as usize] = Some(right),
                }
                true
            }
            (TypeKind::Param(_, i), _) => {
                self.stood[i as usize] = Some(right);
                true
            }
            (_, TypeKind::Param(_, i)) => {
                self.stood[i as usize] = Some(left);
                true
            }
            (TypeKind::Array(l), TypeKind::Array(r))
            | (TypeKind::Optional(l), TypeKind::Optional(r)) => {
                self.join(left, right) || self.unify(l, r)
            }
            (TypeKind::Class(from, l), TypeKind::Class(to, r))
                if from == to && l.get().len() == r.get().len() =>
            {
                self.join(left, right) || self.unify_all(l, r)
            }
            (TypeKind::Enum(from, l), TypeKind::Enum(to, r))
                if from == to && l.get().len() == r.get().len() =>
            {
                self.join(left, right) || self.unify_all(l, r)
            }
            (TypeKind::Any(from, l), TypeKind::Any(to, r))
                if from == to && l.get().len() == r.get().len() =>
            {
                self.join(left, right) || self.unify_all(l, r)
            }
            (
                TypeKind::I64
                | TypeKind::U8
                | TypeKind::F64
                | TypeKind::Bool
                | TypeKind::Char
                | TypeKind::String
                | TypeKind::Unit
                | TypeKind::Never
                | TypeKind::Array(_)
                | TypeKind::Optional(_)
                | TypeKind::Class(..)
                | TypeKind::Enum(..)
                | TypeKind::Assoc(..)
                | TypeKind::Any(..),
                _,
            ) => false,
        }
    }

    fn unify_all(&mut self, left: TypeArgs, right: TypeArgs) -> bool {
        (left.get().iter().zip(right.get())).all(|(&l, &r)| self.unify(l, r))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::{ClassId, Conformance, EnumId, FuncId};

    const BOX: usize = 0;
    const PAIR: usize = 1;
    const CIRCLE: usize = 2;
    const PILE: usize = 3;

    fn class(id: usize, args: &[Type]) -> Type {
        Type::intern(TypeKind::Class(ClassId(id), TypeArgs::intern(args)))
    }

    fn param(i: u32) -> Type {
        Type::intern(TypeKind::Param(Symbol::intern("T"), i))
    }

    fn boxed(arg: Type) -> Type {
        class(BOX, &[arg])
    }

    fn tree(id: usize, args: &[Type]) -> Type {
        Type::intern(TypeKind::Enum(EnumId(id), TypeArgs::intern(args)))
    }

    /// The conformances of interface 0 that `rows` write: the target with `params` parameters,
    /// and the interface's arguments.
    fn conformances(rows: &[(Type, usize, &[Type])]) -> Conformances {
        let mut all = Conformances::default();
        for &(target, type_params, args) in rows {
            all.add(
                InterfaceId(0),
                Conformance {
                    type_params,
                    target,
                    args: TypeArgs::intern(args),
                    methods: Vec::<FuncId>::new(),
                    assoc: Vec::new(),
                    bounds: Vec::new(),
                },
            );
        }
        all
    }

    /// Two lists of a target and interface arguments, with their parameters, and what they cover.
    type Row<'a> = (&'a [Type], usize, &'a [Type], usize, Option<Vec<Type>>);

    /// Two conformances cover a type together when their targets and arguments unify, each side's
    /// parameters its own, and a parameter never stands for a type that holds it.
    #[test]
    fn overlap_should_find_the_most_general_type_both_cover() {
        let (t, u) = (param(0), param(0));
        let pair = |a, b| class(PAIR, &[a, b]);
        let circle = class(CIRCLE, &[]);
        let rows: [Row<'_>; 25] = [
            // `extend<T> Box<T>: Convert<T>` and `extend<U> Box<U>: Convert<i64>`.
            (
                &[boxed(t), t],
                1,
                &[boxed(u), Type::I64],
                1,
                Some(vec![boxed(Type::I64), Type::I64]),
            ),
            // `Convert<[U]>` never meets `Convert<T>` for the same box: `T` would hold itself.
            (&[boxed(t), t], 1, &[boxed(u), Type::array(u)], 1, None),
            // `Pair<T, i64>` and `Pair<i64, U>` meet at `Pair<i64, i64>`.
            (
                &[pair(t, Type::I64)],
                1,
                &[pair(Type::I64, u)],
                1,
                Some(vec![pair(Type::I64, Type::I64)]),
            ),
            // Another class; the second side alone differs; the first side alone differs.
            (&[circle], 0, &[boxed(u)], 1, None),
            (&[boxed(Type::I64)], 0, &[boxed(Type::F64)], 0, None),
            (&[circle, Type::I64], 0, &[circle, Type::F64], 0, None),
            (&[circle, Type::F64], 0, &[circle, Type::I64], 0, None),
            // One parameter twice must stand for one type.
            (&[pair(t, t)], 1, &[pair(Type::I64, Type::F64)], 0, None),
            // An optional meets only an optional; its payload stays free.
            (&[boxed(Type::optional(t))], 1, &[boxed(Type::I64)], 0, None),
            (
                &[boxed(Type::optional(t))],
                1,
                &[boxed(u)],
                1,
                Some(vec![boxed(Type::optional(t))]),
            ),
            // Lists of another length are of other interfaces.
            (&[circle], 0, &[circle, Type::I64], 0, None),
            // Arrays and optionals unify by their elements, and meet only their own kind.
            (
                &[Type::array(t)],
                1,
                &[Type::array(Type::I64)],
                0,
                Some(vec![Type::array(Type::I64)]),
            ),
            (
                &[Type::optional(t)],
                1,
                &[Type::optional(Type::I64)],
                0,
                Some(vec![Type::optional(Type::I64)]),
            ),
            (&[Type::array(t)], 1, &[Type::optional(u)], 1, None),
            // Enums as classes: the same enum with arguments that unify; another enum; another
            // number of arguments; a class of another number of arguments; another class.
            (
                &[tree(0, &[t])],
                1,
                &[tree(0, &[Type::I64])],
                0,
                Some(vec![tree(0, &[Type::I64])]),
            ),
            (&[tree(0, &[t])], 1, &[tree(1, &[u])], 1, None),
            (&[tree(0, &[t, t])], 1, &[tree(0, &[u])], 1, None),
            (&[class(BOX, &[t, t])], 1, &[class(BOX, &[u])], 1, None),
            (&[boxed(t)], 1, &[class(PILE, &[u])], 1, None),
            // A parameter never stands for a type that holds it, in a class or an enum.
            (&[boxed(t), t], 1, &[boxed(u), boxed(u)], 1, None),
            (
                &[tree(0, &[t]), t],
                1,
                &[tree(0, &[u]), tree(0, &[u])],
                1,
                None,
            ),
            // `Pair<T, [T]>` and `Pair<[U], U>` each need the other to hold itself.
            (
                &[pair(t, Type::array(t))],
                1,
                &[pair(Type::array(u), u)],
                1,
                None,
            ),
            // `V := Pair<T>`, then `T := Pair<V>` closes a cycle, and `T` with `V` meets two types
            // that each hold the other: the unifier must end, and the targets cover no type.
            (&[pair(t, t), t, t], 1, &[u, pair(u, u), u], 1, None),
            // Two parameters meet at the first side's, so a report names it, whichever of them
            // the walk meets first.
            (&[boxed(t)], 1, &[boxed(u)], 1, Some(vec![boxed(t)])),
            (
                &[pair(t, param(1))],
                2,
                &[pair(u, u)],
                1,
                Some(vec![pair(t, t)]),
            ),
        ];
        for (i, (a, a_params, b, b_params, expected)) in rows.into_iter().enumerate() {
            assert_eq!(overlap(a, a_params, b, b_params), expected, "row {i}");
        }
    }

    /// Targets that bind `v0 = Pair<w1, w1>`, `w1 = Pair<w2, w2>` and so on share a binding in
    /// each level. The type they unfold to is `2^n` nodes, and the passes read each shared binding
    /// once, so the unifying and the resolution follow `n`.
    #[test]
    fn overlap_should_read_a_shared_binding_once() {
        let pair = |a, b| class(PAIR, &[a, b]);
        let shared = |ty| pair(ty, ty);
        for n in [8_usize, 16] {
            // Each side numbers its parameters from 0.
            let v = |i: usize| param(u32::try_from(i).expect("n should be small"));
            let w = v;
            // `v_k = Pair<w_(k+1), w_(k+1)>` in the first half, then `w_k = v_k` from the last
            // parameter down: each `w_k` takes a type that holds the one before it twice.
            let mut a: Vec<Type> = (0..n).map(v).collect();
            let mut b: Vec<Type> = (0..n - 1).map(|k| shared(w(k + 1))).collect();
            b.push(Type::I64);
            a.extend((1..n).rev().map(v));
            b.extend((1..n).rev().map(w));
            RESOLVE_STEPS.set(0);
            UNIFY_STEPS.set(0);
            let common = overlap(&a, n, &b, n).expect("the targets should cover a type");
            let mut expected = Type::I64;
            for _ in 1..n {
                expected = shared(expected);
            }
            assert_eq!(common[0], expected, "n = {n}");
            let (resolved, unified) = (RESOLVE_STEPS.get(), UNIFY_STEPS.get());
            assert!(resolved <= 8 * n, "n = {n}: {resolved} resolutions");
            assert!(unified <= 2 * n, "n = {n}: {unified} unifications");
        }
    }

    /// Two types that share their parts in each level, `v_k = Pair<w_(k+1), w_(k+1)>` against
    /// `w_k = Pair<v_(k+1), v_(k+1)>`, unify level by level. A pair of types unifies once, so the
    /// second of the two arguments of a level costs nothing, and the work follows `n`, not `2^n`.
    #[test]
    fn overlap_should_unify_a_pair_of_shared_types_once() {
        let pair = |a, b| class(PAIR, &[a, b]);
        let shared = |ty| pair(ty, ty);
        for n in [8_usize, 16] {
            let p = |i: usize| param(u32::try_from(i).expect("n should be small"));
            let (mut a, mut b) = (Vec::new(), Vec::new());
            for k in 0..n - 1 {
                a.push(p(k));
                b.push(shared(p(k + 1)));
                a.push(shared(p(k + 1)));
                b.push(p(k));
            }
            a.extend([p(n - 1), Type::I64, p(0)]);
            b.extend([Type::I64, p(n - 1), p(0)]);
            RESOLVE_STEPS.set(0);
            UNIFY_STEPS.set(0);
            let common = overlap(&a, n, &b, n).expect("the targets should cover a type");
            let mut expected = Type::I64;
            for _ in 1..n {
                expected = shared(expected);
            }
            assert_eq!(common[0], expected, "n = {n}");
            let (resolved, unified) = (RESOLVE_STEPS.get(), UNIFY_STEPS.get());
            assert!(resolved <= 10 * n, "n = {n}: {resolved} resolutions");
            assert!(unified <= 4 * n, "n = {n}: {unified} unifications");
        }
    }

    /// A chain of parameters that stand for each other is no longer than the parameters: the
    /// walk fails with its name when one is, and resolving a parameter that holds itself ends
    /// with no type.
    #[test]
    #[should_panic(expected = "a chain of parameters")]
    fn walk_should_fail_on_a_chain_that_loops() {
        let mut unifier = Unifier::new(2);
        unifier.stood = vec![Some(param(1)), Some(param(0))];
        unifier.walk(param(0));
    }

    /// A target whose first `n` arguments make a chain of parameters, `T(n-1)` stands for `U`,
    /// `T(n-2)` for `T(n-1)` and so on, and whose other `n` arguments are the first end of the
    /// chain again, unifies with a few steps over the chain for each argument, and not `n` steps
    /// for each of the `n` arguments after it.
    #[test]
    fn unify_should_step_over_a_chain_of_parameters_once() {
        for n in [128_usize, 256] {
            let p = |i: usize| param(u32::try_from(i).expect("n should be small"));
            let mut left: Vec<Type> = (0..n).rev().map(p).collect();
            left.extend(std::iter::repeat_n(p(n - 1), n));
            let right = vec![p(0); 2 * n];
            WALK_STEPS.set(0);
            let common = overlap(&[class(PAIR, &left)], n, &[class(PAIR, &right)], 1);
            assert!(common.is_some(), "n = {n}");
            let walked = WALK_STEPS.get();
            assert!(walked <= 8 * n, "n = {n}: {walked} steps");
        }
    }

    #[test]
    fn resolve_should_find_no_type_for_a_parameter_that_holds_itself() {
        let mut unifier = Unifier::new(2);
        unifier.stood = vec![Some(Type::array(param(1))), Some(Type::array(param(0)))];
        assert_eq!(unifier.resolve(param(0)), None);
        let mut unifier = Unifier::new(2);
        unifier.stood = vec![
            Some(Type::optional(param(1))),
            Some(Type::optional(param(0))),
        ];
        assert_eq!(unifier.resolve(param(0)), None);
    }

    /// Finding the class of a type points every type on the way at the root, so a chain of joins
    /// is walked once and not again by each later find.
    #[test]
    fn class_should_point_every_type_on_the_path_at_the_root() {
        let mut unifier = Unifier::new(0);
        let chain = [Type::I64, Type::F64, Type::U8, Type::array(Type::I64)];
        // Each join puts the root of the first under the second: a chain from the first type.
        for pair in chain.windows(2) {
            assert!(
                !unifier.join(pair[0], pair[1]),
                "{pair:?} should be two classes"
            );
        }
        let root = unifier.class(chain[3]);
        assert_eq!(unifier.class(chain[0]), root);
        for ty in chain {
            let at = unifier.classes[&ty];
            assert!(
                at == root || unifier.parents[at] == root,
                "{ty:?} should point at the root"
            );
        }
        assert!(unifier.join(chain[0], chain[2]), "one class joins itself");
    }

    /// The target of a conformance with parameters binds them from the structure of a type; a
    /// parameter stands for one type; an optional is matched by an optional only.
    #[test]
    fn bind_should_match_the_structure_one_sided() {
        let pair = |a, b| class(PAIR, &[a, b]);
        let binds = |target, params, ty| bind(target, params, ty).map(|args| args.get().to_vec());
        let t = param(0);
        assert_eq!(binds(boxed(t), 1, boxed(Type::I64)), Some(vec![Type::I64]));
        let nested = boxed(boxed(Type::I64));
        assert_eq!(binds(boxed(t), 1, nested), Some(vec![boxed(Type::I64)]));
        assert_eq!(binds(boxed(boxed(t)), 1, nested), Some(vec![Type::I64]));
        // The same parameter twice.
        assert_eq!(
            binds(pair(t, t), 1, pair(Type::I64, Type::I64)),
            Some(vec![Type::I64])
        );
        assert_eq!(binds(pair(t, t), 1, pair(Type::I64, Type::F64)), None);
        // Another class, another structure, and a parameter that stands for nothing.
        assert_eq!(binds(boxed(t), 1, class(PILE, &[Type::I64])), None);
        assert_eq!(binds(boxed(t), 1, Type::I64), None);
        assert_eq!(binds(boxed(Type::I64), 1, boxed(Type::I64)), None);
        // A target with no parameter must be the type.
        assert_eq!(binds(boxed(Type::I64), 0, boxed(Type::I64)), Some(vec![]));
        assert_eq!(binds(boxed(Type::I64), 0, boxed(Type::F64)), None);
        assert_eq!(
            binds(Type::array(t), 1, Type::array(Type::U8)),
            Some(vec![Type::U8])
        );
        assert_eq!(binds(Type::array(t), 1, Type::optional(Type::U8)), None);
        // `T?` must not match a plain `i64`, as an argument converts into it.
        let optional = boxed(Type::optional(t));
        assert_eq!(binds(optional, 1, boxed(Type::I64)), None);
        assert_eq!(
            binds(optional, 1, boxed(Type::optional(Type::I64))),
            Some(vec![Type::I64])
        );
        // Enums match enums by their ids and arguments.
        let tree = |id, arg| Type::intern(TypeKind::Enum(EnumId(id), TypeArgs::intern(&[arg])));
        assert_eq!(
            binds(tree(0, t), 1, tree(0, Type::I64)),
            Some(vec![Type::I64])
        );
        assert_eq!(binds(tree(0, t), 1, tree(1, Type::I64)), None);
        assert_eq!(binds(tree(0, t), 1, boxed(Type::I64)), None);
    }

    /// A target without parameters is found by its exact type and its arguments; one with
    /// parameters, among the targets of its class or enum only.
    #[test]
    fn find_should_read_the_exact_target_and_then_the_generic_targets_of_the_head() {
        let (circle, t) = (class(CIRCLE, &[]), param(0));
        let all = conformances(&[
            (circle, 0, &[]),
            (boxed(t), 1, &[t]),
            (class(PILE, &[t]), 1, &[Type::I64]),
        ]);
        let find = |ty, args: &[Type]| {
            find(&all, InterfaceId(0), ty, TypeArgs::intern(args))
                .map(|(k, bound)| (k, bound.get().to_vec()))
        };
        assert_eq!(find(circle, &[]), Some((0, vec![])));
        assert_eq!(find(circle, &[Type::I64]), None);
        assert_eq!(find(class(CIRCLE + 10, &[]), &[]), None);
        // The conformance of `Box<T>` is `Convert<T>`: its arguments follow the binding.
        assert_eq!(
            find(boxed(Type::I64), &[Type::I64]),
            Some((1, vec![Type::I64]))
        );
        assert_eq!(find(boxed(Type::I64), &[Type::F64]), None);
        assert_eq!(
            find(boxed(boxed(Type::U8)), &[boxed(Type::U8)]),
            Some((1, vec![boxed(Type::U8)]))
        );
        // The targets of `Pile` are not read for a `Box`, and the reverse.
        assert_eq!(
            find(class(PILE, &[Type::F64]), &[Type::I64]),
            Some((2, vec![Type::F64]))
        );
        assert_eq!(find(class(PILE, &[Type::F64]), &[Type::F64]), None);
        // Another interface has none.
        assert!(super::find(&all, InterfaceId(1), circle, TypeArgs::NONE).is_none());
        // A type that is no class or enum has no head.
        assert_eq!(find(Type::I64, &[]), None);
    }

    /// Among conformances of many unrelated targets, a lookup reads the one exact target and the
    /// targets with parameters of its own class or enum: the candidates the index names.
    #[test]
    fn lookups_should_read_only_the_candidates_of_the_index() {
        let (circle, t) = (class(CIRCLE, &[]), param(0));
        let own = [t];
        let mut rows = Vec::new();
        for k in 0..64 {
            rows.push((class(10 + k, &[]), 0, &[][..]));
            rows.push((tree(10 + k, &[t]), 1, &[][..]));
        }
        // After the others, so that a scan of the whole list reads them all.
        rows.push((circle, 0, &[][..]));
        rows.push((boxed(t), 1, &own[..]));
        let all = conformances(&rows);
        let reads = |lookup: &dyn Fn()| {
            CANDIDATES_READ.set(0);
            lookup();
            CANDIDATES_READ.get()
        };
        let find = |ty, args: &[Type]| {
            let found = find(&all, InterfaceId(0), ty, TypeArgs::intern(args));
            assert!(found.is_some(), "{ty:?} should have a conformance");
        };
        let clash = |target, params, args: &[Type]| {
            let found = clash(&all, InterfaceId(0), target, params, TypeArgs::intern(args));
            assert!(found.is_some(), "{target:?} should clash");
        };
        assert_eq!(reads(&|| find(circle, &[])), 1);
        assert_eq!(reads(&|| find(boxed(Type::I64), &[Type::I64])), 1);
        assert_eq!(reads(&|| clash(circle, 0, &[])), 1);
        assert_eq!(reads(&|| clash(boxed(param(0)), 1, &[Type::I64])), 1);
    }

    /// A lookup inspects the candidates of its receiver's class or enum only: `n` classes, or `n`
    /// instantiations of one class, each looked up once, cost `n` candidates and not `n` for each.
    /// The recorded exception, `n` blocks `extend<T> Pair<TagK, T>: Run`, makes a lookup inspect
    /// at most `n`, and no block of another class: the `Box` blocks beside them add none.
    #[test]
    fn lookups_should_inspect_only_the_receivers_head() {
        const N: usize = 200;
        let t = param(0);
        // `Stack`, `Pair` and `Box` are classes 3, 4 and 5; the tags and the classes that conform
        // are numbered above them.
        let (stack, pair, boxed3) = (6, 7, 8);
        let tag = |k: usize| class(1000 + k, &[]);
        let reads = |lookup: &dyn Fn()| {
            CANDIDATES_READ.set(0);
            lookup();
            CANDIDATES_READ.get()
        };
        // `n` classes that conform, and `n` instantiations of `Stack`, each found once.
        let mut rows = Vec::new();
        for k in 0..N {
            rows.push((class(2000 + k, &[]), 0, &[][..]));
            rows.push((class(stack, &[tag(k)]), 0, &[][..]));
        }
        let all = conformances(&rows);
        let found = |ty| assert!(find(&all, InterfaceId(0), ty, TypeArgs::NONE).is_some());
        let total = reads(&|| {
            for k in 0..N {
                found(class(2000 + k, &[]));
                found(class(stack, &[tag(k)]));
            }
        });
        assert!(total <= 2 * N, "{total} candidates for {} lookups", 2 * N);
        // `n` blocks of one class with a parameter: each lookup inspects at most `n`.
        let mut blocks = Vec::new();
        for k in 0..N {
            blocks.push((class(pair, &[tag(k), t]), 1, &[][..]));
        }
        let pairs = conformances(&blocks);
        let mut worst = 0;
        for k in 0..N {
            let ty = class(pair, &[tag(k), Type::I64]);
            let n = reads(&|| {
                assert!(find(&pairs, InterfaceId(0), ty, TypeArgs::NONE).is_some());
            });
            worst = worst.max(n);
        }
        assert!(worst <= N, "{worst} candidates for one lookup");
        // The blocks of another class beside them are no candidates.
        for k in 0..N {
            blocks.push((class(boxed3, &[tag(k), t]), 1, &[][..]));
        }
        let beside = conformances(&blocks);
        for k in 0..N {
            let ty = class(pair, &[tag(k), Type::I64]);
            let with = reads(&|| {
                assert!(find(&beside, InterfaceId(0), ty, TypeArgs::NONE).is_some());
            });
            let without = reads(&|| {
                assert!(find(&pairs, InterfaceId(0), ty, TypeArgs::NONE).is_some());
            });
            assert_eq!(with, without);
        }
    }

    /// `n` conformances of one type without parameters to `Convert<Tag1>` ... `Convert<Tagn>` are
    /// told apart by the index: each insertion and each lookup reads at most one candidate, so
    /// `n` of each cost at most `2n` and not `n²` (the exact index holds the interface arguments).
    #[test]
    fn conformances_of_one_type_to_many_interface_arguments_should_cost_linear_candidates() {
        const N: usize = 256;
        let circle = class(CIRCLE, &[]);
        let tag = |k: usize| TypeArgs::intern(&[class(1000 + k, &[])]);
        let mut all = Conformances::default();
        CANDIDATES_READ.set(0);
        for k in 0..N {
            assert!(clash(&all, InterfaceId(0), circle, 0, tag(k)).is_none());
            all.add(
                InterfaceId(0),
                Conformance {
                    type_params: 0,
                    target: circle,
                    args: tag(k),
                    methods: Vec::<FuncId>::new(),
                    assoc: Vec::new(),
                    bounds: Vec::new(),
                },
            );
        }
        for k in 0..N {
            assert!(find(&all, InterfaceId(0), circle, tag(k)).is_some());
        }
        let total = CANDIDATES_READ.get();
        assert!(
            total <= 2 * N,
            "{total} candidates for {N} insertions and {N} lookups"
        );
    }

    /// The targets with parameters of an enum are read as those of a class, and a target that
    /// does not bind is skipped.
    #[test]
    fn find_should_skip_a_generic_target_that_does_not_bind() {
        let t = param(0);
        let all = conformances(&[(tree(0, &[t]), 1, &[]), (boxed(Type::optional(t)), 1, &[])]);
        let find = |ty| {
            find(&all, InterfaceId(0), ty, TypeArgs::NONE)
                .map(|(k, bound)| (k, bound.get().to_vec()))
        };
        assert_eq!(find(tree(0, &[Type::I64])), Some((0, vec![Type::I64])));
        assert_eq!(find(tree(1, &[Type::I64])), None);
        assert_eq!(find(boxed(Type::I64)), None);
        assert_eq!(
            find(boxed(Type::optional(Type::I64))),
            Some((1, vec![Type::I64]))
        );
    }

    /// A new conformance meets the recorded ones of its interface that overlap it: the same exact
    /// target, or a target with parameters of its class or enum.
    #[test]
    fn clash_should_find_the_conformance_that_overlaps() {
        let (circle, t, u) = (class(CIRCLE, &[]), param(0), param(0));
        let all = conformances(&[(circle, 0, &[]), (boxed(t), 1, &[t])]);
        let clash = |interface, target, params, args: &[Type]| {
            clash(
                &all,
                InterfaceId(interface),
                target,
                params,
                TypeArgs::intern(args),
            )
        };
        assert_eq!(clash(0, circle, 0, &[]), Some((0, vec![circle])));
        assert_eq!(clash(0, class(CIRCLE + 10, &[]), 0, &[]), None);
        assert_eq!(clash(1, circle, 0, &[]), None);
        assert_eq!(clash(0, circle, 0, &[Type::I64]), None);
        // A block with parameters meets one with parameters at the most general type.
        assert_eq!(
            clash(0, boxed(u), 1, &[Type::I64]),
            Some((1, vec![boxed(Type::I64), Type::I64]))
        );
        assert_eq!(clash(0, boxed(u), 1, &[Type::array(u)]), None);
        // An exact target meets one with parameters.
        assert_eq!(
            clash(0, boxed(Type::I64), 0, &[Type::I64]),
            Some((1, vec![boxed(Type::I64), Type::I64]))
        );
        // A target with parameters meets the exact targets of its class or enum.
        let exact = conformances(&[(boxed(Type::F64), 0, &[Type::F64])]);
        let meets = |target, params, args: &[Type]| {
            super::clash(
                &exact,
                InterfaceId(0),
                target,
                params,
                TypeArgs::intern(args),
            )
        };
        assert_eq!(
            meets(boxed(u), 1, &[Type::F64]),
            Some((0, vec![boxed(Type::F64), Type::F64]))
        );
        assert_eq!(meets(boxed(u), 1, &[Type::I64]), None);
        // Another class has none, and an exact target meets the exact one only if it is the same.
        assert_eq!(meets(class(PILE, &[u]), 1, &[Type::F64]), None);
        assert_eq!(meets(boxed(Type::I64), 0, &[Type::F64]), None);
    }

    /// Interface 0 has one associated type. `Circle` chooses `i64`, `Box<T>` chooses `[T]`,
    /// `Pair<T, U>` chooses the associated type of `U`, and `Pile` chooses none. A class that
    /// lacks the choice (`PILE + 1`), one whose choice names the associated type of `Pile`
    /// (`PILE + 2`) and an enum that chooses `bool` stand beside them.
    fn with_choices() -> Conformances {
        let item = |base| Type::intern(TypeKind::Assoc(base, InterfaceId(0), TypeArgs::NONE, 0));
        let mut all = Conformances::default();
        let rows = [
            (class(CIRCLE, &[]), 0, vec![Some(Type::I64)]),
            (boxed(param(0)), 1, vec![Some(Type::array(param(0)))]),
            (
                class(PAIR, &[param(0), param(1)]),
                2,
                vec![Some(item(param(1)))],
            ),
            (class(PILE, &[]), 0, vec![None]),
            (class(PILE + 1, &[]), 0, vec![]),
            (class(PILE + 2, &[]), 0, vec![Some(item(class(PILE, &[])))]),
            (tree(0, &[]), 0, vec![Some(Type::BOOL)]),
        ];
        for (target, type_params, assoc) in rows {
            all.add(
                InterfaceId(0),
                Conformance {
                    type_params,
                    target,
                    args: TypeArgs::NONE,
                    methods: Vec::<FuncId>::new(),
                    assoc,
                    bounds: Vec::new(),
                },
            );
        }
        all
    }

    /// A type whose associated type has a base that is a class or an enum is the choice of the
    /// base's conformance, with the types that stand for its parameters in it, innermost first; a
    /// base that is a parameter keeps it.
    #[test]
    fn normalize_should_replace_the_associated_type_of_a_known_type_by_its_choice() {
        let all = with_choices();
        let item = |base| Type::intern(TypeKind::Assoc(base, InterfaceId(0), TypeArgs::NONE, 0));
        let circle = class(CIRCLE, &[]);
        let normalized = |ty| normalize(&all, ty);
        assert_eq!(normalized(item(circle)), Some(Type::I64));
        assert_eq!(
            normalized(item(boxed(Type::STRING))),
            Some(Type::array(Type::STRING))
        );
        // An enum is a base too.
        assert_eq!(normalized(item(tree(0, &[]))), Some(Type::BOOL));
        // Inside an optional, an array and the arguments of a class or an enum, and twice over.
        assert_eq!(
            normalized(Type::optional(Type::array(item(circle)))),
            Some(Type::optional(Type::array(Type::I64)))
        );
        assert_eq!(
            normalized(boxed(item(boxed(Type::U8)))),
            Some(boxed(Type::array(Type::U8)))
        );
        assert_eq!(
            normalized(tree(1, &[item(circle)])),
            Some(tree(1, &[Type::I64]))
        );
        // `Pair<A, B>` chooses the associated type of `B`: a choice that names one.
        let pair = class(PAIR, &[circle, boxed(Type::F64)]);
        assert_eq!(normalized(item(pair)), Some(Type::array(Type::F64)));
        // A parameter's stays, and so does the associated type of one's own associated type.
        let kept = item(item(param(0)));
        assert_eq!(normalized(kept), Some(kept));
        assert_eq!(normalized(boxed(kept)), Some(boxed(kept)));
        // A type without an associated type is itself.
        assert_eq!(normalized(boxed(Type::I64)), Some(boxed(Type::I64)));
    }

    /// A type does not exist when a base has no conformance, the conformance chooses no type or
    /// one that does not exist, or any part of the type is such: nothing is left to use.
    #[test]
    fn normalize_should_give_nothing_for_a_type_that_does_not_exist() {
        let all = with_choices();
        let item = |base| Type::intern(TypeKind::Assoc(base, InterfaceId(0), TypeArgs::NONE, 0));
        let pile = item(class(PILE, &[]));
        // A class that chooses none, one without the slot, one whose choice names the associated
        // type of a class that chooses none, a class that conforms not, and a type that is no
        // class.
        for base in [
            class(PILE, &[]),
            class(PILE + 1, &[]),
            class(PILE + 2, &[]),
            class(PILE + 3, &[]),
            Type::I64,
        ] {
            assert_eq!(normalize(&all, item(base)), None, "{base:?}");
        }
        // Each place that holds the type fails with it, and so does a base and an argument.
        let circle = class(CIRCLE, &[]);
        let with_args = |args: &[Type]| {
            Type::intern(TypeKind::Assoc(
                circle,
                InterfaceId(0),
                TypeArgs::intern(args),
                0,
            ))
        };
        for ty in [
            Type::array(pile),
            Type::optional(pile),
            boxed(pile),
            tree(1, &[pile]),
            item(pile),
            with_args(&[pile]),
        ] {
            assert_eq!(normalize(&all, ty), None, "{ty:?}");
        }
    }

    /// `Leaf` chooses `Looper<Leaf>`, and `Looper<T>` chooses `T.A.A`: the associated type of
    /// `Looper<Leaf>` needs itself. The walk ends at once, gives no type, and says which choice
    /// closed the loop (the conformance of `Looper`, the second). A chain that ends is no loop.
    #[test]
    fn normalize_should_end_at_an_associated_type_that_needs_itself() {
        let item = |base| Type::intern(TypeKind::Assoc(base, InterfaceId(0), TypeArgs::NONE, 0));
        let (leaf, looper) = (class(PILE + 20, &[]), |t| class(PILE + 21, &[t]));
        let mut all = Conformances::default();
        let rows = [
            (leaf, 0, vec![Some(looper(leaf))]),
            (looper(param(0)), 1, vec![Some(item(item(param(0))))]),
            (class(PILE + 22, &[]), 0, vec![Some(Type::I64)]),
        ];
        for (target, type_params, assoc) in rows {
            all.add(
                InterfaceId(0),
                Conformance {
                    type_params,
                    target,
                    args: TypeArgs::NONE,
                    methods: Vec::<FuncId>::new(),
                    assoc,
                    bounds: Vec::new(),
                },
            );
        }
        let mut loops = Vec::new();
        NORMALIZED.set(0);
        assert_eq!(normalize_noting(&all, item(looper(leaf)), &mut loops), None);
        assert_eq!(
            loops,
            vec![Stuck {
                assoc: item(looper(leaf)),
                interface: InterfaceId(0),
                index: 1,
                k: 0,
                cause: Cause::Loops,
            }]
        );
        assert!(NORMALIZED.get() <= 8, "{} visits", NORMALIZED.get());
        // The associated type of the leaf alone is no loop.
        loops.clear();
        assert_eq!(
            normalize_noting(&all, item(leaf), &mut loops),
            Some(looper(leaf))
        );
        assert_eq!(loops, vec![]);
    }

    /// Interface 0 has one associated type. `Leaf` chooses `i64`, `Wrap<T>` chooses the associated
    /// type of `T` (the second conformance), and `Grow<T>` chooses the associated type of
    /// `Grow<Grow<T>>`, which is never the same type twice (the third). Returns them with the
    /// `item` of a base and `Wrap<..Wrap<Leaf>..>` with `n` wraps.
    fn with_chains() -> (Conformances, impl Fn(Type) -> Type, impl Fn(usize) -> Type) {
        let item = |base| Type::intern(TypeKind::Assoc(base, InterfaceId(0), TypeArgs::NONE, 0));
        let grow = |t| class(PILE + 31, &[t]);
        let mut all = Conformances::default();
        let rows = [
            (class(PILE + 29, &[]), 0, vec![Some(Type::I64)]),
            (class(PILE + 30, &[param(0)]), 1, vec![Some(item(param(0)))]),
            (grow(param(0)), 1, vec![Some(item(grow(grow(param(0)))))]),
        ];
        for (target, type_params, assoc) in rows {
            all.add(
                InterfaceId(0),
                Conformance {
                    type_params,
                    target,
                    args: TypeArgs::NONE,
                    methods: Vec::<FuncId>::new(),
                    assoc,
                    bounds: Vec::new(),
                },
            );
        }
        let wrapped = |n: usize| (0..n).fold(class(PILE + 29, &[]), |t, _| class(PILE + 30, &[t]));
        (all, item, wrapped)
    }

    /// The `n`th associated type in progress that names one more is the first past the limit: a
    /// chain of `CHOICE_DEPTH_LIMIT` such choices ends, and one more does not. The walk gives no
    /// type, says which choice it would not enter (the `Wrap` conformance, the second), and names
    /// the associated type that it began at, which needs the steps, and not the one it stopped at.
    #[test]
    fn normalize_should_stop_a_chain_of_more_choices_than_the_limit() {
        let (all, item, wrapped) = with_chains();
        assert_eq!(
            normalize(&all, item(wrapped(CHOICE_DEPTH_LIMIT))),
            Some(Type::I64)
        );
        let mut stuck = Vec::new();
        let past = item(wrapped(CHOICE_DEPTH_LIMIT + 1));
        assert_eq!(normalize_noting(&all, past, &mut stuck), None);
        assert_eq!(
            stuck,
            vec![Stuck {
                assoc: past,
                interface: InterfaceId(0),
                index: 1,
                k: 0,
                cause: Cause::TooDeep,
            }]
        );
    }

    /// `Grow<T>` chooses the associated type of `Grow<Grow<T>>`: no type comes twice, so no loop
    /// closes, and only the limit ends the walk, after about one visit for each step.
    #[test]
    fn normalize_should_stop_a_chain_whose_types_never_repeat() {
        let (all, item, _) = with_chains();
        let leaf = class(PILE + 29, &[]);
        let mut stuck = Vec::new();
        NORMALIZED.set(0);
        let start = item(class(PILE + 31, &[leaf]));
        assert_eq!(normalize_noting(&all, start, &mut stuck), None);
        let [one] = stuck.as_slice() else {
            panic!("one stop should be noted: {stuck:?}");
        };
        assert_eq!((one.index, one.cause), (2, Cause::TooDeep));
        assert_eq!(one.assoc, start, "the type that the walk began at");
        // Each step walks the associated type, its base and the base's argument.
        let visits = NORMALIZED.get();
        assert!(visits <= 4 * (CHOICE_DEPTH_LIMIT + 2), "{visits} visits");
    }

    /// `t_0` is the associated type of `Circle`, and `t_{i+1}` is `Pair<t_i, t_i>`, so `t_20` has
    /// 21 distinct types and 2^20 paths to its leaf. Each distinct type is normalized once.
    #[test]
    fn normalize_should_visit_each_shared_type_once() {
        let all = with_choices();
        let item = |base| Type::intern(TypeKind::Assoc(base, InterfaceId(0), TypeArgs::NONE, 0));
        let circle = class(CIRCLE, &[]);
        let doubled = |leaf, depth| (0..depth).fold(leaf, |t, _| class(PAIR + 10, &[t, t]));
        NORMALIZED.set(0);
        assert_eq!(
            normalize(&all, doubled(item(circle), 20)),
            Some(doubled(Type::I64, 20))
        );
        // The leaf is one associated type and its base, and each level is one type.
        assert!(NORMALIZED.get() <= 2 * 22, "{} visits", NORMALIZED.get());
        // A type without an associated type is not walked at all.
        NORMALIZED.set(0);
        assert_eq!(
            normalize(&all, doubled(Type::I64, 20)),
            Some(doubled(Type::I64, 20))
        );
        assert_eq!(NORMALIZED.get(), 0);
    }

    /// The program with a chain `F0` ... `F{n-1}` that reaches no written method, and `n` enums
    /// `R1` ... `Rn` that each hold `F0` and a `Holder` with a written `eq`. `main` compares each
    /// `Ri` once, and `F0` after them all.
    fn written_program(n: usize) -> String {
        use std::fmt::Write as _;
        let mut src = String::from(
            "class Holder: Eq {\n    mut items: [string]\n    fn eq(self, other: Self) -> bool {\n        self.items = [\"y\"]\n        true\n    }\n}\n",
        );
        for k in 0..n - 1 {
            writeln!(src, "enum F{k} {{\n    A(F{})\n    End\n}}", k + 1)
                .expect("writing to a `String` should succeed");
        }
        writeln!(src, "enum F{} {{\n    End\n}}", n - 1)
            .expect("writing to a `String` should succeed");
        for i in 1..=n {
            writeln!(src, "enum R{i} {{\n    P(F0)\n    Q(Holder)\n}}")
                .expect("writing to a `String` should succeed");
        }
        src += "fn main() {\n    let h = Holder(items: [\"x\"])\n    let f = F0.End\n";
        for i in 1..=n {
            writeln!(src, "    let r{i} = R{i}.Q(h)\n    print(r{i} == r{i})")
                .expect("writing to a `String` should succeed");
        }
        src + "    print(f == f)\n}\n"
    }

    /// The keys `calls_written` listed building the mir of `src`, and the printed mir.
    fn keys_listed(src: &str) -> (usize, String) {
        CALLS_KEYS.set(0);
        let (program, diagnostics) =
            crate::check::check_alone(&crate::parser::parse_ok(src), src, &[]);
        let program = program.unwrap_or_else(|| panic!("{diagnostics:?}"));
        let program = crate::mono::instantiate(program).unwrap_or_else(|found| panic!("{found:?}"));
        let mir = crate::mir::build(&program);
        (CALLS_KEYS.get(), crate::mir::print(&mir, &program))
    }

    /// Each key is listed once, whichever of the 200 roots asks: a chain that reaches no written
    /// method is read once, not once for each root that comes before it. The keys are `Holder`,
    /// the 200 `F`s and the 200 `R`s: at most 401. A walk that read the false chain again from
    /// each true root would list about 40,000. Every `Ri` reaches the `Holder`, and `F0` does not.
    #[test]
    fn calls_written_should_list_each_key_once() {
        let (keys, printed) = keys_listed(&written_program(200));
        assert_eq!(keys, 401, "keys listed");
        assert_eq!(
            printed.matches(" [calls]").count(),
            200,
            "every `Ri` calls `eq`"
        );
        let main = printed
            .find("fn main(")
            .expect("the mir should hold `main`");
        let last = printed[main..].rfind("==").expect("`main` should compare");
        assert!(
            !printed[main + last..]
                .lines()
                .next()
                .unwrap_or_default()
                .contains("[calls]"),
            "`F0` calls no written method"
        );
    }

    /// The keys listed grow in proportion to the types: 20, 40 and 60 of each, ten times the
    /// smallest chain that listing each root's chain again would show.
    #[test]
    fn calls_written_should_list_keys_in_proportion_to_the_types() {
        let keys = |n| keys_listed(&written_program(n)).0;
        crate::mir::tests::assert_near_linear(keys(20), keys(40), keys(60));
    }

    /// The program `src` checks without an error, and `ask` gets what `holds` reads of it.
    fn deciding<R>(
        src: &str,
        ask: impl FnOnce(&crate::tir::Program, &mut Decide, &Source<'_, [Enum]>) -> R,
    ) -> R {
        let (program, diagnostics) =
            crate::check::check_alone(&crate::parser::parse_ok(src), src, &[]);
        let program = program.unwrap_or_else(|| panic!("{diagnostics:?}"));
        let source = Source {
            conformances: &program.conformances,
            enums: program.enums.as_slice(),
            lang: program.lang,
            interfaces: program.interfaces.as_slice(),
        };
        ask(&program, &mut Decide::default(), &source)
    }

    /// The class named `name` of `program`, as a type with the arguments `args`.
    fn class_named(program: &crate::tir::Program, name: &str, args: &[Type]) -> Type {
        let at = program.classes.iter().position(|c| c.name == name);
        Type::intern(TypeKind::Class(
            ClassId(at.expect("the program should hold the class")),
            TypeArgs::intern(args),
        ))
    }

    /// The enum named `name` of `program`, as a type with the arguments `args`.
    fn enum_named(program: &crate::tir::Program, name: &str, args: &[Type]) -> Type {
        let at = program.enums.iter().position(|e| e.name == name);
        Type::intern(TypeKind::Enum(
            EnumId(at.expect("the program should hold the enum")),
            TypeArgs::intern(args),
        ))
    }

    /// An `any I` conforms to `I`, to each interface that `I` inherits, with the arguments of the
    /// interface, and to nothing else: the answer of the checker (`any_conforms`), which `mono`
    /// reads through `holds` for a bound of a written conformance.
    #[test]
    fn holds_should_give_an_any_type_its_interface_and_those_it_inherits() {
        let src = "interface Named {\n    fn name(self) -> string\n}\n\
                   interface Solid: Named {\n    fn volume(self) -> f64\n}\n\
                   interface Sink<T> {\n    fn put(self, x: T) -> i64\n}\n\
                   interface Obs: AnyObject {\n    fn hit(self) -> i64\n}\n\
                   interface Deep: Obs {\n    fn deep(self) -> i64\n}\n\
                   class Cube: Solid {\n    fn name(self) -> string { \"c\" }\n    fn volume(self) -> f64 { 1.0 }\n}\n\
                   class Log: Sink<string> {\n    fn put(self, x: string) -> i64 { 1 }\n}\n\
                   class B: Deep {\n    fn hit(self) -> i64 { 1 }\n    fn deep(self) -> i64 { 2 }\n}\n\
                   fn main() {\n    let a: any Solid = Cube()\n    let b: any Sink<string> = Log()\n    let c: any Deep = B()\n}";
        deciding(src, |program, cx, source| {
            let lang = program.lang;
            let id = |name: &str| {
                InterfaceId(
                    program
                        .interfaces
                        .iter()
                        .position(|i| i.name == name)
                        .unwrap(),
                )
            };
            let any = |name: &str, args: &[Type]| {
                Type::intern(TypeKind::Any(id(name), TypeArgs::intern(args)))
            };
            let mut ask = |ty, interface, args| holds(cx, source, ty, interface, args);
            let solid = any("Solid", &[]);
            assert_eq!(ask(solid, id("Solid"), TypeArgs::NONE), Holds::Generated);
            assert_eq!(ask(solid, id("Named"), TypeArgs::NONE), Holds::Generated);
            assert_eq!(
                ask(solid, id("Sink"), TypeArgs::intern(&[Type::STRING])),
                Holds::No
            );
            assert_eq!(ask(solid, lang.any_object, TypeArgs::NONE), Holds::No);
            assert_eq!(ask(solid, lang.ord, TypeArgs::NONE), Holds::No);
            // The arguments of the interface are part of the answer.
            let sink = any("Sink", &[Type::STRING]);
            let string = TypeArgs::intern(&[Type::STRING]);
            assert_eq!(ask(sink, id("Sink"), string), Holds::Generated);
            assert_eq!(
                ask(sink, id("Sink"), TypeArgs::intern(&[Type::I64])),
                Holds::No
            );
            // `AnyObject` is in the closure of an interface that inherits it, at any depth.
            let deep = any("Deep", &[]);
            assert_eq!(ask(deep, lang.any_object, TypeArgs::NONE), Holds::Generated);
            assert_eq!(ask(deep, id("Obs"), TypeArgs::NONE), Holds::Generated);
            assert_eq!(ask(deep, id("Named"), TypeArgs::NONE), Holds::No);
            // An optional or an array of an `any` value is no `any` value.
            assert_eq!(
                ask(Type::optional(solid), id("Named"), TypeArgs::NONE),
                Holds::No
            );
            assert_eq!(
                ask(Type::array(solid), id("Solid"), TypeArgs::NONE),
                Holds::No
            );
        });
    }

    /// The generated rule over concrete types, with a written conformance deciding where it
    /// covers a type: `holds` follows `Checker::conforms` over the ids of the input.
    #[test]
    fn holds_should_follow_the_rule_of_the_checker() {
        let src = "class Node {}\nclass Circle: Eq {\n    r: i64\n    fn eq(self, other: Self) -> bool { self.r == other.r }\n}\n\
                   enum Expr {\n    Num(i64)\n    Add(Expr, Expr)\n}\n\
                   enum Holds {\n    N(Node)\n}\n\
                   enum Mine {\n    C(Circle)\n}\n\
                   enum A {\n    X(B)\n    Y(Node)\n}\nenum B {\n    Z(A)\n}\n\
                   class Pair<T> {\n    v: T\n}\nextend<T: Ord> Pair<T>: Eq {\n    fn eq(self, other: Self) -> bool { true }\n}\n\
                   enum Outer<T> {\n    P(Pair<T>)\n}\n\
                   interface Tag {}\nclass Tagged: Tag {}\n\
                   class Label<T> {\n    v: T\n}\nextend<T: Tag> Label<T>: Eq {\n    fn eq(self, other: Self) -> bool { false }\n}\n\
                   fn main() {}";
        deciding(src, |program, cx, source| {
            let lang = program.lang;
            let mut ask = |ty, interface| holds(cx, source, ty, interface, TypeArgs::NONE);
            let node = class_named(program, "Node", &[]);
            let circle = class_named(program, "Circle", &[]);
            // Scalars and the parts of a type.
            assert_eq!(ask(Type::I64, lang.eq), Holds::Generated);
            assert_eq!(ask(Type::BOOL, lang.ord), Holds::No);
            assert_eq!(ask(Type::array(Type::I64), lang.eq), Holds::Generated);
            assert_eq!(ask(Type::array(node), lang.eq), Holds::No);
            assert_eq!(ask(Type::optional(Type::STRING), lang.eq), Holds::Generated);
            assert_eq!(ask(Type::array(Type::I64), lang.ord), Holds::No);
            // A class has `Eq` only through a written conformance, which the type's record names.
            assert_eq!(ask(node, lang.eq), Holds::No);
            assert!(matches!(ask(circle, lang.eq), Holds::Written(..)));
            assert_eq!(ask(Type::array(circle), lang.eq), Holds::Generated);
            // An enum holds when its payloads do, itself included.
            assert_eq!(
                ask(enum_named(program, "Expr", &[]), lang.eq),
                Holds::Generated
            );
            assert_eq!(ask(enum_named(program, "Holds", &[]), lang.eq), Holds::No);
            assert_eq!(
                ask(enum_named(program, "Mine", &[]), lang.eq),
                Holds::Generated
            );
            // A cycle that meets a class fails in both enums, whichever is asked first.
            assert_eq!(ask(enum_named(program, "B", &[]), lang.eq), Holds::No);
            assert_eq!(ask(enum_named(program, "A", &[]), lang.eq), Holds::No);
            // A written record whose bound fails decides: no fallback to the generated rule.
            let pair = class_named(program, "Pair", &[Type::BOOL]);
            assert_eq!(ask(pair, lang.eq), Holds::No);
            let outer = enum_named(program, "Outer", &[Type::BOOL]);
            assert_eq!(ask(outer, lang.eq), Holds::No);
            let good = class_named(program, "Pair", &[Type::I64]);
            assert!(matches!(ask(good, lang.eq), Holds::Written(..)));
            // A bound that names a user interface sees the record of the type that has it.
            let tagged = class_named(program, "Tagged", &[]);
            let label = class_named(program, "Label", &[tagged]);
            assert!(matches!(ask(label, lang.eq), Holds::Written(..)));
            assert_eq!(
                ask(class_named(program, "Label", &[node]), lang.eq),
                Holds::No
            );
            // `Display` holds for everything but `unit` and `never`.
            assert_eq!(ask(node, lang.display), Holds::Generated);
            assert_eq!(ask(Type::UNIT, lang.display), Holds::No);
        });
    }

    /// A written `Display` whose bound fails falls back to the generated one: only `Display`
    /// does, as `Eq` and `Ord` have a fallback nowhere.
    #[test]
    fn holds_should_fall_back_to_the_generated_display() {
        let src = "class Node {}\nclass Box<T> {\n    v: T\n}\nextend<T: Eq> Box<T>: Display {\n    fn to_string(self) -> string { \"box\" }\n}\nfn main() {}";
        deciding(src, |program, cx, source| {
            let display = program.lang.display;
            let mut ask = |ty| holds(cx, source, ty, display, TypeArgs::NONE);
            let node = class_named(program, "Node", &[]);
            assert!(matches!(
                ask(class_named(program, "Box", &[Type::I64])),
                Holds::Written(..)
            ));
            assert_eq!(ask(class_named(program, "Box", &[node])), Holds::Generated);
        });
    }

    /// The answer to a query does not depend on the queries before it. Here `Display` of `Wr<B>`
    /// and `Eq` of `H` meet in `Eq` of `A`, which fails after `B` has left it as pending: a
    /// written `Display` whose bound fails must not make that pending answer final.
    #[test]
    fn holds_should_give_one_answer_in_either_order_of_queries() {
        let src = "class Node {}\nclass Box<T> {\n    v: T\n}\nclass Wr<T> {\n    v: T\n}\n\
                   extend<T: Display> Box<T>: Eq {\n    fn eq(self, other: Self) -> bool { true }\n}\n\
                   extend<T: Eq> Wr<T>: Display {\n    fn to_string(self) -> string { \"w\" }\n}\n\
                   enum H {\n    P(Box<Wr<A>>)\n}\n\
                   enum A {\n    X(B)\n    Y(Node)\n}\n\
                   enum B {\n    Z(A)\n    M(H)\n}\nfn main() {}";
        // Each row asks the three keys in its own order, on a fresh state.
        for order in [[0, 1, 2], [1, 0, 2], [2, 1, 0], [1, 2, 0]] {
            deciding(src, |program, cx, source| {
                let lang = program.lang;
                let h = enum_named(program, "H", &[]);
                let b = enum_named(program, "B", &[]);
                let wr_b = class_named(program, "Wr", &[b]);
                let mut answers = [Holds::No; 3];
                for key in order {
                    answers[key] = match key {
                        0 => holds(cx, source, h, lang.eq, TypeArgs::NONE),
                        1 => holds(cx, source, wr_b, lang.display, TypeArgs::NONE),
                        _ => holds(cx, source, b, lang.eq, TypeArgs::NONE),
                    };
                }
                // `A` lacks `Eq` through `Node`, so `B` does; `Wr<B>` then prints as `Display`
                // generates it. `H` holds `Box<Wr<A>>`, whose `Eq` needs only a `Display`.
                assert!(matches!(answers[0], Holds::Written(..) | Holds::Generated));
                assert_eq!(answers[1], Holds::Generated, "order {order:?}");
                assert_eq!(answers[2], Holds::No, "order {order:?}");
            });
        }
    }

    /// `parts` lists the element of an array, the contents of an optional and each payload type of
    /// an enum with its arguments in place, and nothing for the other types, a class included:
    /// a class compares by identity or through a written `Eq`, and prints its name.
    #[test]
    fn parts_should_list_what_a_type_holds() {
        let src = "enum Two<T> {\n    A(T, [T])\n    B\n}\nclass C {}\n\
                   class Holds<T> {\n    first: T\n    rest: [T?]\n}\nfn main() {}";
        deciding(src, |program, _, source| {
            let parts = |ty| {
                let mut out = Vec::new();
                parts(source.conformances, source.enums, ty, |part| out.push(part));
                out
            };
            assert_eq!(parts(Type::array(Type::I64)), [Type::I64]);
            assert_eq!(parts(Type::optional(Type::STRING)), [Type::STRING]);
            let two = enum_named(program, "Two", &[Type::CHAR]);
            assert_eq!(parts(two), [Type::CHAR, Type::array(Type::CHAR)]);
            assert_eq!(parts(Type::I64), Vec::<Type>::new());
            assert_eq!(parts(class_named(program, "C", &[])), Vec::<Type>::new());
            let holds = class_named(program, "Holds", &[Type::STRING]);
            assert_eq!(parts(holds), Vec::<Type>::new());
        });
    }

    /// A bound with interface arguments names them in terms of the block's parameters, and the
    /// arguments are substituted before the bound is asked: `Two<A, B>` has `Eq` when `A` has
    /// `Tag<B>`.
    #[test]
    fn holds_should_substitute_the_arguments_of_a_bound() {
        let src = "interface Tag<X> {}\nclass Two<A, B> {\n    a: A\n    b: B\n}\n\
                   extend<A: Tag<B>, B> Two<A, B>: Eq {\n    fn eq(self, other: Self) -> bool { true }\n}\n\
                   class K {}\nextend K: Tag<[i64]> {}\nfn main() {}";
        deciding(src, |program, cx, source| {
            let eq = program.lang.eq;
            let k = class_named(program, "K", &[]);
            let mut ask = |args: &[Type]| {
                let two = class_named(program, "Two", args);
                holds(cx, source, two, eq, TypeArgs::NONE)
            };
            assert!(matches!(
                ask(&[k, Type::array(Type::I64)]),
                Holds::Written(..)
            ));
            assert_eq!(ask(&[k, Type::I64]), Holds::No);
        });
    }

    /// A type that is no class or enum, and a payload or a bound that has no associated type
    /// choice, are poison for `holds`: it skips them and never panics.
    #[test]
    fn holds_should_skip_what_has_no_choice_and_refuse_a_parameter() {
        let interface = InterfaceId(1);
        let lang = LangInterfaces {
            display: InterfaceId(0),
            eq: interface,
            ord: InterfaceId(2),
            any_object: InterfaceId(3),
        };
        // `enum E { A(C.Item) }` where nothing chooses the `Item` of the class `C`.
        let class = Type::intern(TypeKind::Class(ClassId(0), TypeArgs::NONE));
        let item = Type::intern(TypeKind::Assoc(class, InterfaceId(4), TypeArgs::NONE, 0));
        let mut conformances = Conformances::default();
        // A record of `Eq` for the class `C` whose bound names the same missing choice.
        conformances.add(
            interface,
            Conformance {
                type_params: 1,
                target: Type::intern(TypeKind::Class(
                    ClassId(1),
                    TypeArgs::intern(&[Type::intern(TypeKind::Param(Symbol::intern("T"), 0))]),
                )),
                args: TypeArgs::NONE,
                methods: Vec::new(),
                assoc: Vec::new(),
                bounds: vec![vec![crate::tir::ParamBound {
                    interface: InterfaceId(5),
                    args: TypeArgs::intern(&[item]),
                }]],
            },
        );
        let payloads: Vec<Vec<Type>> = vec![vec![item]];
        let source = Source {
            conformances: &conformances,
            enums: payloads.as_slice(),
            lang,
            interfaces: &[],
        };
        let mut cx = Decide::default();
        let enum_type = Type::intern(TypeKind::Enum(EnumId(0), TypeArgs::NONE));
        // One collector for the calls: the array has a part; the payload and the field have none.
        let mut listed = Vec::new();
        let mut collect = |part| listed.push(part);
        let enums = payloads.as_slice();
        parts(&conformances, enums, Type::array(Type::I64), &mut collect);
        parts(&conformances, enums, enum_type, &mut collect);
        parts(&conformances, enums, class, &mut collect);
        // The fields of the class `C`: one with no choice for its `Item`, and an `i64`.
        let fields: Vec<Vec<Type>> = vec![vec![item, Type::I64]];
        fields_of(&conformances, fields.as_slice(), class, &mut collect);
        // Only a class has fields.
        fields_of(&conformances, fields.as_slice(), Type::I64, &mut collect);
        assert_eq!(
            listed,
            [Type::I64, Type::I64],
            "a payload or a field with no choice has no part"
        );
        assert_eq!(
            holds(&mut cx, &source, enum_type, interface, TypeArgs::NONE),
            Holds::Generated
        );
        let boxed = Type::intern(TypeKind::Class(ClassId(1), TypeArgs::intern(&[Type::I64])));
        assert!(matches!(
            holds(&mut cx, &source, boxed, interface, TypeArgs::NONE),
            Holds::Written(..)
        ));
        let param = Type::intern(TypeKind::Param(Symbol::intern("T"), 0));
        assert_eq!(
            holds(&mut cx, &source, param, interface, TypeArgs::NONE),
            Holds::No
        );
    }

    /// The program of `src` as `mono` instantiates it, with an empty `main` added.
    fn instantiated(src: &str) -> crate::tir::Program {
        let src = format!("{src}\nfn main() {{}}");
        let (program, diagnostics) =
            crate::check::check_alone(&crate::parser::parse_ok(&src), &src, &[]);
        let program = program.unwrap_or_else(|| panic!("{diagnostics:?}"));
        crate::mono::instantiate(program).unwrap_or_else(|found| panic!("{found:?}"))
    }

    /// The type of the enum or class `name` of `program`, which takes no type argument.
    fn named(program: &crate::tir::Program, name: &str) -> Type {
        if let Some(id) = program.enums.iter().position(|e| e.name == name) {
            return tree(id, &[]);
        }
        let id = (program.classes.iter())
            .position(|c| c.name == name)
            .unwrap_or_else(|| panic!("no type {name}"));
        class(id, &[])
    }

    /// `holds_class` on `ty`, from the answers `held`.
    fn holds_a_class(
        program: &crate::tir::Program,
        ty: Type,
        held: &mut FxHashMap<Type, bool>,
    ) -> bool {
        holds_class(
            (&program.conformances, program.lang.display),
            program.enums.as_slice(),
            ty,
            held,
        )
    }

    /// `A` reaches `Node` through its own payload, and `B` through `A`, whichever is asked first.
    /// The pair `C` and `D` reaches no class, whichever is asked first. A walk that kept a
    /// `false` it met while the query on the same cycle was open would answer `false` for `B`
    /// after `A`.
    #[test]
    fn holds_a_class_should_not_depend_on_the_query_order() {
        let program = instantiated(
            "class Node {\n    v: i64\n}\nenum A {\n    B(B)\n    N(Node)\n}\nenum B {\n    A(A)\n}\nenum C {\n    D(D)\n}\nenum D {\n    C(C)\n    End\n}",
        );
        let [a, b, c, d] = ["A", "B", "C", "D"].map(|name| named(&program, name));
        for (first, second) in [(a, b), (b, a)] {
            let mut held = FxHashMap::default();
            assert!(holds_a_class(&program, first, &mut held));
            assert!(holds_a_class(&program, second, &mut held));
        }
        for (first, second) in [(c, d), (d, c)] {
            let mut held = FxHashMap::default();
            assert!(!holds_a_class(&program, first, &mut held));
            assert!(!holds_a_class(&program, second, &mut held));
        }
    }

    /// A class or an enum with a written `Display` shows its text, so `dbg` writes no field of it.
    /// A value that holds only such types holds none, and one that also holds a class with no
    /// written `Display` does.
    #[test]
    fn holds_a_class_should_not_count_a_class_or_an_enum_with_a_written_display() {
        let program = instantiated(
            "class Leaf {\n    v: i64\n}\nextend Leaf: Display {\n    fn to_string(self) -> string {\n        \"leaf\"\n    }\n}\nclass Plain {\n    v: i64\n}\nenum Pick {\n    One(Leaf)\n}\nenum Both {\n    One(Leaf)\n    Two(Plain)\n}\nenum Slot {\n    Full(Plain)\n}\nextend Slot: Display {\n    fn to_string(self) -> string {\n        \"slot\"\n    }\n}\nenum Row {\n    One(Slot)\n    Two(Plain)\n}\nenum Held {\n    One(Slot)\n}",
        );
        let [leaf, plain, pick, both, slot, row, held] =
            ["Leaf", "Plain", "Pick", "Both", "Slot", "Row", "Held"].map(|n| named(&program, n));
        for (row, ty, expected) in [
            ("a class with a written Display", leaf, false),
            ("an array of it", Type::array(leaf), false),
            ("an optional of it", Type::optional(leaf), false),
            ("an enum payload of it", pick, false),
            ("a class with none", plain, true),
            ("an enum that holds one of each", both, true),
            ("an enum with a written Display", slot, false),
            ("an array of it", Type::array(slot), false),
            ("an enum that holds only it", held, false),
            ("an enum that holds it and a class with none", row, true),
        ] {
            let mut held = FxHashMap::default();
            assert_eq!(holds_a_class(&program, ty, &mut held), expected, "{row}");
        }
    }

    /// A chain of 50 enums that hold each other in a ring is walked once. Each type is visited
    /// once for the first query, and the other 49 queries visit none.
    #[test]
    fn holds_a_class_should_visit_each_type_once() {
        use std::fmt::Write as _;
        let n = 50;
        let mut src = String::new();
        for k in 0..n {
            let end = if k == n - 1 { "\n    End" } else { "" };
            writeln!(src, "enum E{k} {{\n    Next(E{}){end}\n}}", (k + 1) % n)
                .expect("writing to a `String` should succeed");
        }
        let program = instantiated(&src);
        let mut held = FxHashMap::default();
        CALLS_KEYS.set(0);
        assert!(!holds_a_class(&program, named(&program, "E0"), &mut held));
        assert_eq!(CALLS_KEYS.get(), n);
        for k in 1..n {
            assert!(!holds_a_class(
                &program,
                named(&program, &format!("E{k}")),
                &mut held
            ));
        }
        assert_eq!(
            CALLS_KEYS.get(),
            n,
            "the other queries read the answers kept"
        );
    }

    /// `calls_written` for `Display` of `ty`, in `print` mode or in `dbg` mode.
    fn calls_display(program: &crate::tir::Program, ty: Type, dbg: bool) -> bool {
        calls_written(
            &program.conformances,
            (program.enums.as_slice(), program.classes.as_slice()),
            ty,
            program.lang.display,
            dbg,
            &mut Written::default(),
        )
    }

    /// The `to_string` of `Leaf` is written. `dbg` of an `Outer` shows its field `inner`, a
    /// `Leaf`, by that text, so it may run it where `print` of an `Outer` does not. A `Shell`
    /// holds the `Outer` in a field, which `dbg` shows by its name, so it runs nothing.
    #[test]
    fn calls_written_should_follow_the_fields_of_one_class_only_for_dbg() {
        let program = instantiated(
            "class Leaf {\n    v: i64\n}\nextend Leaf: Display {\n    fn to_string(self) -> string {\n        \"leaf\"\n    }\n}\nclass Outer {\n    inner: Leaf\n}\nclass Shell {\n    outer: Outer\n}\nenum Pick {\n    One(Outer)\n}\nenum Slot {\n    Full(Outer)\n}\nextend Slot: Display {\n    fn to_string(self) -> string {\n        \"slot\"\n    }\n}",
        );
        let outer = named(&program, "Outer");
        let shell = named(&program, "Shell");
        let pick = named(&program, "Pick");
        for (row, ty, print, dbg) in [
            ("a class that holds a written one", outer, false, true),
            ("an array of it", Type::array(outer), false, true),
            ("an optional of it", Type::optional(outer), false, true),
            ("an enum payload", pick, false, true),
            ("a class that holds that class", shell, false, false),
            ("a written one itself", named(&program, "Leaf"), true, true),
            (
                "an enum with a written one",
                named(&program, "Slot"),
                true,
                true,
            ),
        ] {
            assert_eq!(calls_display(&program, ty, false), print, "{row}, print");
            assert_eq!(calls_display(&program, ty, true), dbg, "{row}, dbg");
        }
    }
}
