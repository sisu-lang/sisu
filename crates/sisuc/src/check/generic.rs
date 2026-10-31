//! Generic declarations: type parameters, the type arguments written for them or inferred at
//! a call, and what a value of a type parameter may not do.

use std::sync::LazyLock;
use std::{fmt, iter};

use rustc_hash::{FxHashMap, FxHashSet};

use super::enums::VariantInfo;
use super::expr::{bare_hole, bare_none, dbg_argument, int_literal, option_written};
use super::interface::{Bound, Required};
use super::pattern::{LetPattern, LetValue, arm_key, matchable};
use super::prelude::PRELUDE;
use super::stmt::BlockSoFar;
use super::{
    Binding, Checked, Checker, Declared, Definition, DiagnosticKind, Poisoned, UNKNOWN,
    as_optional, built_in_type, optional_of, payload, type_of,
};
use crate::ast::{self, StmtKind};
use crate::diagnostic::{ArgumentsForm, Diagnostic, MainForm, Unwrap};
use crate::lexer::{TokenKind, lex};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{Type, TypeArgs, TypeKind};
use crate::types::{HOLE_INDEX, REJECTED_INDEX};

/// Whether `ty` is `want`, where a hole of `want` meets any part of `ty`.
fn meets(ty: Type, want: Type) -> bool {
    if ty == want || is_hole(want) {
        return true;
    }
    if !want.holds_hole() {
        return false;
    }
    match (ty.kind(), want.kind()) {
        (TypeKind::Array(a), TypeKind::Array(b))
        | (TypeKind::Optional(a), TypeKind::Optional(b)) => meets(a, b),
        (TypeKind::Class(a, p), TypeKind::Class(b, v)) if a == b => {
            iter::zip(p.get(), v.get()).all(|(&p, &v)| meets(p, v))
        }
        (TypeKind::Enum(a, p), TypeKind::Enum(b, v)) if a == b => {
            iter::zip(p.get(), v.get()).all(|(&p, &v)| meets(p, v))
        }
        _ => false,
    }
}

/// Marks in `named` each type parameter that `ty` names, by its place; `named` has a place for
/// each type parameter in scope. Each type of `ty` is visited once.
fn mark_params(ty: Type, named: &mut [bool]) {
    #[cfg(test)]
    MARKED.set(MARKED.get() + 1);
    // A type without a parameter names none: its subtree is not walked again at each call.
    if !ty.has_param() {
        return;
    }
    match ty.kind() {
        TypeKind::Param(_, j) => named[j as usize] = true,
        TypeKind::Array(inner) | TypeKind::Optional(inner) => mark_params(inner, named),
        TypeKind::Class(_, args) | TypeKind::Enum(_, args) => {
            for &arg in args.get() {
                mark_params(arg, named);
            }
        }
        TypeKind::Assoc(base, _, args, _) => {
            mark_params(base, named);
            for &arg in args.get() {
                mark_params(arg, named);
            }
        }
        _ => unreachable!("only a kind with a child can name a parameter"),
    }
}

/// Whether `ty` names the type parameter at place `param` other than under an associated type:
/// only such a name lets the type that a result is annotated with bind the parameter.
fn names_outside_assoc(ty: Type, param: usize) -> bool {
    if !ty.has_param() {
        return false;
    }
    match ty.kind() {
        TypeKind::Param(_, j) => j as usize == param,
        TypeKind::Array(inner) | TypeKind::Optional(inner) => names_outside_assoc(inner, param),
        TypeKind::Class(_, args) | TypeKind::Enum(_, args) => {
            (args.get().iter()).any(|&arg| names_outside_assoc(arg, param))
        }
        TypeKind::Assoc(..) => false,
        _ => unreachable!("only a kind with a child can name a parameter"),
    }
}

/// Marks in `outside` each type parameter that `ty` names other than under an associated type,
/// and in `inside` each that it names under one, as the base or an interface argument of it.
/// Each is by its place. One walk finds both: `names_outside_assoc` for each place would cost
/// the number of places times the size of `ty`.
fn mark_by_assoc(ty: Type, under: bool, inside: &mut [bool], outside: &mut [bool]) {
    if !ty.has_param() {
        return;
    }
    match ty.kind() {
        TypeKind::Param(_, j) => {
            let marks = if under { inside } else { outside };
            marks[j as usize] = true;
        }
        TypeKind::Array(t) | TypeKind::Optional(t) => mark_by_assoc(t, under, inside, outside),
        TypeKind::Class(_, args) | TypeKind::Enum(_, args) => {
            for &arg in args.get() {
                mark_by_assoc(arg, under, inside, outside);
            }
        }
        TypeKind::Assoc(base, _, args, _) => {
            mark_by_assoc(base, true, inside, outside);
            for &arg in args.get() {
                mark_by_assoc(arg, true, inside, outside);
            }
        }
        _ => unreachable!("only a kind with a child can name a parameter"),
    }
}

/// Whether an argument for `param`, a parameter type of the call, waits for the others:
/// `param` names a parameter that `bindings` has not bound, and names it only under an
/// associated type. `bind` binds nothing at an associated type, so that argument cannot bind the
/// parameter, and the type it must meet is not known until the others do. A parameter that
/// `param` names elsewhere too is bound by the argument itself, which then does not wait.
fn waits_for_assoc(bindings: &Bindings, param: Type) -> bool {
    if !param.has_assoc() {
        return false;
    }
    let count = bindings.found.len();
    let (mut inside, mut outside) = (vec![false; count], vec![false; count]);
    mark_by_assoc(param, false, &mut inside, &mut outside);
    (0..count).any(|j| inside[j] && !outside[j] && bindings.found[j].is_none())
}

/// Which of the first `len` type parameters `types` name. A type that did not resolve names
/// nothing.
pub(super) fn named_params(types: impl Iterator<Item = Option<Type>>, len: usize) -> Vec<bool> {
    let mut named = vec![false; len];
    for ty in types.flatten() {
        mark_params(ty, &mut named);
    }
    named
}

/// Whether the type parameter `name` is broken, as `type_params` reports it: it takes the name
/// of one in `seen`, which then holds it, or of a built-in type. A call poisons such a parameter
/// rather than leave it uninferred, and no call waits for it.
fn broken_param(seen: &mut FxHashSet<Symbol>, name: Symbol) -> bool {
    !seen.insert(name) || built_in_type(name.as_str()).is_some()
}

/// Binds each of `names`, a declaration's type parameters, that is broken (see `broken_param`)
/// and that no argument bound to `UNKNOWN`, and whether any was. Such a parameter may be one no
/// type names: it poisons the call rather than go uninferred. One pass over the names finds
/// them, and only a call that leaves one unbound makes it.
fn poison_broken_params(bindings: &mut Bindings, names: &[Symbol]) -> bool {
    let mut broken = false;
    if bindings.found.contains(&None) {
        let mut seen = FxHashSet::default();
        for (i, &name) in names.iter().enumerate() {
            #[cfg(test)]
            SCANNED.set(SCANNED.get() + 1);
            let broken_name = broken_param(&mut seen, name);
            if bindings.found[i].is_none() && broken_name {
                bindings.found[i] = Some(UNKNOWN);
                broken = true;
            }
        }
    }
    broken
}

/// Which of `names`, a declaration's type parameters, are broken (see `broken_param`).
fn broken_params(names: &[Symbol]) -> Vec<bool> {
    let mut seen = FxHashSet::default();
    names
        .iter()
        .map(|&name| broken_param(&mut seen, name))
        .collect()
}

/// Whether one of the type parameters in `names` from `owner` on has no type in `params` that
/// names it, so that no argument can bind it. For a construction, whose built type names all
/// of them: the call needs its context, whatever its arguments are. A broken parameter is
/// not open.
fn leaves_open(params: impl Iterator<Item = Option<Type>>, names: &[Symbol], owner: usize) -> bool {
    let bound = named_params(params, names.len());
    let broken = broken_params(names);
    (owner..names.len()).any(|i| !bound[i] && !broken[i])
}

/// Whether a call of a function or method that returns `ret` is open: the context binds only what
/// the type of the call names, so the call is open if `ret` names a parameter that no parameter
/// type does. It is not open, whatever else it leaves, if one parameter is named by no type at
/// all: nothing can bind that one, and the call reports it. `ret` is `None` when it did not
/// resolve; the call is not open then.
fn is_open(
    params: impl Iterator<Item = Option<Type>>,
    ret: Option<Type>,
    names: &[Symbol],
    owner: usize,
) -> bool {
    let Some(ret) = ret else {
        return false;
    };
    let built = named_params(iter::once(Some(ret)), names.len());
    let bound = named_params(params, names.len());
    let broken = broken_params(names);
    let mut open = false;
    for i in (owner..names.len()).filter(|&i| !bound[i] && !broken[i]) {
        if !built[i] {
            return false;
        }
        open = true;
    }
    open
}

/// A call of a generic callee as ranking sees it (see `Checker::rank_value`).
#[derive(Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is a separate fact that ranking or the context asks of a call"
)]
struct CallShape {
    /// Whether the callee has type parameters of its own to infer.
    generic: bool,
    /// Whether the arguments fit the callee's declaration, whose return type resolved.
    shaped: bool,
    /// Whether no argument can bind one of its type parameters, and the type of the call names
    /// it. Then only the call's context can bind it.
    open: bool,
    /// Whether the call is `Some(...)` or `Option.Some(...)`, which `Checker::some` checks.
    some: bool,
    /// How the call makes an optional by itself (see `Ranked::optional`).
    makes: Makes,
    /// The places whose arguments give the call its type (see `Places`).
    places: Places,
}

/// The places of a call's parameters that give the call its type, as bits: bit `i` is set for the
/// place `i`. A place from 64 on does not count (see `Makes::Through`).
#[derive(Clone, Copy, Default)]
struct Places {
    /// The places whose declared type names, other than under an associated type, a type
    /// parameter of the callee's own that the declared return type names too (see
    /// `carried_places`). Only the arguments there give the call its type, so ranking walks no
    /// other argument. A generic call that is not open and has no such place is `Concrete`.
    carried: u64,
    /// The places of `carried` whose declared type names such a parameter by a path with no
    /// optional on it: `T`, `[T]` and `Bx<T>` count, `T?`, `[T?]` and `Bx<T?>` do not. An argument
    /// that holds an optional at such a place makes the call hold one (see
    /// `Ranked::holds_optional`). At a place `T?` an optional fills the place's own `?`.
    plain: u64,
}

impl Places {
    /// The first place alone, which both of its masks have.
    const FIRST: Places = Places {
        carried: 1,
        plain: 1,
    };
}

/// How a call of a generic callee makes an optional by itself, from its declaration.
#[derive(Default)]
enum Makes {
    /// Its declared return type is not an optional, nor a bare type parameter.
    #[default]
    No,
    /// Its declared return type is an optional of a type that is not a type parameter, such as
    /// `[T]?`.
    // shortcut: such a call is not `Ranked::nested`, though `T??` is a nested optional; upgrade if
    // such a call needs a context to meet its return type whole.
    Always,
    /// Its declared return type is a type parameter `P`, or an optional `P?` of one (`wraps`):
    /// `Some`, or `fn opt<T>(x: T) -> T?`. Bit `i` of a mask is set for the place `i`.
    // shortcut: a place from 64 on does not count, so a callee of 64 parameters or more, with an
    // optional argument from place 64 on, reports "expected `u8`, found `u8?`"; upgrade if a callee
    // with more parameters needs it.
    // shortcut: an argument at a place of `inside` counts when it holds an optional at any depth,
    // so `first([[Some(1)]])` for `fn first<T>(xs: [T]) -> T` counts, and `T` is `[i64?]`; upgrade
    // if such a call needs a context that is not an optional.
    // shortcut: a nested optional counts at a place `P?` and a bare place only, not under an array
    // or a type argument. So `g([Some(Some(1))])` for `fn g<T>(xs: [T?]) -> T` is not an optional,
    // and a context `i64?` meets the payload; upgrade if such a call needs the context to meet
    // the return type whole.
    Through {
        /// The places of the declared type `P`. The call is an optional when one of the
        /// arguments there is.
        bare: u64,
        /// The places whose declared type holds `P` under arrays or the type arguments of a class
        /// or an enum, such as `[P]` or `Bx<P>`. The call is an optional when one of the
        /// arguments there holds an optional.
        inside: u64,
        /// The places of the declared type `P?`. An optional there fills the `?`, so the call is
        /// an optional only when the argument is `Ranked::nested`.
        lifted: u64,
        /// Whether the declared return type is `P?`, not `P`. The call is an optional then,
        /// whatever its arguments are, and `Ranked::nested` when one of them makes `P` an
        /// optional.
        wraps: bool,
    },
}

/// Whether bit `i` of `mask` is set. A place from 64 on is not.
fn has_place(mask: u64, i: usize) -> bool {
    i < u64::BITS as usize && (mask >> i) & 1 == 1
}

impl Makes {
    /// How a callee with the return type `ret` and the parameter types `params` makes an
    /// optional.
    fn of(ret: Option<Type>, params: impl Iterator<Item = Option<Type>>) -> Makes {
        let Some(ret) = ret else {
            return Makes::No;
        };
        let (payload, wraps) = match ret.kind() {
            TypeKind::Param(..) => (ret, false),
            TypeKind::Optional(inner) if matches!(inner.kind(), TypeKind::Param(..)) => {
                (inner, true)
            }
            TypeKind::Optional(_) => return Makes::Always,
            _ => return Makes::No,
        };
        let (mut bare, mut inside, mut lifted) = (0, 0, 0);
        for (i, param) in params.take(u64::BITS as usize).enumerate() {
            match param {
                Some(param) if param == payload => bare |= 1 << i,
                Some(param) if matches!(param.kind(), TypeKind::Optional(t) if t == payload) => {
                    lifted |= 1 << i;
                }
                Some(param) if holds_param(param, payload) => inside |= 1 << i,
                _ => {}
            }
        }
        Makes::Through {
            bare,
            inside,
            lifted,
            wraps,
        }
    }

    /// `Some(x)`: its return type is `T?`, and `x` is at the place of `T`.
    const fn some() -> Makes {
        Makes::Through {
            bare: 1,
            inside: 0,
            lifted: 0,
            wraps: true,
        }
    }

    /// Whether the call makes an optional whatever its arguments are: its declared return type is
    /// `P?`.
    fn wraps(&self) -> bool {
        matches!(self, Makes::Always | Makes::Through { wraps: true, .. })
    }

    /// Whether the call returns its argument at place `i` as it is: the place is of the bare return
    /// parameter, and the return type is that parameter, not an optional of it. A literal there
    /// passes its type on (see `Ranked::scalar`).
    fn returns(&self, i: usize) -> bool {
        matches!(self, Makes::Through { wraps: false, .. }) && self.through(i)
    }

    /// Whether the argument at place `i` is of the bare return parameter.
    fn through(&self, i: usize) -> bool {
        matches!(self, Makes::Through { bare, .. } if has_place(*bare, i))
    }

    /// Whether the argument at place `i` is of the return parameter in arrays or in the type
    /// arguments of a class or an enum.
    fn inside(&self, i: usize) -> bool {
        matches!(self, Makes::Through { inside, .. } if has_place(*inside, i))
    }

    /// Whether the argument at place `i` is of the return parameter, as an optional's payload.
    fn lifted(&self, i: usize) -> bool {
        matches!(self, Makes::Through { lifted, .. } if has_place(*lifted, i))
    }

    /// Whether the argument `ranked` at place `i` makes the parameter an optional by its own
    /// type: it is one at a place of the bare parameter, or it holds one at a place that holds
    /// the parameter.
    fn passes_optional(&self, i: usize, ranked: Ranked) -> bool {
        self.through(i) && ranked.optional || self.inside(i) && ranked.holds_optional
    }

    /// Whether the argument `ranked` at place `i` is `nested` at a place of the parameter's
    /// optional (`T?`). It makes the parameter an optional, as `passes_optional` does. Both prove
    /// only that for a return type `T?`; for `T`, see the shortcut on `Checker::rank_arguments`.
    /// When the context has more optionals than the call makes, the payload of the context is the
    /// optional already (see `Checker::infer_call`).
    fn passes_lifted(&self, i: usize, ranked: Ranked) -> bool {
        self.lifted(i) && ranked.nested
    }

