//! Where conformances meet types: which conformance covers a type, and whether two conformances
//! cover a type together. A conformance's target is a class or an enum, with the type parameters
//! of its `extend` block in it; they are the `Param`s numbered from 0.

use std::cmp::Ordering;

use rustc_hash::FxHashMap;

use crate::symbol::Symbol;
use crate::tir::{Conformances, InterfaceId, Type, TypeArgs, TypeKind, head_of};

#[cfg(test)]
thread_local! {
    /// How many conformances `find` and `clash` have read on this thread.
    static CANDIDATES_READ: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many types `Unifier::resolve` has read on this thread.
    static RESOLVE_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many types `Unifier::unify` has read on this thread.
    static UNIFY_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The conformance of `interface` to `ty` with the interface arguments `args`, as its index
/// among those of `interface` and the types that its block's parameters stand for. The bounds of
/// the block are not checked. A target without parameters is found by its exact type. The scan
/// among the targets with parameters reads only those of `ty`'s class or enum.
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
    let exact = conformances.exact(interface, ty);
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
        conformances.exact(interface, target)
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
        // Path halving: every type on the way points at the root.
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

    /// `ty`, with the parameter at its root replaced while one stands for something, and how
    /// many were replaced. A chain of parameters that stand for each other is no longer than the
    /// parameters, so a longer one is a parameter that stands for a type that holds it.
    fn walk(&self, mut ty: Type) -> (Type, usize) {
        let mut steps = 0;
        while let TypeKind::Param(_, i) = ty.kind()
            && let Some(Some(next)) = self.stood.get(i as usize)
        {
            ty = *next;
            steps += 1;
            assert!(steps <= self.stood.len(), "{CHAIN}");
        }
        (ty, steps)
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
        let (left, right) = (self.walk(left).0, self.walk(right).0);
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
        let rows: [Row; 25] = [
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

    #[test]
    fn resolve_should_find_no_type_for_a_parameter_that_holds_itself() {
        let mut unifier = Unifier::new(2);
        unifier.stood = vec![Some(Type::array(param(1))), Some(Type::array(param(0)))];
        assert_eq!(unifier.resolve(param(0)), None);
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
}
