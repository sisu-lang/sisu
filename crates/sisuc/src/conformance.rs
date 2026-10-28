//! Where conformances meet types: which conformance covers a type, and whether two conformances
//! cover a type together. A conformance's target is a class or an enum, with the type parameters
//! of its `extend` block in it; they are the `Param`s numbered from 0.

use std::cmp::Ordering;

use rustc_hash::FxHashMap;

use crate::graph::components;
use crate::symbol::Symbol;
use crate::tir::{
    ChoicePlace, Class, ClassId, Conformances, Enum, EnumId, InterfaceId, LangInterfaces, Type,
    TypeArgs, TypeKind, head_of,
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
    let exact = conformances.exact(interface, ty, args);
    if let Some(&k) = exact.iter().find(|&&k| {
        #[cfg(test)]
        CANDIDATES_READ.set(CANDIDATES_READ.get() + 1);
        conformances.get(interface, k).args == args
    }) {
        return Some((k, TypeArgs::NONE));
    }
    let head = head_of(ty)?;
    conformances.generic(interface, head).iter().find_map(|&k| {
        #[cfg(test)]
        CANDIDATES_READ.set(CANDIDATES_READ.get() + 1);
        let c = conformances.get(interface, k);
        let bound = bind(c.target, c.type_params, ty)?;
        (c.args.subst(bound.get()) == args).then_some((k, bound))
    })
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
/// itself (`enum Expr { Num(i64)\n Add(Expr, Expr) }`), so it is decided assuming that it holds,
/// and then each part is checked: a coinductive check, as Rust's. The state follows Tarjan's
/// algorithm for strongly connected components. It holds the stack of the keys in progress, each
/// with its entry number and its `low`; the pending answers, each with its key and its entry
/// number; and the counter that gives the numbers, which starts again only when the stack is
/// empty, so one number never names two keys in one root query.
///
/// Every query nested in a query is a conjunction (the one fallback rule: a written conformance
/// decides, and `Display` holds without one), so the rule is exact. A key that leaves with `false`
/// holds under assumptions that are all `true`, so its answer is never wrong, and every caller
/// then fails too. A key that leaves with `true` and a `low` equal to its own number is a head:
/// it and every pending answer entered after it are final. A `true` with a lower `low` rests on a
/// key that is still open, so it waits as a pending answer, which a later part query finds by its
/// key: no key is expanded twice in one root query. When the root query returns, the pending
/// answers that are left rest, through conjunctions, on a key that failed, so each is `false`.
///
/// The fourth recorded exception to "A pass is linear in the program" (user, 2026-10-27; ruling
/// F9): a query costs one step for each class or enum type that it forms (see
/// `CODING_STANDARDS.md`, Performance), and the types that a query forms from a concrete type
/// are the types that `mono` instantiates when the program uses them.
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

/// The field types of each class, declared: what [`parts`] reads of a class.
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

/// The parts of `ty`, whatever the interface: an array's element, an optional's contents, each
/// payload type of an enum and each field type of a class, with the type's arguments in place of
/// its parameters. A class lists its fields since the structural `==` of a class with no `Eq`
/// compares them (until ADR 0022 removes it), and a field may have a written `Eq`. Nothing for a
/// scalar, `string` or a type parameter. A part whose associated type has no choice is poison,
/// and is left out. A generated `Display`, which asks no part, still lists them.
pub(crate) fn parts<P: Payloads + ?Sized, F: Fields + ?Sized>(
    conformances: &Conformances,
    (enums, classes): (&P, &F),
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
        TypeKind::Class(id, args) => classes.each(id, &mut |field| {
            if let Some(part) = normalize(conformances, field.subst(args.get())) {
                each(part);
            }
        }),
        _ => {}
    }
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
}

/// The state of `holds` for a program: the answer for each class or enum key, and the `Open`
/// state of the root query that is running. `mono` keeps one, so each key is decided once.
#[derive(Debug, Default)]
pub(crate) struct Decide {
    memo: FxHashMap<Key, Holds>,
    open: Open,
    /// The answer of each key that held and left, until it is final.
    held: FxHashMap<Key, Holds>,
}

/// How `interface` holds for the concrete type `ty` with the arguments `args`: the rule of the
/// checker (`Checker::conforms`) over the input's ids. A written `Eq` or `Ord` that covers `ty`
/// decides, with its bounds, and when they fail the type has none; only `Display` falls back to
/// the generated rule. A key of a class or an enum is decided once (`cx.memo`), and a key whose
/// query is open or pending holds (`Open`).
pub(crate) fn holds<P: Payloads + ?Sized>(
    cx: &mut Decide,
    src: &Source<'_, P>,
    ty: Type,
    interface: InterfaceId,
    args: TypeArgs,
) -> Holds {
    match ty.kind() {
        TypeKind::Class(..) | TypeKind::Enum(..) => holder_holds(cx, src, ty, interface, args),
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
    }
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
        if record_holds(cx, src, interface, k, stands) {
            return Holds::Written(k, stands);
        }
        if interface != src.lang.display {
            return Holds::No;
        }
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
            if !holds(cx, src, stands.get()[i], bound.interface, args).is_yes() {
                return false;
            }
        }
    }
    true
}