    /// Whether the argument `ranked` at place `i` makes the parameter an optional, in any way.
    fn makes_optional(&self, i: usize, ranked: Ranked) -> bool {
        self.passes_optional(i, ranked) || self.passes_lifted(i, ranked)
    }

    /// Whether the argument `ranked` at place `i` makes the call `Ranked::nested`. For a call that
    /// returns `P?`, it is one that makes `P` an optional. For a call that returns `P`, it is one
    /// that is nested at a place of `P`: the call returns it.
    fn passes_nested(&self, i: usize, ranked: Ranked) -> bool {
        match self {
            Makes::Through { wraps: true, .. } => self.makes_optional(i, ranked),
            Makes::Through { wraps: false, .. } => self.through(i) && ranked.nested,
            _ => false,
        }
    }
}

/// Whether `ty` has more than `layers` optionals on it, one inside the other: `i64??` has more than
/// 1, and not more than 2.
fn wider_than(mut ty: Type, layers: usize) -> bool {
    for _ in 0..=layers {
        let TypeKind::Optional(inner) = ty.kind() else {
            return false;
        };
        ty = inner;
    }
    true
}

/// Whether `ty` holds `param` other than as itself: under arrays or the type arguments of a class
/// or an enum, not under an optional nor an associated type.
fn holds_param(ty: Type, param: Type) -> bool {
    match ty.kind() {
        TypeKind::Array(t) => t == param || holds_param(t, param),
        TypeKind::Class(_, args) | TypeKind::Enum(_, args) => {
            (args.get().iter()).any(|&arg| arg == param || holds_param(arg, param))
        }
        _ => false,
    }
}

/// Whether `ty` names a type parameter that `marked` marks by its place, other than under an
/// associated type (see `names_outside_assoc`), and under an optional only if `optionals`.
fn names_marked(ty: Type, marked: &[bool], optionals: bool) -> bool {
    if !ty.has_param() {
        return false;
    }
    match ty.kind() {
        TypeKind::Param(_, j) => marked[j as usize],
        TypeKind::Array(inner) => names_marked(inner, marked, optionals),
        TypeKind::Optional(inner) => optionals && names_marked(inner, marked, optionals),
        TypeKind::Class(_, args) | TypeKind::Enum(_, args) => {
            (args.get().iter()).any(|&arg| names_marked(arg, marked, optionals))
        }
        TypeKind::Assoc(..) => false,
        _ => unreachable!("only a kind with a child can name a parameter"),
    }
}

/// The places among `params` whose type names a type parameter that `marked` marks, as
/// `Places` says. A place from 64 on does not count (see `Makes::Through`).
fn carried_places(params: impl Iterator<Item = Option<Type>>, marked: &[bool]) -> Places {
    let mut places = Places::default();
    for (i, param) in params.take(u64::BITS as usize).enumerate() {
        let Some(param) = param else { continue };
        if names_marked(param, marked, true) {
            places.carried |= 1 << i;
            if names_marked(param, marked, false) {
                places.plain |= 1 << i;
            }
        }
    }
    places
}

/// The places among `params` whose type names a type parameter, of `count`, that the declared
/// return type `ret` names too, other than under an associated type. Only the parameters from
/// `owner` on count, which are the callee's own: the receiver binds the others, so the arguments
/// do not give the call their types. `ret` is `None` when it did not resolve: no place is
/// carried then.
fn carried(
    ret: Option<Type>,
    params: impl Iterator<Item = Option<Type>>,
    count: usize,
    owner: usize,
) -> Places {
    let Some(ret) = ret.filter(|_| count > 0) else {
        return Places::default();
    };
    let (mut inside, mut outside) = (vec![false; count], vec![false; count]);
    mark_by_assoc(ret, false, &mut inside, &mut outside);
    outside[..owner].fill(false);
    carried_places(params, &outside)
}

/// The places among `params` that name a type parameter, of `count`, for a construction, whose
/// built type names all of them.
fn carried_by_construction(params: impl Iterator<Item = Option<Type>>, count: usize) -> Places {
    carried_places(params, &vec![true; count])
}

/// The hole, `_`: the type of a part that a context has not fixed yet (see `Bindings::partial`).
/// No declaration has a parameter of this name, and its index (`HOLE_INDEX`) is out of range for
/// `found`, so a hole that leaks into `known` or `subst` panics at once and does not give a
/// wrong type. `found` never holds it: `bind` keeps it out.
static HOLE: LazyLock<Type> =
    LazyLock::new(|| Type::intern(TypeKind::Param(Symbol::intern("_"), HOLE_INDEX)));

/// The rejected hole, `_`: the type of a part of a context that a failed bound has rejected. It
/// is a hole too, but it says why the part is not fixed: the outer call reported the bound, so a
/// call that meets it in its own context reports nothing for that part (see `Bindings::bind`).
/// Its index (`REJECTED_INDEX`) is out of range for `found`, as the index of `HOLE` is. `found`
/// holds it for one reason only: a parameter that is rejected (see `Bindings::reject`). A call
/// that has a rejected parameter fails, and does not use its list of types (see
/// `Bindings::finish`).
static REJECTED: LazyLock<Type> =
    LazyLock::new(|| Type::intern(TypeKind::Param(Symbol::intern("_"), REJECTED_INDEX)));

/// Whether `ty` is a whole hole: `HOLE` or `REJECTED`, not a type that holds one.
fn is_hole(ty: Type) -> bool {
    ty == *HOLE || ty == *REJECTED
}

/// What `Bindings::resolve` puts for a parameter that is unbound.
#[derive(Clone, Copy)]
enum Unbound {
    /// Nothing: the walk gives `None`.
    Fail,
    /// The hole.
    Hole,
    /// The parameter itself, by the callee's name.
    Keep,
}

/// A callee's type parameters as a call binds them: `found[i]` is parameter `i`'s type once
/// the receiver, an argument or the expected type fixes it. A parameter whose bound failed is
/// rejected: `found[i]` is `REJECTED`, which no argument replaces. The rest of the call meets
/// such a parameter as one that is unbound and that nothing binds.
pub(super) struct Bindings {
    found: Vec<Option<Type>>,
    /// The parameters the last `unify` or `constrain` bound: the only ones whose types need
    /// checking.
    fresh: Vec<usize>,
    /// How many parameters `bind` has bound and kept: a change shows a new binding.
    kept: usize,
    /// For each parameter, the part of a context that holds a hole where `hint` found it (see
    /// `partial`). A bound parameter ignores it. It stays empty until the first hint, as most
    /// calls have no context that holds a hole.
    hints: Vec<Option<Type>>,
}

impl Bindings {
    /// `count` parameters, the first of them bound to `owner`, a receiver's arguments.
    fn new(count: usize, owner: &[Type]) -> Self {
        let unbound = iter::repeat_n(None, count - owner.len());
        Bindings {
            found: owner.iter().copied().map(Some).chain(unbound).collect(),
            fresh: Vec::new(),
            kept: 0,
            hints: Vec::new(),
        }
    }

    /// Binds the parameters `pattern` names, the callee's, so that it is `value`, every
    /// parameter of which is the caller's and a fixed type. A `T?` binds `T` to a value that is
    /// not optional, which the call then wraps. `false`, binding nothing, when they cannot.
    pub(super) fn unify(&mut self, pattern: Type, value: Type) -> bool {
        self.fresh.clear();
        let unified = self.bind(pattern, value);
        if !unified {
            for &i in &self.fresh {
                #[cfg(test)]
                VISITS.set(VISITS.get() + 1);
                self.found[i] = None;
            }
            self.kept -= self.fresh.len();
            self.fresh.clear();
        }
        unified
    }

    /// As `unify`, but what of `pattern` meets `value` stays bound when the rest does not.
    /// Whether all of `pattern` meets it.
    pub(super) fn constrain(&mut self, pattern: Type, value: Type) -> bool {
        self.fresh.clear();
        self.bind(pattern, value)
    }

    /// Binds each unbound parameter `pattern` names where it meets `value`, recording it in
    /// `fresh`; whether all of `pattern` meets it. `HOLE` meets any part of `pattern`, and binds
    /// nothing: the context has not fixed it. A parameter of `pattern` also meets a part that
    /// holds a hole, and binds nothing. The rest of `pattern` must meet the structure of `value`,
    /// as it does when `value` has no hole.
    ///
    /// `REJECTED` meets any part of `pattern` too, and rejects each parameter that part names: a
    /// parameter that meets it is bound to `REJECTED`, as an argument would bind a type, so that
    /// `unify` undoes it when the rest fails. A parameter that is rejected meets every type and
    /// binds nothing.
    fn bind(&mut self, pattern: Type, value: Type) -> bool {
        if !pattern.has_param() && !value.holds_hole() {
            return pattern == value;
        }
        match (pattern.kind(), value.kind()) {
            _ if value == *HOLE => true,
            (TypeKind::Param(_, i), _) => {
                if value.holds_hole() && value != *REJECTED {
                    return true;
                }
                let i = i as usize;
                if let Some(bound) = self.found[i] {
                    return bound == value || bound == *REJECTED || value == *REJECTED;
                }
                self.found[i] = Some(value);
                self.fresh.push(i);
                self.kept += 1;
                true
            }
            // The pattern is not a parameter: each parameter in it is rejected, as it is where
            // the pattern is a parameter.
            _ if value == *REJECTED => {
                match pattern.kind() {
                    TypeKind::Array(p) | TypeKind::Optional(p) => {
                        self.bind(p, value);
                    }
                    TypeKind::Class(_, args) | TypeKind::Enum(_, args) => {
                        for &p in args.get() {
                            self.bind(p, value);
                        }
                    }
                    _ => {}
                }
                true
            }
            (TypeKind::Array(p), TypeKind::Array(v))
            | (TypeKind::Optional(p), TypeKind::Optional(v)) => self.bind(p, v),
            // A value converts through one `?` only.
            (TypeKind::Optional(p), _) if !matches!(p.kind(), TypeKind::Optional(_)) => {
                self.bind(p, value)
            }
            (TypeKind::Class(a, p), TypeKind::Class(b, v)) if a == b => self.bind_all(p, v),
            (TypeKind::Enum(a, p), TypeKind::Enum(b, v)) if a == b => self.bind_all(p, v),
            // An associated type binds nothing: the type it stands for is known once its base is
            // bound, and the argument is then checked against it (see `Checker::known`).
            (TypeKind::Assoc(..), _) => true,
            _ => false,
        }
    }

    /// Records, for each parameter that `pattern` names, the part of `value`, a context that
    /// holds a hole, which lies at its place in `pattern`. The first part found stays, but a part
    /// that holds `REJECTED` and no `HOLE` replaces it. A whole hole is no part to record. Binds
    /// nothing. `partial` gives the part in place of an unbound parameter's hole, so an argument
    /// that waits for the parameter gets more context. A part that holds `REJECTED` and no `HOLE`
    /// tells the call that the context would have bound the parameter if the outer bound had held,
    /// so the call does not report the parameter (see `first_unbound_not_rejected`). An argument
    /// can still bind it.
    fn hint(&mut self, pattern: Type, value: Type) {
        if !pattern.has_param() || !value.holds_hole() || is_hole(value) {
            return;
        }
        match (pattern.kind(), value.kind()) {
            (TypeKind::Param(_, i), _) => {
                self.hints.resize(self.found.len(), None);
                let hint = &mut self.hints[i as usize];
                if hint.is_none() || value.holds_rejected_hole_only() {
                    *hint = Some(value);
                }
            }
            (TypeKind::Array(p), TypeKind::Array(v))
            | (TypeKind::Optional(p), TypeKind::Optional(v)) => self.hint(p, v),
            (TypeKind::Class(a, p), TypeKind::Class(b, v)) if a == b => {
                for (&p, &v) in iter::zip(p.get(), v.get()) {
                    self.hint(p, v);
                }
            }
            (TypeKind::Enum(a, p), TypeKind::Enum(b, v)) if a == b => {
                for (&p, &v) in iter::zip(p.get(), v.get()) {
                    self.hint(p, v);
                }
            }
            _ => {}
        }
    }

    /// Rejects parameter `i`, whose bound failed. It is bound to `REJECTED`, so that no argument
    /// binds it again and it meets every type. `known` and `printed` treat it as unbound, and
    /// `partial` gives `REJECTED` for it, so a call in an argument that waits for it knows that
    /// the fault is reported. No argument waits for it (see `waits_for_assoc`). Without this,
    /// each later argument would be checked against the type that the bound rejected.
    fn reject(&mut self, i: usize) {
        debug_assert!(
            self.found[i].is_some(),
            "a parameter should be bound before its bound fails"
        );
        self.found[i] = Some(*REJECTED);
    }

    /// The first parameter that `ty`, a callee's, names and that is unbound or rejected, which
    /// `known` has no type for. `None` when `ty` names none.
    fn unknown_param(&self, ty: Type) -> Option<usize> {
        if !ty.has_param() {
            return None;
        }
        match ty.kind() {
            TypeKind::Param(_, i) => match self.found[i as usize] {
                Some(bound) if bound != *REJECTED => None,
                _ => Some(i as usize),
            },
            TypeKind::Array(inner) | TypeKind::Optional(inner) => self.unknown_param(inner),
            TypeKind::Class(_, args) | TypeKind::Enum(_, args) => {
                args.get().iter().find_map(|&arg| self.unknown_param(arg))
            }
            TypeKind::Assoc(base, _, args, _) => self
                .unknown_param(base)
                .or_else(|| args.get().iter().find_map(|&arg| self.unknown_param(arg))),
            _ => unreachable!("only a kind with a child can name a parameter"),
        }
    }

    /// `bind` for each pair, every one of them tried.
    fn bind_all(&mut self, patterns: TypeArgs, values: TypeArgs) -> bool {
        iter::zip(patterns.get(), values.get()).fold(true, |all, (&p, &v)| self.bind(p, v) && all)
    }

    /// `ty`, a callee's, with its parameters' bindings in their place; `None` while it names
    /// one that is unbound or rejected.
    pub(super) fn known(&self, ty: Type) -> Option<Type> {
        self.resolve(ty, Unbound::Fail)
    }

    /// As `known`, with a hole for each parameter that is not known, so it always gives a type:
    /// the context that a call gives to an argument that waits for it. An unbound parameter gives
    /// its hint, else `HOLE`; a rejected one gives `REJECTED`. `bind` and `reject` put only
    /// `REJECTED` in `found`.
    fn partial(&self, ty: Type) -> Type {
        self.resolve(ty, Unbound::Hole)
            .expect("a hole should stand for each unbound parameter")
    }

    /// As `known`, with each unbound or rejected parameter by the callee's name, so it always
    /// gives a type. For a diagnostic: a repeated name shows that two places must be one type.
    fn printed(&self, ty: Type) -> Type {
        self.resolve(ty, Unbound::Keep)
            .expect("a parameter should stand for itself while it is unbound")
    }

    /// The walk of `known`, `partial` and `printed`: `unbound` says what an unbound parameter
    /// becomes. A rejected parameter, which `found` binds to `REJECTED`, is unbound here, but
    /// `partial` keeps `REJECTED` for it. An associated type whose base is rejected is rejected.
    fn resolve(&self, ty: Type, unbound: Unbound) -> Option<Type> {
        #[cfg(test)]
        if !matches!(unbound, Unbound::Keep) {
            WALKED.set(WALKED.get() + 1);
        }
        if !ty.has_param() {
            return Some(ty);
        }
        Some(match ty.kind() {
            TypeKind::Param(_, i) => match (self.found[i as usize], unbound) {
                (Some(bound), _) if bound != *REJECTED => bound,
                (Some(rejected), Unbound::Hole) => rejected,
                (_, Unbound::Fail) => return None,
                (_, Unbound::Hole) => self
                    .hints
                    .get(i as usize)
                    .copied()
                    .flatten()
                    .unwrap_or(*HOLE),
                (_, Unbound::Keep) => ty,
            },
            TypeKind::Array(element) => Type::array(self.resolve(element, unbound)?),
            TypeKind::Optional(payload) => Type::optional(self.resolve(payload, unbound)?),
            TypeKind::Class(id, args) => {
                Type::intern(TypeKind::Class(id, self.resolve_all(args, unbound)?))
            }
            TypeKind::Enum(id, args) => {
                Type::intern(TypeKind::Enum(id, self.resolve_all(args, unbound)?))
            }
            TypeKind::Assoc(base, interface, args, k) => {
                let base = self.resolve(base, unbound)?;
                // A hole stands for the whole type, which its base fixes. A rejected base
                // rejects it.
                if base.holds_hole() {
                    return Some(if base == *REJECTED { base } else { *HOLE });
                }
                let args = self.resolve_all(args, unbound)?;
                Type::intern(TypeKind::Assoc(base, interface, args, k))
            }
            _ => unreachable!("only a kind with a child can name a parameter"),
        })
    }