/// The types whose `Eq` or `Display` reaches a written method, settled for a program: the keys
/// `calls_written` has decided.
#[derive(Debug, Default)]
pub(crate) struct Written {
    settled: FxHashMap<(Type, InterfaceId), bool>,
}

/// Whether the helper for `interface` on the concrete type `ty` may call a written method:
/// `ty`, or a part of it through [`parts`], is a class or an enum with a written conformance. The
/// keys are the pairs of a type and `interface`; the key of a type with a conformance in
/// `conformances` is a leaf, and the key of any other type reaches the keys of its parts. A key
/// is true exactly when it reaches a leaf. The keys that are not settled yet are listed once, with
/// their parts, and settled by their strongly connected components in completion order, so each
/// key of a program is decided once.
pub(crate) fn calls_written<P: Payloads + ?Sized, F: Fields + ?Sized>(
    conformances: &Conformances,
    types: (&P, &F),
    ty: Type,
    interface: InterfaceId,
    written: &mut Written,
) -> bool {
    if let Some(&answer) = written.settled.get(&(ty, interface)) {
        return answer;
    }
    // The new keys, with the edges to the new keys they reach and whether they are a leaf or
    // reach a settled `true`.
    let mut keys: Vec<Type> = vec![ty];
    let mut at: FxHashMap<Type, usize> = FxHashMap::default();
    at.insert(ty, 0);
    let mut edges: Vec<Vec<usize>> = Vec::new();
    let mut yes: Vec<bool> = Vec::new();
    let mut next = 0;
    while next < keys.len() {
        let key = keys[next];
        next += 1;
        #[cfg(test)]
        CALLS_KEYS.set(CALLS_KEYS.get() + 1);
        let leaf =
            head_of(key).is_some() && find(conformances, interface, key, TypeArgs::NONE).is_some();
        let mut reach = Vec::new();
        let mut settled_true = leaf;
        // A written `eq` is the whole comparison: the helper compares no part of the type.
        if !leaf {
            parts(conformances, types, key, |part| {
                if let Some(&answer) = written.settled.get(&(part, interface)) {
                    settled_true |= answer;
                    return;
                }
                let index = *at.entry(part).or_insert_with(|| {
                    keys.push(part);
                    keys.len() - 1
                });
                reach.push(index);
            });
        }
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
                edges[m].iter().any(|&n| {
                    component[n] != component[m] && written.settled[&(keys[n], interface)]
                })
            });
        for &m in members {
            written.settled.insert((keys[m], interface), answer);
        }
    }
    written.settled[&(ty, interface)]
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
        _ => (pattern == ty).then_some(()),
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
            _ => ty,
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
            _ => false,
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

    /// `parts` lists the element of an array, the contents of an optional, each payload type of an
    /// enum and each field type of a class with its arguments in place, and nothing for the other
    /// types.
    #[test]
    fn parts_should_list_what_a_type_holds() {
        let src = "enum Two<T> {\n    A(T, [T])\n    B\n}\nclass C {}\n\
                   class Holds<T> {\n    first: T\n    rest: [T?]\n}\nfn main() {}";
        deciding(src, |program, _, source| {
            let parts = |ty| {
                let mut out = Vec::new();
                let types = (source.enums, program.classes.as_slice());
                parts(source.conformances, types, ty, |part| out.push(part));
                out
            };
            assert_eq!(parts(Type::array(Type::I64)), [Type::I64]);
            assert_eq!(parts(Type::optional(Type::STRING)), [Type::STRING]);
            let two = enum_named(program, "Two", &[Type::CHAR]);
            assert_eq!(parts(two), [Type::CHAR, Type::array(Type::CHAR)]);
            assert_eq!(parts(Type::I64), Vec::<Type>::new());
            assert_eq!(parts(class_named(program, "C", &[])), Vec::<Type>::new());
            let holds = class_named(program, "Holds", &[Type::STRING]);
            assert_eq!(
                parts(holds),
                [Type::STRING, Type::array(Type::optional(Type::STRING))]
            );
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
        };
        let mut cx = Decide::default();
        let enum_type = Type::intern(TypeKind::Enum(EnumId(0), TypeArgs::NONE));
        // One collector for both calls: the array has a part, the enum's payload has none.
        let mut listed = Vec::new();
        let mut collect = |part| listed.push(part);
        let no_classes: &[Vec<Type>] = &[];
        let types = (payloads.as_slice(), no_classes);
        parts(&conformances, types, Type::array(Type::I64), &mut collect);
        parts(&conformances, types, enum_type, &mut collect);
        assert_eq!(listed, [Type::I64], "a payload with no choice has no part");
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
}