    fn resolve_all(&self, args: TypeArgs, unbound: Unbound) -> Option<TypeArgs> {
        let list: Option<Vec<Type>> = args
            .get()
            .iter()
            .map(|&ty| self.resolve(ty, unbound))
            .collect();
        list.map(|list| TypeArgs::intern(&list))
    }

    /// The first parameter from `from` on that is unbound and that is not rejected-only: its
    /// hint holds no `REJECTED`, or holds `HOLE` as well. A parameter whose hint holds
    /// `REJECTED` and no `HOLE` would have been bound had the outer bound held, so the call
    /// reports nothing for it. It reads one flag of each hint, so it costs nothing to the call
    /// that binds every parameter: only the error path runs it.
    fn first_unbound_not_rejected(&self, from: usize) -> Option<usize> {
        (from..self.found.len()).find(|&i| {
            self.found[i].is_none()
                && !self
                    .hints
                    .get(i)
                    .copied()
                    .flatten()
                    .is_some_and(Type::holds_rejected_hole_only)
        })
    }

    /// Every parameter's binding; `Err` holds the first unbound one's index. A rejected parameter
    /// is bound, to `REJECTED`: the list holds it, and only a call that has failed may have one,
    /// so no call uses that list.
    pub(super) fn finish(&self) -> Result<TypeArgs, usize> {
        let list = self
            .found
            .iter()
            .enumerate()
            .map(|(i, ty)| ty.ok_or(i))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TypeArgs::intern(&list))
    }
}

#[cfg(test)]
thread_local! {
    /// How many bindings inference has checked or undone on this thread.
    pub(super) static VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many types `Bindings::known` and `partial` have looked at on this thread, which
    /// checking a call uses. A `printed` walk is not counted: it runs once for each context a
    /// call gives (see `infer_waiting`), and for a diagnostic.
    pub(super) static WALKED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many type parameter names the search for reported ones has looked at on this
    /// thread.
    pub(super) static SCANNED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many types the walk for the type parameters that types name has visited on this
    /// thread.
    pub(super) static MARKED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many entries of the substitution lists of early bound checks have been written on this
    /// thread.
    pub(super) static LISTED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many due bounds `check_early` has looked at on this thread.
    pub(super) static EXAMINED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many lookups of `body.ahead` have found an entry on this thread.
    pub(super) static INSPECTED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many expressions `takes_context` has looked at on this thread.
    pub(super) static CONTEXT_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many expressions `rank_value` has walked on this thread, those it answers from
    /// `body.ranked` not counted.
    pub(super) static RANK_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A generic callee as a call sees it: a function or method, a class's constructor, or an
/// enum's variant.
pub(super) struct GenericSig<'a> {
    /// The name the call writes: the function's, the method's, the class's, or the variant's
    /// enum's.
    pub(super) name: &'a ast::Ident,
    /// The variant after `name`, which the call builds.
    pub(super) variant: Option<&'a ast::Ident>,
    /// Every type parameter, a method's receiver's first.
    pub(super) declared: &'a [Symbol],
    /// The receiver's type arguments, which bind the first of `declared`.
    pub(super) owner: &'a [Type],
    pub(super) params: &'a [Option<Type>],
    pub(super) ret: Option<Type>,
    /// The bounds of the parameters after the receiver's, in order; a parameter past the end has
    /// none. The call checks each as soon as its parameter is bound, and at no other time (see
    /// `Early`).
    pub(super) bounds: &'a [Vec<Bound>],
}

/// How a call writes its callee: its name, the type arguments in `angles` (none written when
/// `written` is empty), and the type the call's context expects.
#[derive(Clone, Copy)]
pub(super) struct Site<'a> {
    pub(super) name: &'a ast::Ident,
    pub(super) written: &'a [ast::TypeExpr],
    pub(super) angles: Span,
    pub(super) expected: Option<Type>,
}

/// A waiting generic call that an outer call checks as an argument (see `Checker::infer_waiting`).
/// The call reports a result that cannot meet the context as a mismatch with `want`.
#[derive(Clone, Copy)]
pub(super) struct ContextHint {
    /// The call's span.
    at: Span,
    /// The parameter type of the outer call, as that call prints it (`Bindings::printed`).
    want: Type,
    /// Whether the call is `receiver?.method(...)`, whose result is optional.
    safe: bool,
}

/// A generic call while its arguments bind its callee's parameters.
struct Inference<'s> {
    bindings: Bindings,
    /// Each argument once checked, in source order.
    slots: Vec<Option<Checked>>,
    /// The arguments of type `unit` or `never`, which bind only what no other argument does.
    last: Vec<(usize, Type)>,
    /// Whether a parameter was bound to a type that no parameter takes, or its bound failed: the
    /// call reported it.
    failed: bool,
    /// Whether the context rejected a parameter (see `REJECTED`). The call fails, but the outer
    /// call reported the bound, so this one reports nothing for that parameter. It still reports
    /// a parameter that the context leaves unbound. The field stays false for a parameter whose
    /// hint holds a rejected part only: `infer_call` does not set it, and it skips the E0050 of
    /// that parameter (see `Bindings::first_unbound_not_rejected`).
    rejected: bool,
    /// Where each parameter was bound: the argument, or the type argument written for it.
    origin: Vec<Option<Span>>,
    /// The bounds that the call checks as soon as a parameter is bound; none when the callee has
    /// no bound and no parameter type writes an associated type.
    early: Option<Early<'s>>,
}

/// `found`, a parameter's binding, as a bound sees it: `UNKNOWN` while it is unbound or rejected.
fn listed(found: Option<Type>) -> Type {
    found.filter(|&t| t != *REJECTED).unwrap_or(UNKNOWN)
}

/// The bounds a call checks as soon as the parameter they bound is bound. An argument that waits
/// for an associated type of the parameter must not meet a type that has none. A parameter whose
/// bound fails is rejected at once (see `Bindings::reject`), so no later argument is checked
/// against the type that the bound rejected. This is the only check of a bound: each binding of a
/// parameter reaches `check_early`, from the type arguments that the call writes, or from
/// `report_fresh` for an argument, the expected type and `blame_arguments`. A receiver's
/// parameters have no bound of the method to check, and `poison_broken_params` fails the call.
struct Early<'s> {
    /// Whether a parameter type writes an associated type, which `Checker::known_in` replaces.
    assoc: bool,
    /// How many of the callee's parameters its receiver binds.
    owned: usize,
    /// The bounds of the parameters after the receiver's, as `GenericSig::bounds`.
    bounds: &'s [Vec<Bound>],
    /// The type of each parameter as a bound sees it: its binding, or `UNKNOWN` while it is
    /// unbound or rejected. The first check that needs it builds it. Each later binding changes
    /// only its own entry, so the list is built once for the call.
    with: Option<Vec<Type>>,
    /// The bounds of a parameter that is bound, that name a parameter that is not bound yet, by
    /// the place of the first such parameter, which the bound waits for. Each bound has its place
    /// and its parameter's place. A binding takes only the lists of the parameters that it binds,
    /// and a bound that still names an unbound parameter goes to the list of that one. So a bound
    /// moves at most once for each parameter that it names.
    waiting: FxHashMap<usize, Vec<(usize, usize)>>,
}

impl<'s> Early<'s> {
    /// The bounds `sig`'s call checks early: none unless the callee has a bound or a parameter
    /// type writes an associated type. The second needs no bound of the callee's when the
    /// receiver binds the parameter that the type writes it through.
    fn of(sig: &GenericSig<'s>) -> Option<Early<'s>> {
        let assoc = sig.params.iter().flatten().any(|param| param.has_assoc());
        if !assoc && sig.bounds.iter().all(Vec::is_empty) {
            return None;
        }
        Some(Early {
            assoc,
            owned: sig.owner.len(),
            bounds: sig.bounds,
            with: None,
            waiting: FxHashMap::default(),
        })
    }
}

/// `checked`, an argument, made `Error` that its rule rejected when `mismatched`.
pub(super) fn mark((checked, mismatched): (Checked, bool)) -> Checked {
    if mismatched {
        Err(Poisoned {
            ty: None,
            mismatch: true,
        })
    } else {
        checked
    }
}

/// Whether `args` fit parameters `params`, each a resolved type and its label (`None` for a
/// positional one) in order: a call whose own check reports nothing of its shape.
fn fits(
    params: impl ExactSizeIterator<Item = (Option<Type>, Option<Symbol>)>,
    args: &[ast::Arg],
) -> bool {
    params.len() == args.len()
        && iter::zip(params, args)
            .all(|((ty, label), arg)| ty.is_some() && arg.label.as_ref().map(|l| l.name) == label)
}

/// Work that ranking an argument did ahead of its turn (see `rank_value`). It is kept in
/// `body.ahead` by the span it was done for, until checking reaches it. It lives no longer than
/// the generic call, join or operator that ranked it (see `Checker::ranked`), nor than the
/// diagnostics and locals it made.
pub(super) enum Ahead {
    /// A method's checked receiver, or a `match`'s scrutinee.
    Receiver(Checked),
    /// An `if let`'s value, as `let_value` gives it.
    Let(Checked, LetValue, bool),
    /// A block's scope, and its statements before its value.
    Block(FxHashMap<Symbol, Binding>, BlockSoFar),
    /// A branch with a pattern, an `if let`'s first block or a `match` arm: what the pattern
    /// bound, the arm's checked guard, the scope that holds the pattern's names and the
    /// branch's statements, and its statements before its value (none for an arm whose body
    /// is not a block).
    Bound(
        LetPattern,
        Option<Checked>,
        FxHashMap<Symbol, Binding>,
        BlockSoFar,
    ),
}

/// How much a value needs the context of its call, from least to most (see `rank_value`). The
/// rank orders the values of a call. A `Concrete` value binds first. A `Literal` value takes the
/// type that the others fix. A `Waits` value takes what the call's context gives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Rank {
    /// Its type comes from itself.
    Concrete,
    /// Its type comes from the defaults of its integer literals, or from the arguments of a
    /// generic call that rank so. It takes the type of the others when they fix one.
    ///
    /// A generic call ranks as the arguments do that give it its type: the arguments at the
    /// places of a type parameter of its own that its declared return type names (see
    /// `Places::carried`). The `1` of `k2(1, 2)` for `fn k2<T, U>(a: T, b: U) -> U` does not give
    /// the call its type. A generic call that is not open and has no such place is `Concrete`: its
    /// arguments do not give it its type. A call of a callee that is not generic is `Concrete` too
    /// (`len_of([1])` is `i64`; `bx.get(1)` is the `T` of the receiver).
    Literal,
    /// Its type cannot come from itself: it needs the context of its call.
    Waits,
}

/// What `rank_value` finds about a value.
#[derive(Clone, Copy, Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is a separate fact that ranking reads of a value"
)]
pub(super) struct Ranked {
    pub(super) rank: Rank,
    /// Whether the value can make an optional by itself, which a literal beside it does not. A
    /// bare `None` can, and so can a safe call (`receiver?.method(...)`). So can a call of a
    /// generic callee whose declared return type is an optional. So can a call whose declared
    /// return type is a bare type parameter, when an argument of that type is `optional`. A
    /// block can when its tail can. An `if`, an `if let` with `else` or a `match` can when a
    /// value branch can. A context that is an optional then meets the type of the value whole,
    /// not its payload. The field is complete only for a `Literal` or `Waits` value, as ranking
    /// stops at a `Concrete` part. A `Concrete` value can carry `true` (`opt(x)` with a concrete
    /// `x`), and no reader reads it.
    pub(super) optional: bool,
    /// Whether the type of the value has an optional that a plain literal in its place would
    /// not have. It is set where `optional` is. An array literal keeps it from its elements:
    /// `[Some(2)]` holds an optional, but is not an optional element. So does a call that has an
    /// argument that holds one at a place whose declared type names a type parameter that the
    /// declared return type names too, by a path with no optional on it (see `Places::plain`):
    /// `id([Some(2)])`, `wrap(Some(2))` for `-> [T]`, and `Bx(v: Some(2))`. Not `k2(Some(1), [2])`
    /// for `fn k2<T, U>(a: T, b: U) -> U`, and not `h(1, Some(2))` for `fn h<T>(a: T, b: T?) -> T`:
    /// there the optional fills the `?` of the place. The order of the arguments of a call reads
    /// it (see `infer_call`), and joins do not.
    pub(super) holds_optional: bool,
    /// Whether the value is an optional that has an optional payload, which the value makes by
    /// itself: `Some(Some(2))`, `opt(Some(2))` for `fn opt<T>(x: T) -> T?`, and `Some(if c {
    /// Some(2) } else { None })`. Not `Some(2)`, `None`, an array or a literal. A safe call keeps
    /// it from its call, as it flattens only the optional that it adds: `m?.o(Some(2))` for
    /// `fn o<T>(self, x: T) -> T?` is nested. An optional at a place `T?` of a call that returns
    /// `T` makes the call an optional only when it is nested (see `Makes::Through`). It is set
    /// from the facts of the parts, as `optional` is.
    nested: bool,
    /// For a `Literal` value: whether its type is the type of an integer literal it holds, which
    /// is a scalar (see `sibling`). An integer literal is. So is a value that passes such a
    /// literal on as it is: `dbg`, a block, a branch, or a call that returns a type parameter,
    /// when each literal argument that gives the call its type is at a place of that parameter.
    /// The `1` of `k2(1, 2)` for `fn k2<T, U>(a: T, b: U) -> U` gives the call no type, so it is
    /// not ranked. The `1` of `Bx(v: 1)` is not passed on. An array, or a call that returns
    /// `Some(...)` or a class, is not.
    scalar: bool,
}

impl Ranked {
    const CONCRETE: Self = Self {
        rank: Rank::Concrete,
        optional: false,
        holds_optional: false,
        nested: false,
        scalar: false,
    };
    const LITERAL: Self = Self {
        rank: Rank::Literal,
        optional: false,
        holds_optional: false,
        nested: false,
        scalar: true,
    };
    const WAITS: Self = Self {
        rank: Rank::Waits,
        optional: false,
        holds_optional: false,
        nested: false,
        scalar: false,
    };

    /// The rank that joins and operators order siblings by. They type a `Literal` value with
    /// `literal_type`, which knows scalars only. So a `Literal` value that is not `scalar` is
    /// `Concrete` for them. It is checked alone and takes its default type, as `[[1], [2]]` and
    /// `W(x: 1) < W(x: 2)` always did.
    // shortcut: such a value does not take the type of a sibling, so `[[1], x2]` with
    // `x2: [u8]` does not give `[1]` the type `[u8]`; upgrade when `literal_type` knows arrays
    // and the types of a call.
    ///
    /// A call that does not fit its declaration (`fit` is false) is `Concrete` too. It is checked
    /// first and alone, and reports its own mistakes, not those of a sibling's type.
    pub(super) fn sibling(self, fit: bool) -> Rank {
        if !fit || self.rank == Rank::Literal && !self.scalar {
            Rank::Concrete
        } else {
            self.rank
        }
    }
}

/// The rank of a value made of `parts`, each a value that its type comes from. The rank is
/// `Waits` when every part waits, `Literal` when no part is `Concrete` and one is `Literal`,
/// else `Concrete`. It is `optional`, `holds_optional` or `nested` when a part is, and `scalar`
/// when each `Literal` part is.
/// It stops at the first `Concrete` part, so the parts after it are not ranked.
fn combine(parts: impl IntoIterator<Item = Ranked>) -> Ranked {
    let mut found = Ranked::WAITS;
    let mut scalar = true;
    for part in parts {
        match part.rank {
            Rank::Concrete => return Ranked::CONCRETE,
            Rank::Literal => found.rank = Rank::Literal,
            Rank::Waits => {}
        }
        found.optional |= part.optional;
        found.holds_optional |= part.holds_optional;
        found.nested |= part.nested;
        if part.rank == Rank::Literal {
            scalar &= part.scalar;
        }
    }
    found.scalar = scalar;
    found
}

/// `combine` of the values of `branches`: each is `None` when its branch ends in an exit, else
/// the rank of its value. `Concrete` when no branch has a value.
fn combine_branches(branches: impl IntoIterator<Item = Option<Ranked>>) -> Ranked {
    let mut any = false;
    let found = combine(branches.into_iter().flatten().inspect(|_| any = true));
    if any { found } else { Ranked::CONCRETE }
}

/// Whether `checked`, an argument, was rejected by its parameter's type (see `mark`).
pub(super) fn mismatched(checked: &Checked) -> bool {
    matches!(checked, Err(Poisoned { mismatch: true, .. }))
}

impl Checker<'_> {
    /// `bindings.known(ty)`, with each associated type of a known type replaced by the type its
    /// conformance chose. `None` while `ty` names a parameter that is unbound, and too when a base
    /// has no conformance, which its bound check reports: the argument then waits.
    fn known(&self, bindings: &Bindings, ty: Type) -> Option<Type> {
        bindings.known(ty).and_then(|ty| self.normalize(ty))
    }

    /// `known` for `ty`, a parameter type of the call or a part of one: only a call whose
    /// parameter types write an associated type has any to replace.
    fn known_in(&self, call: &Inference<'_>, ty: Type) -> Option<Type> {
        match call.early {
            Some(Early { assoc: true, .. }) => self.known(&call.bindings, ty),
            _ => call.bindings.known(ty),
        }
    }

    /// `bindings.partial(ty)`, with the associated types of known types replaced.
    fn partial(&self, bindings: &Bindings, ty: Type) -> Type {
        let partial = bindings.partial(ty);
        self.normalize(partial).unwrap_or(partial)
    }

    /// `bindings.printed(ty)`, with the associated types of known types replaced.
    fn printed(&self, bindings: &Bindings, ty: Type) -> Type {
        let printed = bindings.printed(ty);
        self.normalize(printed).unwrap_or(printed)
    }

    /// The arguments of a call of the generic `sig`, checked in four passes that bind its type
    /// parameters. The caller has matched the arguments to the parameters by count and label.
    /// `written` gives every type parameter that the receiver does not give.
    ///
    /// 1. The arguments of rank `Concrete`. Each is checked against
    ///    its parameter's type once that type names no unbound parameter, and on its own before.
    /// 2. `expected` against the return type, whole. When the return type cannot meet an
    ///    optional `expected` whole, it meets its payload. When it meets neither with the help
    ///    of pass 1, but does meet one alone, each argument of pass 1 that contradicts it is
    ///    reported at the argument.
    ///    When `expected` holds a hole (see `call_context`) that the return type cannot meet,
    ///    and a parameter is left unbound, the call reports a mismatch at itself in place of
    ///    E0050.
    /// 3. The arguments of rank `Literal`, as in pass 1.
    /// 4. The arguments of rank `Waits`, against their parameter's type.
    ///
    /// An argument of type `unit` or `never` binds only what none of the others binds. Returns
    /// the arguments in source order, the type arguments and the call's type. Returns `None`,
    /// reported, when a parameter is left unbound or bound to a type that no parameter takes.
    /// A parameter that is left unbound, and whose context holds a rejected part only, is not
    /// reported: the failed bound of the outer call is the fault (see `Bindings::hint`).
    /// Then an argument that needs context and has none is checked against `UNKNOWN`, for its
    /// own mistakes only. What ranking the arguments checked ahead (see `Ahead`) and left unused
    /// is dropped.
    pub(super) fn generic_call(
        &mut self,
        span: Span,
        sig: &GenericSig<'_>,
        written: (&[ast::TypeExpr], Span),
        args: &[ast::Arg],
        expected: Option<Type>,
    ) -> Option<(Vec<Checked>, TypeArgs, Option<Type>)> {
        // The context of the quiet value failed, as an `UNKNOWN` one did.
        let expected = expected.or_else(|| self.quiet(span, expected).then_some(UNKNOWN));
        self.ranked(|c| c.infer_call(span, sig, written, args, expected))
    }

    /// `generic_call`, before what ranking kept is dropped.
    fn infer_call(
        &mut self,
        span: Span,
        sig: &GenericSig<'_>,
        (written, angles): (&[ast::TypeExpr], Span),
        args: &[ast::Arg],
        expected: Option<Type>,
    ) -> Option<(Vec<Checked>, TypeArgs, Option<Type>)> {
        let owned = sig.owner.len();
        let mut call = Inference {
            bindings: Bindings::new(sig.declared.len(), sig.owner),
            slots: args.iter().map(|_| None).collect(),
            last: Vec::new(),
            failed: false,
            rejected: false,
            origin: vec![None; sig.declared.len()],
            early: Early::of(sig),
        };
        if !written.is_empty() {
            let Some(list) = self.type_args(sig.name, &sig.declared[owned..], written, angles)
            else {
                self.stray(args);
                return None;
            };
            // `type_args` rejects each type that `report_fresh` would.
            for (i, &ty) in list.get().iter().enumerate() {
                call.bindings.found[owned + i] = Some(ty);
                call.origin[owned + i] = written.get(i).map(|ty| ty.span);
            }
            let bound: Vec<usize> = (owned..sig.declared.len()).collect();
            self.check_early(&mut call, &bound, span);
        }
        // Whether every call in an argument fits its declaration.
        let mut fit = vec![true; args.len()];
        let (ranks, holds, (through, layers)) = self.rank_arguments(&call, sig, args, &mut fit);
        // A context that holds a hole gives the arguments below more context (see `hint`).
        if let (Some(ret), Some(want)) = (sig.ret, expected) {
            call.bindings.hint(ret, want);
        }
        for (i, arg) in args.iter().enumerate().filter(|&(i, _)| ranks[i] == 0) {
            self.infer_arg(&mut call, i, &arg.value, sig.params[i], None);
        }
        let mut misfit = None;
        if let (Some(ret), Some(want)) = (sig.ret, expected.filter(|&ty| ty != UNKNOWN)) {
            // The return type meets `want` whole if it can and an argument waits for it, or
            // makes the optional itself and has the type of the return type (`through`), or the
            // arguments prove that the call makes optionals (`layers`) and `want` has no more:
            // `let x: i64? = pick(5, None)` binds `T = i64?`, and so does `id(Some(1))`. Else it
            // meets the payload of an optional `want`, and the call's value is wrapped:
            // `y ?? id(2)` on an `i64?` `y` gives the `i64` that `y ?? 2` gives. What of it
            // cannot meet either is reported where the value is used, unless the context fits
            // alone: then the arguments that contradict it are reported, each at its own place.
            // Type arguments that the call writes are its own.
            let whole = through || ranks.contains(&2) || layers > 0 && !wider_than(want, layers);
            let met = (whole && call.bindings.unify(ret, want))
                || call.bindings.constrain(ret, payload(want));
            misfit = self.misfit(span, ret, want, met).then_some(ret);
            if !met && written.is_empty() && !call.failed {
                self.blame_arguments(&mut call, sig, args, ret, want, span);
            }
            self.report_fresh(&mut call, span);
        }
        // With no context, the first argument of rank 1 binds the parameter, and one that holds
        // an optional gives the wider type: `pick(1, Some(2))` binds `T = i64?`, where `1` first
        // would bind `T = i64` and reject `Some(2)`. So does `pick([1], [Some(2)])`, which binds
        // `T = [i64?]`. A literal then fits the optional, as `1` fits `i64?`. So the arguments
        // that hold an optional are checked before the others.
        for first in [true, false] {
            let wanted = |&(i, _): &(usize, _)| ranks[i] == 1 && holds[i] == first;
            for (i, arg) in args.iter().enumerate().filter(wanted) {
                self.infer_arg(&mut call, i, &arg.value, sig.params[i], None);
            }
        }
        self.sweep_waiting(&mut call, sig, args, &ranks, &fit);
        for (i, ty) in std::mem::take(&mut call.last) {
            let (e, param) = (&args[i].value, sig.params[i]);
            if let Some(param) = param
                && self.known_in(&call, param).is_none()
                && self.bind_param(&mut call, param, ty, e.span)
            {
                continue;
            }
            self.no_value(e.span, ty);
            call.slots[i] = Some(Err(Poisoned::error()));
        }
        let broken = poison_broken_params(&mut call.bindings, sig.declared);
        // Once every parameter is bound, an argument is still unchecked only when its parameter
        // type names an associated type that has no choice. The conformance reported that, and
        // the call fails after it.
        let unchecked = call.slots.iter().any(Option::is_none);
        let list = match call.bindings.finish() {
            Ok(list) if !call.failed && !call.rejected && !broken && !unchecked => Some(list),
            Ok(_) => None,
            Err(first) => {
                // An argument that failed, or a parameter's or the return type that did, may be
                // what would have bound it, and an `UNKNOWN` context is one that failed.
                let errored = call.slots.iter().flatten().any(|c| type_of(c).is_none())
                    || sig.params.contains(&None)
                    || sig.ret.is_none();
                if !errored && !call.failed && expected != Some(UNKNOWN) {
                    // A parameter that meets a rejected part only is rejected as well: the
                    // failed bound of the outer call is its only fault.
                    if let Some(i) = call.bindings.first_unbound_not_rejected(first) {
                        let found = misfit.map(|ret| self.printed(&call.bindings, ret));
                        self.report_unbound(span, sig, i, found);
                    }
                }
                None
            }
        };
        let Some(list) = list else {
            // What waited for context and has none is checked against a failed one, so it
            // reports only its own mistakes, and uses what ranking kept for it.
            for (arg, slot) in iter::zip(args, &call.slots) {
                if slot.is_none() {
                    drop(self.expr(&arg.value, Some(UNKNOWN)));
                }
            }
            return None;
        };
        self.instantiates = true;
        let checked = call
            .slots
            .into_iter()
            .map(|slot| {
                slot.expect("every argument should be checked once each parameter is bound")
            })
            .collect();
        let ret = sig.ret.and_then(|ret| self.subst(ret, list.get()));
        Some((checked, list, ret))
    }

    /// The rank of each argument of the call: 2 for `Rank::Waits`, 1 for `Rank::Literal`, else 0
    /// for `Rank::Concrete`. An argument for a type that names an unbound parameter only under
    /// an associated type waits for it, whatever the argument is (see `waits_for_assoc`).
    ///
    /// Also whether each argument of rank 1 is `Ranked::holds_optional`, which sets the order of
    /// rank 1 (see `infer_call`). `fit` is cleared where a call in an argument does not fit its
    /// declaration.
    ///
    /// Also whether an argument of rank 1 makes the parameter that the return type names an
    /// optional by its own type, for a return type `T` (see `Makes::passes_optional`): the
    /// context then meets the return type whole. And how many optionals the call makes at least,
    /// as the other arguments of rank 1 prove (see `Makes::makes_optional`): 1 for a return type
    /// `T`, 2 for `T?`, else 0. The context meets the return type whole when it has no more
    /// optionals than that. Else its payload meets the return type, and `T` is an optional
    /// already (`let v: i64?? = h(1, Some(Some(2)))`).
    // shortcut: for a return type `T`, an optional at a place of `T` forces the context to meet
    // the return type whole, whatever the context is. `let v: i64?? = first([None, Some(1)])`
    // binds `T = i64??` (a run test pins it), so a rule by the count would change its value, and
    // `h(Some(1), Some(2))` at `i64??` for `fn h<T>(a: T, b: T?) -> T` is an error (as before);
    // upgrade if such a call must meet the payload when the argument at the `T?` place needs it.
    fn rank_arguments(
        &mut self,
        call: &Inference<'_>,
        sig: &GenericSig<'_>,
        args: &[ast::Arg],
        fit: &mut [bool],
    ) -> (Vec<u8>, Vec<bool>, (bool, usize)) {
        let makes = Makes::of(sig.ret, sig.params.iter().copied());
        let (mut through, mut layers) = (false, 0);
        let (ranks, holds) = iter::zip(args, fit)
            .enumerate()
            .map(|(i, (arg, fit))| {
                let writes_assoc =
                    sig.params[i].is_some_and(|param| waits_for_assoc(&call.bindings, param));
                if writes_assoc {
                    return (2, false);
                }
                let ranked = self.rank_value(&arg.value, fit);
                match ranked.rank {
                    Rank::Waits => (2, false),
                    Rank::Literal => {
                        if makes.passes_optional(i, ranked) && !makes.wraps() {
                            through = true;
                        } else if makes.makes_optional(i, ranked) {
                            layers = if makes.wraps() { 2 } else { 1 };
                        }
                        (1, ranked.holds_optional)
                    }
                    Rank::Concrete => (0, false),
                }
            })
            .unzip();
        (ranks, holds, (through, layers))
    }

    /// Checks the bounds of the parameters `bound`, which a binding has just bound, and the bounds
    /// that wait for them. An associated type that a parameter type writes through the parameter
    /// exists only if the bound holds. A bound whose arguments name a parameter that is not bound
    /// yet waits for it (see `Early::waiting`). It is checked as soon as a binding binds what it
    /// names, before any argument that waits for the associated type is checked.
    ///
    /// A bound that fails is reported once, and the call fails. Each parameter whose bound failed
    /// is then rejected (see `Bindings::reject`). The arguments after it are checked for their own
    /// mistakes, not against the type that the bound rejected. All the due bounds are checked
    /// first, so that each bound that fails is reported, also two of one parameter. A bound of a
    /// parameter that is rejected already has nothing to check.
    ///
    /// The work is linear in the parameters. The substitution list is built once for the call
    /// (see `Early::with`), and a binding takes only the bounds that wait for its parameters.
    fn check_early(&mut self, call: &mut Inference<'_>, bound: &[usize], at: Span) {
        let Inference {
            bindings,
            origin,
            failed,
            early,
            ..
        } = call;
        let Some(early) = early else { return };
        let (owned, bounds) = (early.owned, early.bounds);
        if let Some(with) = &mut early.with {
            for &i in bound {
                #[cfg(test)]
                LISTED.set(LISTED.get() + 1);
                with[i] = listed(bindings.found[i]);
            }
        }
        let count =
            |i: usize| (i.checked_sub(owned).and_then(|k| bounds.get(k))).map_or(0, Vec::len);
        if early.waiting.is_empty() && bound.iter().all(|&i| count(i) == 0) {
            return;
        }
        let mut due: Vec<(usize, usize)> = Vec::new();
        for &i in bound {
            due.extend(early.waiting.remove(&i).into_iter().flatten());
        }
        for &i in bound {
            due.extend((0..count(i)).map(|j| (i, j)));
        }
        let mut rejected = Vec::new();
        for (i, j) in due {
            #[cfg(test)]
            EXAMINED.set(EXAMINED.get() + 1);
            // A bound of a parameter that is rejected already has nothing to check.
            if bindings.found[i] == Some(*REJECTED) {
                continue;
            }
            let bound = bounds[i - owned][j];
            let args = bound.args.get();
            if let Some(waits_for) = args.iter().find_map(|&arg| bindings.unknown_param(arg)) {
                early.waiting.entry(waits_for).or_default().push((i, j));
                continue;
            }
            // An associated type with no conformance for its base never resolves. The bound that
            // fails for that base reports it, so this bound has nothing to wait for.
            if !args.iter().all(|&arg| self.known(bindings, arg).is_some()) {
                continue;
            }
            let ty =
                bindings.found[i].expect("a parameter that a binding bound should have a type");
            let with = early.with.get_or_insert_with(|| {
                #[cfg(test)]
                LISTED.set(LISTED.get() + bindings.found.len());
                bindings.found.iter().map(|&t| listed(t)).collect()
            });
            if !self.check_bound(origin[i].unwrap_or(at), ty, bound, with) {
                *failed = true;
                rejected.push(i);
            }
        }
        for i in rejected {
            bindings.reject(i);
            if let Some(with) = &mut early.with {
                with[i] = UNKNOWN;
            }
        }
    }

    /// Whether the call at `span` is a safe call that is checked against a context.
    fn is_safe_call(&self, span: Span) -> bool {
        (self.body.context_hint).is_some_and(|hint| hint.at == span && hint.safe)
    }

    /// Whether the call at `span` is a misfit: `want`, its context, holds a hole, and the result
    /// cannot meet it. `met` says if the return type `ret` met `want`. The result of a safe
    /// call is optional, as is a result whose return type is, so a context that is not optional
    /// cannot meet it, whatever the return type meets.
    fn misfit(&self, span: Span, ret: Type, want: Type, met: bool) -> bool {
        let safe = self.is_safe_call(span);
        let optional = |ty: Type| matches!(ty.kind(), TypeKind::Optional(_));
        want.holds_hole() && (!met || ((safe || optional(ret)) && !optional(want)))
    }

    /// Reports each argument already checked whose type is not what `want`, the call's context,
    /// makes of its parameter, once `ret` meets `want` or its payload with no argument's help.
    /// Such an argument is rejected, as a rule rejects its operand, and the later arguments
    /// meet the context's types in place of the blamed arguments'. Reports nothing, and leaves
    /// the bindings, when the context does not fit `ret` alone as a value of `ret` fits it, when
    /// an argument was rejected already, or when every argument fits.
    fn blame_arguments(
        &mut self,
        call: &mut Inference<'_>,
        sig: &GenericSig<'_>,
        args: &[ast::Arg],
        ret: Type,
        want: Type,
        span: Span,
    ) {
        let mut context = Bindings::new(sig.declared.len(), sig.owner);
        let met = if context.unify(ret, want) {
            want
        } else if context.unify(ret, payload(want)) {
            payload(want)
        } else {
            return;
        };
        // The context has hints of its own, which stay when it replaces the bindings below.
        context.hint(ret, met);
        // `unify` lets a `T?` take a value that is not optional, which the result cannot do. The
        // value of a safe call is always optional, so it cannot meet a context that is not.
        let result = self.partial(&context, ret);
        if !meets(optional_of(result), want) && (self.is_safe_call(span) || !meets(result, want)) {
            return;
        }
        // The error of an earlier argument is the call's error, as it is without a context.
        if call.slots.iter().flatten().any(mismatched) {
            return;
        }
        // A context that fails a bound is not a type that the parameter can have: no argument is
        // to blame for not being it, and the bound check would report the types that it set.
        if !self.meets_bounds(sig, &context) {
            return;
        }
        let mut blamed = false;
        for (i, arg) in args.iter().enumerate() {
            // An argument of type `unit` or `never` binds only what no other does (see `last`).
            let got = call.slots[i].as_ref().and_then(type_of);
            let (Some(param), Some(got)) = (sig.params[i], got) else {
                continue;
            };
            let Some(fit) = self.known(&context, param) else {
                continue;
            };
            // As `against`: the type, or its payload wrapped.
            if got == fit
                || matches!(got, Type::UNIT | Type::NEVER)
                || matches!(fit.kind(), TypeKind::Optional(inner) if inner == got)
            {
                continue;
            }
            let d = self.mismatch(arg.value.span, Some(&arg.value), fit, got);
            self.diagnostics.push(d);
            call.slots[i] = Some(Err(Poisoned {
                ty: None,
                mismatch: true,
            }));
            blamed = true;
        }
        if blamed {
            // A parameter that the context leaves open keeps what an argument bound.
            for (found, bound) in iter::zip(&mut context.found, &call.bindings.found) {
                *found = found.or(*bound);
            }
            call.bindings = context;
        }
    }

    /// Whether each parameter that `context` binds meets its bounds (`Checker::conforms`, which
    /// reports nothing). A bound whose arguments name a parameter that `context` leaves unbound
    /// is not decided yet, as in `check_early`, and holds here. The work is one query for each
    /// bound of the callee.
    fn meets_bounds(&mut self, sig: &GenericSig<'_>, context: &Bindings) -> bool {
        let with: Vec<Type> = context.found.iter().map(|&t| listed(t)).collect();
        let owned = sig.owner.len();
        for (k, bounds) in sig.bounds.iter().enumerate() {
            let Some(ty) = context.found[owned + k].filter(|&t| t != *REJECTED) else {
                continue;
            };
            for bound in bounds {
                let args = bound.args.get();
                if !args.iter().all(|&arg| self.known(context, arg).is_some()) {
                    continue;
                }
                let args = self.subst_args(bound.args, &with);
                if !self.conforms(ty, bound.interface, args) {
                    return false;
                }
            }
        }
        true
    }

    /// Checks the arguments that need context (rank 2) once the others are checked, in two
    /// sweeps: a call checked after an argument that was left unchecked can have bound the
    /// parameter it waits for. Two are enough: an argument that the second sweep checks has its
    /// parameter bound, so it binds nothing new, and a third sweep would find nothing.
    fn sweep_waiting(
        &mut self,
        call: &mut Inference<'_>,
        sig: &GenericSig<'_>,
        args: &[ast::Arg],
        ranks: &[u8],
        fit: &[bool],
    ) {
        for _ in 0..2 {
            for (i, arg) in args.iter().enumerate() {
                if ranks[i] == 2 && call.slots[i].is_none() {
                    self.infer_waiting(call, i, &arg.value, sig.params[i], fit[i]);
                }
            }
        }
    }

    /// Checks `e`, the waiting argument at `i` for `param`. It stays unchecked while `param`
    /// names an unbound parameter only under an associated type, whatever `e` is (see
    /// `waits_for_assoc`). A rejected parameter is not unbound there: nothing will bind it, so no
    /// argument waits for it. It stays unchecked too while `param` names an unbound parameter,
    /// unless it holds a call that reports its own mistake (`fit` is false), which is checked on
    /// its own, or `call_context` gives it a context: it is checked against what the call binds
    /// of `param`. A call reports a result that cannot meet the context as a mismatch with
    /// `param`, by `context_hint`. A bare type parameter has no hint, as the user did not write
    /// that name at the call.
    fn infer_waiting(
        &mut self,
        call: &mut Inference<'_>,
        i: usize,
        e: &ast::Expr,
        param: Option<Type>,
        fit: bool,
    ) {
        if param.is_some_and(|param| waits_for_assoc(&call.bindings, param)) {
            return;
        }
        if !fit || param.is_none_or(|param| self.known_in(call, param).is_some()) {
            self.infer_arg(call, i, e, param, None);
        } else if let Some(param) = param {
            // The call reports a context it cannot meet as a mismatch with this type. A bare
            // type parameter has no hint. The user did not write that name at the call. So a
            // safe call under a plain context gets no context there.
            let outer = self.body.context_hint;
            if !matches!(param.kind(), TypeKind::Param(..)) {
                self.body.context_hint = Some(ContextHint {
                    at: e.span,
                    want: self.printed(&call.bindings, param),
                    safe: matches!(e.kind, ast::ExprKind::MethodCall { safe: true, .. }),
                });
            }
            if let Some(context) = self.call_context(call, e, param) {
                self.infer_arg(call, i, e, Some(param), Some(context));
            }
            self.body.context_hint = outer;
        }
    }

    /// The context for `e`, an argument for `param`, when `param` names an unbound parameter.
    /// It is what the call binds of `param`, with a hole for each part it does not bind. `e`
    /// gets it when `e` can take it (see `takes_context`) and it says more than a hole. A
    /// generic call binds its own parameters from it. `Some(...)` and an array literal pass it
    /// to their parts. A rejected parameter gives `REJECTED`, which rejects the parameter that
    /// the call binds at that place.
    // shortcut: a waiting `if`, `match` or block gets no context. `expected` flows into its
    // branches, `None`, `[]` and integer literals, and each would need a rule for a hole. Add
    // them one kind at a time when a program needs one. The call gives context in argument
    // order. The first waiting call that it checks binds what the context of the next one
    // holds. For `mk2<E>() -> Two<E, bool>` and `mk<E>() -> Result<i64, E>`, `g(mk2(), mk())`
    // with `g<T>(c: Two<T, bool>, d: Result<T, string>)` reports E0050 at `mk2`. `g(mk(), mk2())`
    // with the parameters in the other order checks. To remove this, check a call again after
    // a later call binds its parameter (#217).
    fn call_context(&mut self, call: &Inference<'_>, e: &ast::Expr, param: Type) -> Option<Type> {
        let partial = self.partial_context(call, e, param)?;
        self.takes_context(e, partial).then_some(partial)
    }

    /// `call_context` before `takes_context`. It gives what the call binds of `param`. It gives
    /// nothing if `e` is of a kind that cannot take it, or if that says no more than a hole.
    fn partial_context(&self, call: &Inference<'_>, e: &ast::Expr, param: Type) -> Option<Type> {
        use ast::ExprKind as K;
        // An array literal takes the context from its elements, one by one (`inferred_array`).
        // That needs an array type to name them. For a type parameter it would take a hole.
        let literal = matches!(e.kind, K::ArrayLit(_));
        if !(literal && matches!(param.kind(), TypeKind::Array(_))
            || matches!(e.kind, K::Call { .. } | K::MethodCall { .. }))
        {
            return None;
        }
        let partial = self.partial(&call.bindings, param);
        (partial != *HOLE).then_some(partial)
    }

    /// Whether `e` can take `context`, which holds a hole, as its expected type with no hole
    /// reaching a type. A generic call binds its own parameters from it. `Some(x)` and a
    /// non-empty array literal take their type from the context, so each needs the same of
    /// its parts. `dbg(x)` binds nothing and passes the context to `x`, so it takes what `x`
    /// takes, unless `x` is an array literal. Another call that is not generic could pass the
    /// hole into a type. A safe call has an optional result, so it
    /// takes an optional context only: for a plain one, its shape is the mistake to report.
    /// The exception is a safe call with a `ContextHint` (see `infer_waiting`). `misfit` and
    /// `report_unbound` report its shape, and `blame_arguments` blames none of its arguments.
    ///
    /// The work is one step for each expression that the walk reaches. `inferred_array` does
    /// not walk the elements of an array literal again after this accepts it. `taken` keeps
    /// the answer for each array literal, which `inferred_array` asks for again when it
    /// refuses the literal, and for each `Some(...)`, which the call of `Some` asks for again.
    fn takes_context(&mut self, e: &ast::Expr, context: Type) -> bool {
        #[cfg(test)]
        CONTEXT_STEPS.set(CONTEXT_STEPS.get() + 1);
        if context == *HOLE {
            return false;
        }
        if let ast::ExprKind::ArrayLit(elements) = &e.kind {
            // A literal with an element that takes no context fails after a walk of all it
            // holds, and `inferred_array` asks for each element again. The answer is kept.
            if let Some(&taken) = self.body.taken.get(&(e.span, context)) {
                return taken;
            }
            let taken = matches!(context.kind(), TypeKind::Array(element)
                if !elements.is_empty() && elements.iter().all(|e| self.takes_context(e, element)));
            self.body.taken.insert((e.span, context), taken);
            return taken;
        }
        if let Some(arg) = dbg_argument(e) {
            // An array literal under `dbg` would take the hole as its type: only a direct
            // argument goes to `inferred_array`.
            return !matches!(arg.kind, ast::ExprKind::ArrayLit(_))
                && self.takes_context(arg, context);
        }
        let safe = matches!(e.kind, ast::ExprKind::MethodCall { safe: true, .. });
        match self.call_shape(e) {
            Some((shape, [arg])) if shape.some && arg.label.is_none() => {
                // `Some` checks its argument as a generic call, which asks again for the
                // context of that argument. The answer is the same, so it is kept.
                if let Some(&taken) = self.body.taken.get(&(e.span, context)) {
                    return taken;
                }
                let taken = matches!(context.kind(), TypeKind::Optional(payload)
                    if self.takes_context(&arg.value, payload));
                self.body.taken.insert((e.span, context), taken);
                taken
            }
            Some((shape, _)) => {
                shape.generic
                    && !shape.some
                    && (!safe
                        || matches!(context.kind(), TypeKind::Optional(_))
                        || self.is_safe_call(e.span))
            }
            None => false,
        }
    }

    /// Checks `e`, the argument at `i` for `param`: against `param` once the call binds what it
    /// names, and otherwise on its own, which binds what its type fixes. `context` is what
    /// `call_context` gave, the expected type of `e` while `param` names an unbound parameter.
    /// If the caller gives none, this asks `call_context`. So an argument that does not wait
    /// still meets what the call binds of `param`.
    fn infer_arg(
        &mut self,
        call: &mut Inference<'_>,
        i: usize,
        e: &ast::Expr,
        param: Option<Type>,
        context: Option<Type>,
    ) {
        let Some(pattern) = param else {
            call.slots[i] = Some(mark(self.operand(e, None)));
            return;
        };
        let unbound = self.known_in(call, pattern).is_none();
        let context = context.or_else(|| {
            unbound
                .then(|| self.call_context(call, e, pattern))
                .flatten()
        });
        let checked = self.infer_value(call, e, pattern, context);
        if unbound && let Some(ty @ (Type::UNIT | Type::NEVER)) = type_of(&checked) {
            call.last.push((i, ty));
        }
        call.slots[i] = Some(checked);
    }

    /// Checks `e` against `pattern`, a parameter's type or an element's: against what the call
    /// binds of it, or, while it names an unbound parameter, on its own, which binds what its
    /// type fixes. A non-empty array literal is the exception: unless its element type is a
    /// type parameter or an associated type that nothing binds or hints, its elements bind the
    /// parameters one by one (see `inferred_array`).
    /// A value of type `unit` or `never` binds nothing, for `infer_arg` to defer.
    fn infer_value(
        &mut self,
        call: &mut Inference<'_>,
        e: &ast::Expr,
        pattern: Type,
        context: Option<Type>,
    ) -> Checked {
        match self.known_in(call, pattern) {
            Some(want) => mark(self.operand(e, Some(want))),
            None => self.infer_unbound(call, e, pattern, context),
        }
    }

    /// `infer_value` for a `pattern` that names an unbound parameter, with no walk of
    /// `pattern` to find this out. `context` is the expected type of `e`, if any. A non-empty
    /// array literal goes to `inferred_array` unless its element type is a type parameter or an
    /// associated type that nothing binds or hints, as `infer_value` says. Any other element
    /// type holds a part that is known or can be wrapped to. In `inferred_array`, an element
    /// that is a local variable takes no context: it is checked on its own, and it binds or
    /// meets the element type, so the context of the other elements stays. A part of `context`
    /// may be `REJECTED`, when `pattern` names a rejected parameter there. A call in `e` that
    /// meets it rejects its own parameter at that place and reports nothing for it: the failed
    /// bound, which is reported already, is the fault. A part that is only `HOLE` leaves the
    /// parameter there unbound, and E0050 reports it.
    fn infer_unbound(
        &mut self,
        call: &mut Inference<'_>,
        e: &ast::Expr,
        pattern: Type,
        context: Option<Type>,
    ) -> Checked {
        if let ast::ExprKind::ArrayLit(elements) = &e.kind
            && !elements.is_empty()
            && let TypeKind::Array(element) = pattern.kind()
            // The `matches!` only skips the walk of `partial`: any other kind is never a hole.
            && (!matches!(element.kind(), TypeKind::Param(..) | TypeKind::Assoc(..))
                || self.partial(&call.bindings, element) != *HOLE)
        {
            return self.inferred_array(call, e.span, elements, element, context.is_some());
        }
        let checked = self.expr(e, context);
        debug_assert!(
            type_of(&checked).is_none_or(|ty| !ty.holds_hole()),
            "a context's hole should not reach the type of the argument it was for"
        );
        match type_of(&checked) {
            Some(Type::UNIT | Type::NEVER) | None => checked,
            Some(ty) if !self.bind_param(call, pattern, ty, e.span) => {
                let want = self.printed(&call.bindings, pattern);
                let d = self.mismatch(e.span, Some(e), want, ty);
                self.diagnostics.push(d);
                mark((checked, true))
            }
            Some(_) => {
                let want = self.known_in(call, pattern);
                debug_assert!(
                    want.is_some() || call.failed || call.rejected,
                    "a unified pattern should be known, unless a bound check failed"
                );
                mark(self.against(e, checked, want, None))
            }
        }
    }

    /// The array literal `elements` for `[element]`, where `element` names an unbound parameter.
    /// The elements are checked in the three waves of `Rank` (see `sibling_rank`), as `infer_value`
    /// does. So a later `None` or integer literal meets the parameter that an earlier element
    /// bound, and so wraps. A `None` or `[]` with nothing bound is reported as `joined` reports it:
    /// the first alone, unless an element failed already. Each element gets its own context, so no
    /// hole reaches the type of the array. `accepted` says that `takes_context` has accepted the
    /// literal, so each element takes its context with no new walk. Else each element asks
    /// `call_context`: an element that takes no context, such as a local variable, is checked on
    /// its own, and the others keep theirs. An element of type `unit` or `never` has no value,
    /// which is reported at once, and binds nothing.
    fn inferred_array(
        &mut self,
        call: &mut Inference<'_>,
        span: Span,
        elements: &[ast::Expr],
        element: Type,
        accepted: bool,
    ) -> Checked {
        let mut slots: Vec<Option<Checked>> = elements.iter().map(|_| None).collect();
        let mut quiet = false;
        // `element`'s type once an element binds all it names. A later element meets it with
        // no new walk of `element`, so the work does not grow with elements times its depth.
        let mut bound = None;
        let ranks: Vec<Rank> = elements.iter().map(|e| self.sibling_rank(e)).collect();
        for wave in [Rank::Concrete, Rank::Literal, Rank::Waits] {
            for (i, e) in elements
                .iter()
                .enumerate()
                .filter(|&(i, _)| ranks[i] == wave)
            {
                let checked = if let Some(want) = bound {
                    mark(self.operand(e, Some(want)))
                } else if !bare_hole(e) {
                    let kept = call.bindings.kept;
                    let context = if accepted {
                        self.partial_context(call, e, element)
                    } else {
                        self.call_context(call, e, element)
                    };
                    let checked = self.infer_unbound(call, e, element, context);
                    // Only a new binding can bind `element`.
                    if call.bindings.kept != kept {
                        bound = self.known_in(call, element);
                    }
                    checked
                } else {
                    quiet = quiet
                        || call.failed
                        || call.rejected
                        || slots.iter().flatten().any(|c| type_of(c).is_none());
                    // Only its own diagnostics matter.
                    drop(self.value(e, quiet.then_some(UNKNOWN)));
                    quiet = true;
                    Err(Poisoned::error())
                };
                // An element has a value: `infer_value` leaves `unit` and `never` for
                // `infer_arg`, which an element has not.
                slots[i] = Some(match type_of(&checked) {
                    Some(ty @ (Type::UNIT | Type::NEVER)) => {
                        self.no_value(e.span, ty);
                        Err(Poisoned::error())
                    }
                    _ => checked,
                });
            }
        }
        let Some(bound) = bound else {
            return Err(Poisoned::error());
        };
        let mismatched = slots
            .iter()
            .flatten()
            .any(|c| matches!(c, Err(Poisoned { mismatch: true, .. })));
        let checked = slots
            .into_iter()
            .map(|slot| slot.expect("the three waves should check every element, one rank each"))
            .collect();
        self.array(span, bound, checked, mismatched)
    }

    /// `Bindings::unify`, reporting at `at` each parameter it binds to a type that no type
    /// parameter takes.
    fn bind_param(
        &mut self,
        call: &mut Inference<'_>,
        pattern: Type,
        value: Type,
        at: Span,
    ) -> bool {
        let unified = call.bindings.unify(pattern, value);
        self.report_fresh(call, at);
        unified
    }

    /// Reports at `at` each parameter the last binding bound to a type that no type parameter
    /// takes.
    fn report_fresh(&mut self, call: &mut Inference<'_>, at: Span) {
        for &i in &call.bindings.fresh {
            #[cfg(test)]
            VISITS.set(VISITS.get() + 1);
            call.origin[i].get_or_insert(at);
            let ty = call.bindings.found[i].expect("a fresh parameter should be bound");
            // The context rejected it, and the outer call reported why.
            if ty == *REJECTED {
                call.rejected = true;
            } else if matches!(ty, Type::UNIT | Type::NEVER) {
                self.report(DiagnosticKind::TypeArgument {
                    ty: self.type_name(ty),
                    at,
                });
                call.failed = true;
            }
        }
        if call.early.is_some() {
            // `check_early` leaves `fresh` as it is.
            let fresh = std::mem::take(&mut call.bindings.fresh);
            self.check_early(call, &fresh, at);
            call.bindings.fresh = fresh;
        }
    }

    /// Reports the call at `span` that leaves parameter `param` unbound, the first one that is not
    /// rejected-only (see `Bindings::first_unbound_not_rejected`): E0050, unless its result
    /// `found` cannot meet the context its caller gave (see `infer_waiting`). Then the mismatch is
    /// the fault, and it reports the mismatch instead. A call that returns `unit` or `never` never
    /// gets here: it names no type parameter in its return type, and no type parameter can be
    /// `unit` (E0138), so it does not wait for a context.
    fn report_unbound(
        &mut self,
        span: Span,
        sig: &GenericSig<'_>,
        param: usize,
        found: Option<Type>,
    ) {
        let hint = self.body.context_hint.filter(|hint| hint.at == span);
        if let Some((found, hint)) = found.zip(hint) {
            // The result of a safe call is optional (see `Checker::member`).
            let found = if hint.safe { as_optional(found) } else { found };
            let d = self.mismatch(span, None, hint.want, found);
            self.diagnostics.push(d);
        } else {
            self.cannot_infer(sig, param);
        }
    }

    /// Reports the call of `sig` that binds nothing to its parameter at `param`.
    fn cannot_infer(&mut self, sig: &GenericSig<'_>, param: usize) {
        let name = sig.name;
        // `Some(...)`, or `Option.Some(...)`. A user variant `Some`, which E0026 reports, is
        // neither.
        let option = sig.variant.is_none_or(|_| name.name.as_str() == "Option");
        if option && sig.variant.unwrap_or(name).name.as_str() == "Some" {
            let (callee, at) = match sig.variant {
                Some(variant) => (
                    format!("{}.{}", name.name, variant.name),
                    name.span.to(variant.span),
                ),
                None => (name.name.to_string(), name.span),
            };
            self.report(DiagnosticKind::CannotInferSome { callee, at });
            return;
        }
        // `Ok(...)` and `Err(...)` write the variant as the callee and the enum not at all.
        if let Some(variant) = sig.variant.filter(|v| v.span == name.span) {
            self.report(DiagnosticKind::CannotInferResult {
                param: sig.declared[param].to_string(),
                variant: variant.name.to_string(),
                at: name.span,
            });
            return;
        }
        let dots = vec!["..."; sig.declared.len() - sig.owner.len()].join(", ");
        let (callee, example, at) = match sig.variant {
            Some(variant) => {
                let payload = if sig.params.is_empty() { "" } else { "(...)" };
                (
                    format!("{}.{}", name.name, variant.name),
                    format!("{}<{dots}>.{}{payload}", name.name, variant.name),
                    name.span.to(variant.span),
                )
            }
            None => (
                name.name.to_string(),
                format!("{}<{dots}>(...)", name.name),
                name.span,
            ),
        };
        self.report(DiagnosticKind::CannotInferParam {
            param: sig.declared[param].to_string(),
            callee,
            example,
            at,
            in_result: sig.ret.is_some_and(|ret| names_outside_assoc(ret, param)),
        });
    }

    /// How much `e` needs the context of its call, and whether it can make an optional by itself
    /// (see `Ranked`).
    ///
    /// The value `Waits` when its type cannot come from itself. A bare `[]` or `None` waits, and
    /// so does a payload-free variant of a generic enum. So does a call of a generic function or
    /// method, or a construction of a generic class or variant, its type arguments not written.
    /// Such a call waits when its arguments fit its declaration and all wait.
    ///
    /// Such a call waits also in one more case. Its arguments fit, and the parameter types leave
    /// one of its own type parameters unnamed. The call's type names that parameter, so no
    /// argument can bind it.
    ///
    /// The value is `Literal` when its type comes from the defaults of its integer literals
    /// alone, with values that wait. These are an integer literal, arithmetic over them, or such
    /// a call. Else it is `Concrete`.
    ///
    /// A value that passes its context to its parts has the rank of its parts (see `combine`).
    /// An array literal has the rank of its elements. A call of a generic callee, `dbg` among
    /// them, has the rank of its arguments. An `if`, an `if let` or a `match` has the rank of its
    /// value branches (see `combine_branches`). A block has the rank of its value.
    ///
    /// A name resolves as the checker resolves it: a call's callee by its declaration, whatever
    /// local takes it. A method's receiver, an `if let`'s value and a `match`'s scrutinee are
    /// checked here, ahead of their turn, and kept for it (see `Ahead`). `fit` is cleared when a
    /// call that waits does not fit its declaration: count, labels, and every parameter's and the
    /// return type resolved.
    ///
    /// The answer for a span is kept in `body.ranked`. So a nested generic call, which ranks its
    /// arguments again when it is checked, costs no new walk.
    pub(super) fn rank_value(&mut self, e: &ast::Expr, fit: &mut bool) -> Ranked {
        if let Some(&(ranked, cleared)) = self.body.ranked.get(&e.span) {
            *fit &= !cleared;
            return ranked;
        }
        #[cfg(test)]
        RANK_STEPS.set(RANK_STEPS.get() + 1);
        let mut here = true;
        let ranked = self.rank_walk(e, &mut here);
        *fit &= here;
        self.body.ranked.insert(e.span, (ranked, !here));
        ranked
    }

    /// How much `e` needs the context of the siblings it is checked with, in a join or beside an
    /// operator: its `Ranked::sibling`. A sibling that waits is checked after the others, and
    /// takes their type.
    pub(super) fn sibling_rank(&mut self, e: &ast::Expr) -> Rank {
        self.sibling_ranked(e).0
    }

    /// `sibling_rank`, and whether `e` can make an optional by itself (`Ranked::optional`). A
    /// sibling ranked `Concrete` says `false`: it is not ranked to the end.
    pub(super) fn sibling_ranked(&mut self, e: &ast::Expr) -> (Rank, bool) {
        let mut fit = true;
        let ranked = self.rank_value(e, &mut fit);
        let rank = ranked.sibling(fit);
        (rank, ranked.optional && rank != Rank::Concrete)
    }

    /// Runs `f`, which ranks values and checks them. Then it drops the work that ranking kept
    /// (see `Ahead`) and `f` did not use. A key is kept again only once its last entry is used.
    /// So an earlier entry of a key truncated here, if any, is used, and dropping the key loses
    /// nothing. A placeholder (`keep` with no work) is the exception, but ranking fills each one
    /// before `f` returns.
    pub(super) fn ranked<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let mark = self.body.ahead.len();
        let result = f(self);
        for (key, _) in self.body.ahead.drain(mark..) {
            self.body.ahead_at.remove(&key);
        }
        result
    }

    /// `rank_value`, for a span not ranked yet.
    fn rank_walk(&mut self, e: &ast::Expr, fit: &mut bool) -> Ranked {
        use ast::ExprKind as K;
        match &e.kind {
            _ if bare_none(e) => Ranked {
                optional: true,
                holds_optional: true,
                ..Ranked::WAITS
            },
            _ if int_literal(e) => Ranked::LITERAL,
            K::ArrayLit(elements) => {
                let found = combine(elements.iter().map(|e| self.rank_value(e, fit)));
                Ranked {
                    optional: false,
                    nested: false,
                    scalar: false,
                    ..found
                }
            }
            K::Field {
                base,
                name,
                safe: false,
            } => {
                let waits = self
                    .generic_variant(base, name)
                    .is_some_and(|(_, v)| v.fields.is_empty());
                if waits {
                    Ranked::WAITS
                } else {
                    Ranked::CONCRETE
                }
            }
            K::MethodCall { .. } | K::Call { .. } => match self.call_shape(e) {
                Some((shape, args)) => {
                    let ranked = self.rank_call(shape, args, fit);
                    // The result of a safe call is optional, whatever the method returns.
                    let safe = matches!(e.kind, K::MethodCall { safe: true, .. });
                    Ranked {
                        optional: ranked.optional || safe,
                        holds_optional: ranked.holds_optional || safe,
                        nested: ranked.nested,
                        ..ranked
                    }
                }
                None => Ranked::CONCRETE,
            },
            K::If {
                then_block,
                else_block: Some(else_block),
                ..
            } => {
                let blocks = [then_block, else_block].into_iter();
                combine_branches(blocks.map(|b| self.block_rank(b, fit, |_| None)))
            }
            K::IfLet {
                pattern,
                value,
                then_block,
                else_block: Some(else_block),
            } => self.if_let_rank(pattern, value, then_block, else_block, fit),
            K::Match {
                scrutinee, arms, ..
            } => {
                let form = LetValue::Match {
                    ty: self.value_ahead(scrutinee).filter(|&ty| matchable(ty)),
                };
                combine_branches(arms.iter().map(|arm| {
                    let bind =
                        |c: &mut Self| Some(c.bind_pattern(None, &form, &arm.pattern, Some(arm)));
                    match &arm.body.kind {
                        K::Block(block) => self.block_rank(block, fit, bind),
                        _ => Some(self.tail_rank(arm_key(arm), None, &arm.body, fit, bind)),
                    }
                }))
            }
            K::Block(block) => self
                .block_rank(block, fit, |_| None)
                .unwrap_or(Ranked::CONCRETE),
            _ => Ranked::CONCRETE,
        }
    }

    /// The rank of a call of `shape` with `args`, a call that `rank_walk` found. Only the
    /// arguments at the carried places give the call its type, so only they are ranked.
    fn rank_call(&mut self, shape: CallShape, args: &[ast::Arg], fit: &mut bool) -> Ranked {
        let CallShape {
            generic,
            shaped,
            open,
            makes,
            places,
            ..
        } = shape;
        // The arguments of a call with no carried place do not give it its type. If it is not
        // open, no context fills what it leaves, and `infer_call` reports it.
        if !generic || !open && places.carried == 0 {
            return Ranked::CONCRETE;
        }
        // An argument is walked here and only here: an `open` call walks none, and the others
        // are ranked when `infer_call` checks the call.
        let mut through = false;
        let mut nests = false;
        let mut holds = false;
        let mut passes = true;
        let mut found = if open {
            Ranked::WAITS
        } else {
            let carried = |&(i, _): &(usize, _)| has_place(places.carried, i);
            combine(args.iter().enumerate().filter(carried).map(|(i, arg)| {
                let ranked = self.rank_value(&arg.value, fit);
                through |= makes.makes_optional(i, ranked);
                nests |= makes.passes_nested(i, ranked);
                holds |= ranked.holds_optional && has_place(places.plain, i);
                passes &= ranked.rank != Rank::Literal || makes.returns(i);
                ranked
            }))
        };
        // Only a literal at a place of the returned type parameter passes its type on.
        found.scalar &= passes;
        found.optional = through || makes.wraps();
        found.nested = nests;
        found.holds_optional = holds || found.optional;
        *fit &= found.rank != Rank::Waits || shaped;
        found
    }

    /// The shape of `e`, a call of a function or method, a construction or a variant, its type
    /// arguments not written, with its arguments; `None` for anything else.
    fn call_shape<'e>(&mut self, e: &'e ast::Expr) -> Option<(CallShape, &'e [ast::Arg])> {
        use ast::ExprKind as K;
        // A `push` on a place checks its receiver as the place that it writes. A receiver checked
        // ahead is a value, which leaves the call no place to write: it is not ranked. A branch
        // that ends in such a call is ranked, so this must hold.
        // shortcut: a method named `push` that a class writes with type parameters of its own, on
        // a field, is not ranked as a waiting call; upgrade if such a call needs the context.
        if let K::MethodCall {
            receiver,
            method,
            safe,
            ..
        } = &e.kind
            && method.name.as_str() == "push"
            && self.is_push_place(receiver, *safe)
        {
            return None;
        }
        match &e.kind {
            K::MethodCall {
                receiver,
                method,
                types,
                args,
                safe,
                ..
            } if !types.is_empty() => {
                // Type arguments written for a method fix its own parameters, so the call waits
                // only when several methods answer it, and its context picks one.
                if !self.answered_by_several(receiver, method, *safe) {
                    return None;
                }
                Some((self.generic_method(receiver, method, args, *safe), args))
            }
            K::MethodCall {
                receiver,
                method,
                types,
                args,
                safe,
                ..
            } if types.is_empty() => {
                let variant = self.generic_variant(receiver, method).filter(|_| !safe);
                let shape = if let Some((names, v)) = variant {
                    // Mixed labels are reported at the declaration.
                    let shaped = !v.fields.is_empty()
                        && (v.labels.iter().all(Option::is_some)
                            || v.labels.iter().all(Option::is_none))
                        && fits(v.fields.iter().copied().zip(v.labels.iter().copied()), args);
                    CallShape {
                        generic: true,
                        shaped,
                        open: shaped && leaves_open(v.fields.iter().copied(), names, 0),
                        places: carried_by_construction(v.fields.iter().copied(), names.len()),
                        ..CallShape::default()
                    }
                } else if !safe
                    && method.name.as_str() == "Some"
                    && option_written(receiver) == Some(false)
                {
                    // `Option.Some` is `Some`, as `generic_callee` takes it.
                    CallShape {
                        generic: true,
                        shaped: matches!(&args[..], [arg] if arg.label.is_none()),
                        open: false,
                        some: true,
                        makes: Makes::some(),
                        places: Places::FIRST,
                    }
                } else {
                    self.generic_method(receiver, method, args, *safe)
                };
                Some((shape, args))
            }
            K::Call {
                callee,
                types,
                args,
                ..
            } if types.is_empty() => Some((self.generic_callee(callee.name, args), args)),
            _ => None,
        }
    }

    /// The rank of the `if let pattern = value { then_block } else { else_block }`, as
    /// `combine_branches` decides. Its value is checked once, and kept for it (see
    /// `Ahead::Let`).
    fn if_let_rank(
        &mut self,
        pattern: &ast::Pattern,
        value: &ast::Expr,
        then_block: &ast::Block,
        else_block: &ast::Block,
        fit: &mut bool,
    ) -> Ranked {
        // An argument is not part of an assignment target's chain.
        let target = std::mem::take(&mut self.body.in_target);
        let (checked, form, mismatched) = self.let_value(Unwrap::IfLet, pattern, value);
        self.body.in_target = target;
        let index = self.keep((value.span, false), None);
        let then = self.block_rank(then_block, fit, |c| {
            Some(c.bind_pattern(Some(Unwrap::IfLet), &form, pattern, None))
        });
        self.body.ahead[index].1 = Some(Ahead::Let(checked, form, mismatched));
        let other = iter::once_with(|| self.block_rank(else_block, fit, |_| None));
        combine_branches(iter::once(then).chain(other))
    }

    /// The rank of the value that `block` ends in (see `tail_rank`, which `bind` binds for);
    /// `None` when it ends in an exit, which leaves no value. A block with no value is
    /// `Concrete`.
    pub(super) fn block_rank(
        &mut self,
        block: &ast::Block,
        fit: &mut bool,
        bind: impl FnOnce(&mut Self) -> Option<(LetPattern, Option<Checked>)>,
    ) -> Option<Ranked> {
        match block.stmts.last().map(|stmt| &stmt.kind) {
            Some(StmtKind::Return(_) | StmtKind::Break { .. } | StmtKind::Continue { .. }) => None,
            Some(StmtKind::Expr(tail)) => {
                Some(self.tail_rank(block.span, Some(block), tail, fit, bind))
            }
            _ => Some(Ranked::CONCRETE),
        }
    }

    /// The rank of `tail`, the value of the branch at `key`, in the branch's scope.
    /// Here, once, `bind` binds its names in that scope, if any, and the statements of `block`
    /// before `tail` are checked. The scope is kept for the branch (see `Ahead`). `block` is
    /// `None` for a `match` arm whose body is `tail`.
    pub(super) fn tail_rank(
        &mut self,
        key: Span,
        block: Option<&ast::Block>,
        tail: &ast::Expr,
        fit: &mut bool,
        bind: impl FnOnce(&mut Self) -> Option<(LetPattern, Option<Checked>)>,
    ) -> Ranked {
        // A join ranks a branch that an argument's ranking ranked before: its work is kept.
        if self.body.ranked.contains_key(&tail.span) {
            return self.rank_value(tail, fit);
        }
        self.body.scopes.push(FxHashMap::default());
        // Neither a pattern's guard nor a statement is part of an assignment target's chain.
        let target = std::mem::take(&mut self.body.in_target);
        let bound = bind(self);
        let mut so_far = BlockSoFar::default();
        if let Some(block) = block {
            self.block_statements(block, None, &mut so_far, block.stmts.len() - 1);
        }
        self.body.in_target = target;
        // Its place, held before the value's ranking keeps work of its own.
        let index = self.keep((key, true), None);
        let rank = self.rank_value(tail, fit);
        let scope = self
            .body
            .scopes
            .pop()
            .expect("the branch's scope should be open while its value is ranked");
        self.body.ahead[index].1 = Some(match bound {
            Some((bound, guard)) => Ahead::Bound(bound, guard, scope, so_far),
            None => Ahead::Block(scope, so_far),
        });
        rank
    }

    /// Takes the work kept for `span` (see `Ahead`), a block's if `block`, else a receiver's,
    /// with its place in `body.ahead`.
    pub(super) fn kept(&mut self, span: Span, block: bool) -> Option<(usize, Ahead)> {
        let index = *self.body.ahead_at.get(&(span, block))?;
        #[cfg(test)]
        INSPECTED.set(INSPECTED.get() + 1);
        self.body.ahead[index].1.take().map(|kept| (index, kept))
    }

    /// Keeps `work` in `body.ahead` for `key`, a span and whether it is a block's, and returns
    /// its place there.
    fn keep(&mut self, key: (Span, bool), work: Option<Ahead>) -> usize {
        let index = self.body.ahead.len();
        self.body.ahead.push((key, work));
        self.body.ahead_at.insert(key, index);
        index
    }

    /// The variant `base.member` of a generic enum that `base` names without type arguments,
    /// and the enum's type parameters.
    pub(super) fn generic_variant(
        &self,
        base: &ast::Expr,
        member: &ast::Ident,
    ) -> Option<(&[Symbol], &VariantInfo)> {
        let ast::ExprKind::Name(name) = base.kind else {
            return None;
        };
        let Some(&Declared::Known(id)) = self.enum_ids.get(&name) else {
            return None;
        };
        let info = &self.enums[id.0];
        let &variant = info.index.get(&member.name)?;
        (!info.type_params.is_empty()).then(|| (&info.type_params[..], &info.variants[variant]))
    }

    /// The shape of `receiver.method(args)`, or of `receiver?.method(args)` with `safe`, a call
    /// of a method with type parameters of its own.
    fn generic_method(
        &mut self,
        receiver: &ast::Expr,
        method: &ast::Ident,
        args: &[ast::Arg],
        safe: bool,
    ) -> CallShape {
        let mut ty = self.receiver_ahead(receiver);
        if safe {
            // `?.` calls the method on the payload; on a value that is not optional, which is
            // reported, on the value.
            ty = ty.map(|ty| match ty.kind() {
                TypeKind::Optional(payload) => payload,
                _ => ty,
            });
        }
        let Some(ty) = ty else {
            return CallShape::default();
        };
        match ty.kind() {
            TypeKind::Class(..) | TypeKind::Enum(..) => {}
            TypeKind::Param(..) | TypeKind::Assoc(..) => return self.bound_shape(ty, method, args),
            _ => return CallShape::default(),
        }
        // The method the call will find, as `find_method` finds it, without its reports.
        let func = match self.applicable_method(ty, method.name) {
            Some(Declared::Known(func)) => func,
            Some(Declared::Broken) => return CallShape::default(),
            None => {
                self.conforming_methods(ty, method.name);
                match self.candidates[..] {
                    [] => return CallShape::default(),
                    [(func, ..)] => func,
                    // The call's context picks among several (`pick_conforming`): it waits for
                    // it, as a call whose arguments cannot bind its parameters does.
                    _ => {
                        // Candidates that all return one known type give the call that type: the
                        // context cannot pick among them, and the call reports it at once.
                        let first = self.signatures[self.candidates[0].0.0].ret;
                        let alike = first.is_some_and(|ret| !ret.has_param())
                            && (self.candidates.iter())
                                .all(|&(func, ..)| self.signatures[func.0].ret == first);
                        if alike {
                            return CallShape::default();
                        }
                        let shaped = self.candidates.iter().all(|&(func, ..)| {
                            let sig = &self.signatures[func.0];
                            sig.ret.is_some() && fits(sig.params.iter().map(|&ty| (ty, None)), args)
                        });
                        return CallShape {
                            generic: true,
                            shaped,
                            open: shaped,
                            ..CallShape::default()
                        };
                    }
                }
            }
        };
        let sig = &self.signatures[func.0];
        // The receiver binds the owner's type parameters: its class's, or its block's.
        let owner = sig.type_params.len() - sig.own;
        let shaped = sig.ret.is_some() && fits(sig.params.iter().map(|&ty| (ty, None)), args);
        let ret = sig.ret.filter(|_| shaped);
        let open = is_open(sig.params.iter().copied(), ret, &sig.type_params, owner);
        CallShape {
            generic: sig.own > 0,
            shaped,
            open,
            makes: Makes::of(sig.ret, sig.params.iter().copied()),
            places: carried(
                sig.ret,
                sig.params.iter().copied(),
                sig.type_params.len(),
                owner,
            ),
            ..CallShape::default()
        }
    }

    /// Whether several methods answer `receiver.method(...)`, or `receiver?.method(...)` with
    /// `safe`: the requirements of a type parameter's bounds, or the methods of the conformance
    /// blocks of a class or an enum.
    fn answered_by_several(
        &mut self,
        receiver: &ast::Expr,
        method: &ast::Ident,
        safe: bool,
    ) -> bool {
        let Some(mut ty) = self.receiver_ahead(receiver) else {
            return false;
        };
        if let (true, TypeKind::Optional(payload)) = (safe, ty.kind()) {
            ty = payload;
        }
        match ty.kind() {
            TypeKind::Param(..) | TypeKind::Assoc(..) => self.required(ty, method.name).len() > 1,
            TypeKind::Class(..) | TypeKind::Enum(..) => {
                if self.applicable_method(ty, method.name).is_some() {
                    return false;
                }
                self.conforming_methods(ty, method.name);
                self.candidates.len() > 1
            }
            _ => false,
        }
    }

    /// The shape of a call of `method` on a value whose type `receiver` is a type parameter or an
    /// associated type: the requirements its bounds give. One is a generic call if it has type
    /// parameters of its own. Several wait for the context that picks one, unless all return one
    /// type.
    fn bound_shape(&mut self, receiver: Type, method: &ast::Ident, args: &[ast::Arg]) -> CallShape {
        let found = self.required(receiver, method.name);
        let requirement = |found: &Required| {
            let interface = &self.interfaces[found.interface.0];
            &interface.requirements[found.method]
        };
        let shaped = |found: &Required| {
            let requirement = requirement(found);
            requirement.ret.is_some() && fits(requirement.params.iter().map(|&ty| (ty, None)), args)
        };
        match &found[..] {
            [] => CallShape::default(),
            [one] => {
                let interface = &self.interfaces[one.interface.0];
                let requirement = &interface.requirements[one.method];
                let shaped = shaped(one);
                let mut names = interface.type_params.clone();
                names.extend_from_slice(&requirement.own);
                let ret = requirement.ret.filter(|_| shaped);
                let params = requirement.params.iter().copied();
                let owner = interface.type_params.len();
                let open = is_open(params, ret, &names, owner);
                CallShape {
                    generic: !requirement.own.is_empty(),
                    shaped,
                    open,
                    makes: Makes::of(requirement.ret, requirement.params.iter().copied()),
                    places: carried(
                        requirement.ret,
                        requirement.params.iter().copied(),
                        names.len(),
                        owner,
                    ),
                    ..CallShape::default()
                }
            }
            _ => {
                // The results for this receiver, where only the requirement's own parameters
                // are open: `Self` is the receiver, whatever it is.
                let results: Vec<Option<Type>> = (found.iter())
                    .map(|one| {
                        let ret = requirement(one).ret?;
                        self.requirement_result(receiver, one, ret, &[])
                    })
                    .collect();
                let alike = results[0].is_some() && results.iter().all(|ret| *ret == results[0]);
                if alike {
                    return CallShape::default();
                }
                let shaped = found.iter().all(shaped);
                CallShape {
                    generic: true,
                    shaped,
                    open: shaped,
                    ..CallShape::default()
                }
            }
        }
    }

    /// The type of `receiver`, a method's, checked ahead of the call and kept for `member`.
    /// `None` for a name no local takes: a path, which `type_path` checks, or an unknown name.
    fn receiver_ahead(&mut self, receiver: &ast::Expr) -> Option<Type> {
        match receiver.kind {
            ast::ExprKind::Name(name) if self.lookup(name).is_none() => return None,
            ast::ExprKind::Applied { .. } => return None,
            _ => {}
        }
        self.value_ahead(receiver)
    }

    /// `e` checked as a value: as ranking checked it ahead, a method's receiver or a `match`'s
    /// scrutinee (see `value_ahead`), else now.
    pub(super) fn value_kept(&mut self, e: &ast::Expr) -> Checked {
        match self.kept(e.span, false) {
            Some((_, Ahead::Receiver(checked))) => checked,
            _ => self.value(e, None),
        }
    }

    /// The type of `e`, checked ahead of its turn and kept for it as `Ahead::Receiver`.
    fn value_ahead(&mut self, e: &ast::Expr) -> Option<Type> {
        let kept = self.body.ahead_at.get(&(e.span, false)).and_then(|&index| {
            #[cfg(test)]
            INSPECTED.set(INSPECTED.get() + 1);
            self.body.ahead[index].1.as_ref()
        });
        if let Some(Ahead::Receiver(checked)) = kept {
            return type_of(checked);
        }
        // An argument is not part of an assignment target's chain.
        let target = std::mem::take(&mut self.body.in_target);
        let checked = self.value(e, None);
        self.body.in_target = target;
        let ty = type_of(&checked);
        self.keep((e.span, false), Some(Ahead::Receiver(checked)));
        ty
    }

    /// The shape of a call of `name` with `args` that calls a generic function, constructs a
    /// generic class, or builds `Some`, `Ok` or `Err`, or calls `dbg`.
    fn generic_callee(&self, name: Symbol, args: &[ast::Arg]) -> CallShape {
        // `dbg<T>(x: T) -> T`: its type is its argument's, so it waits when that argument waits;
        // its parameter names `T`, so it is not `open`.
        // It comes first, as `call` runs the prelude's call before it looks up a user's item: a
        // user's `class dbg` or `fn dbg` is rejected, and never the callee.
        if name.as_str() == "dbg" {
            return CallShape {
                generic: true,
                shaped: matches!(args, [arg] if arg.label.is_none()),
                makes: Makes::Through {
                    bare: 1,
                    inside: 0,
                    lifted: 0,
                    wraps: false,
                },
                places: Places::FIRST,
                ..CallShape::default()
            };
        }
        match (self.class_ids.get(&name), self.function_ids.get(&name)) {
            (Some(&Declared::Known(class)), _) => {
                let info = &self.classes[class.0];
                let shaped = fits(info.fields.iter().map(|f| (f.ty, Some(f.name.name))), args);
                CallShape {
                    generic: !info.type_params.is_empty(),
                    shaped,
                    open: shaped
                        && leaves_open(info.fields.iter().map(|f| f.ty), &info.type_params, 0),
                    places: carried_by_construction(
                        info.fields.iter().map(|f| f.ty),
                        info.type_params.len(),
                    ),
                    ..CallShape::default()
                }
            }
            (None, Some(&Declared::Known(func))) => {
                let sig = &self.signatures[func.0];
                let shaped =
                    sig.ret.is_some() && fits(sig.params.iter().map(|&ty| (ty, None)), args);
                let ret = sig.ret.filter(|_| shaped);
                let open = is_open(sig.params.iter().copied(), ret, &sig.type_params, 0);
                CallShape {
                    generic: !sig.type_params.is_empty(),
                    shaped,
                    open,
                    makes: Makes::of(sig.ret, sig.params.iter().copied()),
                    places: carried(
                        sig.ret,
                        sig.params.iter().copied(),
                        sig.type_params.len(),
                        0,
                    ),
                    ..CallShape::default()
                }
            }
            // `Some<T>(x: T) -> T?`, and the prelude's `Ok` and `Err`, which no user item can
            // take the name of.
            (None, None) if name.as_str() == "Some" => CallShape {
                generic: true,
                shaped: matches!(args, [arg] if arg.label.is_none()),
                open: false,
                some: true,
                makes: Makes::some(),
                places: Places::FIRST,
            },
            (None, None) => self
                .result_variant(name)
                .map_or_else(CallShape::default, |(id, v)| {
                    let info = &self.enums[id.0];
                    let shaped = matches!(args, [arg] if arg.label.is_none());
                    CallShape {
                        generic: true,
                        shaped,
                        open: shaped
                            && leaves_open(
                                info.variants[v].fields.iter().copied(),
                                &info.type_params,
                                0,
                            ),
                        places: carried_by_construction(
                            info.variants[v].fields.iter().copied(),
                            info.type_params.len(),
                        ),
                        ..CallShape::default()
                    }
                }),
            _ => CallShape::default(),
        }
    }

    /// The type parameters `owner`, a method's class's or enum's, then `own`, a declaration's.
    /// Reports each of `own` that takes a built-in type's name, or the name of one before it;
    /// `owner`'s are reported where they are declared. Every name keeps its place, so that a
    /// `Param`'s index is its place.
    pub(super) fn type_params(
        &mut self,
        owner: &[ast::TypeParam],
        own: &[ast::TypeParam],
    ) -> Vec<Symbol> {
        let mut first = FxHashMap::default();
        for param in owner {
            first
                .entry(param.name.name)
                .or_insert((param.name.span, Definition::Type));
        }
        self.own_type_params(&mut first, own);
        owner
            .iter()
            .chain(own)
            .map(|param| param.name.name)
            .collect()
    }

    /// Reports each of `own` that takes a built-in type's name, or a name in `first`, where
    /// `first` holds the type parameters before them; `own` join `first`.
    pub(super) fn own_type_params(
        &mut self,
        first: &mut FxHashMap<Symbol, (Span, Definition)>,
        own: &[ast::TypeParam],
    ) {
        for param in own {
            if !self.built_in(&param.name) {
                self.first_definition(first, Definition::Type, None, &param.name);
            }
        }
    }

    /// The type arguments `written` in `angles` after `name`, whose declaration's type
    /// parameters are `declared`. `None`, reported, when they do not fit `declared`, or one is
    /// `Error` or cannot be a type argument. A list after a name that takes none is reported,
    /// and the name stands without it.
    pub(super) fn type_args(
        &mut self,
        name: &ast::Ident,
        declared: &[Symbol],
        written: &[ast::TypeExpr],
        angles: Span,
    ) -> Option<TypeArgs> {
        self.type_args_of(name, declared, written, angles, true)
    }

    /// `type_args`, where `instantiates` says whether a list that fits records a type argument
    /// (see `tir::Program::instantiates`). An interface's list records none: `mono` copies no
    /// interface, so a program with generic interfaces and no generic item takes its fast path.
    /// A type argument that is itself an applied type records its own.
    pub(super) fn type_args_of(
        &mut self,
        name: &ast::Ident,
        declared: &[Symbol],
        written: &[ast::TypeExpr],
        angles: Span,
        instantiates: bool,
    ) -> Option<TypeArgs> {
        let form = match (declared.len(), written.len()) {
            (0, 0) => return Some(TypeArgs::NONE),
            (0, _) => {
                self.not_generic(name.name, angles);
                return Some(TypeArgs::NONE);
            }
            (_, 0) => ArgumentsForm::Missing {
                params: declared.iter().map(Symbol::to_string).collect(),
            },
            (want, found) if want != found => ArgumentsForm::Count { want, found },
            _ => {
                // Each argument is checked, so this collects into a `Vec` before it gives up.
                let args: Vec<_> = written.iter().map(|arg| self.type_argument(arg)).collect();
                self.instantiates |= instantiates;
                return args
                    .into_iter()
                    .collect::<Option<Vec<_>>>()
                    .map(|args| TypeArgs::intern(&args));
            }
        };
        let at = match form {
            ArgumentsForm::Missing { .. } => name.span,
            _ => angles,
        };
        self.report(DiagnosticKind::TypeArguments {
            name: name.name.to_string(),
            at,
            form,
        });
        None
    }

    /// Reports the type arguments in `angles` after `name`, which takes none: the fix removes
    /// them.
    pub(super) fn not_generic(&mut self, name: Symbol, angles: Span) {
        let safe = !self.comment_in(&[angles.range()]);
        self.report(DiagnosticKind::TypeArguments {
            name: name.to_string(),
            at: angles,
            form: ArgumentsForm::NotGeneric { safe },
        });
    }

    /// The type of the type argument `arg`: `None`, reported, for `unit` or `never`, which no
    /// type parameter takes.
    fn type_argument(&mut self, arg: &ast::TypeExpr) -> Option<Type> {
        if let ast::TypeKind::Name(name) = arg.kind
            && matches!(name.as_str(), "unit" | "never")
            && self.body.type_param(name).is_none()
            && !self.class_ids.contains_key(&name)
            && !self.enum_ids.contains_key(&name)
        {
            self.report(DiagnosticKind::TypeArgument {
                ty: arg.to_string(),
                at: arg.span,
            });
            return None;
        }
        self.resolve(arg)
    }

    /// Reports `main`, which has type parameters. The fix removes them, unless a comment is
    /// among them or a name in the rest of `main`, outside its strings and comments, is one.
    pub(super) fn main_type_params(&mut self, main: &ast::Function) {
        let angles = main.angles;
        let rest = self.source.slice(angles.end()..main.span.end());
        // What does not lex alone is held to name one.
        let named = lex(rest).map_or(true, |lexed| {
            lexed.tokens.iter().any(|token| {
                matches!(token.kind, TokenKind::Ident(name)
                    if main.type_params.iter().any(|p| p.name.name == name))
            })
        });
        let remove = (!named && !self.comment_in(&[angles.range()])).then_some(angles);
        self.report(DiagnosticKind::MainSignature {
            at: angles,
            form: MainForm::TypeParams { remove },
        });
    }

    /// Reports `op` on the operand at `at`, of type `ty`, which names a type parameter that lacks
    /// the operator.
    pub(super) fn param_lacks(&mut self, at: Span, ty: Type, op: impl fmt::Display) {
        let d = self.lacks(at, ty, op);
        self.diagnostics.push(d);
    }

    /// The report that `param_lacks` makes.
    pub(super) fn lacks(&self, at: Span, ty: Type, op: impl fmt::Display) -> Diagnostic {
        DiagnosticKind::ParamLacks {
            ty: self.type_name(ty),
            op: op.to_string(),
            at,
            add: None,
        }
        .into()
    }

    /// Reports `name<...>`, a value with type arguments in `angles`: the `<` and `>` were
    /// comparisons. `name` is the value as written, a name or a field, and `first` the first
    /// type argument, which the help puts in parentheses with `name`, when a `(` follows the list
    /// and `first` could be a value: a name that is not a built-in type, with no `?`.
    pub(super) fn misread(&mut self, name: Span, first: Option<&ast::TypeExpr>) -> Poisoned {
        let first = first.filter(|first| self.could_be_value(first));
        let compare = first.map(|first| self.source.slice(name.start()..first.span.end()).into());
        self.misread_as(name, compare)
    }

    /// Whether the type argument `arg` could be a value: a local, or a name that names no type.
    pub(super) fn could_be_value(&self, arg: &ast::TypeExpr) -> bool {
        let ast::TypeKind::Name(n) = arg.kind else {
            return false;
        };
        arg.optional == 0 && self.body.scopes.iter().any(|scope| scope.contains_key(&n))
            || self.names_no_type(arg)
    }

    /// Whether the type argument `arg` is a name, with no `?`, that is no built-in type, class,
    /// enum or type parameter.
    pub(super) fn names_no_type(&self, arg: &ast::TypeExpr) -> bool {
        let ast::TypeKind::Name(n) = arg.kind else {
            return false;
        };
        arg.optional == 0
            && built_in_type(n.as_str()).is_none()
            && !self.class_ids.contains_key(&n)
            && !self.enum_ids.contains_key(&n)
            && self.body.type_param(n).is_none()
    }

    /// `misread` with the comparison `compare` its help writes, if any.
    pub(super) fn misread_as(&mut self, name: Span, compare: Option<String>) -> Poisoned {
        self.body.misread.insert(name.start());
        self.report(DiagnosticKind::TypeArguments {
            name: self.source.slice(name.start()..name.end()).to_string(),
            at: name,
            form: ArgumentsForm::Value { compare },
        });
        Poisoned::error()
    }

    /// Marks `span`, an `a<b, c>(d)` whose type arguments `types` took a comma, as misread when
    /// it does not check and the first and last of `types` name no type: the comma may be its
    /// argument list's, whose count is then not reported (see `misread_in`).
    pub(super) fn misread_unless_checked(
        &mut self,
        span: Span,
        types: &[ast::TypeExpr],
        checked: &Checked,
    ) {
        // Only a list whose first and last argument name no type could join `a <` and `> (d)`.
        if let [first, .., last] = types
            && checked.is_err()
            && self.names_no_type(first)
            && self.names_no_type(last)
        {
            self.body.misread.insert(span.start());
        }
    }

    /// The type arguments `types`, in `angles`, written after `name` in an expression, which
    /// are reported unless a generic declaration takes them. `Some` when `name` is a value,
    /// whose `<` and `>` were comparisons: the expression is poisoned. `call` when a `(`
    /// follows the list, rather than a `.`.
    pub(super) fn named_args(
        &mut self,
        name: &ast::Ident,
        types: &[ast::TypeExpr],
        angles: Span,
        call: bool,
    ) -> Option<Poisoned> {
        let first = types.first()?;
        let n = name.name;
        let params = match (
            self.class_ids.get(&n),
            self.function_ids.get(&n),
            self.enum_ids.get(&n),
        ) {
            (Some(&Declared::Known(id)), ..) => Some(self.classes[id.0].type_params.len()),
            (_, Some(&Declared::Known(id)), _) => Some(self.signatures[id.0].type_params.len()),
            (.., Some(&Declared::Known(id))) => Some(self.enums[id.0].type_params.len()),
            _ => None,
        };
        match params {
            Some(0) => self.not_generic(n, angles),
            None if PRELUDE.contains(&n.as_str()) || built_in_type(n.as_str()).is_some() => {
                self.not_generic(n, angles);
            }
            None if self.lookup(n).is_some()
                || matches!(self.constants.get(&n), Some(Declared::Known(_))) =>
            {
                return Some(self.misread(name.span, call.then_some(first)));
            }
            // A generic's call or use checks them; a broken or unknown name's use reports it.
            _ => {}
        }
        None
    }

    /// Whether `receiver` before `.method` is a path, not a value: an enum, `Option`, or `math`
    /// or a built-in type that no local takes.
    pub(super) fn path_method(&mut self, receiver: &ast::Expr, method: &ast::Ident) -> bool {
        let name = match &receiver.kind {
            ast::ExprKind::Name(name) => *name,
            ast::ExprKind::Applied { name, .. } => name.name,
            _ => return false,
        };
        // No item can take the name of the optional type, whose members are its variants.
        if name.as_str() == "Option" {
            return true;
        }
        if let Some(&Declared::Known(id)) = self.enum_ids.get(&name) {
            // A generic method's list waits for the member's report, which says it is a method.
            let info = &self.enums[id.0];
            return match info.methods.get(&method.name) {
                Some(&Declared::Known(func)) => {
                    self.signatures[func.0].type_params.len() == info.type_params.len()
                }
                _ => true,
            };
        }
        (built_in_type(name.as_str()).is_some() || matches!(name.as_str(), "math" | "Math"))
            && self.lookup(name).is_none()
    }

    /// Reports the type arguments in `angles` of `receiver.method<first, ...>`, `receiver`
    /// checked from `receiver_expr`: a method that takes none, or a field, a value whose `<` and
    /// `>` were comparisons, which is `Some` and poisons the expression. A member the receiver
    /// does not have is reported as that alone, and a built-in receiver's list is left to the
    /// caller.
    pub(super) fn method_args(
        &mut self,
        receiver: &Checked,
        receiver_expr: &ast::Expr,
        method: &ast::Ident,
        first: &ast::TypeExpr,
        angles: Span,
    ) -> Option<Poisoned> {
        let ty = type_of(receiver)?;
        let field = match ty.kind() {
            TypeKind::Class(id, _) => self.classes[id.0].field(method.name).is_some(),
            TypeKind::Enum(..) => false,
            // A built-in's list is reported once its method is found.
            _ => return None,
        };
        match self.own_method(ty, method.name) {
            // A method with parameters of its own takes the list at its call.
            Some(Declared::Known(func)) if self.signatures[func.0].own > 0 => {}
            Some(Declared::Known(_)) => self.not_generic(method.name, angles),
            None if field => {
                return Some(self.misread(receiver_expr.span.to(method.span), Some(first)));
            }
            _ => {}
        }
        None
    }

    /// Reports the type arguments in `angles` after `variant`, a variant of the generic enum
    /// `base` names. The enum takes them, so the help writes them on the enum. There is no help
    /// when one of `types` names no type: the list may be a comparison.
    pub(super) fn variant_args(
        &mut self,
        base: &ast::Expr,
        variant: &ast::Ident,
        types: &[ast::TypeExpr],
        angles: Span,
    ) {
        let text = self.source;
        let instead = types.iter().all(|ty| !self.names_no_type(ty)).then(|| {
            format!(
                "{}{}.{}",
                text.text(base.span),
                text.text(angles),
                variant.name
            )
        });
        self.report(DiagnosticKind::TypeArguments {
            name: text
                .slice(base.span.start()..variant.span.end())
                .to_string(),
            at: angles,
            form: ArgumentsForm::Variant { instead },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::InterfaceId;

    /// The type of `S.Item`, where `S` is the parameter at place 0.
    fn item() -> Type {
        let s = Type::intern(TypeKind::Param(Symbol::intern("S"), 0));
        Type::intern(TypeKind::Assoc(s, InterfaceId(0), TypeArgs::NONE, 0))
    }

    /// An associated type of a parameter binds nothing: what it stands for is known once the
    /// parameter is bound, and an argument is then checked against it. A binding of the
    /// parameter by the type of an argument would take the item for the type of the source.
    #[test]
    fn an_associated_type_pattern_should_bind_nothing() {
        let mut bindings = Bindings::new(1, &[]);
        assert!(bindings.unify(item(), Type::I64));
        assert!(bindings.unify(Type::optional(item()), Type::optional(Type::I64)));
        assert_eq!(bindings.finish(), Err(0));
        // It does not hide a parameter that is bound beside it.
        let s = Type::intern(TypeKind::Param(Symbol::intern("S"), 0));
        let two = Type::array(s);
        assert!(bindings.unify(two, Type::array(Type::STRING)));
        assert_eq!(
            bindings.finish().map(|list| list.get().to_vec()),
            Ok(vec![Type::STRING])
        );
        // The type waits for its base, and then names it.
        assert_eq!(
            bindings.known(item()),
            Some(Type::intern(TypeKind::Assoc(
                Type::STRING,
                InterfaceId(0),
                TypeArgs::NONE,
                0
            )))
        );
    }

    /// An associated type of a parameter that is not bound is the hole as a whole, and is not known
    /// while its base or an argument of its interface is unbound; `printed` keeps the names.
    #[test]
    fn an_associated_type_should_wait_for_its_base_and_its_arguments() {
        let param = |name, place| Type::intern(TypeKind::Param(Symbol::intern(name), place));
        let (s, t) = (param("S", 0), param("T", 1));
        let item = Type::intern(TypeKind::Assoc(s, InterfaceId(0), TypeArgs::NONE, 0));
        let with_arg = Type::intern(TypeKind::Assoc(
            s,
            InterfaceId(0),
            TypeArgs::intern(&[t]),
            0,
        ));
        let mut bindings = Bindings::new(2, &[]);
        assert_eq!(bindings.partial(item), *HOLE);
        assert_eq!(bindings.known(item), None);
        assert_eq!(bindings.printed(item), item);
        assert!(bindings.unify(s, Type::STRING));
        // The base is bound, and the argument is not: the type waits, and a hole stands for it.
        assert_eq!(bindings.known(with_arg), None);
        assert_ne!(bindings.partial(with_arg), *HOLE);
        assert!(bindings.partial(with_arg).holds_hole());
        assert!(bindings.unify(t, Type::I64));
        let string_item = Type::intern(TypeKind::Assoc(
            Type::STRING,
            InterfaceId(0),
            TypeArgs::intern(&[Type::I64]),
            0,
        ));
        assert_eq!(bindings.known(with_arg), Some(string_item));
    }
}
