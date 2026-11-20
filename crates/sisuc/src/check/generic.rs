//! Generic declarations: type parameters, the type arguments written for them or inferred at
//! a call, and what a value of a type parameter may not do.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::hash::Hash;
use std::rc::Rc;
use std::sync::LazyLock;
use std::{fmt, iter};

use rustc_hash::{FxHashMap, FxHashSet};

use super::alias::{Through, Via};
use super::array::innermost;
use super::expr::{nested_join, option_written, read_jump};
use super::infer::{Kind, ObligationId, OpenCall};
use super::interface::{Bound, Bounds, Required};
use super::join::{Heading, JoinForm, JoinMember, Place, part_at, target};
use super::member::FieldOwner;
use super::mut_args::{MutParts, MutSite, PlaceShape, marked};
use super::names::{self, ItemId, Resolved};
use super::pattern::{LetPattern, LetValue, arm_key, matchable};
use super::prelude::PRELUDE;
use super::reports::{Held, Reported};
use super::stmt::BlockSoFar;
use super::{
    Binding, Checked, Checker, Declared, Definition, DiagnosticKind, Expectation, Origin, Poisoned,
    block_value, built_in_type, is_kept, own_args, payload, tail, type_of,
};
use crate::ast::{self, StmtKind};
use crate::diagnostic::{ArgumentsForm, Diagnostic, Expected, MainForm, NoValueCause, Unwrap};
use crate::lexer::{TokenKind, lex};
use crate::parser::is_place;
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, Type, TypeArgs, TypeKind};
use crate::types::{HOLE_INDEX, Head, MARK_INDEX, REJECTED_INDEX};

/// How many visits a walk makes before its `WalkMemo` keeps answers. The types of a normal call
/// are small, so such a walk makes no entry and does no hashing.
const MEMO_AFTER: usize = 64;

/// The answers of one walk over types, by the type (or pair of types) it has met. A type can
/// share its children, so it has more paths than distinct types: an alias that names the one
/// before it twice has 2^n paths for n types. A walk that asks its `WalkMemo` before it visits a
/// type, and that puts its answer after, visits each distinct type once. A walk whose answer to a
/// question is the same on each visit within one top-level call can use one. A set is a `WalkMemo`
/// of `()`.
struct WalkMemo<K, V> {
    visits: usize,
    /// Made at the first answer, so a walk that keeps none allocates and hashes nothing.
    answers: Option<FxHashMap<K, V>>,
}

impl<K, V> Default for WalkMemo<K, V> {
    fn default() -> Self {
        WalkMemo {
            visits: 0,
            answers: None,
        }
    }
}

impl<K: Eq + Hash, V: Copy> WalkMemo<K, V> {
    /// The answer that the walk put for `key`, if any. It counts one visit.
    fn get(&mut self, key: &K) -> Option<V> {
        self.visits += 1;
        self.answers.as_ref()?.get(key).copied()
    }

    /// Keeps `answer` for `key`, once the walk has made `MEMO_AFTER` visits.
    fn put(&mut self, key: K, answer: V) {
        if self.visits > MEMO_AFTER {
            self.answers.get_or_insert_default().insert(key, answer);
        }
    }

    /// Forgets each answer, for a new walk.
    fn clear(&mut self) {
        self.visits = 0;
        if let Some(answers) = &mut self.answers {
            answers.clear();
        }
    }
}

/// Marks in `named` each type parameter that `ty` names, by its place; `named` has a place for
/// each type parameter in scope. `seen` holds each type already visited, so each distinct type of
/// `ty` is visited once, whatever the number of paths to it. It can be shared by several types.
fn mark_params(ty: Type, named: &mut [bool], seen: &mut WalkMemo<Type, ()>) {
    #[cfg(test)]
    MARKED.set(MARKED.get() + 1);
    type_step();
    // A type without a parameter names none: its subtree is not walked again at each call.
    if !ty.has_param() {
        return;
    }
    if let TypeKind::Param(_, j) = ty.kind() {
        named[j as usize] = true;
        return;
    }
    if seen.get(&ty).is_some() {
        return;
    }
    match ty.kind() {
        TypeKind::Array(inner) | TypeKind::Optional(inner) => mark_params(inner, named, seen),
        TypeKind::Class(_, args)
        | TypeKind::Struct(_, args)
        | TypeKind::Enum(_, args)
        | TypeKind::Any(_, args) => {
            for &arg in args.get() {
                mark_params(arg, named, seen);
            }
        }
        TypeKind::Assoc(base, _, args, _) => {
            mark_params(base, named, seen);
            for &arg in args.get() {
                mark_params(arg, named, seen);
            }
        }
        TypeKind::I64
        | TypeKind::U8
        | TypeKind::F64
        | TypeKind::Bool
        | TypeKind::Char
        | TypeKind::String
        | TypeKind::Unit
        | TypeKind::Never
        | TypeKind::Infer(_)
        | TypeKind::Param(..) => unreachable!("only a kind with a child can name a parameter"),
    }
    seen.put(ty, ());
}

/// Where `params_named` looks for the type parameters that a type names.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reach {
    /// Under an associated type too.
    Everywhere,
    /// Only outside each associated type, as `names_outside_assoc` does.
    OutsideAssoc,
}

/// The places of the type parameters that `ty` names where `reach` looks, each once, in order.
/// One walk finds them all: `names_outside_assoc` for each place costs the number of places
/// times the size of `ty`.
fn params_named(ty: Type, reach: Reach) -> Vec<usize> {
    fn walk(ty: Type, reach: Reach, found: &mut Vec<usize>, seen: &mut WalkMemo<Type, ()>) {
        type_step();
        if !ty.has_param() {
            return;
        }
        if let TypeKind::Param(_, j) = ty.kind() {
            found.push(j as usize);
            return;
        }
        if seen.get(&ty).is_some() {
            return;
        }
        match ty.kind() {
            TypeKind::Array(inner) | TypeKind::Optional(inner) => walk(inner, reach, found, seen),
            TypeKind::Class(_, args)
            | TypeKind::Struct(_, args)
            | TypeKind::Enum(_, args)
            | TypeKind::Any(_, args) => {
                for &arg in args.get() {
                    walk(arg, reach, found, seen);
                }
            }
            TypeKind::Assoc(base, _, args, _) => {
                if reach == Reach::Everywhere {
                    walk(base, reach, found, seen);
                    for &arg in args.get() {
                        walk(arg, reach, found, seen);
                    }
                }
            }
            _ => unreachable!("only a kind with a child can name a parameter"),
        }
        seen.put(ty, ());
    }
    let mut found = Vec::new();
    walk(ty, reach, &mut found, &mut WalkMemo::default());
    found.sort_unstable();
    found.dedup();
    found
}

/// Whether the result type of `sig` names each of its type parameters other than under an
/// associated type, found in one walk.
fn result_names(sig: &GenericSig<'_>) -> Vec<bool> {
    let mut named = vec![false; sig.declared.len()];
    for k in sig
        .ret
        .map_or_else(Vec::new, |ret| params_named(ret, Reach::OutsideAssoc))
    {
        named[k] = true;
    }
    named
}

/// The value of `e` through each block that has no statement but its value, `e` itself for any
/// other expression.
fn lone_value(mut e: &ast::Expr) -> &ast::Expr {
    while let ast::ExprKind::Block(block) = &e.kind
        && let [_] = block.stmts.as_slice()
        && let Some(value) = tail(block)
    {
        e = value;
    }
    e
}

/// Whether `ty` names the type parameter at place `param` other than under an associated type:
/// only such a name lets the type that a result is annotated with bind the parameter.
fn names_outside_assoc(ty: Type, param: usize) -> bool {
    names_outside_in(ty, param, &mut WalkMemo::default())
}

/// The walk of `names_outside_assoc`; `seen` holds each type that does not, so each distinct type
/// is visited once.
fn names_outside_in(ty: Type, param: usize, seen: &mut WalkMemo<Type, ()>) -> bool {
    type_step();
    if !ty.has_param() {
        return false;
    }
    if let TypeKind::Param(_, j) = ty.kind() {
        return j as usize == param;
    }
    if seen.get(&ty).is_some() {
        return false;
    }
    let names = match ty.kind() {
        TypeKind::Array(inner) | TypeKind::Optional(inner) => names_outside_in(inner, param, seen),
        TypeKind::Class(_, args)
        | TypeKind::Struct(_, args)
        | TypeKind::Enum(_, args)
        | TypeKind::Any(_, args) => {
            (args.get().iter()).any(|&arg| names_outside_in(arg, param, seen))
        }
        TypeKind::Assoc(..) => false,
        _ => unreachable!("only a kind with a child can name a parameter"),
    };
    if !names {
        seen.put(ty, ());
    }
    names
}

/// Marks in `outside` each type parameter that `ty` names other than under an associated type,
/// and in `inside` each that it names under one, as the base or an interface argument of it.
/// Each is by its place. One walk finds both: `names_outside_assoc` for each place would cost
/// the number of places times the size of `ty`. `seen` holds each type already visited, with its
/// `under`, so each distinct type is visited once.
fn mark_by_assoc(
    ty: Type,
    under: bool,
    inside: &mut [bool],
    outside: &mut [bool],
    seen: &mut WalkMemo<(Type, bool), ()>,
) {
    type_step();
    if !ty.has_param() {
        return;
    }
    if let TypeKind::Param(_, j) = ty.kind() {
        let marks = if under { inside } else { outside };
        marks[j as usize] = true;
        return;
    }
    if seen.get(&(ty, under)).is_some() {
        return;
    }
    match ty.kind() {
        TypeKind::Array(t) | TypeKind::Optional(t) => {
            mark_by_assoc(t, under, inside, outside, seen);
        }
        TypeKind::Class(_, args)
        | TypeKind::Struct(_, args)
        | TypeKind::Enum(_, args)
        | TypeKind::Any(_, args) => {
            for &arg in args.get() {
                mark_by_assoc(arg, under, inside, outside, seen);
            }
        }
        TypeKind::Assoc(base, _, args, _) => {
            mark_by_assoc(base, true, inside, outside, seen);
            for &arg in args.get() {
                mark_by_assoc(arg, true, inside, outside, seen);
            }
        }
        _ => unreachable!("only a kind with a child can name a parameter"),
    }
    seen.put((ty, under), ());
}

/// Which of the first `len` type parameters `types` name. A type that did not resolve names
/// nothing.
pub(super) fn named_params(types: impl Iterator<Item = Option<Type>>, len: usize) -> Vec<bool> {
    let mut named = vec![false; len];
    let mut seen = WalkMemo::default();
    for ty in types.flatten() {
        mark_params(ty, &mut named, &mut seen);
    }
    named
}

/// Whether the type parameter `name` is broken, as `type_params` reports it: it takes the name
/// of one in `seen`, which then holds it, or of a built-in type. A call poisons such a parameter
/// rather than leave it uninferred, and no call waits for it.
fn broken_param(seen: &mut FxHashSet<Symbol>, name: Symbol) -> bool {
    !seen.insert(name) || built_in_type(name.as_str()).is_some()
}

/// The binding of a broken type parameter, or of an unbound or rejected one as a bound sees it.
const NO_BINDING: Type = Type::NEVER;

/// Finds each of `names`, a declaration's type parameters, that is broken (see `broken_param`)
/// and that nothing decided: `unbound` holds such a parameter. Each one gets `NO_BINDING`, and the
/// result tells whether any did. Such a parameter may be one no type names: it poisons the call
/// rather than go uninferred. One pass over the names finds them, and only a call that leaves a
/// parameter open (`leaves_open`) makes it.
fn poison_broken_params(
    bindings: &mut Bindings,
    names: &[Symbol],
    leaves_open: bool,
    unbound: &[bool],
) -> bool {
    let mut broken = false;
    if leaves_open {
        let mut seen = FxHashSet::default();
        for (i, &name) in names.iter().enumerate() {
            #[cfg(test)]
            SCANNED.set(SCANNED.get() + 1);
            let broken_name = broken_param(&mut seen, name);
            if unbound[i] && broken_name {
                bindings.found[i] = Some(NO_BINDING);
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

/// The type parameters in `names` from `owner` on that no type in `params` names, so that no
/// argument can bind them. For a construction, whose built type names all of them: the call needs
/// its context, whatever its arguments are. A broken parameter is not open.
fn open_params(
    params: impl Iterator<Item = Option<Type>>,
    names: &[Symbol],
    owner: usize,
) -> Vec<usize> {
    let bound = named_params(params, names.len());
    let broken = broken_params(names);
    (owner..names.len())
        .filter(|&i| !bound[i] && !broken[i])
        .collect()
}

/// The type parameters of a callee after its `owner` ones, as themselves at their indices.
/// They follow the owner's arguments in a list that `Type::subst` reads.
fn own_params(names: &[Symbol], owner: usize) -> impl Iterator<Item = Type> {
    (0u32..)
        .zip(names)
        .skip(owner)
        .map(|(i, &name)| Type::intern(TypeKind::Param(name, i)))
}

/// The open parameters (see `open_params`) of a call of a function or method that returns `ret`,
/// or none when the call is not open. The context binds only what the type of the call names. So
/// the call is open if `ret` names a parameter that no parameter type does. It is not open if
/// one parameter is named by no type at all. Nothing can bind that parameter, so the call reports
/// it. `ret` is `None` when it did not resolve; the call is not open then.
fn open_params_of_call(
    params: impl Iterator<Item = Option<Type>>,
    ret: Option<Type>,
    names: &[Symbol],
    owner: usize,
) -> Vec<usize> {
    let Some(ret) = ret else {
        return Vec::new();
    };
    let built = named_params(iter::once(Some(ret)), names.len());
    let open = open_params(params, names, owner);
    if open.iter().all(|&i| built[i]) {
        open
    } else {
        Vec::new()
    }
}

/// Counts one visit of a walk over a type (see `TYPE_STEPS`).
fn type_step() {
    #[cfg(test)]
    TYPE_STEPS.set(TYPE_STEPS.get() + 1);
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
    /// How the call makes an optional by itself (see `Ranked::optional`).
    makes: Makes,
    /// Whether the declared type of the call holds an optional: `[X?]`, `Bx<X?>`, or the type that
    /// a construction through an alias builds (see `Ranked::holds_optional`). For a method it
    /// reads the type after the receiver binds the owner's type parameters.
    ret_optional: bool,
    /// The places whose arguments give the call its type (see `Places`).
    places: Places,
    /// The places of the callee's `mut` parameters, as bits. A `mut` argument is a place, whose
    /// type is known without checking it ahead.
    muts: u64,
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
        let mut seen = WalkMemo::default();
        for (i, param) in params.take(u64::BITS as usize).enumerate() {
            match param {
                Some(param) if param == payload => bare |= 1 << i,
                Some(param) if matches!(param.kind(), TypeKind::Optional(t) if t == payload) => {
                    lifted |= 1 << i;
                }
                Some(param) if holds_param(param, payload, &mut seen) => inside |= 1 << i,
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
    /// only that for a return type `T?`.
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

/// Whether `ty` holds `param`, a type parameter, other than as itself: under arrays or the type
/// arguments of a class or an enum, not under an optional nor an associated type. `seen` holds
/// each type that does not, so each distinct type is visited once.
fn holds_param(ty: Type, param: Type, seen: &mut WalkMemo<Type, ()>) -> bool {
    type_step();
    // A type without a parameter cannot hold `param`.
    if !ty.has_param() {
        return false;
    }
    if seen.get(&ty).is_some() {
        return false;
    }
    let holds = match ty.kind() {
        TypeKind::Array(t) => t == param || holds_param(t, param, seen),
        TypeKind::Class(_, args)
        | TypeKind::Struct(_, args)
        | TypeKind::Enum(_, args)
        | TypeKind::Any(_, args) => {
            (args.get().iter()).any(|&arg| arg == param || holds_param(arg, param, seen))
        }
        TypeKind::I64
        | TypeKind::U8
        | TypeKind::F64
        | TypeKind::Bool
        | TypeKind::Char
        | TypeKind::String
        | TypeKind::Unit
        | TypeKind::Never
        | TypeKind::Infer(_)
        | TypeKind::Optional(_)
        | TypeKind::Param(..)
        | TypeKind::Assoc(..) => false,
    };
    if !holds {
        seen.put(ty, ());
    }
    holds
}

/// Whether `ty` names a type parameter that `marked` marks by its place, other than under an
/// associated type (see `names_outside_assoc`), and under an optional only if `optionals`.
fn names_marked(ty: Type, marked: impl Fn(usize) -> bool + Copy, optionals: bool) -> bool {
    names_marked_in(ty, marked, optionals, &mut WalkMemo::default())
}

/// The walk of `names_marked`; `seen` holds each type that names none, so each distinct type is
/// visited once.
fn names_marked_in(
    ty: Type,
    marked: impl Fn(usize) -> bool + Copy,
    optionals: bool,
    seen: &mut WalkMemo<Type, ()>,
) -> bool {
    type_step();
    if !ty.has_param() {
        return false;
    }
    if let TypeKind::Param(_, j) = ty.kind() {
        return marked(j as usize);
    }
    if seen.get(&ty).is_some() {
        return false;
    }
    let names = match ty.kind() {
        TypeKind::Array(inner) => names_marked_in(inner, marked, optionals, seen),
        TypeKind::Optional(inner) => optionals && names_marked_in(inner, marked, optionals, seen),
        TypeKind::Class(_, args)
        | TypeKind::Struct(_, args)
        | TypeKind::Enum(_, args)
        | TypeKind::Any(_, args) => {
            (args.get().iter()).any(|&arg| names_marked_in(arg, marked, optionals, seen))
        }
        TypeKind::Assoc(..) => false,
        _ => unreachable!("only a kind with a child can name a parameter"),
    };
    if !names {
        seen.put(ty, ());
    }
    names
}

/// The places of the `mut` parameters among `muts`, as bits.
fn mut_places(muts: &[bool]) -> u64 {
    muts.iter()
        .take(u64::BITS as usize)
        .enumerate()
        .filter(|&(_, &m)| m)
        .fold(0, |bits, (i, _)| bits | 1 << i)
}

/// The places among `params` whose type names a type parameter that `marked` marks, as
/// `Places` says. A place from 64 on does not count (see `Makes::Through`).
fn carried_places(params: impl Iterator<Item = Option<Type>>, marked: &[bool]) -> Places {
    let mut places = Places::default();
    for (i, param) in params.take(u64::BITS as usize).enumerate() {
        let Some(param) = param else { continue };
        if names_marked(param, |j| marked[j], true) {
            places.carried |= 1 << i;
            if names_marked(param, |j| marked[j], false) {
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
    mark_by_assoc(
        ret,
        false,
        &mut inside,
        &mut outside,
        &mut WalkMemo::default(),
    );
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

/// `ty`, a type to show, with each hole that meets a known part of `want` replaced by that part.
/// A failed call shows a parameter that nothing bound as a hole (`misfit_type`); the place it
/// was checked against can know that part, as `u8` in `Two<i64, _>` against `Two<i64?, u8>`. An
/// optional meets a place that is not optional, and a value an optional place, through the
/// payload, as in `meet_parts`.
fn fill_holes(ty: Type, want: Type) -> Type {
    if !ty.holds_hole() {
        return ty;
    }
    crate::conformance::ensure_stack(|| match (ty.kind(), want.kind()) {
        _ if ty == *HOLE => {
            if want.holds_hole() || want.holds_infer() {
                ty
            } else {
                want
            }
        }
        (TypeKind::Optional(t), TypeKind::Optional(w)) => Type::optional(fill_holes(t, w)),
        (TypeKind::Optional(t), _) => Type::optional(fill_holes(t, want)),
        (_, TypeKind::Optional(w)) => fill_holes(ty, w),
        (TypeKind::Array(t), TypeKind::Array(w)) => Type::array(fill_holes(t, w)),
        (TypeKind::Class(a, ta), TypeKind::Class(b, wa)) if a == b => {
            Type::intern(TypeKind::Class(a, fill_args(ta, wa)))
        }
        (TypeKind::Struct(a, ta), TypeKind::Struct(b, wa)) if a == b => {
            Type::intern(TypeKind::Struct(a, fill_args(ta, wa)))
        }
        (TypeKind::Enum(a, ta), TypeKind::Enum(b, wa)) if a == b => {
            Type::intern(TypeKind::Enum(a, fill_args(ta, wa)))
        }
        _ => ty,
    })
}

/// `fill_holes` for each pair of type arguments.
fn fill_args(ty: TypeArgs, want: TypeArgs) -> TypeArgs {
    let filled: Vec<Type> = (ty.get().iter().zip(want.get()))
        .map(|(&t, &w)| fill_holes(t, w))
        .collect();
    TypeArgs::intern(&filled)
}

/// The rejected hole, `_`: the type of a part of a context that a failed bound has rejected. It
/// is a hole too, but it says why the part is not fixed: the outer call reported the bound, so a
/// call that meets it in its own context reports nothing for that part (see `Bindings::bind`).
/// Its index (`REJECTED_INDEX`) is out of range for `found`, as the index of `HOLE` is. `found`
/// holds it for one reason only: a parameter that is rejected (see `Bindings::reject`). A call
/// that has a rejected parameter fails, and does not use its list of types (see
/// `Bindings::finish`).
static REJECTED: LazyLock<Type> =
    LazyLock::new(|| Type::intern(TypeKind::Param(Symbol::intern("_"), REJECTED_INDEX)));

/// Whether `kind` is of a class, a struct, an enum or an `any` type. Such a type can hold the same
/// type in more than one of its arguments, so a walk over it can use a `WalkMemo`.
fn has_type_args(kind: TypeKind) -> bool {
    matches!(
        kind,
        TypeKind::Class(..) | TypeKind::Struct(..) | TypeKind::Enum(..) | TypeKind::Any(..)
    )
}

/// What `Bindings::resolve` puts for a parameter that is unbound.
#[derive(Clone, Copy)]
enum Unbound {
    /// Nothing: the walk gives `None`.
    Fail,
    /// The hole, without `REJECTED`: a callee's name for a parameter means
    /// nothing at the call.
    Print,
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
    /// What the walk in progress of `bind` has met: for a pair of a pattern and a
    /// value, its answer. A walk begins with `clear`.
    walked: WalkMemo<(Type, Type), bool>,
}

impl Bindings {
    /// `count` parameters, the first of them bound to `owner`, a receiver's arguments.
    fn new(count: usize, owner: &[Type]) -> Self {
        let unbound = iter::repeat_n(None, count - owner.len());
        Bindings {
            found: owner.iter().copied().map(Some).chain(unbound).collect(),
            fresh: Vec::new(),
            kept: 0,
            walked: WalkMemo::default(),
        }
    }

    /// Binds the parameters `pattern` names, the callee's, so that it is `value`, every
    /// parameter of which is the caller's and a fixed type. A `T?` binds `T` to a value that is
    /// not optional, which the call then wraps. `false`, binding nothing, when they cannot.
    pub(super) fn unify(&mut self, pattern: Type, value: Type) -> bool {
        self.fresh.clear();
        self.walked.clear();
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
    ///
    /// A pair of a pattern and a value gives the same answer on each visit within one `unify` or
    /// `constrain`: a parameter binds once and never changes. So a pattern with type arguments is
    /// visited once for each value, however many paths lead to it.
    fn bind(&mut self, pattern: Type, value: Type) -> bool {
        type_step();
        if !pattern.has_param() && !value.holds_hole() {
            return pattern == value;
        }
        let kind = pattern.kind();
        let memo = has_type_args(kind);
        if memo && let Some(met) = self.walked.get(&(pattern, value)) {
            return met;
        }
        let met = self.bind_kinds(kind, value);
        if memo {
            self.walked.put((pattern, value), met);
        }
        met
    }

    /// `bind`, by the kinds of `pattern` and `value`.
    fn bind_kinds(&mut self, pattern: TypeKind, value: Type) -> bool {
        match (pattern, value.kind()) {
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
                match pattern {
                    TypeKind::Array(p) | TypeKind::Optional(p) => {
                        self.bind(p, value);
                    }
                    TypeKind::Class(_, args)
                    | TypeKind::Struct(_, args)
                    | TypeKind::Enum(_, args)
                    | TypeKind::Any(_, args) => {
                        for &p in args.get() {
                            self.bind(p, value);
                        }
                    }
                    TypeKind::I64
                    | TypeKind::U8
                    | TypeKind::F64
                    | TypeKind::Bool
                    | TypeKind::Char
                    | TypeKind::String
                    | TypeKind::Unit
                    | TypeKind::Never
                    | TypeKind::Infer(_)
                    | TypeKind::Param(..)
                    | TypeKind::Assoc(..) => {}
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
            (TypeKind::Struct(a, p), TypeKind::Struct(b, v)) if a == b => self.bind_all(p, v),
            (TypeKind::Enum(a, p), TypeKind::Enum(b, v)) if a == b => self.bind_all(p, v),
            (TypeKind::Any(a, p), TypeKind::Any(b, v)) if a == b => self.bind_all(p, v),
            // An associated type binds nothing: the type it stands for is known once its base is
            // bound, and the argument is then checked against it (see `Checker::known`).
            (TypeKind::Assoc(..), _) => true,
            (
                TypeKind::I64
                | TypeKind::U8
                | TypeKind::F64
                | TypeKind::Bool
                | TypeKind::Char
                | TypeKind::String
                | TypeKind::Unit
                | TypeKind::Never
                | TypeKind::Infer(_)
                | TypeKind::Array(_)
                | TypeKind::Optional(_)
                | TypeKind::Class(..)
                | TypeKind::Enum(..)
                | TypeKind::Struct(..)
                | TypeKind::Any(..),
                _,
            ) => false,
        }
    }

    /// Rejects parameter `i`, whose bound failed. It is bound to `REJECTED`, so that no argument
    /// binds it again and it meets every type. `known` and `printed` treat it as unbound, and
    /// a call in an argument that waits for it sees `REJECTED`. It then knows that the fault is
    /// reported. Without this, each later argument would meet the type that the bound rejected.
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
        self.unknown_in(ty, &mut WalkMemo::default())
    }

    /// The walk of `unknown_param`; `seen` holds each type that names none, so each distinct type
    /// is visited once.
    fn unknown_in(&self, ty: Type, seen: &mut WalkMemo<Type, ()>) -> Option<usize> {
        type_step();
        if !ty.has_param() {
            return None;
        }
        if let TypeKind::Param(_, i) = ty.kind() {
            return match self.found[i as usize] {
                Some(bound) if bound != *REJECTED => None,
                _ => Some(i as usize),
            };
        }
        if seen.get(&ty).is_some() {
            return None;
        }
        let unknown = match ty.kind() {
            TypeKind::Array(inner) | TypeKind::Optional(inner) => self.unknown_in(inner, seen),
            TypeKind::Class(_, args)
            | TypeKind::Struct(_, args)
            | TypeKind::Enum(_, args)
            | TypeKind::Any(_, args) => args
                .get()
                .iter()
                .find_map(|&arg| self.unknown_in(arg, seen)),
            TypeKind::Assoc(base, _, args, _) => self.unknown_in(base, seen).or_else(|| {
                args.get()
                    .iter()
                    .find_map(|&arg| self.unknown_in(arg, seen))
            }),
            _ => unreachable!("only a kind with a child can name a parameter"),
        };
        if unknown.is_none() {
            seen.put(ty, ());
        }
        unknown
    }

    /// `bind` for each pair, every one of them tried.
    fn bind_all(&mut self, patterns: TypeArgs, values: TypeArgs) -> bool {
        iter::zip(patterns.get(), values.get()).fold(true, |all, (&p, &v)| self.bind(p, v) && all)
    }

    /// `ty`, a callee's, with its parameters' bindings in their place; `None` while it names
    /// one that is unbound or rejected.
    pub(super) fn known(&self, ty: Type) -> Option<Type> {
        self.resolve(ty, Unbound::Fail, &mut WalkMemo::default())
    }

    /// As `known`, with a hole for each unbound or rejected parameter, so it always gives a type.
    /// For a diagnostic: the callee's name for a parameter is not a type of the caller's code.
    fn printed(&self, ty: Type) -> Type {
        self.resolve(ty, Unbound::Print, &mut WalkMemo::default())
            .expect("a hole should stand for each unbound parameter")
    }

    /// The walk of `known`, `partial` and `printed`: `unbound` says what an unbound parameter
    /// becomes. A rejected parameter, which `found` binds to `REJECTED`, is unbound here, but
    /// `partial` keeps `REJECTED` for it. An associated type whose base is rejected is rejected.
    /// `seen` holds the answer for each type with a parameter that the walk has finished, so each
    /// distinct type is visited once.
    fn resolve(
        &self,
        ty: Type,
        unbound: Unbound,
        seen: &mut WalkMemo<Type, Option<Type>>,
    ) -> Option<Type> {
        type_step();
        #[cfg(test)]
        if !matches!(unbound, Unbound::Print) {
            WALKED.set(WALKED.get() + 1);
        }
        if !ty.has_param() {
            return Some(ty);
        }
        let kind = ty.kind();
        if matches!(kind, TypeKind::Param(..)) {
            return self.resolve_kind(kind, unbound, seen);
        }
        if let Some(resolved) = seen.get(&ty) {
            return resolved;
        }
        let resolved = self.resolve_kind(kind, unbound, seen);
        seen.put(ty, resolved);
        resolved
    }

    /// `resolve`, by `kind`, the kind of a type that has a parameter.
    fn resolve_kind(
        &self,
        kind: TypeKind,
        unbound: Unbound,
        seen: &mut WalkMemo<Type, Option<Type>>,
    ) -> Option<Type> {
        Some(match kind {
            TypeKind::Param(_, i) => match (self.found[i as usize], unbound) {
                (Some(bound), _) if bound != *REJECTED => bound,
                (_, Unbound::Fail) => return None,
                (_, Unbound::Print) => *HOLE,
            },
            TypeKind::Array(element) => Type::array(self.resolve(element, unbound, seen)?),
            TypeKind::Optional(payload) => Type::optional(self.resolve(payload, unbound, seen)?),
            TypeKind::Class(id, args) => {
                Type::intern(TypeKind::Class(id, self.resolve_all(args, unbound, seen)?))
            }
            TypeKind::Struct(id, args) => {
                Type::intern(TypeKind::Struct(id, self.resolve_all(args, unbound, seen)?))
            }
            TypeKind::Enum(id, args) => {
                Type::intern(TypeKind::Enum(id, self.resolve_all(args, unbound, seen)?))
            }
            TypeKind::Any(id, args) => {
                Type::intern(TypeKind::Any(id, self.resolve_all(args, unbound, seen)?))
            }
            TypeKind::Assoc(base, interface, args, k) => {
                let base = self.resolve(base, unbound, seen)?;
                // A hole stands for the whole type, which its base fixes. A rejected base
                // rejects it.
                if base.holds_hole() {
                    return Some(if base == *REJECTED { base } else { *HOLE });
                }
                let args = self.resolve_all(args, unbound, seen)?;
                Type::intern(TypeKind::Assoc(base, interface, args, k))
            }
            _ => unreachable!("only a kind with a child can name a parameter"),
        })
    }

    fn resolve_all(
        &self,
        args: TypeArgs,
        unbound: Unbound,
        seen: &mut WalkMemo<Type, Option<Type>>,
    ) -> Option<TypeArgs> {
        let list: Option<Vec<Type>> = args
            .get()
            .iter()
            .map(|&ty| self.resolve(ty, unbound, seen))
            .collect();
        list.map(|list| TypeArgs::intern(&list))
    }
}

/// Whether `value` is `pattern` for some types of its `count` parameters.
pub(super) fn type_meets(count: usize, pattern: Type, value: Type) -> bool {
    #[cfg(test)]
    super::reached("type_meets");
    Bindings::new(count, &[]).unify(pattern, value)
}

#[cfg(test)]
thread_local! {
    /// How many bindings inference has checked or undone on this thread.
    pub(super) static VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many types `Bindings::known` and `partial` have looked at on this thread, which
    /// checking a call uses. A `printed` walk is not counted: it runs for a diagnostic.
    pub(super) static WALKED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many type parameter names the search for reported ones has looked at on this
    /// thread.
    pub(super) static SCANNED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many types the walk for the type parameters that types name has visited on this
    /// thread.
    pub(super) static MARKED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many layers of patterns the search for the argument that bound a parameter has read on
    /// this thread (see `inferred_origin`).
    pub(super) static PEELED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many reads of a call's origins (`Origins::get`) and searches for the argument that
    /// bound a parameter (`inferred_origin`) there have been on this thread.
    pub(super) static ORIGIN_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
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
    /// How many expressions `takes_member` has walked on this thread, those it answers from
    /// `body.members` not counted.
    pub(super) static MEMBER_WALKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many entries of a call's lone arguments `note_lone_arg` has read, written or added on
    /// this thread. The guard against a scan of `args` is its indexed form, `args[t]`: the counter
    /// counts one step for each note and one for each entry that the vector grows by, not a scan.
    pub(super) static LONE_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many times `waits_for` has unified the type of a waiting call with its context on this
    /// thread.
    pub(super) static WAIT_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many times a walk over a type has visited a type on this thread, a visit that a memo
    /// answers too: `Bindings::bind`, `resolve` and `unknown_param`, `holds_param`,
    /// `mark_params`, `mark_by_assoc`, `names_marked` and `names_outside_assoc`.
    pub(super) static TYPE_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many `...` the example of an E0050 has written on this thread: one for each type
    /// parameter of the callee, each time the report is made.
    pub(super) static EXAMPLE_DOTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
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
    pub(super) bounds: &'a [Bounds],
    /// The callee's `mut` parameters, when it has any: an argument at one is a place, checked
    /// before the others.
    pub(super) mut_site: Option<&'a MutSite<'a>>,
    /// What declares `params`.
    pub(super) declarer: Declarer,
}

/// What the E0050 of a parameter that a call cannot infer reads of the callee (see
/// `GenericSig`). A call keeps it, and makes the text of the report only when it reports.
#[derive(Clone)]
pub(super) struct OpenParam {
    name: ast::Ident,
    variant: Option<ast::Ident>,
    /// The parameter's name.
    param: Symbol,
    /// The type parameters of the callee after the receiver's: one `...` each in the example.
    count: usize,
    /// Whether the variant has a payload.
    payload: bool,
}

impl OpenParam {
    /// The parameter at `param` of `sig`.
    pub(super) fn of(sig: &GenericSig<'_>, param: usize) -> Self {
        Self {
            name: sig.name.clone(),
            variant: sig.variant.cloned(),
            param: sig.declared[param],
            count: sig.declared.len() - sig.owner.len(),
            payload: !sig.params.is_empty(),
        }
    }

    /// The E0050. `in_result` tells whether the result type names the parameter other than
    /// under an associated type.
    pub(super) fn kind(&self, in_result: bool) -> DiagnosticKind {
        let name = &self.name;
        let variant = self.variant.as_ref();
        // `Some(...)`, or `Option.Some(...)`. A user variant `Some`, which E0026 reports, is
        // neither.
        let option = variant.is_none_or(|_| name.name == Symbol::OPTION);
        if option && variant.unwrap_or(name).name == Symbol::SOME {
            let (callee, at) = match variant {
                Some(variant) => (
                    format!("{}.{}", name.name, variant.name),
                    name.span.to(variant.span),
                ),
                None => (name.name.to_string(), name.span),
            };
            return DiagnosticKind::CannotInferSome { callee, at };
        }
        // `Ok(...)` and `Err(...)` write the variant as the callee and the enum not at all.
        // `qualifier` (enums.rs) tests for a bare variant in the same way; change both.
        if let Some(variant) = variant.filter(|v| v.span == name.span) {
            return DiagnosticKind::CannotInferResult {
                param: self.param.to_string(),
                variant: variant.name.to_string(),
                at: name.span,
            };
        }
        #[cfg(test)]
        EXAMPLE_DOTS.set(EXAMPLE_DOTS.get() + self.count);
        let dots = vec!["..."; self.count].join(", ");
        let (callee, example, at) = match variant {
            Some(variant) => {
                let payload = if self.payload { "(...)" } else { "" };
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
        DiagnosticKind::CannotInferParam {
            param: self.param.to_string(),
            callee,
            example,
            at,
            in_result,
        }
    }
}

/// What declares the parameters of a generic callee. It gives the origin and the shown text of
/// the type of each argument: argument `i` is parameter `i` of a function, field `i` of a class
/// or a struct, or payload field `i` of a variant. A call writes the fields and the payload in
/// declaration order (see `Checker::shaped`).
#[derive(Clone, Copy)]
pub(super) enum Declarer {
    /// Nothing that a report names: a prelude variant, or a requirement of an interface.
    None,
    /// The function or method.
    Function(tir::FuncId),
    /// The class or struct that a constructor builds.
    Fields(FieldOwner),
    /// The variant, by its index, of the enum.
    Variant(tir::EnumId, usize),
}

impl Declarer {
    /// The origin of the type of argument `i`.
    fn origin(self, i: usize) -> Origin {
        let index = Origin::id(i);
        match self {
            Self::None => Origin::None,
            Self::Function(f) => Origin::Parameter {
                function: Origin::id(f.0),
                index,
            },
            Self::Fields(FieldOwner::Class(class)) => Origin::Field {
                class: Origin::id(class.0),
                index,
            },
            Self::Fields(FieldOwner::Struct(id)) => Origin::StructField {
                id: Origin::id(id.0),
                index,
            },
            Self::Variant(id, variant) => Origin::Payload {
                enum_: Origin::id(id.0),
                variant: Origin::id(variant),
                field: index,
            },
        }
    }
}

/// The shape of a variant that a call of a generic enum builds: the type that it builds, its
/// enum's type parameters, or an alias's, the types of its payload and its labels.
pub(super) struct VariantShape {
    pub(super) built: Type,
    pub(super) names: Vec<Symbol>,
    pub(super) fields: Vec<Option<Type>>,
    pub(super) labels: Vec<Option<Symbol>>,
}

/// How a call writes its callee: its name, the type arguments in `angles` (none written when
/// `written` is empty), and the type the call's context expects. A method call also has the span
/// of its `(args)`.
#[derive(Clone, Copy)]
pub(super) struct Site<'a> {
    pub(super) name: &'a ast::Ident,
    pub(super) written: &'a [ast::TypeExpr],
    pub(super) angles: Span,
    /// The span of the `(args)` of a method call, none for any other call.
    pub(super) parens: Option<Span>,
    pub(super) expected: Expectation,
}

/// The type arguments after the first `owner` of `found`, which inference found for a callee,
/// when `site` writes them. Else none: no argument settles a parameter in a call with a failed
/// part.
pub(super) fn written_args<'t>(site: Site<'_>, found: &'t [Type], owner: usize) -> &'t [Type] {
    if site.written.is_empty() {
        &[]
    } else {
        found.get(owner..).unwrap_or_default()
    }
}

/// The one member of a parameter's join: the parameter, the member's span, its type and its
/// parameter type (see `Checker::lone_mismatch`).
type Lone = (usize, Span, Type, Option<Type>);

/// The type arguments of `call` so far, each one that is not known `NO_BINDING`.
fn listed_all(call: &Inference<'_>) -> Vec<Type> {
    call.bindings.found.iter().map(|&ty| listed(ty)).collect()
}

/// A bound of a decided type parameter of a call, which waits for the parameter's type and then
/// for the bound's arguments (see `Table::obligation`): where it is reported, the bound, the
/// parameter's type, and the call's type arguments as a bound sees them (see `listed`). `call`
/// is the call node of a closed form whose open form checks the bound again: once that form
/// settles, this check is stale (see `Table::settle`).
pub(super) struct BoundObligation {
    pub(super) at: Span,
    pub(super) bound: Bound,
    pub(super) on: Type,
    pub(super) with: Vec<Type>,
    pub(super) call: Option<Span>,
}

/// What the open form of a generic call needs (see `Checker::open_at`).
pub(super) struct OpenEntry {
    /// The call's result type, as the callee declares it.
    ret: Type,
    /// The result type with each open parameter as its mark (`MARK_INDEX`), and each other
    /// parameter as the closed form decided it. A decided type can hold a parameter of the caller
    /// with the name of an open one, never its mark.
    marked: Type,
    /// The call's type arguments, a method's receiver's first. An open parameter has its mark.
    types: Vec<Type>,
    /// The name and the index of each open parameter.
    params: Vec<(Symbol, usize)>,
    /// Each bound that an open parameter can change, with the index of its parameter and the
    /// place that it reports at: each bound of an open parameter, and each bound of another
    /// parameter whose interface arguments name an open one.
    bounds: Vec<(usize, Bound, Span)>,
    /// Each member at a place of an open parameter: the span of its argument and the type of its
    /// parameter, as the callee declares it.
    members: Vec<(Span, Type)>,
    /// The report of a join of an open parameter that had no candidate, held for the outer join
    /// (review sol 4). The open form that settles drops it. Else it is reported where the outer
    /// join fails, where the value closes, or at the end of the function.
    conflict: Option<(Held, Reported)>,
}

impl OpenEntry {
    /// Whether the call holds a conflict: its closed form has no type, so a join tries only its
    /// open form.
    pub(super) fn conflicts(&self) -> bool {
        self.conflict.is_some()
    }
}

/// A bound of an open form whose subject holds an open variable: the subject, the bound, and the
/// place that it reports at.
type Waits = (Type, Bound, Span);

/// Where a bound of the parameter `t` of `call` reports: at the argument that bound the parameter,
/// as `check_early` does, else at `span`, the call. `members` are the arguments whose parameter
/// type names `t` (see `CallIndex`).
fn bound_site(call: &Inference<'_>, members: &[usize], span: Span) -> Span {
    (members.iter())
        .find(|&&i| call.slots[i].is_some())
        .map_or(span, |&i| call.args[i].value.span)
}

/// The arguments of the call node `e`, each found by its span in one step.
struct ArgumentsAt<'e> {
    args: &'e mut [tir::Expr],
    /// The place of each argument in `args`, by its span: the first, when two have one span.
    places: FxHashMap<Span, usize>,
}

impl<'e> ArgumentsAt<'e> {
    /// The arguments of `e`, or `None` when `e` is not a call node.
    fn of(e: &'e mut tir::Expr) -> Option<Self> {
        let (tir::ExprKind::Call { args, .. }
        | tir::ExprKind::Struct { args, .. }
        | tir::ExprKind::Variant { args, .. }
        | tir::ExprKind::New { args, .. }) = &mut e.kind
        else {
            return None;
        };
        let mut places = FxHashMap::default();
        for (k, arg) in args.iter().enumerate() {
            places.entry(arg.span).or_insert(k);
        }
        Some(ArgumentsAt { args, places })
    }

    /// The argument at `at`.
    fn get(&mut self, at: Span) -> Option<&mut tir::Expr> {
        let &k = self.places.get(&at)?;
        Some(&mut self.args[k])
    }
}

/// The index of a generic call, made once for each call: the join of each parameter then looks
/// only at its own arguments and bounds, not at all of them.
struct CallIndex {
    /// For each argument, the parameters that its parameter type names other than under an
    /// associated type (see `names_outside_assoc`).
    outside: Vec<Vec<usize>>,
    /// For each argument, each parameter that its parameter type names.
    everywhere: Vec<Vec<usize>>,
    /// For each parameter, the arguments whose parameter type names it other than under an
    /// associated type, in order: the members of its join.
    members: Vec<Vec<usize>>,
    /// For each parameter, the parameters that its bounds name.
    bound_named: Vec<Vec<usize>>,
    /// Whether the arguments decide each parameter.
    decided: Vec<bool>,
}

impl CallIndex {
    fn new(sig: &GenericSig<'_>, decided: &[(usize, Type)]) -> Self {
        let len = sig.declared.len();
        let named = |reach| -> Vec<Vec<usize>> {
            (sig.params.iter())
                .map(|param| param.map_or_else(Vec::new, |param| params_named(param, reach)))
                .collect()
        };
        let (outside, everywhere) = (named(Reach::OutsideAssoc), named(Reach::Everywhere));
        let mut members = vec![Vec::new(); len];
        for (i, params) in outside.iter().enumerate() {
            for &t in params {
                members[t].push(i);
            }
        }
        let bound_named = (0..len)
            .map(|t| {
                let bounds = t
                    .checked_sub(sig.owner.len())
                    .and_then(|b| sig.bounds.get(b));
                let mut named: Vec<usize> = (bounds.into_iter())
                    .flat_map(|bounds| bounds.list.iter())
                    .flat_map(|bound| bound.args.get().iter())
                    .flat_map(|&arg| params_named(arg, Reach::Everywhere))
                    .collect();
                named.sort_unstable();
                named.dedup();
                named
            })
            .collect();
        let mut flags = vec![false; len];
        for &(t, _) in decided {
            flags[t] = true;
        }
        CallIndex {
            outside,
            everywhere,
            members,
            bound_named,
            decided: flags,
        }
    }

    /// The decided parameters that the type of the argument `i` names other than under an
    /// associated type.
    fn decided_at(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        (self.outside[i].iter().copied()).filter(|&t| self.decided[t])
    }

    /// The order in which the `decided` parameters join. A parameter whose bound names another
    /// decided one joins after it, so that the bound is closed when the members are checked
    /// against it (Q22): `T` of `fn h<T: K<U>, U>`. The first in `decided` whose bound names no
    /// parameter still to join goes next; when each one does, as in a cycle, the first goes.
    fn join_order(&self, decided: &[(usize, Type)]) -> Vec<(usize, Type)> {
        let mut at = vec![0; self.decided.len()];
        for (p, &(t, _)) in decided.iter().enumerate() {
            at[t] = p;
        }
        // For each one, how many parameters it still waits for, and which ones wait for it.
        let mut waits = vec![0_usize; decided.len()];
        let mut after = vec![Vec::new(); decided.len()];
        for (p, &(t, _)) in decided.iter().enumerate() {
            for &u in &self.bound_named[t] {
                if u != t && self.decided[u] {
                    waits[p] += 1;
                    after[at[u]].push(p);
                }
            }
        }
        let mut ready: BinaryHeap<Reverse<usize>> = (0..decided.len())
            .filter(|&p| waits[p] == 0)
            .map(Reverse)
            .collect();
        let mut done = vec![false; decided.len()];
        let mut first = 0;
        let mut order = Vec::with_capacity(decided.len());
        while order.len() < decided.len() {
            let p = loop {
                match ready.pop() {
                    Some(Reverse(p)) if !done[p] => break p,
                    Some(_) => {}
                    None => {
                        while done[first] {
                            first += 1;
                        }
                        break first;
                    }
                }
            };
            done[p] = true;
            order.push(decided[p]);
            for &q in &after[p] {
                waits[q] -= 1;
                if waits[q] == 0 && !done[q] {
                    ready.push(Reverse(q));
                }
            }
        }
        order
    }
}

/// A generic call while its arguments bind its callee's parameters.
pub(super) struct Inference<'s> {
    /// The call's span.
    at: Span,
    pub(super) bindings: Bindings,
    /// Each argument once checked, in source order.
    slots: Vec<Option<Checked>>,
    /// The proof of the report, when a parameter was bound to a type that no parameter takes, or
    /// its bound failed: the call reported it.
    failed: Option<Reported>,
    /// Whether the context rejected a parameter (see `REJECTED`), with the proof. The call fails,
    /// but the outer call reported the bound, so this one reports nothing for that parameter. It
    /// still reports a parameter that the context leaves unbound. The field stays false for a
    /// parameter whose hint holds a rejected part only: `infer_call` does not set it, and it skips
    /// the E0050 of that parameter (see `Bindings::first_unbound_not_rejected`).
    rejected: Option<Reported>,
    /// Where each parameter was bound: the argument, or the type argument written for it.
    origin: Origins,
    /// The bounds that the call checks as soon as a parameter is bound; none when the callee has
    /// no bound and no parameter type writes an associated type.
    early: Option<Early<'s>>,
    /// The place of each `mut` argument that checked, by its place among the arguments, until
    /// `mut_final` compares it with the parameter's final type.
    pub(super) mut_shapes: Vec<(usize, PlaceShape)>,
    /// What declares the parameters (see `GenericSig::declarer`).
    declarer: Declarer,
    /// The call's arguments, for the one that bound a parameter (see `inferred_origin`).
    args: &'s [ast::Arg],
    /// The place among `args` of the argument that the call checks now, which a binding in it
    /// records (see `Origins::set_first`).
    arg: Option<usize>,
}

mod origins {
    use crate::source::Span;

    /// Where each parameter of a call was bound (see `Inference::origin`), and the place of the
    /// argument that the call checked then. The list is private, so each read goes through `get`
    /// or `arg`.
    pub(super) struct Origins(Vec<Option<(Span, Option<usize>)>>);

    impl Origins {
        /// No place yet for each of `count` parameters.
        pub(super) fn new(count: usize) -> Self {
            Self(vec![None; count])
        }

        /// Where parameter `i` was bound. Panics when `i` is not a parameter of the call.
        pub(super) fn get(&self, i: usize) -> Option<Span> {
            #[cfg(test)]
            super::ORIGIN_READS.set(super::ORIGIN_READS.get() + 1);
            self.0[i].map(|(at, _)| at)
        }

        /// The place of the argument that the call checked when parameter `i` was bound. Panics
        /// when `i` is not a parameter of the call.
        pub(super) fn arg(&self, i: usize) -> Option<usize> {
            self.0[i].and_then(|(_, arg)| arg)
        }

        /// Records that parameter `i` was bound at `at`, outside an argument.
        pub(super) fn set(&mut self, i: usize, at: Option<Span>) {
            self.0[i] = at.map(|at| (at, None));
        }

        /// Records that parameter `i` was bound at `at` while the call checked the argument at
        /// `arg`, unless it has a place already.
        pub(super) fn set_first(&mut self, i: usize, at: Span, arg: Option<usize>) {
            self.0[i].get_or_insert((at, arg));
        }
    }
}

use origins::Origins;

/// The argument that bound the one type parameter that `pattern` is, under its array and optional
/// layers. None for any other pattern, and for a parameter that a written type argument or the
/// context bound: neither is in an argument.
fn inferred_origin(call: &Inference<'_>, pattern: Type) -> Option<Origin> {
    #[cfg(test)]
    ORIGIN_READS.set(ORIGIN_READS.get() + 1);
    // A pattern with no type parameter has none to name: its layers are not read.
    if !pattern.has_param() {
        return None;
    }
    let mut ty = pattern;
    let (param, j) = loop {
        #[cfg(test)]
        PEELED.set(PEELED.get() + 1);
        match ty.kind() {
            TypeKind::Array(inner) | TypeKind::Optional(inner) => ty = inner,
            TypeKind::Param(param, j) => break (param, j as usize),
            _ => return None,
        }
    };
    let bound = call.origin.get(j)?;
    let arg = call.args.get(call.origin.arg(j)?)?;
    // A binding by the context while the argument is checked is not in the argument.
    let inside = arg.value.span.start() <= bound.start() && bound.end() <= arg.value.span.end();
    inside.then_some(Origin::Inferred {
        at: arg.value.span,
        param,
        ty: call.bindings.found[j]?,
    })
}

/// The proof of the first failed part of `call`: a failed bound, a rejected context, or a failed
/// argument.
fn failure(call: &Inference<'_>) -> Option<Reported> {
    call.failed
        .or(call.rejected)
        .or_else(|| first_failed(&call.slots))
}

/// The proof of the first of `slots` that failed.
fn first_failed(slots: &[Option<Checked>]) -> Option<Reported> {
    (slots.iter().flatten()).find_map(|checked| checked.as_ref().err().map(Poisoned::reported))
}

/// `found`, a parameter's binding, as a bound sees it: `NO_BINDING` while it is unbound or
/// rejected.
fn listed(found: Option<Type>) -> Type {
    found.filter(|&t| t != *REJECTED).unwrap_or(NO_BINDING)
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
    bounds: &'s [Bounds],
    /// The type of each parameter as a bound sees it: its binding, or `NO_BINDING` while it is
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
        if !assoc && sig.bounds.iter().all(|b| b.list.is_empty()) {
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

/// `checked`, an argument, made `Error` that its rule rejected when `mismatched`. A mismatched
/// argument is poisoned by its report already, and keeps that proof.
pub(super) fn mark((checked, mismatched): (Checked, bool)) -> Checked {
    debug_assert!(
        !mismatched || checked.is_err(),
        "a mismatched argument should be poisoned"
    );
    match checked {
        Err(poisoned) if mismatched => Err(mismatch_poison(poisoned.reported())),
        checked => checked,
    }
}

/// The poison of an argument that its rule rejected, with the proof of the report.
pub(super) fn mismatch_poison(reported: Reported) -> Poisoned {
    Poisoned {
        mismatch: true,
        ..Poisoned::untyped(reported)
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
    Let(Checked, LetValue),
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
    /// there the optional fills the `?` of the place. So does a call that waits for its type, when
    /// its declared type holds an optional: `mkl()` for `fn mkl<X>() -> [X?]`, and a variant
    /// through an alias that builds one: `BE.B(v: None)` and `BE.E` for `type BE<X> = Bx<X?>`. Not
    /// a call of literals, as `kp(5)` for `fn kp<Y>(y: Y) -> Pr<i64?, Y>`: its argument binds `Y`,
    /// and the optional of its declared type is not at a place that an outer call binds. The order
    /// of the arguments of a call reads it (see `infer_call`), and joins do not.
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
    /// Whether the value never ends and gives no type: `todo`, a call of `panic`, a `loop` that
    /// no `break` leaves, a block that ends in a jump, or a branch of these only. Every reader but
    /// `combine_branches` reads `rank` alone, so such a value is `Concrete` for them.
    never: bool,
    /// Whether the value is an array fill, an arithmetic operator or a `-`, whose rank is that of
    /// its parts. Ranking an argument looks inside such a value. A sibling does not: it ranks the
    /// value `Concrete` (see `sibling`), so the leader of an operator and the first anchor of a
    /// join stay as they were. The `Branch::literal` of a join's branch reads `of_literals`, which
    /// does look inside: in a join whose context failed, `1 + id(2)`, `-id(2)` and `[5; 2]` are
    /// branches of literals and take the type of the anchor. A value that only waited, such as
    /// `[mk(); 2]` or `-mk()`, is not (see `waited`).
    leads: bool,
    /// Whether the rank is `Literal` only because the value waited (see `or_literal_if_waiting`),
    /// so the value holds no integer literal. `combine` sets it when each `Literal` part has it,
    /// and `combine_branches` when one branch has it.
    waited: bool,
}

impl Ranked {
    const CONCRETE: Self = Self {
        rank: Rank::Concrete,
        optional: false,
        holds_optional: false,
        nested: false,
        scalar: false,
        never: false,
        leads: false,
        waited: false,
    };
    const LITERAL: Self = Self {
        rank: Rank::Literal,
        optional: false,
        holds_optional: false,
        nested: false,
        scalar: true,
        never: false,
        leads: false,
        waited: false,
    };
    /// A value that never ends (see `never`). It ranks `Concrete`, as it always did.
    const NEVER: Self = Self {
        never: true,
        ..Self::CONCRETE
    };
    const WAITS: Self = Self {
        rank: Rank::Waits,
        optional: false,
        holds_optional: false,
        nested: false,
        scalar: false,
        never: false,
        leads: false,
        waited: false,
    };

    /// `self`, but `Literal` when it waits. An array fill, an arithmetic operator and a `-` never
    /// wait for a context: with none they take their default type, as a literal does. So an
    /// argument of such a value is not a rank-2 argument, which the call could not infer a type
    /// for. Under a written context the value is still checked after the context binds.
    fn or_literal_if_waiting(self) -> Self {
        if self.rank == Rank::Waits {
            Self {
                rank: Rank::Literal,
                waited: true,
                ..self
            }
        } else {
            self
        }
    }

    /// The rank that joins and operators order siblings by. They type a `Literal` value with
    /// `literal_type`, which knows scalars only. So a `Literal` value that is not `scalar` is
    /// `Concrete` for them. It is checked alone and takes its default type, as `[[1], [2]]` and
    /// `W(x: 1) < W(x: 2)` always did.
    // shortcut: such a value does not take the type of a sibling, so `[[1], x2]` with
    // `x2: [u8]` does not give `[1]` the type `[u8]`; upgrade when `literal_type` knows arrays
    // and the types of a call.
    ///
    /// A value that `leads` is `Concrete` too, and so is a call that does not fit its declaration
    /// (`fit` is false). The call is checked first and alone, and reports its own mistakes, not
    /// those of a sibling's type.
    pub(super) fn sibling(self, fit: bool) -> Rank {
        if !fit || self.leads || self.rank == Rank::Literal && !self.scalar {
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
/// It is `leads` when a part is. It is `waited` when it is `Literal` and each `Literal` part is
/// `waited`, so `-mk() + 1` is not: it holds the literal `1`. The parts of an operator make one
/// value. The branches of a join do not, and `combine_branches` sets `waited` by its own rule. It
/// stops at the first `Concrete` part. The parts after it are not ranked.
fn combine(parts: impl IntoIterator<Item = Ranked>) -> Ranked {
    let mut found = Ranked::WAITS;
    let mut scalar = true;
    let mut waited = true;
    for part in parts {
        match part.rank {
            Rank::Concrete => return Ranked::CONCRETE,
            Rank::Literal => found.rank = Rank::Literal,
            Rank::Waits => {}
        }
        found.optional |= part.optional;
        found.holds_optional |= part.holds_optional;
        found.nested |= part.nested;
        found.leads |= part.leads;
        if part.rank == Rank::Literal {
            scalar &= part.scalar;
            waited &= part.waited;
        }
    }
    found.scalar = scalar;
    found.waited = waited && found.rank == Rank::Literal;
    found
}

/// `combine` of the values of `branches`: each is `None` when its branch ends in an exit, else
/// the rank of its value. A value that never ends gives no type, so it counts as an exit. The
/// result never ends when no branch has a value. The result is `waited` when it is `Literal` and
/// one of the branches is: each branch is a value of its own, and a branch that only waited holds
/// no literal. The iterator stays lazy: ranking a branch has effects (see `tail_rank`).
fn combine_branches(branches: impl IntoIterator<Item = Option<Ranked>>) -> Ranked {
    let mut any = false;
    let mut waited = false;
    let values = branches.into_iter().flatten().filter(|part| !part.never);
    let found = combine(values.inspect(|part| {
        any = true;
        waited |= part.waited;
    }));
    if any {
        Ranked {
            waited: waited && found.rank == Rank::Literal,
            ..found
        }
    } else {
        Ranked::NEVER
    }
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

    /// `bindings.printed(ty)`, with the associated types of known types replaced.
    pub(super) fn printed(&self, bindings: &Bindings, ty: Type) -> Type {
        let printed = bindings.printed(ty);
        self.normalize(printed).unwrap_or(printed)
    }

    /// The arguments of a call of the generic `sig`, which decide its type parameters. The caller
    /// has matched the arguments to the parameters by count and label. `written` gives every type
    /// parameter that the receiver does not give; each of the others is an `Any` variable made
    /// at the call.
    ///
    /// 1. Each argument at a parameter that names a decided parameter is a member of that
    ///    parameter's join (`JoinForm::Call`). A value that is a join of its own waits (see
    ///    `takes_member`).
    /// 2. Each join decides its parameter from its members alone.
    /// 3. The arguments that wait meet the decided types (see `late_argument`), and the others
    ///    meet their parameter types.
    /// 4. The result meets `expected` at the top only (see `close_call`), and each bound is an
    ///    obligation (see `bound_obligations`).
    ///
    /// Returns the arguments in source order, the type arguments and the call's type. Returns
    /// `Error`, reported, when a parameter is left open or a join fails.
    pub(super) fn generic_call(
        &mut self,
        span: Span,
        sig: &GenericSig<'_>,
        written: (&[ast::TypeExpr], Span),
        args: &[ast::Arg],
        expected: Expectation,
    ) -> Result<(Vec<Checked>, TypeArgs, Option<Type>), Poisoned> {
        self.generic_call_mut(span, sig, written, args, expected, &mut MutParts::default())
    }

    /// `generic_call`, which gives the parts of the `mut` arguments to `parts`.
    pub(super) fn generic_call_mut(
        &mut self,
        span: Span,
        sig: &GenericSig<'_>,
        written: (&[ast::TypeExpr], Span),
        args: &[ast::Arg],
        expected: Expectation,
        parts: &mut MutParts,
    ) -> Result<(Vec<Checked>, TypeArgs, Option<Type>), Poisoned> {
        self.ranked(|c| c.infer_call(span, sig, written, args, expected, parts))
    }

    /// The result type `ret` of a failed call of a callee with `declared` type parameters, when
    /// the call fixes it without its arguments. `owner` binds the first parameters. `named` is
    /// the type arguments that the call writes, and binds the next ones.
    ///
    /// `None` when `ret` names a parameter that only the arguments bind, and for a callee with
    /// no result type.
    ///
    /// `None` also when `bounds` holds a bound of the callee's own parameters, and a bound fails
    /// for `named`. `None` too when such a bound exists and `named` is empty: the arguments would
    /// bind it. The corrected call is `Error` in both cases. This reports nothing. A call that
    /// inference checked passes no `bounds`.
    pub(super) fn fixed_result(
        &mut self,
        ret: Option<Type>,
        declared: usize,
        owner: &[Type],
        bounds: &[Bounds],
        named: &[Type],
    ) -> Option<Type> {
        let ret = ret?;
        let bounded = bounds.iter().any(|b| !b.list.is_empty());
        if !bounded && !ret.has_param() {
            return Some(ret);
        }
        let mut bindings = Bindings::new(declared, owner);
        // A `REJECTED` written type argument binds nothing.
        for (slot, &ty) in bindings.found[owner.len()..].iter_mut().zip(named) {
            *slot = (ty != *REJECTED).then_some(ty);
        }
        if bounded && (named.is_empty() || !self.meets_bounds(bounds, owner.len(), &bindings)) {
            return None;
        }
        if ret.has_param() {
            self.known(&bindings, ret)
        } else {
            Some(ret)
        }
    }

    /// The result of a construction of a generic class or enum whose arguments `Err`. `held` says
    /// whether an alias bound held. `ret` is the type with its parameters (`declared` of them) and
    /// `written` the written type arguments. No argument settles a parameter in a call with a
    /// failed part, so the call has the type that `fixed_result` gives.
    pub(super) fn failed_build(
        &mut self,
        (held, reported): (bool, Reported),
        (ret, declared): (Type, usize),
        written: &[Type],
    ) -> Checked {
        let ty = held
            .then(|| self.fixed_result(Some(ret), declared, &[], &[], written))
            .flatten();
        Err(Poisoned::new(ty, reported))
    }

    /// The type `ty` that a generic construction that checked builds. `None` when it names an
    /// associated type that has no choice. `held` is the result of the alias checks of the
    /// construction, which run first: when they failed, their token is the one of the type.
    /// Else the conformance of a bound reports the type, and gives no token.
    pub(super) fn built_type(
        &mut self,
        ty: Option<Type>,
        held: Result<(), Reported>,
        at: Span,
    ) -> Result<Type, Poisoned> {
        let Some(ty) = ty else {
            let reported = held.err().unwrap_or_else(|| self.proof(at)); // proof: declaration
            return Err(Poisoned::untyped(reported));
        };
        Ok(ty)
    }

    /// The poison of a failed call to a callee with `declared` type parameters and the result
    /// `ret`. `owner` is the receiver's type arguments, `written` the type arguments that the
    /// call writes, and `solved` what inference found. `reported` proves the failure.
    ///
    /// When only the receiver failed (`parted` is false), the receiver is typed and reads as its
    /// type, so `solved` stands. Otherwise the call has a failed part, and no argument settles a
    /// parameter: the result is typed only when the receiver or a written type argument binds it.
    pub(super) fn failed_call(
        &mut self,
        (solved, parted, reported): (Option<Type>, bool, Reported),
        (ret, declared, owner): (Option<Type>, usize, &[Type]),
        written: &[Type],
    ) -> Poisoned {
        match solved {
            Some(solved) if !parted => Poisoned::typed(solved, reported),
            _ => Poisoned::new(
                self.fixed_result(ret, declared, owner, &[], written),
                reported,
            ),
        }
    }

    /// `generic_call`, before what ranking kept is dropped. The closed form follows. Each own type
    /// parameter that the call does not write and that no `mut` argument binds is an `Any`
    /// variable of the call. The arguments at the places of such a parameter are the members
    /// of its join (`JoinForm::Call`). The join decides the parameter from them alone, before
    /// the context. The other arguments meet their parameter's type. Each bound of a decided
    /// parameter is an obligation, which runs once its type holds no open variable. The result
    /// converts to `expected` at the top only (see `close_call`).
    #[expect(
        clippy::too_many_lines,
        reason = "the steps share the call's bindings and slots; a split passes each one"
    )]
    fn infer_call(
        &mut self,
        span: Span,
        sig: &GenericSig<'_>,
        (written, angles): (&[ast::TypeExpr], Span),
        args: &[ast::Arg],
        expected: Expectation,
        parts: &mut MutParts,
    ) -> Result<(Vec<Checked>, TypeArgs, Option<Type>), Poisoned> {
        let owned = sig.owner.len();
        // A check of the call again keeps its own open form, or none.
        self.table.forget_open(span);
        let mut call = Inference {
            at: span,
            bindings: Bindings::new(sig.declared.len(), sig.owner),
            slots: args.iter().map(|_| None).collect(),
            failed: None,
            rejected: None,
            origin: Origins::new(sig.declared.len()),
            early: Early::of(sig),
            mut_shapes: Vec::new(),
            declarer: sig.declarer,
            args,
            arg: None,
        };
        // How many type arguments the call writes. They bind the result of a call that fails.
        let mut named = 0;
        if !written.is_empty() {
            let list = match self.type_args(sig.name, &sig.declared[owned..], written, angles) {
                Ok(list) => list,
                Err(reported) => {
                    self.stray(args);
                    // A list that failed binds nothing: only the receiver binds the result.
                    let ty = self.fixed_result(sig.ret, sig.declared.len(), sig.owner, &[], &[]);
                    return Err(Poisoned::new(ty, reported));
                }
            };
            named = list.get().len();
            // `type_args` rejects each type that `report_fresh` would.
            for (i, &ty) in list.get().iter().enumerate() {
                call.bindings.found[owned + i] = Some(ty);
                call.origin.set(owned + i, written.get(i).map(|ty| ty.span));
            }
            let bound: Vec<usize> = (owned..sig.declared.len()).collect();
            self.check_early(&mut call, &bound, span);
        }
        // An argument at a `mut` parameter is a place. It binds the parameters that its
        // parameter names first, so that the other arguments convert to them.
        let is_mut = |i: usize| sig.mut_site.is_some_and(|site| site.muts[i]);
        if let Some(site) = sig.mut_site {
            for (i, arg) in args.iter().enumerate().filter(|&(i, _)| is_mut(i)) {
                call.arg = Some(i);
                let slot = if marked(arg) {
                    Some(self.infer_mut_arg(&mut call, arg, i, (sig.params[i], site), parts))
                } else {
                    // No `mut`: E0166 is the report, and the place still binds, so that the
                    // other arguments are no cascade.
                    self.unmarked_place(arg, i, parts, Some((&mut call, sig.params[i])))
                };
                call.arg = None;
                call.slots[i] = slot;
            }
        }
        if !call.mut_shapes.is_empty() {
            self.mut_against_context(&mut call, sig, named, expected);
        }
        // The arguments that the `mut` loop checked as places, which join nothing.
        let placed: Vec<bool> = call.slots.iter().map(Option::is_some).collect();
        // The parameters that the arguments decide, each a variable made at the call.
        let mut decided: Vec<(usize, Type)> = (owned..sig.declared.len())
            .filter(|&i| call.bindings.found[i].is_none())
            .map(|i| (i, self.table.fresh(Kind::Any, span)))
            .collect();
        for &(i, var) in &decided {
            call.bindings.found[i] = Some(var);
        }
        // The index of the call, made once: for each argument, the parameters that its parameter
        // type names, and for each parameter, the arguments whose type names it. A join then looks
        // only at its own members.
        let index = CallIndex::new(sig, &decided);
        let joins = |i: usize| index.decided_at(i).next().is_some();
        let mut never_members = Vec::new();
        for (i, arg) in args.iter().enumerate() {
            if call.slots[i].is_some() {
                continue;
            }
            let checked = match sig.params[i] {
                None => mark(self.operand(&arg.value, Expectation::Poisoned)),
                Some(param)
                    if joins(i)
                        && let Some(member) = self
                            .takes_member(
                                &arg.value,
                                self.body.closed_place || index.decided_at(i).nth(1).is_some(),
                            )
                            .or_else(|| self.own_typed(&arg.value, param, expected)) =>
                {
                    let e = &arg.value;
                    let checked = read_jump(self.expr(e, member));
                    // A member that never finishes waits for its join (see `never_members` below).
                    // Its part that never finishes is found now, before a later literal's.
                    if type_of(&checked) == Some(Type::NEVER) {
                        never_members.push((i, self.never_at(e.span)));
                        checked
                    } else {
                        let checked = self.refuse_unwanted(e, checked, member, false);
                        let checked = mark(self.settled(e, checked, member));
                        self.refuse_valueless(read_jump(checked))
                    }
                }
                // A join of its own meets what the members decide (see `late_argument`).
                Some(_) if joins(i) => continue,
                // An associated type of a decided parameter waits for the joins.
                Some(param) if param.has_assoc() && !decided.is_empty() => continue,
                Some(param) => self.known_argument(&call, i, &arg.value, param),
            };
            // A call at a place that names two or more decided parameters is closed only (Q28).
            if let Ok(e) = &checked
                && index.decided_at(i).nth(1).is_some()
            {
                // Through `dbg`, a wrap and a block's value, the call keeps its own span.
                let mut at = e;
                loop {
                    // The call is closed only, so its held conflict is final.
                    if let Some(call) = self.table.open_call(at)
                        && self.report_held(call).is_some()
                    {
                        self.table.poison(at.ty);
                    }
                    self.table.forget_open(at.span);
                    at = match &at.kind {
                        tir::ExprKind::Dbg { operand, .. } | tir::ExprKind::Wrap(operand) => {
                            operand
                        }
                        tir::ExprKind::Block(tir::Block {
                            value: Some(value), ..
                        }) => value,
                        _ => break,
                    };
                }
            }
            call.slots[i] = Some(checked);
        }
        let mut failed = None;
        // The part of a required type at the place of each parameter in the result. One walk
        // finds them, at the first join that needs them: a walk of the whole result for each
        // parameter, as in `required_join`, makes a call quadratic in its parameters.
        let mut want_parts: Option<FxHashMap<Symbol, Type>> = None;
        // The same for the two types of a hint: the siblings' type and the hint.
        let mut hint_parts: [Option<FxHashMap<Symbol, Type>>; 2] = [None, None];
        // The one member of a parameter's join, when it has one, which a mismatch of the result
        // names (see `close_call`): its parameter, its span, its type, and its parameter type.
        let mut lone = Vec::new();
        // The members whose parameter type also writes an associated type: the join leaves it
        // open, and it meets the argument once the parameters are decided.
        let mut tied = Vec::new();
        // The parameters whose join reported for its members that never finish, or holds one that
        // was reported.
        let mut quiet = Vec::new();
        // The parameters whose join reported its own mistake, with no poisoned member: a member
        // that never finishes is still E0043 there, as on `main`.
        let mut loud = Vec::new();
        // The members that never finish and got the report of their join's `never` type.
        let mut first_never = Vec::new();
        // Whether a member broke a bound before its join (`broken_member`).
        let mut bound_broke = false;
        // The parameters that are open for an outer join, each with its name and the arguments of
        // its members (see `OpenEntry`).
        let mut openable = Vec::new();
        // The held report of each join of an open parameter that had no candidate.
        let mut conflict: Option<(Held, Reported)> = None;
        // The arguments that fit their place with no type for any parameter: each was reported.
        let mut misfits = vec![false; args.len()];
        // The parameters with a bound whose members all leave a part open, as `[]` and `[None]`
        // do: the context gives each its type, so a bound of it reports at the call, as on `main`.
        let mut by_context = vec![false; sig.declared.len()];
        let join_order = index.join_order(&decided);
        // The place of each decided parameter in `join_order`: a later one has a larger place.
        let mut joins_at = vec![0; sig.declared.len()];
        for (n, &(t, _)) in join_order.iter().enumerate() {
            joins_at[t] = n;
        }
        // The call's type arguments, made once. The join of each parameter brings up to date only
        // the entries that it reads. `scratch` is a copy whose entries a member changes before
        // its place is filled.
        let mut with: Vec<Type> = (call.bindings.found.iter())
            .map(|&ty| self.table.resolve(listed(ty)))
            .collect();
        let mut scratch = with.clone();
        let ret_named = sig
            .ret
            .map_or_else(Vec::new, |ret| params_named(ret, Reach::Everywhere));
        let ret_outside = result_names(sig);
        // The members that are split (see `Place::Split`), and the part of each at each decided
        // parameter that its place names. The first join of a member tries the split once.
        let mut split = vec![false; args.len()];
        let mut split_tried = vec![false; args.len()];
        let mut parts: FxHashMap<(usize, usize), Type> = FxHashMap::default();
        // For each split member, the parameters whose join did not unify its part with their
        // variable. A member that is not split meets that variable in its place at each later
        // join. So a split member takes such a join as a member that is not split, and then is
        // split again.
        let mut pending: Vec<Vec<usize>> = vec![Vec::new(); args.len()];
        // Whether a split member was open at a join: then it unified each part with the variable
        // of its parameter, as the place of a member that is not split does.
        let mut linked_all = vec![false; args.len()];
        // An open member gave the variable of a later parameter a type. A member that is not
        // split meets that type in its trial at each later join, and can be a misfit there. So no
        // member is split after that.
        let mut bound_early = false;
        for (n, &(t, var)) in join_order.iter().enumerate() {
            let name = sig.declared[t];
            // The join before this one left its `Param` in `with`. Its variable goes back, so that
            // each entry of the result type is a type that resolves as the entry would. Only
            // `required_join` reads the result type, and it resolves what it reads. The heading
            // reads it only for a report.
            if let Some(&(before, its_var)) = n.checked_sub(1).and_then(|n| join_order.get(n)) {
                with[before] = its_var;
            }
            // A split member reads no entry of `with`.
            let reads = (index.members[t].iter())
                .filter(|&&i| !split[i] || !pending[i].is_empty() || bound_early)
                .flat_map(|&i| &index.everywhere[i])
                .chain(&index.bound_named[t]);
            for &k in reads {
                with[k] = self.table.resolve(listed(call.bindings.found[k]));
            }
            // The mark of `T`'s place: a type that the call decided before can hold a parameter of
            // the caller named `T`, which is not the place.
            let param_t = Type::intern(TypeKind::Param(name, MARK_INDEX));
            with[t] = param_t;
            let found = &call.bindings.found;
            // A split member goes back to the place that it has when it is not split: a join
            // that fails reports each member as on `main`.
            let unsplit = |c: &mut Self, scratch: &mut [Type], i: usize, at: &mut Place| {
                let (Place::Split { argument, .. }, Some(param)) = (*at, sig.params[i]) else {
                    return;
                };
                for &k in &index.everywhere[i] {
                    let now = if k == t {
                        param_t
                    } else {
                        c.table.resolve(listed(found[k]))
                    };
                    scratch[k] = match now.kind() {
                        TypeKind::Infer(_) if index.decided[k] && joins_at[k] > n => {
                            c.table.fresh(Kind::Any, argument)
                        }
                        _ => now,
                    };
                }
                *at = Place::Part {
                    place: param.subst(scratch),
                    argument,
                };
            };
            let mut members = Vec::new();
            // The members that have a value, each with its place filled with `var`, its type and
            // its span, for the bounds of `T` (see `broken_member`).
            let mut valued = Vec::new();
            // The members whose part at `T` is still open decide nothing of it. Each takes the
            // type that the others decide, or that the context gives.
            let mut open = Vec::new();
            // The members that fit their place for no type of `T`: the span of each, its type,
            // its parameter type and the origin of that type.
            let mut misfit_at = Vec::new();
            let tied_from = tied.len();
            let mut slots = call.slots.iter_mut();
            let mut next = 0;
            for &i in &index.members[t] {
                let slot = slots.nth(i - next).expect("a member should have a slot");
                next = i + 1;
                let (Some(value), Some(param)) = (slot.as_mut(), sig.params[i]) else {
                    continue;
                };
                if placed[i] {
                    continue;
                }
                let argument = args[i].value.span;
                if misfits[i] {
                    continue;
                }
                // An argument that does not fit its place, whatever type each parameter takes, is
                // a mistake of its shape: it gets the directed report, as on main, and joins
                // nothing. A nested join is tested by the join, leaf by leaf, and so is a call
                // that waits with a held conflict. A nested join whose leaves decide nothing of
                // `T` gives the join no type: it is tested here and waits as a member that is open.
                let joins = nested_join(&args[i].value)
                    && self.leaves_decide(value, param.subst(&with), name);
                let tested = value.as_mut().is_ok_and(|e| !self.holds_conflict(e)) && !joins;
                if !(split[i] && pending[i].is_empty() && !bound_early)
                    && tested
                    && !param.has_assoc()
                    && let Some(ty) = type_of(value)
                    && ty != Type::NEVER
                {
                    for &k in &index.everywhere[i] {
                        scratch[k] = match with[k].kind() {
                            TypeKind::Infer(_) | TypeKind::Param(..) if k >= owned || k == t => {
                                self.table.fresh(Kind::Any, argument)
                            }
                            _ => with[k],
                        };
                    }
                    let place = param.subst(&scratch);
                    // A member at a place that names two or more decided parameters is split at
                    // its first join, when it converts there only by unification. Its type then
                    // keeps each part that it unified with the place: a fresh variable of each
                    // parameter that is open, as later joins take it.
                    let wide = !std::mem::replace(&mut split_tried[i], true)
                        && index.decided_at(i).nth(1).is_some();
                    let snapshot = self.table.snapshot();
                    let steps = match value {
                        Ok(e) if wide && self.table.open_call(e).is_none() => self
                            .converts_at_top(e, place)
                            .then(|| self.conversion(e.ty, place).map(|steps| steps.len())),
                        _ => None,
                    };
                    let fits = match steps {
                        Some(steps) => steps.is_some(),
                        None => self.converts(value, place, false).is_some(),
                    };
                    if steps == Some(Some(0)) {
                        self.keep_snapshot(snapshot);
                        split[i] = true;
                        for &k in index.everywhere[i].iter().filter(|&&k| index.decided[k]) {
                            parts.insert((i, k), scratch[k]);
                        }
                    } else {
                        self.table.rollback(snapshot);
                    }
                    if !fits {
                        misfits[i] = true;
                        // A `unit` that fits for no type is no value (E0043), as for a callee
                        // that is not generic.
                        if ty == Type::UNIT {
                            let cause = self.no_value_cause(
                                &args[i].value,
                                value.as_ref().ok(),
                                (ty, None),
                                false,
                            );
                            let reported = self.no_value(argument, cause);
                            failed.get_or_insert(reported);
                            continue;
                        }
                        // A nested join that decides nothing of `T` reports each leaf that does
                        // not fit, as under a written type (`let x: [[i64?]] = [1]`).
                        let origin = call.declarer.origin(i);
                        // `T` is a hole there: the leaves are checked against what no type of
                        // `T` changes, and the report shows `T` as `_`.
                        with[t] = *HOLE;
                        let shown_place =
                            (self.table).default_open(self.table.resolve(param.subst(&with)));
                        with[t] = param_t;
                        if nested_join(&args[i].value)
                            && let Ok(e) = value
                            && let Some(reported) =
                                self.report_misfit_leaves(e, shown_place, origin)
                        {
                            *value = Err(Poisoned::untyped(reported));
                            failed.get_or_insert(reported);
                            continue;
                        }
                        // The report waits for the other members, which can give `T` its type.
                        misfit_at.push((argument, ty, param, origin));
                        continue;
                    }
                }
                // A split member at its first join was split above, and has nothing pending.
                if split[i] && pending[i].is_empty() && !bound_early {
                    let part = *parts
                        .get(&(i, t))
                        .expect("a split member should have its part");
                    if self.decides_nothing(part, var, var) {
                        open.push((var, part, argument, i, true));
                    } else {
                        valued.push((var, part, argument));
                        let at = Place::Split { part, argument };
                        let label = argument;
                        members.push((i, JoinMember { value, at, label }));
                    }
                    continue;
                }
                with[t] = var;
                let filled = param.subst(&with);
                with[t] = param_t;
                // A nested join decides by its leaves, which the join walks, and a call that waits
                // with a held conflict by its open form.
                if let Some(ty) = type_of(value)
                    && ty != Type::NEVER
                    && !joins
                    && !value.as_mut().is_ok_and(|e| self.holds_conflict(e))
                    && self.decides_nothing(ty, filled, var)
                {
                    open.push((filled, ty, argument, i, false));
                    continue;
                }
                if value.is_ok()
                    && let Some(ty) = type_of(value)
                    && ty != Type::NEVER
                {
                    // An associated type in the place takes the member's part there.
                    valued.push((self.assoc_open(filled, argument), ty, argument));
                }
                // Each member has its own part at a parameter whose join comes later: a conflict
                // there belongs to the join of that parameter, not to the join of `T`.
                for &k in &index.everywhere[i] {
                    scratch[k] = match with[k].kind() {
                        TypeKind::Infer(_) if index.decided[k] && joins_at[k] > n => {
                            self.table.fresh(Kind::Any, argument)
                        }
                        _ => with[k],
                    };
                }
                let place = param.subst(&scratch);
                let place = if place.has_assoc() {
                    tied.push(i);
                    let place = self.normalize(place).unwrap_or(place);
                    self.assoc_open(place, argument)
                } else {
                    place
                };
                let at = if place == with[t] {
                    Place::Whole
                } else {
                    Place::Part { place, argument }
                };
                members.push((
                    i,
                    JoinMember {
                        value,
                        at,
                        label: argument,
                    },
                ));
            }
            // Only a bound reads it. The resolve walks each member's whole type, which can name
            // each parameter of the call.
            let bounded = (sig.bounds.get(t - sig.owner.len())).is_some_and(|b| !b.list.is_empty());
            // A nested join, as `[n]`, has an open type of its own until the join: its leaves
            // tell whether it decides `t`.
            by_context[t] = bounded
                && (!open.is_empty() || !members.is_empty())
                && (members.iter_mut()).all(|(i, m)| {
                    let ty = if nested_join(&args[*i].value) {
                        let at = m.at;
                        let member = JoinMember {
                            value: &mut *m.value,
                            at,
                            label: m.label,
                        };
                        self.join_type(&mut [member], Some(name), None)
                    } else {
                        type_of(m.value)
                    };
                    ty.is_some_and(|ty| self.table.resolve(ty).holds_infer())
                });
            if let [(i, member)] = members.as_slice()
                && open.is_empty()
                && let Some(ty) = type_of(member.value)
            {
                lone.push((t, member.label, ty, sig.params[*i]));
                if matches!(member.at, Place::Whole) {
                    self.note_lone_arg(span, sig.ret, (t, member.label, ty));
                }
            }
            // A lone member that leaves `T` open takes the place of the call's result.
            if members.is_empty()
                && let [(filled, ty, argument, _, false)] = open[..]
                && filled == var
            {
                self.note_lone_arg(span, sig.ret, (t, argument, ty));
            }
            // Unifies the parts of the split member `i` with the variables of the parameters in
            // `ks`. A place that is not split does this at its next join.
            let link = |c: &mut Self, i: usize, ks: &[usize]| {
                let mut linked = true;
                for &k in ks {
                    let part = *parts
                        .get(&(i, k))
                        .expect("a split member should have its part");
                    let now = c.table.resolve(listed(found[k]));
                    linked &= c.unify(now, part);
                }
                linked
            };
            // The closed call checks a bound of `T` before its join (Q22): E0154 at a member that
            // breaks it poisons the call, and the join adds no conflict. A bound of each parameter
            // is checked, as on `main`, also after another parameter broke its own.
            let broke = (failed.is_none() || bound_broke)
                .then(|| self.broken_member(sig, (t, var), &mut with, &valued))
                .flatten();
            with[t] = param_t;
            // A misfit is reported after the bound check. After a broken bound, the rejected `T`
            // meets each type, as on `main`: only a misfit with no open part is still wrong there.
            if broke.is_some() {
                with[t] = var;
                let mut kept = Vec::new();
                for misfit in misfit_at {
                    let (at, ty, param, _) = misfit;
                    let want = self
                        .table
                        .default_open(self.table.resolve(param.subst(&with)));
                    if !(self.table.default_open(self.table.resolve(ty))).holds_infer()
                        || self.blamed_part(at, want).is_some()
                    {
                        kept.push(misfit);
                    }
                }
                misfit_at = kept;
                with[t] = param_t;
            }
            if !misfit_at.is_empty() {
                for (i, m) in &mut members {
                    unsplit(self, &mut scratch, *i, &mut m.at);
                }
                let mut whole: Vec<JoinMember<'_>> = (members.iter_mut())
                    .map(|(_, m)| JoinMember {
                        value: &mut *m.value,
                        at: m.at,
                        label: m.label,
                    })
                    .collect();
                let reported = self.report_misfits(
                    &misfit_at,
                    (name, t, var, broke.is_some()),
                    &mut with,
                    &mut whole,
                );
                with[t] = param_t;
                if let Some(reported) = reported {
                    failed.get_or_insert(reported);
                }
            }
            if let Some(reported) = broke {
                self.table.poison(var);
                failed = Some(reported);
                quiet.push(t);
                bound_broke = true;
                let open_members = open.iter().map(|&(.., i, _)| i);
                for i in members.iter().map(|&(i, _)| i).chain(open_members) {
                    if split[i] {
                        pending[i].push(t);
                    }
                }
                continue;
            }
            let of: Vec<usize> = members.iter().map(|&(i, _)| i).collect();
            let mut members: Vec<JoinMember<'_>> = members.into_iter().map(|(_, m)| m).collect();
            // The result type with `T` in it, when `T` has a place there that no associated type
            // ties: the open form takes `T` from it.
            let ret_t = (sig.ret).filter(|_| tied.len() == tied_from && ret_outside[t]);
            if let Some(first) = members.first().map(|member| member.label) {
                let returns = |c: &mut Self| {
                    let ret = (sig.ret).filter(|&ret| ret != with[t] && ret_outside[t])?;
                    let mut now = with.clone();
                    for &k in &ret_named {
                        now[k] = c.table.resolve(listed(found[k]));
                    }
                    now[t] = with[t];
                    Some(ret.subst(&now))
                };
                let heading = Heading {
                    head: sig.name.span,
                    whole: span,
                    returns: None,
                };
                // A poisoned member was reported: the others that never finish add no report. A
                // member that never finishes, and failed only inside, is no such member: a valid
                // jump makes it `never`, as `{ if c { break }; return 1 }`.
                let held = (members.iter()).find_map(|m| {
                    let poisoned = m.value.as_ref().err()?;
                    (poisoned.ty != Some(Type::NEVER)).then(|| poisoned.reported())
                });
                let form = JoinForm::Call { param: name };
                // A call that failed already reports nothing more against `want`.
                let mut joined =
                    |c: &mut Self, members: &mut [JoinMember<'_>]| match (expected, ret_t) {
                        (Expectation::Required(want, _), Some(ret)) if failed.is_none() => {
                            // A closed form that takes the part of `want` converts, as
                            // `required_join` finds by a walk of the whole result. The
                            // declared result has each parameter at the place of `with`.
                            let parts = want_parts.get_or_insert_with(|| c.result_parts(ret, want));
                            if let Some(&part) = parts.get(&name)
                                && c.closed_takes(members, name, part)
                            {
                                return Expectation::Member;
                            }
                            let at = (t, var, sig.bounds.get(t - owned));
                            c.required_join(members, at, (ret, &with), want)
                        }
                        // Under a hint, as under a required type, but only a part that the
                        // members take: the hint is no mistake. The siblings' type first, as in
                        // `hinted_join`. So the elements of `[1]` in `id([1])` take the `u8?` of a
                        // hint `[u8?]`.
                        (Expectation::Hint { want, siblings, .. }, Some(ret))
                            if failed.is_none() =>
                        {
                            let at = (t, var, sig.bounds.get(t - owned));
                            let [sibling_parts, want_parts] = &mut hint_parts;
                            let mut takes = |want: Type, parts: &mut Option<_>| {
                                // A closed form that takes the part of the hint needs no walk
                                // of the whole result, as under `Required`.
                                let parts = parts.get_or_insert_with(|| c.result_parts(ret, want));
                                if let Some(&part) = parts.get(&name)
                                    && c.closed_takes(members, name, part)
                                {
                                    return None;
                                }
                                match c.required_join(members, at, (ret, &with), want) {
                                    Expectation::Required(part, origin)
                                        if c.join_type(members, Some(name), Some(part))
                                            .is_some() =>
                                    {
                                        Some(Expectation::Required(part, origin))
                                    }
                                    _ => None,
                                }
                            };
                            takes(siblings, sibling_parts)
                                .or_else(|| takes(want, want_parts))
                                .unwrap_or(Expectation::Member)
                        }
                        _ => Expectation::Member,
                    };
                let mut expected_here = joined(self, &mut members);
                if members.iter().any(|m| matches!(m.at, Place::Split { .. })) {
                    let required = match expected_here {
                        Expectation::Required(ty, _) => Some(ty),
                        _ => None,
                    };
                    if self.join_type(&mut members, Some(name), required).is_none() {
                        for (m, &i) in members.iter_mut().zip(&of) {
                            unsplit(self, &mut scratch, i, &mut m.at);
                        }
                        expected_here = joined(self, &mut members);
                    }
                }
                let before: Vec<Option<Type>> = members.iter().map(JoinMember::ty).collect();
                // Under `Member`, a join with no candidate waits for the outer join, which can
                // open the call (review sol 4). Its report is held until then.
                let waits = expected == Expectation::Member
                    && ret_t.is_some()
                    && open.is_empty()
                    && failed.is_none();
                let (result, waiting) = self.join_form_holding(
                    form,
                    (heading, returns),
                    &mut members,
                    (expected_here, None),
                    waits,
                );
                if let (Some(waiting), Err(poisoned)) = (waiting, &result) {
                    match &mut conflict {
                        Some((all, _)) => all.absorb(waiting),
                        None => conflict = Some((waiting, poisoned.reported())),
                    }
                    for &i in of.iter().filter(|&&i| split[i]) {
                        pending[i].push(t);
                    }
                    openable.push((t, name, of));
                    continue;
                }
                // A join that settled no member unified no part of a split member.
                if !matches!(result, Ok(ty) if ty != Type::NEVER) {
                    for &i in of.iter().filter(|&&i| split[i]) {
                        pending[i].push(t);
                    }
                }
                match result {
                    Ok(Type::NEVER) if let Some(reported) = held => {
                        self.table.poison(var);
                        failed.get_or_insert(reported);
                        quiet.push(t);
                    }
                    // `never` binds no type parameter: each member never finishes.
                    // The first member gets the report, and each other member E0043, as on `main`.
                    Ok(Type::NEVER) => {
                        quiet.push(t);
                        loud.push(t);
                        first_never.push(of[0]);
                        let at = (never_members.iter())
                            .find(|&&(i, _)| i == of[0])
                            .map_or(first, |&(_, at)| at);
                        let ty = self.type_name(Type::NEVER);
                        let reported = self.error(DiagnosticKind::TypeArgument { ty, at });
                        failed.get_or_insert(reported);
                    }
                    Ok(ty) => {
                        if !self.unify(var, ty) {
                            // The join of an earlier parameter bound it through a shared argument.
                            failed.get_or_insert_with(|| self.proof(first)); // proof: quiet reader
                        }
                        // A nested join knows only now whether its value is a valueless `if`.
                        for (m, &i) in members.iter_mut().zip(&of) {
                            if nested_join(&args[i].value)
                                && let Some(reported) = self.refuse_settled_valueless(m.value)
                            {
                                failed.get_or_insert(reported);
                            }
                        }
                        // A member that went back to its place unified each part there.
                        for (m, &i) in members.iter().zip(&of).filter(|&(_, &i)| split[i]) {
                            let ks = std::mem::take(&mut pending[i]);
                            if matches!(m.at, Place::Split { .. }) {
                                link(self, i, &ks);
                            }
                        }
                        // shortcut: a parameter whose closed join wraps a member is closed only,
                        // so that no member takes two wraps; keep each member's own type in the
                        // entry to open it too.
                        let wrapped = (members.iter().zip(&before)).any(|(m, &own)| {
                            own.is_some_and(|own| {
                                let want = target(m.at, Some(name), ty);
                                self.table.resolve(own) != self.table.resolve(want)
                            })
                        });
                        if expected == Expectation::Member
                            && ret_t.is_some()
                            && open.is_empty()
                            && !wrapped
                        {
                            openable.push((t, name, of));
                        }
                    }
                    Err(poisoned) => {
                        self.table.poison(var);
                        failed.get_or_insert_with(|| poisoned.reported());
                        quiet.push(t);
                        if held.is_none() {
                            loud.push(t);
                        }
                    }
                }
            }
            // A split member that took this join as one that is not split unified each part.
            for (filled, ty, at, i, split_here) in open {
                let pending = std::mem::take(&mut pending[i]);
                // The first time, a split member links each part, which can bind a later one.
                let all = split_here && !std::mem::replace(&mut linked_all[i], true);
                let linked = match (split_here, all) {
                    (false, _) => true,
                    (true, false) => link(self, i, &pending),
                    (true, true) => {
                        let decided = (index.everywhere[i].iter()).filter(|&&k| index.decided[k]);
                        link(self, i, &decided.copied().collect::<Vec<_>>())
                    }
                };
                let unified = self.unify(filled, ty);
                if !(linked && unified) {
                    failed.get_or_insert_with(|| self.proof(at)); // proof: quiet reader
                }
                if !split_here || all {
                    bound_early |= (index.everywhere[i].iter()).any(|&k| {
                        index.decided[k]
                            && joins_at[k] > n
                            && !matches!(
                                self.table.resolve(listed(call.bindings.found[k])).kind(),
                                TypeKind::Infer(_)
                            )
                    });
                }
            }
        }
        for &(i, var) in &decided {
            call.bindings.found[i] = Some(self.table.resolve(var));
        }
        // A member that never finishes beside one that has a value is E0043, as a value.
        let mut hushed = vec![false; sig.declared.len()];
        for &t in quiet.iter().filter(|t| !loud.contains(t)) {
            hushed[t] = true;
        }
        for (i, at) in never_members {
            if !first_never.contains(&i) && !index.outside[i].iter().any(|&t| hushed[t]) {
                let reported = self.no_value(at, NoValueCause::Never);
                failed.get_or_insert(reported);
                call.slots[i] = Some(Err(Poisoned::untyped(reported)));
            }
        }
        if failed.is_none() {
            tied.dedup();
            for i in tied {
                let (Some(ty), Some(param)) =
                    (call.slots[i].as_ref().and_then(type_of), sig.params[i])
                else {
                    continue;
                };
                let mut want = self.known_in(&call, param);
                // A bound whose argument is an open `Int` finds no conformance, so its associated
                // type shows the `Int` as `i64`, as the bound takes it. This check binds nothing.
                let defaulted = want.is_none();
                if defaulted {
                    for &(t, var) in &decided {
                        call.bindings.found[t] = Some(self.table.default_open(var));
                    }
                    want = self.known_in(&call, param);
                    for &(t, var) in &decided {
                        call.bindings.found[t] = Some(self.table.resolve(var));
                    }
                }
                let Some(want) = want else { continue };
                let want = self.table.resolve(want);
                let snapshot = self.table.snapshot();
                let fits = self.unify(want, ty);
                if defaulted {
                    self.table.rollback(snapshot);
                }
                if !fits {
                    let origin = call.declarer.origin(i);
                    let found = self.shown_against(ty, want);
                    let d = self.mismatch(args[i].value.span, None, want, found, origin);
                    failed.get_or_insert_with(|| self.diagnostics.error(d));
                }
            }
        }
        // An argument that is still to check takes the whole context as its guide: the widest type
        // that it can meet.
        let late = call.slots.iter().any(Option::is_none);
        let mut met = if failed.is_none() && late {
            self.meet_context(&call, sig, &decided, (expected, true))
        } else {
            None
        };
        for &(i, var) in &decided {
            call.bindings.found[i] = Some(self.table.resolve(var));
        }
        // An argument that is a join of its own, or whose parameter type is an associated type of
        // a decided parameter. One whose type comes from its literals goes first: it decides what
        // one that waits for its context meets.
        let mut order: Vec<(bool, usize)> = (0..args.len())
            .filter(|&i| call.slots[i].is_none())
            .map(|i| (self.sibling_rank(&args[i].value) == Rank::Waits, i))
            .collect();
        order.sort_unstable();
        for (waits, i) in order {
            let arg = &args[i];
            let param =
                sig.params[i].expect("an argument with no parameter type should be checked");
            // A method call that waits for its context to pick a method, at a bare parameter that
            // nothing decides: the call cannot infer that parameter, as on `main`. In a member
            // call whose result names the parameter, the join around the call gives it
            // (`id(b.opt(true)) == o`), unless several methods answer the method call.
            // A block gives its value's context to that value.
            let value = block_value(&arg.value);
            if waits
                && failed.is_none()
                && matches!(value.kind, ast::ExprKind::MethodCall { .. })
                && let TypeKind::Param(_, t) = param.kind()
                && let Ok(found) = decided.binary_search_by_key(&(t as usize), |&(k, _)| k)
                && let (t, var) = decided[found]
                && (expected != Expectation::Member
                    || !sig.ret.is_some_and(|ret| names_outside_assoc(ret, t))
                    || self.picks_by_context(value))
                && let open = self.table.resolve(var)
                && matches!(open.kind(), TypeKind::Infer(_))
                && self.table.default_open(open) == open
            {
                failed = Some(self.cannot_infer(sig, t));
                self.table.poison(var);
                quiet.push(t);
                hushed[t] = true;
                call.slots[i] = Some(mark(self.operand(&arg.value, Expectation::Poisoned)));
                continue;
            }
            // A poisoned member poisoned the parameter, or a poisoned context the call: the
            // argument reports its own mistakes only. A part at a parameter that no failure
            // reached still meets that parameter, as on `main`.
            if expected == Expectation::Poisoned
                || index.decided_at(i).all(|t| hushed[t]) && index.decided_at(i).any(|t| hushed[t])
            {
                call.slots[i] = Some(mark(self.operand(&arg.value, Expectation::Poisoned)));
                continue;
            }
            if joins(i) {
                let closed = self.body.closed_place;
                self.body.closed_place |= index.decided_at(i).nth(1).is_some();
                let checked = self.late_argument(&call, i, &arg.value, (param, expected));
                self.body.closed_place = closed;
                // A poisoned argument decides nothing, and its report is the call's. A parameter
                // that it leaves open is poisoned, so a later argument there reports only its own
                // mistakes, as on `main`.
                if let Err(poisoned) = &checked {
                    failed.get_or_insert_with(|| poisoned.reported());
                    for t in index.decided_at(i) {
                        let Ok(found) = decided.binary_search_by_key(&t, |&(k, _)| k) else {
                            continue;
                        };
                        let var = decided[found].1;
                        let open = self.table.resolve(var);
                        if matches!(open.kind(), TypeKind::Infer(_))
                            && self.table.default_open(open) == open
                        {
                            self.table.poison(var);
                            quiet.push(t);
                            hushed[t] = true;
                        }
                    }
                }
                call.slots[i] = Some(checked);
                continue;
            }
            call.slots[i] = Some(match self.known_in(&call, param) {
                Some(_) => self.known_argument(&call, i, &arg.value, param),
                // An associated type of a parameter that nothing decides: the call reports
                // that parameter (see `close_call`).
                None if let want = param.subst(&listed_all(&call))
                    && self.table.resolve(want).holds_infer() =>
                {
                    if expected == Expectation::Member {
                        // Only the join decides the parameter. The argument is checked by itself
                        // now, and meets its type when the join gives it (see `settle_held`).
                        let at = arg.value.span;
                        let value = self.value(&arg.value, Expectation::Member);
                        let exits = value.as_ref().is_ok_and(tir::Expr::exits);
                        self.table.hold(at, want, value);
                        Ok(tir::Expr::placeholder(want, at, exits))
                    } else {
                        mark(self.operand(&arg.value, Expectation::Poisoned))
                    }
                }
                // An associated type with no choice: the conformance reported it.
                None => {
                    failed.get_or_insert_with(|| self.proof(span)); // proof: declaration
                    mark(self.operand(&arg.value, Expectation::Poisoned))
                }
            });
        }
        if failed.is_none() && met.is_none() {
            met = self.meet_context(&call, sig, &decided, (expected, false));
        }
        // A broken parameter that nothing decided poisons the call, as on `main`: the arguments
        // and the context are checked first, so a call that decides each parameter does not look
        // at the names.
        let mut unbound = vec![false; sig.declared.len()];
        let mut leaves_open = false;
        for &(t, var) in &decided {
            let ty = self.table.resolve(var);
            leaves_open |= ty.holds_infer();
            unbound[t] =
                matches!(ty.kind(), TypeKind::Infer(_)) && self.table.default_open(ty) == ty;
        }
        let broken = poison_broken_params(&mut call.bindings, sig.declared, leaves_open, &unbound);
        if broken {
            // `unbound` holds an open variable, so only a poisoned parameter is `NO_BINDING`.
            let poisoned = |t: usize| unbound[t] && call.bindings.found[t] == Some(NO_BINDING);
            for &(_, var) in decided.iter().filter(|&&(t, _)| poisoned(t)) {
                self.table.poison(var);
            }
            decided.retain(|&(t, _)| !poisoned(t));
        }
        let mut failed = failed.or(call.failed).or(call.rejected);
        // A broken parameter fails the call, but another one that nothing decides is still E0050.
        if failed.is_none() {
            match self.close_call(&mut call, sig, (&decided, &lone), (expected, met)) {
                Ok(()) => {}
                // A mismatch, or a misfit that the code using the value reports.
                Err(poisoned) if poisoned.mismatch || poisoned.found.is_some() => {
                    self.mut_final(&mut call, sig, args, None);
                    return Err(poisoned);
                }
                Err(poisoned) => failed = Some(poisoned.reported()),
            }
        } else if bound_broke && !broken && expected != Expectation::Poisoned {
            self.report_left_open(&mut call, sig, &decided, &quiet);
        } else if !broken && expected != Expectation::Poisoned {
            // The call failed and settles no type. On `main`, a call that waits is checked after
            // the other arguments, in order: each before the first failed argument reports the
            // parameter that it leaves open, unless an argument that does not wait failed.
            let bad: Vec<usize> = (0..args.len())
                .filter(|&i| misfits[i] || matches!(call.slots[i], Some(Err(_))))
                .collect();
            // A member argument is checked with no rank first, so its rank walks it again: that
            // walk checks its scrutinee once more. It must not report or bind a second time.
            let (snapshot, reports) = (self.table.snapshot(), self.diagnostics.mark());
            let wait = (bad.iter()).all(|&i| self.sibling_rank(&args[i].value) == Rank::Waits);
            self.table.rollback(snapshot);
            self.diagnostics.drop_since(reports);
            if let Some(&before) = bad.first()
                && wait
            {
                for &(t, var) in decided.iter().filter(|&&(t, _)| !hushed[t]) {
                    let ty = self.table.resolve(var);
                    if ty.holds_infer() && !self.table.reported_open(ty) {
                        let _ = self.report_waiting_calls(&call, sig, (t, ty, before));
                    }
                }
            }
        }
        if broken {
            failed.get_or_insert_with(|| self.proof(span)); // proof: declaration
        }
        // A parameter whose join or member reported has no bound left to check.
        // A bound that names such a parameter has no check left either: that failure is the report.
        let bounded: Vec<(usize, Type)> = (decided.iter().copied())
            .filter(|&(t, _)| !hushed[t])
            .filter(|&(t, _)| !index.bound_named[t].iter().any(|&q| hushed[q]))
            .collect();
        let first_obligation = self.obligations.len();
        if let Some(reported) =
            self.bound_obligations(&call, (sig, &index, &by_context), &bounded, (span, failed))
        {
            failed = Some(reported);
        }
        let list = TypeArgs::intern(
            &(call.bindings.found.iter())
                .map(|&ty| self.table.resolve(listed(ty)))
                .collect::<Vec<_>>(),
        );
        self.mut_final(&mut call, sig, args, failed.is_none().then_some(list));
        if let Some(reported) = failed {
            // A held conflict is an error of this call too.
            if let Some((held, _)) = conflict {
                self.diagnostics.restore(held);
            }
            // No argument settles a parameter in a failed call: the receiver and the written
            // type arguments bind the result, or it is untyped. A written type argument that its
            // bound rejects is `REJECTED` there, and binds no parameter.
            let named: Vec<Type> = call.bindings.found[owned..owned + named]
                .iter()
                .map(|ty| ty.unwrap_or(*REJECTED))
                .collect();
            let ty = self.fixed_result(sig.ret, sig.declared.len(), sig.owner, &[], &named);
            return Err(Poisoned {
                kept: ty.is_none() && call.slots.iter().flatten().any(is_kept),
                ..Poisoned::new(ty, reported)
            });
        }
        if !openable.is_empty()
            && let Some(ret) = sig.ret
        {
            let mut types = list.get().to_vec();
            for &(t, name, _) in &openable {
                types[t] = Type::intern(TypeKind::Param(name, MARK_INDEX));
            }
            let members = (openable.iter())
                .flat_map(|(_, _, of)| of.iter())
                .filter_map(|&i| Some((args[i].value.span, sig.params[i]?)))
                .collect();
            let mut opened = vec![false; sig.declared.len()];
            for &(t, ..) in &openable {
                opened[t] = true;
            }
            let open = |t: usize| opened[t];
            // An associated type in an argument may name an open parameter too: it is kept.
            let names_open = |bound: &Bound| {
                (bound.args.get().iter())
                    .any(|&arg| arg.has_assoc() || names_marked(arg, open, true))
            };
            let bounds = (sig.bounds.iter().enumerate())
                .flat_map(|(i, b)| b.list.iter().map(move |&bound| (owned + i, bound)))
                .filter(|(t, bound)| open(*t) || names_open(bound))
                .map(|(t, bound)| (t, bound, bound_site(&call, &index.members[t], span)))
                .collect::<Vec<_>>();
            let again: FxHashSet<_> = (bounds.iter())
                .map(|(_, b, _)| (b.at, b.interface))
                .collect();
            for obligation in &mut self.obligations[first_obligation..] {
                if again.contains(&(obligation.bound.at, obligation.bound.interface)) {
                    obligation.call = Some(span);
                }
            }
            let entry = OpenEntry {
                ret,
                marked: ret.subst(&types),
                params: openable.iter().map(|&(t, name, _)| (name, t)).collect(),
                types,
                bounds,
                members,
                conflict,
            };
            self.table.add_open(span, entry);
        }
        self.instantiates = true;
        let checked = call
            .slots
            .into_iter()
            .map(|slot| slot.expect("every argument should be checked once the joins are done"))
            .collect();
        let ret = sig.ret.and_then(|ret| self.subst(ret, list.get()));
        Ok((checked, list, ret))
    }

    /// The expectation of the join of the type parameter `t` (its variable `var`, its `bounds`) of
    /// a call under the required type `want`. `ret` is the call's result type, which names `t`,
    /// and `with` the call's type arguments with `t` as its `Param`.
    ///
    /// Closed first (Q26): `Member` when the members alone give `t` a type with which the result
    /// converts to `want`. Else `t` is open at the part of `want` at its place in `ret`. Each
    /// member that does not take that part gets the directed report (S26b). When the members
    /// differ from the part only by its wraps, the closed form reports at the call (Q24).
    fn required_join(
        &mut self,
        members: &mut [JoinMember<'_>],
        (t, var, bounds): (usize, Type, Option<&Bounds>),
        (ret, with): (Type, &[Type]),
        want: Type,
    ) -> Expectation {
        #[cfg(test)]
        super::join::REQUIRED_JOINS.set(super::join::REQUIRED_JOINS.get() + 1);
        let TypeKind::Param(name, _) = with[t].kind() else {
            return Expectation::Member;
        };
        let want = self.table.resolve(want);
        // A result that is not optional meets the payload of an optional `want`, and the call's
        // value is wrapped (see `meet_context`).
        let shown = self.table.resolve(ret.subst(with));
        // A member whose type holds no optional, and has a head that is known, cannot make an
        // optional. So the result wraps once at the top (Q24), and its report names the payload,
        // as on `main`. A member that holds an optional, as `Result<u8?, bool>`, gets the whole
        // `want` in its report, as on `main`.
        let wraps_top = matches!(want.kind(), TypeKind::Optional(_))
            && !members.is_empty()
            && (members.iter()).all(|m| self.closed_plain(m, name));
        // An optional result never converts to a `want` that is not optional: no part of it
        // opens the call, and the closed form reports at the call, as on `main`.
        let unwrapped = matches!(shown.kind(), TypeKind::Optional(_))
            && !matches!(want.kind(), TypeKind::Optional(_) | TypeKind::Infer(_));
        let part = if wraps_top || unwrapped {
            None
        } else {
            part_at(shown, name, want)
        };
        let Some(part) = part.or_else(|| match want.kind() {
            TypeKind::Optional(inner) => part_at(shown, name, inner),
            _ => None,
        }) else {
            return Expectation::Member;
        };
        let closed = self.join_type(members, Some(name), None);
        if let Some(ty) = closed {
            let mut with_ty = with.to_vec();
            with_ty[t] = ty;
            let snapshot = self.table.snapshot();
            // Each member converts to `ty`, which binds what the join left open in it: with
            // `None` first, `pair(None, s)` joins as `T?` and `s` makes it `string?`.
            let converts = self.unify(var, ty)
                && (members.iter_mut()).all(|m| {
                    let target = target(m.at, Some(name), ty);
                    self.member_converts(m, target, false).is_some()
                })
                && {
                    // A number unifies with `want`, else it is `i64`, as in `close_call`.
                    let ret = self.table.resolve(ret.subst(&with_ty));
                    self.conversion(ret, want).is_some() || {
                        let ret = self.table.default_open(ret);
                        self.conversion(ret, want).is_some()
                    }
                };
            self.table.rollback(snapshot);
            if converts {
                return Expectation::Member;
            }
        }
        let mut with_part = with.to_vec();
        with_part[t] = part;
        // A part of the result that no join decides, as the `T` of a receiver, that does not
        // fit `want` makes the shape wrong: the closed form reports the call as a whole. The
        // other joins of the call are open here. An entry of an earlier join is its variable,
        // which resolves to its type.
        let snapshot = self.table.snapshot();
        let fits = members.first().is_none_or(|m| {
            let shape: Vec<Type> = (with_part.iter())
                .map(|&ty| match self.table.resolve(ty) {
                    ty if matches!(ty.kind(), TypeKind::Param(..)) => {
                        self.table.fresh(Kind::Any, m.label)
                    }
                    ty => ty,
                })
                .collect();
            let shaped = self.table.resolve(ret.subst(&shape));
            self.unify(shaped, want)
                || matches!(want.kind(), TypeKind::Optional(inner) if self.unify(shaped, inner))
        });
        self.table.rollback(snapshot);
        if !fits {
            return Expectation::Member;
        }
        let snapshot = self.table.snapshot();
        let holds_bounds = bounds.is_none_or(|bounds| {
            (bounds.list.iter()).all(|bound| {
                let args = self.subst_args(bound.args, &with_part);
                // A bound that names another open parameter, as `K<U>`, is not known to fail.
                let open = (args.get().iter()).any(|&arg| arg.has_param() || arg.holds_infer());
                part.holds_infer() || open || self.conforms(part, bound.interface, args)
            })
        });
        let opens = holds_bounds
            && self.join_type(members, Some(name), Some(part)).is_some()
            && self.unify(ret.subst(&with_part), want);
        self.table.rollback(snapshot);
        // A part that breaks a bound of `t` is no type of `t`. When the closed members agree, the
        // closed form reports the call as a whole, as on `main`. When they do not, each member
        // gets the directed report (S26b).
        let closed_members = (members.iter())
            .all(|m| type_of(m.value).is_some_and(|ty| !self.table.resolve(ty).holds_infer()));
        if !holds_bounds && closed_members && closed.is_some() {
            return Expectation::Member;
        }
        // Neither form takes `want`. Members that differ from the part only by its wraps take too
        // many (Q24): the call reports as a whole. Else each member that does not take the part
        // gets the directed report.
        let wraps_only = self.wraps_only(members, name, part);
        let first = if closed.is_none() {
            self.first_binder(members, name, part)
        } else {
            None
        };
        // The directed report of a member names no origin, as on main (owner ruling 2).
        if let Some(first) = first {
            Expectation::Required(first, Origin::None)
        } else if opens || !wraps_only {
            Expectation::Required(part, Origin::None)
        } else {
            Expectation::Member
        }
    }

    /// Whether the member `m` of the parameter `name` gives it a type with a known head and no
    /// optional. A member at a part of the parameter gives that part: an array `[X]` at `[T]`
    /// gives the `X` that its elements join to.
    fn closed_plain(&self, m: &JoinMember<'_>, name: Symbol) -> bool {
        type_of(m.value).is_some_and(|ty| {
            let ty = self.table.resolve(ty);
            let ty = match m.at {
                Place::Part { place, .. } => {
                    part_at(self.table.resolve(place), name, ty).unwrap_or(ty)
                }
                Place::Whole | Place::Split { .. } => ty,
            };
            !matches!(ty.kind(), TypeKind::Infer(_)) && !ty.holds_optional()
        })
    }

    /// The part of `want` at the place of each parameter in the declared result `ret`, as
    /// `required_join` finds it: a result that is not optional meets the payload of an optional
    /// `want` too.
    fn result_parts(&self, ret: Type, want: Type) -> FxHashMap<Symbol, Type> {
        let want = self.table.resolve(want);
        let mut parts = FxHashMap::default();
        parts_at(ret, want, &mut parts);
        if let TypeKind::Optional(inner) = want.kind()
            && !matches!(ret.kind(), TypeKind::Optional(_))
        {
            parts_at(ret, inner, &mut parts);
        }
        parts
    }

    /// Whether the closed form of the join of `members` (the members of the parameter `name`) is
    /// `part`. Each member converts to it first, as in `required_join`. The members bind the
    /// closed form first: `part` must not bind what they leave open, as the `?x` of `?x?`.
    fn closed_takes(&mut self, members: &mut [JoinMember<'_>], name: Symbol, part: Type) -> bool {
        let Some(ty) = self.join_type(members, Some(name), None) else {
            return false;
        };
        let snapshot = self.table.snapshot();
        let takes = (members.iter_mut()).all(|m| {
            let target = target(m.at, Some(name), ty);
            self.member_converts(m, target, false).is_some()
        }) && self.unify(ty, part);
        self.table.rollback(snapshot);
        takes
    }

    /// Whether `members` differ from `part` only by its wraps: each takes `part` without its
    /// optionals at the place of the parameter `name`.
    fn wraps_only(&mut self, members: &[JoinMember<'_>], name: Symbol, part: Type) -> bool {
        let mut bare = part;
        while let TypeKind::Optional(inner) = bare.kind() {
            bare = inner;
        }
        let snapshot = self.table.snapshot();
        let wraps_only = bare != part
            && (members.iter()).all(|m| {
                m.ty()
                    .is_none_or(|ty| self.unify(ty, target(m.at, Some(name), bare)))
            });
        self.table.rollback(snapshot);
        wraps_only
    }

    /// The type of the first of `members`, which do not agree, when it binds the parameter
    /// `name` as on `main`: it is a closed whole argument that does not take `part`, and a later
    /// closed member takes it. Each later member that differs then gets the directed report. A
    /// literal has no type of its own, so it does not count.
    fn first_binder(
        &mut self,
        members: &[JoinMember<'_>],
        name: Symbol,
        part: Type,
    ) -> Option<Type> {
        let (m, rest) = members.split_first()?;
        let ty = self.table.resolve(type_of(m.value)?);
        let takes = |this: &mut Self, m: &JoinMember<'_>| {
            m.ty().is_some_and(|got| {
                let snapshot = this.table.snapshot();
                let takes = this.unify(got, target(m.at, Some(name), part));
                this.table.rollback(snapshot);
                takes
            })
        };
        let binds = matches!(m.at, Place::Whole)
            && !ty.holds_infer()
            && !takes(self, m)
            && rest.iter().any(|m| {
                let closed =
                    type_of(m.value).is_some_and(|got| !self.table.resolve(got).holds_infer());
                closed && takes(self, m)
            });
        binds.then_some(ty)
    }

    /// The reports of the arguments in `misfits`, which fit their place for no type of the
    /// parameter `t` (`name`, its variable `var`). Each is its own mistake, as on `main`. The
    /// other members of `t` are `members`. When one of them has a type of its own, the place
    /// shows the type that they give `t`. A literal has no type of its own. When the place is
    /// then known, a label names the argument that gives it. An open part can be left, where the
    /// place clashes with the argument. Then the report names the outer shape of the place, as
    /// "an array" or "a `Box`". Else it shows each open part as `_`, as on `main`. A bound of `t`
    /// that broke (`rejected`) leaves `t` open in the place that blames a part. It changes
    /// `with[t]`, which the caller puts back. `None` when `misfits` is empty.
    fn report_misfits(
        &mut self,
        misfits: &[(Span, Type, Type, Origin)],
        (name, t, var, rejected): (Symbol, usize, Type, bool),
        with: &mut [Type],
        members: &mut [JoinMember<'_>],
    ) -> Option<Reported> {
        let binder = self.binder_origin(name, members, &vec![false; members.len()]);
        let joined = binder.and_then(|_| self.join_type(members, Some(name), None));
        // A literal gives `T` the type that a blamed argument's place shows, as on `main`.
        let known = joined.or_else(|| self.join_type(members, Some(name), None));
        let mut first = None;
        for &(at, ty, param, origin) in misfits {
            with[t] = var;
            let open_want = self
                .table
                .default_open(self.table.resolve(param.subst(with)));
            with[t] = known.unwrap_or(var);
            let known_want = self
                .table
                .default_open(self.table.resolve(param.subst(with)));
            with[t] = joined.unwrap_or(var);
            let want = self
                .table
                .default_open(self.table.resolve(param.subst(with)));
            let got = self.shown_against(ty, want);
            // A generic call that gives its one argument's type, as `id(true)`, takes a known
            // place on to that argument: the report is at `true`, with no label, as on `main`.
            let mut inner = at;
            while !want.holds_infer()
                && let Some((arg, _)) = self.lone_arg(inner)
            {
                inner = arg;
            }
            // A part of the place that is known and clashes with the type that one argument of the
            // call gives alone: the report is at that argument, as on `main`. A literal gives `T`
            // its type there only for an open call, and a rejected `T` meets each type.
            let blamed_want = if rejected {
                open_want
            } else if (self.table.default_open(self.table.resolve(ty))).holds_infer() {
                known_want
            } else {
                want
            };
            if inner == at
                && let Some((arg, part, ty)) = self.blamed_part(at, blamed_want)
            {
                let reported = self.mismatch(arg, None, part, ty, Origin::None);
                first.get_or_insert_with(|| self.diagnostics.error(reported));
                continue;
            }
            let (at, binder) = if inner == at {
                (at, binder)
            } else {
                (inner, Some(Origin::None))
            };
            let d = if let Some(shape) = self.clashing_shape(want, got) {
                DiagnosticKind::Mismatch {
                    want: shape,
                    got: self.type_name(got),
                    at,
                    origin: None,
                    fix: None,
                    conversion: false,
                    layers: false,
                }
                .into()
            } else {
                let origin = if want.holds_infer() {
                    origin
                } else {
                    binder.unwrap_or(origin)
                };
                // A literal that gives `T` its type fills the place that the report shows.
                if want.holds_infer() && known.is_some() {
                    let got = self.shown_against(ty, known_want);
                    self.mismatch(at, None, known_want, got, origin)
                } else {
                    self.mismatch(at, None, want, got, origin)
                }
            };
            let reported = self.diagnostics.error(d);
            first.get_or_insert(reported);
        }
        first
    }

    /// Notes `arg` for the generic call at `call` that returns `ret` (see `Body::lone_args`).
    /// `arg` is the parameter, span and type of an argument that alone decides its bare type
    /// parameter.
    fn note_lone_arg(&mut self, call: Span, ret: Option<Type>, arg: (usize, Span, Type)) {
        if let Some(ret) = ret {
            let (_, args) = (self.body.lone_args.entry(call)).or_insert_with(|| (ret, Vec::new()));
            let (t, at, ty) = arg;
            #[cfg(test)]
            LONE_STEPS.set(LONE_STEPS.get() + 1 + (t + 1).saturating_sub(args.len()));
            if args.len() <= t {
                args.resize(t + 1, None);
            }
            args[t] = Some((at, ty));
        }
    }

    /// The span and type of the one argument of the generic call at `call` whose bare type
    /// parameter is the call's result type (see `Body::lone_args`).
    pub(super) fn lone_arg(&self, call: Span) -> Option<(Span, Type)> {
        let (ret, args) = self.body.lone_args.get(&call)?;
        let TypeKind::Param(_, k) = ret.kind() else {
            return None;
        };
        args.get(k as usize).copied().flatten()
    }

    /// The argument of the generic call at `call` that a known part of `want` clashes with, as
    /// `main` blames it. `want` is the call's place. The argument alone decides a type parameter.
    /// Each known part of `want` at that parameter is one type, and the argument's type does not
    /// convert to it. It gives the argument's span, that part and the argument's type. The result
    /// can wrap once at its top. Each pair of types is walked once, so a type with repeated parts
    /// is walked in linear time.
    fn blamed_part(&mut self, call: Span, want: Type) -> Option<(Span, Type, Type)> {
        let (ret, args) = self.body.lone_args.get(&call)?.clone();
        let want = match (ret.kind(), want.kind()) {
            (TypeKind::Optional(_), _) => want,
            (_, TypeKind::Optional(payload)) => payload,
            _ => want,
        };
        let args: Vec<_> = (args.iter())
            .map(|arg| arg.map(|(at, ty)| (at, self.table.resolve(ty))))
            .collect();
        let mut seen = FxHashSet::default();
        let mut pairs = vec![(ret, want)];
        // The known part of `want` at each parameter of `args`, or `None` after two that differ.
        // An open argument takes the first part, left to right, as its context.
        let mut parts: Vec<Option<Option<Type>>> = vec![Some(None); args.len()];
        while let Some((ret, want)) = pairs.pop() {
            if !seen.insert((ret, want)) {
                continue;
            }
            let list = |pairs: &mut Vec<_>, ret: TypeArgs, want: TypeArgs| {
                pairs.extend(
                    ret.get()
                        .iter()
                        .copied()
                        .zip(want.get().iter().copied())
                        .rev(),
                );
            };
            match (ret.kind(), want.kind()) {
                (TypeKind::Param(_, k), _) if !want.holds_infer() => {
                    let n = k as usize;
                    if let Some(Some((_, ty))) = args.get(n) {
                        let open = ty.holds_infer();
                        parts[n] = match parts[n] {
                            Some(None) => Some(Some(want)),
                            Some(Some(part)) if part != want && !open => None,
                            same => same,
                        };
                    }
                }
                (TypeKind::Optional(r), TypeKind::Optional(w))
                | (TypeKind::Array(r), TypeKind::Array(w)) => pairs.push((r, w)),
                (TypeKind::Class(a, r), TypeKind::Class(b, w)) if a == b => list(&mut pairs, r, w),
                (TypeKind::Struct(a, r), TypeKind::Struct(b, w)) if a == b => {
                    list(&mut pairs, r, w);
                }
                (TypeKind::Enum(a, r), TypeKind::Enum(b, w)) if a == b => list(&mut pairs, r, w),
                _ => {}
            }
        }
        for (arg, part) in args.iter().zip(parts) {
            let (Some((at, ty)), Some(Some(part))) = (*arg, part) else {
                continue;
            };
            let ty = self.shown_against(ty, part);
            if !ty.holds_infer() && !super::expr::fits(part, ty) && !self.converts_to_any(ty, part)
            {
                return Some((at, part, ty));
            }
        }
        None
    }

    /// The type `got` of an argument as its report against `want` shows it: each open part that
    /// meets a known part of `want` takes it, as the argument would when it is checked against
    /// `want` (`meet_parts`). It binds nothing.
    pub(super) fn shown_against(&mut self, got: Type, want: Type) -> Type {
        let snapshot = self.table.snapshot();
        self.meet_parts(got, want);
        let met = self.table.resolve(got);
        self.table.rollback(snapshot);
        self.table.default_open(fill_holes(met, want))
    }

    /// Binds each open part of `got` that meets a known part of `want`, as the argument takes it
    /// when it is checked against `want`. An optional meets a place that is not optional through
    /// its payload: `X?` against `[i64]` takes `X = [i64]`. A value that is not optional meets an
    /// optional place through the place's payload. The report then shows the known part of `got`,
    /// not `_`. The caller rolls the bindings back.
    fn meet_parts(&mut self, got: Type, want: Type) {
        crate::conformance::ensure_stack(|| {
            let got = self.table.resolve(got);
            match (got.kind(), want.kind()) {
                // A type parameter in `want` can be the callee's own, which no argument takes.
                (TypeKind::Infer(_), _) if !want.holds_infer() && !want.has_param() => {
                    // A part that does not meet stays open. Inside the caller's snapshot, the
                    // checks that wait for the part stay queued, and its rollback drops them.
                    let _queued = self.table.unify(got, want);
                }
                (TypeKind::Optional(g), TypeKind::Optional(w))
                | (TypeKind::Array(g), TypeKind::Array(w)) => self.meet_parts(g, w),
                (TypeKind::Optional(g), _) => self.meet_parts(g, want),
                (_, TypeKind::Optional(w)) => self.meet_parts(got, w),
                (TypeKind::Class(a, ga), TypeKind::Class(b, wa)) if a == b => {
                    self.meet_args(ga, wa);
                }
                (TypeKind::Struct(a, ga), TypeKind::Struct(b, wa)) if a == b => {
                    self.meet_args(ga, wa);
                }
                (TypeKind::Enum(a, ga), TypeKind::Enum(b, wa)) if a == b => {
                    self.meet_args(ga, wa);
                }
                _ => {}
            }
        });
    }

    /// `meet_parts` for each pair of type arguments.
    fn meet_args(&mut self, got: TypeArgs, want: TypeArgs) {
        for (&g, &w) in got.get().iter().zip(want.get()) {
            self.meet_parts(g, w);
        }
    }

    /// The outer shape of `want`, the place of an argument of type `got`, when the two clash
    /// and `want` still holds an open part: an array, or the class or enum. The name of the
    /// callee's parameter would mislead there, as the arguments may give it a type. Another
    /// clash, or a shape that `got` has, gives `None`, and the report writes the type.
    fn clashing_shape(&self, want: Type, got: Type) -> Option<Expected> {
        let shape = match (want.kind(), got.kind()) {
            (TypeKind::Array(_), TypeKind::Array(_)) => return None,
            (TypeKind::Array(_), _) => Expected::Array,
            (TypeKind::Class(a, _), TypeKind::Class(b, _)) if a == b => return None,
            (TypeKind::Struct(a, _), TypeKind::Struct(b, _)) if a == b => return None,
            (TypeKind::Enum(a, _), TypeKind::Enum(b, _)) if a == b => return None,
            (TypeKind::Class(id, _), _) => Expected::Named(self.head(Head::Class(id)).to_string()),
            (TypeKind::Enum(id, _), _) => Expected::Named(self.head(Head::Enum(id)).to_string()),
            _ => return None,
        };
        // An argument with an open part of its own shows its type, as on `main`.
        (want.holds_infer() && !got.holds_infer()).then_some(shape)
    }

    /// Whether a member of type `ty`, at a place that is `filled` with the variable `var` of its
    /// type parameter, decides nothing of it: it meets `filled`, and `var` stays an open `Any`.
    fn decides_nothing(&mut self, ty: Type, filled: Type, var: Type) -> bool {
        let snapshot = self.table.snapshot();
        let open = self.unify(filled, ty) && {
            let ty = self.table.resolve(var);
            matches!(ty.kind(), TypeKind::Infer(_)) && self.table.default_open(ty) == ty
        };
        self.table.rollback(snapshot);
        open
    }

    /// `ty` with each associated type in it replaced by a new `Any` variable made at `at`: a
    /// place of a join, which decides the parameters that the associated types are written
    /// through.
    fn assoc_open(&mut self, ty: Type, at: Span) -> Type {
        if !ty.has_assoc() {
            return ty;
        }
        let list = |c: &mut Self, list: TypeArgs| {
            let opened: Vec<Type> = (list.get().iter())
                .map(|&arg| c.assoc_open(arg, at))
                .collect();
            TypeArgs::intern(&opened)
        };
        match ty.kind() {
            TypeKind::Assoc(..) => self.table.fresh(Kind::Any, at),
            TypeKind::Array(element) => Type::array(self.assoc_open(element, at)),
            TypeKind::Optional(payload) => Type::optional(self.assoc_open(payload, at)),
            TypeKind::Class(id, args) => Type::intern(TypeKind::Class(id, list(self, args))),
            TypeKind::Struct(id, args) => Type::intern(TypeKind::Struct(id, list(self, args))),
            TypeKind::Enum(id, args) => Type::intern(TypeKind::Enum(id, list(self, args))),
            TypeKind::Any(id, args) => Type::intern(TypeKind::Any(id, list(self, args))),
            _ => ty,
        }
    }

    /// An argument at a place of a decided parameter that is a join of its own (see
    /// `takes_member`): it meets the type that the members decided, each open `Int` as `i64`. A
    /// parameter that they left open takes the argument's own type, except under a method call,
    /// which meets the open type. In a call that `expected` makes a member, the method call is a
    /// member too.
    fn late_argument(
        &mut self,
        call: &Inference<'_>,
        i: usize,
        e: &ast::Expr,
        (param, expected): (Type, Expectation),
    ) -> Checked {
        let with: Vec<Type> = (call.bindings.found.iter())
            .map(|&ty| ty.unwrap_or(NO_BINDING))
            .collect();
        let filled = self.table.resolve(param.subst(&with));
        let filled = self.normalize(filled).unwrap_or(filled);
        let closed = self.table.default_open(filled);
        if !closed.holds_infer() {
            let _ = self.unify(filled, closed);
            return self.known_argument(call, i, e, param);
        }
        // A method call is no join: the parts that are still open stay open in its context. An
        // associated type of an open parameter is no context: the argument decides the parameter.
        // The members may decide a part of the context. That part reaches each call inside a
        // call or an array argument, which keeps its open parts open as a member does.
        let shaped = |e: &ast::Expr| {
            matches!(
                &e.kind,
                ast::ExprKind::Call { .. } | ast::ExprKind::ArrayLit(_)
            )
        };
        let informed = shaped(e)
            && (with.iter().enumerate()).any(|(k, &ty)| {
                names_outside_assoc(param, k)
                    && !matches!(self.table.resolve(ty).kind(), TypeKind::Infer(_))
            });
        let method = matches!(e.kind, ast::ExprKind::MethodCall { .. });
        // In a call that is a member, a method call, a call or an array literal is a member too,
        // at each depth: a parameter that it cannot infer stays open, and the join around the
        // outer call gives it (`id(first(b.make(true))) == o`). So is a block whose value is one
        // (`id({ b.opt(true) }) == o`). A method call that several methods answer waits for its
        // context to pick one, and a closed place keeps its own check.
        let value = block_value(e);
        let member = expected == Expectation::Member
            && !self.body.closed_place
            && !filled.has_assoc()
            && (shaped(value)
                || matches!(value.kind, ast::ExprKind::MethodCall { .. })
                    && !self.picks_by_context(value));
        if (informed || method) && !member && !filled.has_assoc() {
            let outer = self.body.late_argument;
            if !matches!(param.kind(), TypeKind::Param(..)) {
                self.body.late_argument = Some(e.span);
            }
            let checked = self.known_argument(call, i, e, param);
            self.body.late_argument = outer;
            return checked;
        }
        // An array literal takes the open context as its type, so that each element meets its part
        // with its own wraps (`[[None], [1]]` at `[[T?]]`). A parameter `T` or `[T]` gives the
        // elements nothing: they join, as with no context, so that a `[]` waits for its siblings
        // (`[[], 1]`). A part that a failure reached gives it none: the elements report their
        // own mistakes only.
        // `dbg(x)` gives `x` its context, so the literal inside it is checked the same way.
        let bare = |ty: Type| matches!(ty.kind(), TypeKind::Param(..));
        let mut shown = e;
        while let ast::ExprKind::Call { callee, args, .. } = &shown.kind
            && callee.name.as_str() == "dbg"
            && let [arg] = args.as_slice()
        {
            shown = &arg.value;
        }
        if matches!(&shown.kind, ast::ExprKind::ArrayLit(elements) if !elements.is_empty())
            && !bare(param)
            && !matches!(param.kind(), TypeKind::Array(inner) if bare(inner))
            && !filled.has_assoc()
            && !(with.iter().enumerate())
                .any(|(k, &ty)| names_outside_assoc(param, k) && self.table.reported_open(ty))
        {
            let origin = call.declarer.origin(i);
            let checked = mark(self.operand(e, Expectation::Required(filled, origin)));
            return self.refuse_valueless(read_jump(checked));
        }
        let context = if member {
            Expectation::Member
        } else {
            Expectation::None
        };
        let checked = mark(self.operand(e, context));
        let checked = self.refuse_valueless(read_jump(checked));
        match type_of(&checked) {
            Some(ty) if !self.meets_late(filled, ty, e.span) => {
                let origin = call.declarer.origin(i);
                let want = self.table.resolve(filled);
                let want = self
                    .table
                    .default_open(self.normalize(want).unwrap_or(want));
                let got = self.shown_against(ty, want);
                let d = self.mismatch(e.span, None, want, got, origin);
                Err(mismatch_poison(self.diagnostics.error(d)))
            }
            _ => checked,
        }
    }

    /// The type arguments of the open form of `call` at `candidate`: each open parameter takes the
    /// part of `candidate` at its place in the result type. `None` when the result does not meet
    /// `candidate` then (two places that give two parts), or a bound of an open parameter fails.
    /// It binds what the result meets: the caller holds a snapshot. With the types, it gives each
    /// bound whose subject still holds an open variable, which settling must keep as an obligation.
    fn open_types(&mut self, call: OpenCall, candidate: Type) -> Option<(Vec<Type>, Vec<Waits>)> {
        let entry = self.table.open_entry(call);
        let (ret, mut with) = (entry.ret, entry.types.clone());
        let candidate = self.table.resolve(candidate);
        for &(name, t) in &entry.params {
            with[t] = part_at(entry.marked, name, candidate)?;
        }
        let bounds = entry.bounds.clone();
        if !self.unify(ret.subst(&with), candidate) {
            return None;
        }
        let mut waits = Vec::new();
        for (t, bound, at) in bounds {
            let part = self.table.resolve(with[t]);
            if part.holds_infer() {
                waits.push((part, bound, at));
                continue;
            }
            let args = self.subst_args(bound.args, &with);
            if !self.conforms(part, bound.interface, args) {
                return None;
            }
        }
        Some((with, waits))
    }

    /// The wraps that the open form of `call`, the call node `e`, takes at `candidate`, or `None`.
    /// Each member converts to its parameter's type with the open parameters filled, and takes at
    /// most one wrap; the result itself takes none. It binds what it unifies: `join` calls it
    /// under a snapshot.
    pub(super) fn open_at(
        &mut self,
        call: OpenCall,
        e: &mut tir::Expr,
        candidate: Type,
    ) -> Option<usize> {
        let (with, waits) = self.open_types(call, candidate)?;
        // A later member can close a bound that waits: the join tests it with the candidate.
        for (part, bound, _) in waits {
            self.body.candidate_bounds.push((part, bound, with.clone()));
        }
        let members = self.table.open_entry(call).members.clone();
        let mut wraps = 0;
        let mut args = ArgumentsAt::of(e);
        for (at, param) in members {
            let arg = args.as_mut()?.get(at)?;
            wraps += self.converts_at(arg, param.subst(&with))?;
        }
        Some(wraps)
    }

    /// Drops the held conflict of `call`, at `e`, when one of `candidates` opens it. Each test runs
    /// under a snapshot, which it rolls back.
    pub(super) fn drop_held_if_opened(
        &mut self,
        call: OpenCall,
        e: &mut tir::Expr,
        candidates: &[Type],
    ) {
        if self.table.open_entry(call).conflict.is_none() {
            return;
        }
        let opens = candidates.iter().any(|&candidate| {
            let snapshot = self.table.snapshot();
            let opens = self.open_at(call, e, candidate).is_some();
            self.table.rollback(snapshot);
            opens
        });
        if opens {
            self.table.open_entry_mut(call).conflict = None;
        }
    }

    /// Reports the held conflict of `call` once (see `OpenEntry`), and gives its proof. A rollback
    /// does not hold it again: a check of the call again makes a new entry.
    pub(super) fn report_held(&mut self, call: OpenCall) -> Option<Reported> {
        let (held, reported) = self.table.open_entry_mut(call).conflict.take()?;
        self.diagnostics.restore(held);
        Some(reported)
    }

    /// Reports each held conflict that no join settled or reported, so that none is lost: the
    /// net at the end of a function.
    pub(super) fn report_held_conflicts(&mut self) {
        for call in self.table.unsettled_opens() {
            let _reported = self.report_held(call);
        }
    }

    /// Settles the open form of `call`, the call node `e`, at `candidate`, which `open_at` took:
    /// each member gets its `Wrap` and `ToAny`, and the node its type and its type arguments.
    pub(super) fn settle_open(
        &mut self,
        call: OpenCall,
        e: &mut tir::Expr,
        candidate: Type,
    ) -> Result<(), Reported> {
        let Some((with, waits)) = self.open_types(call, candidate) else {
            return Err(self.proof(e.span)); // proof: quiet reader
        };
        self.table.settle(e.span);
        // The open form can give a bound a type with an open variable. That bound is checked once
        // the variable has its type, as `bound_obligations` does for the closed form.
        let resolved: Vec<Type> = with.iter().map(|&ty| self.table.resolve(ty)).collect();
        for (part, bound, at) in waits {
            let id = ObligationId(self.obligations.len());
            self.obligations.push(BoundObligation {
                at,
                bound,
                on: part,
                with: resolved.clone(),
                call: None,
            });
            let snapshot = self.table.snapshot();
            self.table.obligation(part, id);
            self.keep_snapshot(snapshot);
        }
        let entry = self.table.open_entry(call);
        let (ret, members) = (entry.ret, entry.members.clone());
        let mut outcome = Ok(());
        let mut args = ArgumentsAt::of(e);
        for (at, param) in members {
            if let Some(arg) = args.as_mut().and_then(|args| args.get(at)) {
                let done = self.settle_at(arg, param.subst(&with));
                outcome = outcome.and(done);
            }
        }
        e.ty = self.table.resolve(ret.subst(&with));
        if let tir::ExprKind::Call { types, .. } = &mut e.kind {
            let resolved: Vec<Type> = with.iter().map(|&ty| self.table.resolve(ty)).collect();
            *types = TypeArgs::intern(&resolved);
        }
        outcome
    }

    /// Whether each bound of an open form that the candidate of a join closes holds: the bounds
    /// in `candidate_bounds[from..]`, which it removes. A bound whose subject or arguments are
    /// still open holds here, as it is checked when its last open part closes. The bound of a
    /// closed form is an obligation, which reports on its own (E0154).
    pub(super) fn candidate_bounds_hold(&mut self, from: usize) -> bool {
        let bounds = self.body.candidate_bounds.split_off(from);
        bounds.into_iter().all(|(on, bound, with)| {
            let with: Vec<Type> = with.iter().map(|&t| self.table.resolve(t)).collect();
            !self.bound_breaks(on, bound, &with)
        })
    }

    /// Whether the bound `bound` on `on`, with the type arguments `with`, is known to fail: `on`
    /// and the bound's arguments hold no open variable, and `on` does not conform. It reports
    /// nothing.
    pub(super) fn bound_breaks(&mut self, on: Type, bound: Bound, with: &[Type]) -> bool {
        let (on, args) = (self.table.resolve(on), self.subst_args(bound.args, with));
        !on.holds_infer() && !args.holds_infer() && !self.conforms(on, bound.interface, args)
    }

    /// Whether the type `ty` of a late argument meets `filled`. An associated type of an open
    /// parameter meets the argument once the argument decides that parameter. One that the
    /// decided type cannot give meets it too: the parameter's bound reports.
    fn meets_late(&mut self, filled: Type, ty: Type, at: Span) -> bool {
        if !filled.has_assoc() {
            return self.unify(filled, ty);
        }
        // A value meets an optional as its payload: the wrap comes after.
        let (filled, ty) = match (filled.kind(), ty.kind()) {
            (TypeKind::Optional(inner), kind) if !matches!(kind, TypeKind::Optional(_)) => {
                (inner, ty)
            }
            _ => (filled, ty),
        };
        let open = self.assoc_open(filled, at);
        if !self.unify(open, ty) {
            return false;
        }
        let filled = self.table.resolve(filled);
        self.normalize(filled)
            .is_none_or(|filled| self.unify(filled, ty))
    }

    /// The expectation that `e` takes as a member of a call's join, or `None` when it waits. It is
    /// `Expectation::Member`, or `Expectation::None` for a method call that is closed. Such a
    /// value has its own type, or a type that stays open where the join decides it: an integer
    /// literal, a `None`, a `[]`, a generic call or method call, a case of a generic enum
    /// (`Two.P(5, 7)`). An `if`, a `match` or a `loop` whose values join is a nested join: its
    /// branches or `break` values are members, and so is an array, whose elements are members.
    /// An operator and a `??` are members when their operands are. A value that waits meets what
    /// the members decide (see `late_argument`). A call or an array that has such an argument or
    /// element waits too. A block does when its value does.
    ///
    /// A method call with no argument is closed, unless it waits for its context to pick among
    /// conformances (`Rank::Waits`). A method call that checks its receiver as a place is closed.
    /// Under `closed`, at a place that names two or more decided parameters (Q28), a method call
    /// with arguments waits: it meets its parameter type, so that the part that is wrong is
    /// reported (`h(true, C().m("s", 1))` at `"s"`). So does such a call in an argument that
    /// waits at such a place, as `h(true, id(c?.m("s", 1)))` (`Body::closed_place`).
    fn takes_member(&mut self, e: &ast::Expr, closed: bool) -> Option<Expectation> {
        if let Some(&found) = self.body.members.get(&(e.span, closed)) {
            return found;
        }
        #[cfg(test)]
        MEMBER_WALKS.set(MEMBER_WALKS.get() + 1);
        let found = self.member_walk(e, closed);
        self.body.members.insert((e.span, closed), found);
        found
    }

    /// `takes_member` for `e`, found by a walk of it.
    fn member_walk(&mut self, e: &ast::Expr, closed: bool) -> Option<Expectation> {
        use ast::ExprKind as K;
        match &e.kind {
            // An array is a nested join, unless an element waits.
            K::ArrayLit(elements) => (elements.iter())
                .all(|e| self.takes_member(e, closed).is_some())
                .then_some(Expectation::Member),
            K::ArrayFill { value, .. } => self
                .takes_member(value, closed)
                .map(|_| Expectation::Member),
            K::Call { args, .. } => (args
                .iter()
                .all(|arg| self.takes_member(&arg.value, closed).is_some()))
            .then_some(Expectation::Member),
            K::MethodCall {
                receiver,
                method,
                types,
                args,
                safe: false,
                ..
            } if types.is_empty() && self.generic_variant(receiver, method).is_some() => (args
                .iter()
                .all(|arg| self.takes_member(&arg.value, closed).is_some()))
            .then_some(Expectation::Member),
            // `Option.Some(x)` is `Some(x)`, as `generic_callee` takes it.
            K::MethodCall {
                receiver,
                method,
                types,
                args,
                safe: false,
                ..
            } if types.is_empty()
                && method.name == Symbol::SOME
                && (option_written(receiver) == Some(false)
                    || matches!(receiver.kind, K::Name(n) if self.names_option(n))) =>
            {
                (args
                    .iter()
                    .all(|arg| self.takes_member(&arg.value, closed).is_some()))
                .then_some(Expectation::Member)
            }
            // A method call that checks its receiver as a place is not ranked (see `call_shape`):
            // it is a member closed, and a literal argument takes its default type.
            K::MethodCall {
                receiver,
                method,
                safe,
                ..
            } if self.parks_receiver(receiver, method.name, *safe) => Some(Expectation::None),
            // A method call with no argument is a member closed, as a local is, unless it waits
            // for its context to pick a method.
            K::MethodCall { args, types, .. }
                if args.is_empty() && types.is_empty() && self.sibling_rank(e) != Rank::Waits =>
            {
                Some(Expectation::None)
            }
            K::MethodCall { args, .. }
                if !args.is_empty() && !closed && self.sibling_rank(e) != Rank::Waits =>
            {
                (args
                    .iter()
                    .all(|arg| self.takes_member(&arg.value, closed).is_some()))
                .then_some(Expectation::Member)
            }
            // Ranking checks the statements in the block's scope, where its value is seen, and
            // keeps that work for the block's check. The value is walked in that kept scope, as
            // the walk can rank a nested block or a receiver, which reads names. With no kept
            // scope, the block waits: its value is not walked outside its scope.
            K::Block(block) => {
                self.rank_value(e, &mut true);
                let Some(value) = tail(block) else {
                    return Some(Expectation::Member);
                };
                let index = *self.body.ahead_at.get(&(block.span, true))?;
                let Some(Ahead::Block(scope, _)) = &mut self.body.ahead[index].1 else {
                    return None;
                };
                let scope = std::mem::take(scope);
                self.body.scopes.push(scope);
                let found = self.takes_member(value, closed);
                let scope = (self.body.scopes.pop())
                    .expect("the block's scope should be open while its value is walked");
                if let Some(Ahead::Block(kept, _)) = &mut self.body.ahead[index].1 {
                    *kept = scope;
                }
                found
            }
            // A branch join or a loop is a nested join, and an `if` with no `else` is a `unit`
            // member, which no optional lifts (E0043). At a place that names two decided
            // parameters each waits, as a call there is closed (Q28).
            K::If { .. } | K::IfLet { .. } | K::Match { .. } | K::Loop { .. } => {
                (!closed).then_some(Expectation::Member)
            }
            // A comparison or a logic operator is a `bool`, which is a member as `true` is.
            K::Binary {
                op: ast::BinaryOp::And | ast::BinaryOp::Or | ast::BinaryOp::Eq | ast::BinaryOp::Ne,
                ..
            } => Some(Expectation::Member),
            // A `-` is a member when its operand is: it converts at its top.
            K::Unary {
                op: ast::UnaryOp::Neg,
                operand,
            } => self
                .takes_member(operand, closed)
                .map(|_| Expectation::Member),
            // An arithmetic operator is a member when its operands are: it converts at its top.
            K::Binary { lhs, rhs, .. } => (self.takes_member(lhs, closed).is_some()
                && self.takes_member(rhs, closed).is_some())
            .then_some(Expectation::Member),
            // A `??` is a member when its default is: the left side is optional by itself.
            K::Coalesce { rhs, .. } => self.takes_member(rhs, closed).map(|_| Expectation::Member),
            K::MethodCall { .. } => None,
            _ => Some(Expectation::Member),
        }
    }

    /// The expectation of a waiting argument `e` at a place of type `param`, when it is a member of
    /// its join all the same. Under a hint or a required optional, the context could decide the
    /// parameter before the call's own arguments. Its `?` is not in the program. So an argument
    /// that gives itself a type (`Rank::Concrete`) is a member then, checked with no context: the
    /// call decides each parameter from its own arguments (Q22). These stay out, and keep their
    /// reports:
    /// - a value that is an optional by itself, as a safe call: the context's `?` is its own;
    /// - a value that never finishes, and an `if` with no `else` (E0043);
    /// - an array literal with elements at a place that is not `T` or `[T]`, whose elements meet
    ///   their part (see `late_argument`).
    fn own_typed(
        &mut self,
        e: &ast::Expr,
        param: Type,
        expected: Expectation,
    ) -> Option<Expectation> {
        let adds = match expected {
            Expectation::Hint { .. } => true,
            Expectation::Required(want, _) => matches!(want.kind(), TypeKind::Optional(_)),
            Expectation::None | Expectation::Poisoned | Expectation::Member => false,
        };
        if !adds {
            return None;
        }
        let bare = |ty: Type| matches!(ty.kind(), TypeKind::Param(..));
        let mut shown = e;
        while let ast::ExprKind::Call { callee, args, .. } = &shown.kind
            && callee.name.as_str() == "dbg"
            && let [arg] = args.as_slice()
        {
            shown = &arg.value;
        }
        let elements = matches!(&shown.kind, ast::ExprKind::ArrayLit(elements) if !elements.is_empty())
            && !bare(param)
            && !matches!(param.kind(), TypeKind::Array(inner) if bare(inner));
        let no_else = matches!(
            &e.kind,
            ast::ExprKind::If {
                else_block: None,
                ..
            } | ast::ExprKind::IfLet {
                else_block: None,
                ..
            }
        );
        let ranked = self.rank_value(e, &mut true);
        (ranked.rank == Rank::Concrete
            && !ranked.optional
            && !ranked.never
            && !elements
            && !no_else)
            .then_some(Expectation::None)
    }

    /// Whether an argument of `call` that is still to check can make an optional by itself, or
    /// holds one (see `Ranked`). Ranking stops at a `Concrete` value, so such a value can, unless
    /// it is an operator, whose value is never optional.
    fn late_optional(&mut self, call: &Inference<'_>) -> bool {
        (call.slots.iter().zip(call.args)).any(|(slot, arg)| {
            let e = &arg.value;
            let operator = matches!(
                e.kind,
                ast::ExprKind::Binary { .. } | ast::ExprKind::Unary { .. }
            );
            let ranked = slot.is_none().then(|| self.rank_value(e, &mut true));
            ranked.is_some_and(|r| {
                r.rank == Rank::Concrete && !operator || r.optional || r.holds_optional
            })
        })
    }

    /// An argument at a parameter of type `param`, which the call knows: it meets that type.
    fn known_argument(
        &mut self,
        call: &Inference<'_>,
        i: usize,
        e: &ast::Expr,
        param: Type,
    ) -> Checked {
        let Some(want) = self.known_in(call, param) else {
            return mark(self.operand(e, Expectation::Poisoned));
        };
        let want = self.table.resolve(want);
        // A mismatch shows the type as its declaration writes it. The text exists only for a type
        // that names no type parameter (see `resolve_field`), so `param` is that type.
        if let Some(text) = self.declared_text(call.declarer, i) {
            self.show(e.span, param, &text);
        }
        let origin = inferred_origin(call, param).unwrap_or_else(|| call.declarer.origin(i));
        let checked = mark(self.operand(e, Expectation::Required(want, origin)));
        self.refuse_valueless(read_jump(checked))
    }

    /// The report of the first member in `valued` whose part at the type parameter `t` is closed
    /// and breaks a bound of `t`. The bound's arguments are closed too. Each entry of `valued` is
    /// a member's place filled with `var`, its type and its span. A closed call checks it before
    /// its join (Q22), so that E0154, and no conflict, is the report, as `main` gives. It changes
    /// `with[t]`, which the caller puts back.
    fn broken_member(
        &mut self,
        sig: &GenericSig<'_>,
        (t, var): (usize, Type),
        with: &mut [Type],
        valued: &[(Type, Type, Span)],
    ) -> Option<Reported> {
        let bounds = sig.bounds.get(t - sig.owner.len())?;
        if bounds.list.is_empty() {
            return None;
        }
        let mut parts = Vec::new();
        for &(filled, ty, at) in valued {
            let snapshot = self.table.snapshot();
            let part = self.unify(filled, ty).then(|| {
                let part = self.table.resolve(var);
                // A bare literal resolves to `var`, which is an `Any` again after the rollback.
                let literal = matches!(part.kind(), TypeKind::Infer(_))
                    && self.table.default_open(part) != part;
                (part, literal)
            });
            self.table.rollback(snapshot);
            if let Some((part, literal)) = part {
                let part = if literal {
                    self.table.fresh(Kind::Int, at)
                } else {
                    part
                };
                parts.push((part, at));
            }
        }
        // The closed parts, each once: an open part meets each of them at most once.
        let mut closed = Vec::new();
        let mut seen = FxHashSet::default();
        for &(part, _) in &parts {
            if !part.holds_infer() && seen.insert(part) {
                closed.push(part);
            }
        }
        // Whether each bound names a parameter other than `t`, found once for all the parts.
        let names_other: Vec<bool> = (bounds.list.iter())
            .map(|bound| {
                (bound.args.get().iter())
                    .flat_map(|&arg| params_named(arg, Reach::Everywhere))
                    .any(|k| k != t)
            })
            .collect();
        for &(part, at) in &parts {
            // A part that is an open `Any` variable takes its type from the join, which checks the
            // bound. A part that holds an `Int` inside, as `Boxed<Int>`, breaks the bound when it
            // does with each `Int` as `i64`, as on `main`. A closed part of another member that
            // meets it decides it first. A bare `Int`, as `5`, is `i64` only when no other part is
            // closed. A closed part decides the join, and the join reports a literal that it does
            // not take. No number type conforms to an interface that `i64` does not.
            let bare = matches!(part.kind(), TypeKind::Infer(_));
            if bare && self.table.default_open(part) == part {
                continue;
            }
            if part.holds_infer()
                && (closed.iter()).any(|&other| {
                    let snapshot = self.table.snapshot();
                    let meets = bare || self.unify(part, other);
                    self.table.rollback(snapshot);
                    meets
                })
            {
                continue;
            }
            let part = self.table.default_open(part);
            with[t] = part;
            // Each bound that the part breaks is a report, as on `main`. A bound that names
            // another parameter is checked only when no bound of the part alone breaks.
            for others in [false, true] {
                let mut broken = None;
                for (&bound, &named) in bounds.list.iter().zip(&names_other) {
                    if named != others {
                        continue;
                    }
                    let args = self.subst_args(bound.args, with);
                    if args.holds_infer() || part.holds_infer() {
                        continue;
                    }
                    if !self.conforms(part, bound.interface, args)
                        && let Err(reported) =
                            self.check_bound_of(at, part, bound, with, None, None)
                    {
                        broken.get_or_insert(reported);
                    }
                }
                if broken.is_some() {
                    return broken;
                }
            }
        }
        None
    }

    /// Checks each bound of a decided parameter of `call` whose type is closed, and gives the
    /// first failure, which poisons the call. Each other bound becomes an obligation on its
    /// variable, which runs once the variable's type holds no open variable (see
    /// `Checker::report_waiting`). A bound of a parameter in `by_context` reports at the call.
    fn bound_obligations(
        &mut self,
        call: &Inference<'_>,
        (sig, index, by_context): (&GenericSig<'_>, &CallIndex, &[bool]),
        decided: &[(usize, Type)],
        (span, failed): (Span, Option<Reported>),
    ) -> Option<Reported> {
        let owned = sig.owner.len();
        let mut broken = None;
        // A failed call decides nothing more: each open `Int` of it is `i64`, as on main.
        let close = |c: &Self, ty: Type| match failed {
            Some(_) => c.table.default_open(ty),
            None => c.table.resolve(ty),
        };
        // The type arguments are the same for each parameter: they are made once.
        let mut with = None;
        for &(t, var) in decided {
            let Some(bounds) = sig.bounds.get(t - owned).filter(|b| !b.list.is_empty()) else {
                continue;
            };
            let at = if by_context[t] {
                span
            } else {
                bound_site(call, &index.members[t], span)
            };
            let (with, open) = with.get_or_insert_with(|| {
                let with: Vec<Type> = (call.bindings.found.iter())
                    .map(|&ty| close(self, listed(ty)))
                    .collect();
                let open = with.iter().any(|t| t.holds_infer());
                (with, open)
            });
            let ty = close(self, var);
            for &bound in &bounds.list {
                // A bound on a closed type is checked at the call: a failed one poisons it.
                if !ty.holds_infer() && !*open {
                    if let Err(reported) = self.check_bound_of(at, ty, bound, with, None, None) {
                        broken.get_or_insert(reported);
                    }
                    continue;
                }
                let id = ObligationId(self.obligations.len());
                self.obligations.push(BoundObligation {
                    at,
                    bound,
                    on: var,
                    with: with.clone(),
                    call: None,
                });
                let snapshot = self.table.snapshot();
                self.table.obligation(var, id);
                self.keep_snapshot(snapshot);
            }
        }
        broken
    }

    /// Meets `expected`, the call's context, after the members decided what they could. Under a
    /// required type the return type meets the whole type first (unify before wrap), also for a
    /// parameter that no argument decides. A unification that breaks a bound gives way to the wrap.
    /// Under a hint it meets the payload of an optional first, so that the call's value is wrapped.
    /// As the guide of a late argument (`whole`) it meets the whole context first, which takes the
    /// most values, and checks no bound: the late arguments decide more, and the bounds are checked
    /// when the call is closed. A hint can add a `?` to the type of its siblings (see `branch.rs`),
    /// which they did not write. A late argument that can make an optional takes that `?`
    /// (`first(m.mkl())` for `-> [X?]`). Else the call meets the siblings' type first, and the hint
    /// only when that fails, as for `id(None)` beside a `1`. Under a required optional, the late
    /// arguments are closed first (Q26). When none can make an optional, the call meets the payload
    /// first, and its result wraps once at the top. An example is `fst(p.mkb())` under `i64?`,
    /// where `mkb` gives `Pr<i64, X>`. `None` when the call is a member of an outer join, which
    /// decides its variables, or has no context to meet; else whether it met it.
    fn meet_context(
        &mut self,
        call: &Inference<'_>,
        sig: &GenericSig<'_>,
        decided: &[(usize, Type)],
        (expected, whole): (Expectation, bool),
    ) -> Option<bool> {
        let want = match expected {
            Expectation::Hint { want, siblings, .. } if whole && !self.late_optional(call) => {
                (siblings, want)
            }
            Expectation::Required(want, _) if whole && !self.late_optional(call) => {
                (payload(want), want)
            }
            Expectation::Required(want, _) | Expectation::Hint { want, .. } => (want, want),
            Expectation::Member | Expectation::None | Expectation::Poisoned => return None,
        };
        let (first, want) = want;
        let ret = sig.ret?;
        if want.holds_hole() || decided.is_empty() {
            return None;
        }
        let optional = matches!(want.kind(), TypeKind::Optional(_));
        // The value of a safe call is optional: a context that is not cannot meet it.
        if self.body.safe_call == Some(call.at) && !optional {
            return Some(false);
        }
        let with: Vec<Type> = (call.bindings.found.iter())
            .map(|&ty| self.table.resolve(listed(ty)))
            .collect();
        let ret = ret.subst(&with);
        // Unify before wrap: under a required type, the result meets the whole type first, also
        // for a parameter that no argument decides, as the `T` of `none()`. A unification that
        // breaks a bound gives way to the wrap. A hint meets the payload first (Task 5c). As a
        // guide (`whole`), the call is no conversion point yet: its late arguments decide more, and
        // a bound is checked when the call is closed.
        let whole_first = whole || matches!(expected, Expectation::Required(..));
        let open_before: Vec<bool> = (decided.iter())
            .map(|&(_, var)| self.table.resolve(var).holds_infer())
            .collect();
        let mut meets = |want: Type| {
            let optional = matches!(want.kind(), TypeKind::Optional(_));
            if !whole_first {
                return self.unify(ret, payload(want)) || optional && self.unify(ret, want);
            }
            if whole || !optional {
                return self.unify(ret, want) || optional && self.unify(ret, payload(want));
            }
            let snapshot = self.table.snapshot();
            if self.unify(ret, want)
                && self.ready_bounds_hold(snapshot)
                && self.closed_bounds_hold(call, sig, decided, &open_before)
            {
                self.keep_snapshot(snapshot);
                return true;
            }
            self.table.rollback(snapshot);
            // The wrap, else the unification that breaks a bound: the bound reports (E0154).
            self.unify(ret, payload(want)) || self.unify(ret, want)
        };
        Some(meets(first) || first != want && meets(want))
    }

    /// Whether each bound of a parameter in `decided` holds, for each parameter that was open
    /// before (`open_before`) and that the result's unification closed. Such a bound is checked
    /// after the context is met (`bound_obligations`), so the unification tests it first.
    fn closed_bounds_hold(
        &mut self,
        call: &Inference<'_>,
        sig: &GenericSig<'_>,
        decided: &[(usize, Type)],
        open_before: &[bool],
    ) -> bool {
        let owned = sig.owner.len();
        let mut with = None;
        (decided.iter().zip(open_before)).all(|(&(t, var), &open)| {
            let bounds = sig.bounds.get(t - owned).map_or(&[][..], |b| &b.list[..]);
            if !open || bounds.is_empty() {
                return true;
            }
            let with: &Vec<Type> = with.get_or_insert_with(|| {
                (call.bindings.found.iter())
                    .map(|&ty| self.table.resolve(listed(ty)))
                    .collect()
            });
            (bounds.iter()).all(|&bound| !self.bound_breaks(var, bound, with))
        })
    }

    /// Closes the decided parameters of `call` at the top. A member of an outer join leaves them
    /// open: that join decides them. Else each open `Int` takes `i64`, and a parameter that is
    /// still open is E0050. A result that did not meet a required type (`met`) is left to the code
    /// that uses it, unless its parameter has one member (`lone`): then the mismatch is reported
    /// at that member, against the part of the required type at it (S26b).
    fn close_call(
        &mut self,
        call: &mut Inference<'_>,
        sig: &GenericSig<'_>,
        (decided, lone): (&[(usize, Type)], &[Lone]),
        (expected, met): (Expectation, Option<bool>),
    ) -> Result<(), Poisoned> {
        if expected == Expectation::Member {
            let in_result = result_names(sig);
            // A parameter that the result names and that is still open waits for the outer call.
            // That call reports it when it settles no type for it.
            let waits: Vec<(Type, OpenParam)> = (decided.iter())
                .filter(|&&(t, var)| in_result[t] && self.table.resolve(var).holds_infer())
                .map(|&(t, var)| (var, OpenParam::of(sig, t)))
                .collect();
            if waits.is_empty() {
                self.body.waits.remove(&super::wait_key(call.at));
            } else {
                self.body.waits.insert(super::wait_key(call.at), waits);
                // A function call with a labeled argument failed (E0055). On `main` it is then
                // checked as alone, so it reports its own parameter at any place.
                if matches!(call.declarer, Declarer::Function(_))
                    && call.args.iter().any(|arg| arg.label.is_some())
                {
                    self.body.labeled_waits.insert(call.at);
                }
            }
            // The outer join reaches a parameter only through the result: one that the result
            // does not name gets its type from no context, so it is E0050 now. The value keeps
            // its type, which does not name the parameter.
            for &(t, var) in decided {
                let ty = self.table.resolve(var);
                if sig.ret.is_none() || in_result[t] || !ty.holds_infer() {
                    continue;
                }
                let closed = self.table.default_open(ty);
                if closed != ty {
                    let _ = self.unify(ty, closed);
                }
                if self.table.resolve(var).holds_infer() {
                    if self
                        .report_waiting_calls(call, sig, (t, ty, usize::MAX))
                        .is_none()
                    {
                        self.cannot_infer(sig, t);
                    }
                    // One E0050 for the call: its other parameters report nothing more.
                    for &(_, var) in decided {
                        self.table.poison(var);
                    }
                    break;
                }
            }
            return Ok(());
        }
        if let Some(poisoned) = self.required_mismatch(call, sig, lone, (expected, met)) {
            return Err(poisoned);
        }
        for &(_, var) in decided {
            let ty = self.table.resolve(var);
            let closed = self.table.default_open(ty);
            if closed != ty {
                let _ = self.unify(ty, closed);
            }
        }
        for &(t, var) in decided {
            let ty = self.table.resolve(var);
            call.bindings.found[t] = Some(ty);
            if ty.holds_infer() {
                // An inner call reported the parameter that this one holds (see the `Member` case).
                if self.table.reported_open(ty) {
                    let reported = self.proof(call.at); // proof: quiet reader
                    return Err(Poisoned::untyped(reported));
                }
                self.table.poison(ty);
                // A parameter's or the result type that is not found may be what would decide it.
                // So may an argument at a place of `t` that failed with no type: a `mut` argument
                // that is no place.
                let failed_at = |(slot, param): (&Option<Checked>, &Option<Type>)| {
                    matches!(slot, Some(Err(p)) if p.ty.is_none())
                        && param.is_some_and(|param| names_outside_assoc(param, t))
                };
                if sig.params.contains(&None)
                    || sig.ret.is_none()
                    || call.slots.iter().zip(sig.params).any(failed_at)
                {
                    let reported = self.proof(call.at); // proof: declaration
                    return Err(Poisoned::untyped(reported));
                }
                if expected == Expectation::Poisoned {
                    let reported = self.proof(call.at); // proof: quiet reader
                    return Err(Poisoned::untyped(reported));
                }
                // A misfit needs a known context, and a parameter that the result names and that
                // no member gave a part of: else the open part is the fault, as `None` for `B` is.
                if met == Some(false)
                    && matches!(ty.kind(), TypeKind::Infer(_))
                    && sig.ret.is_some_and(|ret| names_outside_assoc(ret, t))
                    && let Expectation::Required(want, _) | Expectation::Hint { want, .. } =
                        expected
                    && self.known_context(call, want)
                {
                    return Err(self.misfit(call, sig, t));
                }
                let reported = (self.report_waiting_calls(call, sig, (t, ty, usize::MAX)))
                    .unwrap_or_else(|| self.cannot_infer(sig, t));
                return Err(Poisoned::untyped(reported));
            }
        }
        Ok(())
    }

    /// E0050 at each waiting call among the first `before` arguments of `call` at places of the
    /// parameter `t`: an argument that is a generic call or construction whose type holds `ty`,
    /// which `call` leaves open. Each reports its own parameter, as on `main`, where such a call
    /// waits for the others and is then checked with no context. A call at a parameter type that
    /// is `t` alone, or one inside another value, does not wait there: `call` reports `t`. `None`
    /// when no such call reports.
    fn report_waiting_calls(
        &mut self,
        call: &Inference<'_>,
        sig: &GenericSig<'_>,
        (t, ty, before): (usize, Type, usize),
    ) -> Option<Reported> {
        let roots = self.table.open_roots(ty);
        let holds = |table: &super::infer::Table, ty: Type| {
            table.open_roots(ty).iter().any(|r| roots.contains(r))
        };
        let waiting: Vec<Span> = (call.slots.iter().zip(sig.params).take(before))
            .filter(|(_, param)| {
                param.is_some_and(|param| {
                    names_outside_assoc(param, t) && !matches!(param.kind(), TypeKind::Param(..))
                })
            })
            .filter_map(|(slot, _)| slot.as_ref()?.as_ref().ok())
            // `dbg(x)` gives `x` its context, so `x` waits in its place.
            .map(|mut e| {
                while let tir::ExprKind::Dbg { operand, .. } = &e.kind {
                    e = operand;
                }
                e
            })
            .filter(|e| {
                matches!(
                    e.kind,
                    tir::ExprKind::Call { .. }
                        | tir::ExprKind::Struct { .. }
                        | tir::ExprKind::Variant { .. }
                        | tir::ExprKind::New { .. }
                ) && holds(&self.table, e.ty)
            })
            .map(|e| e.span)
            .collect();
        let labeled = (call.args.iter().take(before))
            .map(|arg| arg.value.span)
            .filter(|span| self.body.labeled_waits.contains(span));
        let waiting: Vec<Span> = labeled.chain(waiting).collect();
        let mut first = None;
        for span in waiting {
            let Some(waits) = self.body.waits.remove(&super::wait_key(span)) else {
                continue;
            };
            if let Some((_, open)) = waits.into_iter().find(|&(var, _)| holds(&self.table, var)) {
                // Each waiting call reports, as on `main`; the first is the call's report.
                let reported = self.error(open.kind(true));
                first.get_or_insert(reported);
            }
        }
        if first.is_some() {
            self.table.poison(ty);
        }
        first
    }

    /// After the call `call` failed: E0050 for the first parameter that no failure reached and that
    /// a waiting call in one of its members leaves open. The outer call settles no type now, so
    /// the waiting call gets none, as on `main`. A `None` or `[]` member waits for nothing.
    fn report_left_open(
        &mut self,
        call: &mut Inference<'_>,
        sig: &GenericSig<'_>,
        decided: &[(usize, Type)],
        quiet: &[usize],
    ) {
        if sig.params.contains(&None) || sig.ret.is_none() {
            return;
        }
        for &(t, var) in decided.iter().filter(|(t, _)| !quiet.contains(t)) {
            let ty = self.table.resolve(var);
            if !ty.holds_infer() || self.table.reported_open(ty) {
                continue;
            }
            let waits = (call.slots.iter_mut().zip(sig.params))
                .filter(|(_, param)| param.is_some_and(|param| names_outside_assoc(param, t)))
                .filter_map(|(slot, _)| slot.as_mut()?.as_mut().ok())
                .any(|e| super::join::waiting_call(&self.table, e, ty));
            if waits {
                self.cannot_infer(sig, t);
                for &(_, var) in decided {
                    self.table.poison(var);
                }
                return;
            }
        }
    }

    /// The mismatch of a result that does not meet the required type of `expected`. It is
    /// reported at the one member of the result's parameter (S26b, see `lone_mismatch`). `None`
    /// when the result converts, or no such member exists.
    fn required_mismatch(
        &mut self,
        call: &Inference<'_>,
        sig: &GenericSig<'_>,
        lone: &[Lone],
        (expected, met): (Expectation, Option<bool>),
    ) -> Option<Poisoned> {
        let Expectation::Required(want, origin) = expected else {
            return None;
        };
        if met != Some(false) {
            return None;
        }
        let with: Vec<Type> = (call.bindings.found.iter())
            .map(|&ty| self.table.resolve(listed(ty)))
            .collect();
        // A result that converts to `want` at the top, as `Circle` to `any Shape`, fits.
        let ret = sig.ret.map(|ret| self.table.resolve(ret.subst(&with)));
        let snapshot = self.table.snapshot();
        let converts = ret.is_some_and(|ret| {
            let ret = self.table.default_open(ret);
            self.conversion(ret, want).is_some()
        });
        self.table.rollback(snapshot);
        if converts {
            return None;
        }
        let reported = self.lone_mismatch(sig, (lone, &with), want, origin)?;
        Some(Poisoned {
            mismatch: true,
            ..Poisoned::typed(want, reported)
        })
    }

    /// The `mut` places of `call` against `want`, the type of `expected`, before the other
    /// arguments. A place binds the parameters that its parameter names (see `infer_call`): that is
    /// the closed form. When its result does not convert to `want`, the call is open at `want`, and
    /// a place is no member of the open form (Q19). So each parameter that a place binds, and that
    /// the call does not write, takes the type that `want` makes of it. As in `meet_context`, a
    /// required optional comes whole first, and a hint's payload first. Each place that does not
    /// have the type of its parameter then gets E0170, and the other arguments meet the types that
    /// `want` gave. Nothing changes when a place failed already, or when `want` does not decide the
    /// type of each place. `named` is the count of the type arguments that the call writes.
    fn mut_against_context(
        &mut self,
        call: &mut Inference<'_>,
        sig: &GenericSig<'_>,
        named: usize,
        expected: Expectation,
    ) {
        let (Some(site), Some(ret)) = (sig.mut_site, sig.ret) else {
            return;
        };
        let (Expectation::Required(want, _) | Expectation::Hint { want, .. }) = expected else {
            return;
        };
        let held = |i: usize| call.mut_shapes.iter().any(|&(k, _)| k == i);
        if (0..call.slots.len())
            .any(|i| site.muts[i] && matches!(call.slots[i], Some(Err(_))) && !held(i))
        {
            return;
        }
        let snapshot = self.table.snapshot();
        // A parameter that no place binds is open in both forms.
        let mut with: Vec<Type> = (call.bindings.found.iter())
            .map(|&ty| match ty.filter(|&ty| ty != *REJECTED) {
                Some(ty) => self.table.resolve(ty),
                None => self.table.fresh(Kind::Any, call.at),
            })
            .collect();
        let closed = self.table.resolve(ret.subst(&with));
        let closed = self.table.default_open(closed);
        if self.conversion(closed, want).is_some() {
            self.table.rollback(snapshot);
            return;
        }
        // Each parameter that a place binds is open again, for the context to decide.
        let written = sig.owner.len() + named;
        let mut opened = vec![false; with.len()];
        for pattern in (call.mut_shapes.iter()).filter_map(|&(i, _)| sig.params[i]) {
            for (t, ty) in with.iter_mut().enumerate().skip(written) {
                if !opened[t] && names_outside_assoc(pattern, t) {
                    opened[t] = true;
                    *ty = self.table.fresh(Kind::Any, call.at);
                }
            }
        }
        let ret = ret.subst(&with);
        // Unify before wrap, as `meet_context` does: under a required type the places take the
        // whole type first, when no other argument can decide their parameters. The other
        // arguments are not checked yet: in `put(mut names, 1)` the `1` decides `T`, so the
        // result wraps. A hint meets its payload first (Task 5c).
        let decided_elsewhere = (0..call.args.len()).any(|i| {
            !site.muts[i]
                && sig.params[i].is_some_and(|param| {
                    (opened.iter().enumerate())
                        .any(|(t, &open)| open && names_outside_assoc(param, t))
                })
        });
        let (first, then) = match expected {
            Expectation::Required(..) if !decided_elsewhere => (want, payload(want)),
            _ => (payload(want), want),
        };
        let snapshot_first = self.table.snapshot();
        let mut meets = self.unify(ret, first);
        if !meets && first != then {
            self.table.rollback(snapshot_first);
            meets = self.unify(ret, then);
        }
        let wants: Vec<Option<Type>> = (call.mut_shapes.iter())
            .map(|&(i, _)| {
                let ty = self.table.resolve(sig.params[i]?.subst(&with));
                (meets && !ty.holds_infer()).then_some(ty)
            })
            .collect();
        let decided: Vec<Option<Type>> = (with.iter().zip(&opened))
            .map(|(&ty, &open)| {
                let ty = self.table.resolve(ty);
                (open && !ty.holds_infer()).then_some(ty)
            })
            .collect();
        self.table.rollback(snapshot);
        if wants.contains(&None) {
            return;
        }
        let mut misfit = false;
        for ((i, shape), fit) in std::mem::take(&mut call.mut_shapes).into_iter().zip(wants) {
            let (Some(pattern), Some(fit)) = (sig.params[i], fit) else {
                continue;
            };
            if shape.writes(fit) {
                call.mut_shapes.push((i, shape));
                continue;
            }
            misfit = true;
            let reported =
                self.report_mut_type(&call.args[i].value, &shape, (site, i), (fit, pattern));
            call.failed.get_or_insert(reported);
            call.slots[i] = Some(Err(mismatch_poison(reported)));
        }
        // The call failed at its places: it checks no bound of these types.
        if misfit {
            // The context binds these parameters, not the `mut` argument: no label names it.
            for (t, ty) in decided.into_iter().enumerate() {
                if ty.is_some() {
                    call.bindings.found[t] = ty;
                    call.origin.set(t, None);
                }
            }
        }
    }

    /// The E0032 of S26b: the result type `sig.ret` is a decided parameter or its optional, which
    /// has one member in `lone`, and `want` does not take it. The mismatch stands at that member,
    /// against the part of `want` at it. `with` is the call's type arguments so far.
    fn lone_mismatch(
        &mut self,
        sig: &GenericSig<'_>,
        (lone, with): (&[Lone], &[Type]),
        want: Type,
        origin: Origin,
    ) -> Option<Reported> {
        let ret = sig.ret?;
        let (bare, wanted) = match ret.kind() {
            // An optional result of a function never converts to a `want` that is not optional:
            // the call reports as a whole, as on `main`. `Some(1)` under `string` reports at `1`.
            TypeKind::Optional(_)
                if matches!(sig.declarer, Declarer::Function(_))
                    && !matches!(want.kind(), TypeKind::Optional(_)) =>
            {
                return None;
            }
            TypeKind::Optional(inner) => (inner, payload(want)),
            _ => (ret, want),
        };
        let ret_bare = bare;
        let TypeKind::Param(_, t) = bare.kind() else {
            return None;
        };
        // A bound of `T` takes the member's own type, so the call reports as a whole, as on
        // `main`.
        let bounded = (t as usize)
            .checked_sub(sig.owner.len())
            .and_then(|k| sig.bounds.get(k))
            .is_some_and(|bounds| !bounds.list.is_empty());
        if bounded {
            return None;
        }
        let &(_, at, got, param) = lone.iter().find(|&&(i, ..)| i == t as usize)?;
        let param = param?;
        let mut with = with.to_vec();
        with[t as usize] = wanted;
        let member_want = param.subst(&with);
        let got = self.table.resolve(got);
        // A member whose type differs only by the wraps of `wanted` makes the result take too
        // many (Q24): the call reports as a whole. For a whole member, only an open one counts,
        // as `1` in `id(1)` under `u8??`. A closed one, as `g: G` in `id(g)` under `G??`, gets the
        // directed report, as on `main`. A part, as `kp(5)` in `snd(kp(5))`, counts open or
        // closed: its wraps are inside the member's place.
        let mut unwrapped = wanted;
        while let TypeKind::Optional(inner) = unwrapped.kind() {
            unwrapped = inner;
        }
        with[t as usize] = unwrapped;
        let bare = param.subst(&with);
        let snapshot = self.table.snapshot();
        let wraps_only = unwrapped != wanted
            && (param != ret_bare || got.holds_infer())
            && self.unify(got, bare);
        self.table.rollback(snapshot);
        let got = self.table.default_open(got);
        if got == member_want || wraps_only || wanted.holds_infer() || member_want.holds_infer() {
            return None;
        }
        let d = self.mismatch(at, None, member_want, got, origin);
        Some(self.diagnostics.error(d))
    }

    /// Compares each `mut` argument's place with its parameter's type once inference is done,
    /// associated types resolved: the types must be the same, as no conversion applies to a
    /// `mut` argument, and the unification of the argument took a payload for an optional. A
    /// place that goes through `?.` is reported here too: its label is picked from the final
    /// type. `list` is the type arguments, when the call inferred them all.
    fn mut_final(
        &mut self,
        call: &mut Inference<'_>,
        sig: &GenericSig<'_>,
        args: &[ast::Arg],
        list: Option<TypeArgs>,
    ) {
        let Some(site) = sig.mut_site else { return };
        for (i, shape) in std::mem::take(&mut call.mut_shapes) {
            let pattern = sig.params[i].expect("a `mut` argument should have a parameter type");
            let want = match list {
                Some(list) => self
                    .subst(pattern, list.get())
                    .and_then(|ty| self.normalize(ty)),
                None => Some(self.printed(&call.bindings, pattern)),
            };
            let Some(want) = want else { continue };
            let chain = shape.has_chain();
            if !chain && (list.is_none() || shape.writes(want)) {
                continue;
            }
            let reported = self.report_mut_type(&args[i].value, &shape, (site, i), (want, pattern));
            if !chain {
                call.slots[i] = Some(Err(mismatch_poison(reported)));
            }
        }
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
        let count = |i: usize| {
            (i.checked_sub(owned).and_then(|k| bounds.get(k))).map_or(0, |b| b.list.len())
        };
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
            let bound = bounds[i - owned].list[j];
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
            if let Err(reported) =
                self.check_bound_of(origin.get(i).unwrap_or(at), ty, bound, with, None, None)
            {
                *failed = Some(reported);
                rejected.push(i);
            }
        }
        for i in rejected {
            bindings.reject(i);
            if let Some(with) = &mut early.with {
                with[i] = NO_BINDING;
            }
        }
    }

    /// Whether each parameter that `context` binds meets its bounds (`Checker::conforms`, which
    /// reports only a query that is too deep). A bound whose arguments name a parameter that
    /// `context` leaves unbound is not decided yet, as in `check_early`, and holds here. The work
    /// is one query for each bound of the callee.
    fn meets_bounds(&mut self, all: &[Bounds], owned: usize, context: &Bindings) -> bool {
        let with: Vec<Type> = context.found.iter().map(|&t| listed(t)).collect();
        for (k, bounds) in all.iter().enumerate() {
            let Some(ty) = context.found[owned + k].filter(|&t| t != *REJECTED) else {
                continue;
            };
            for bound in &bounds.list {
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

    /// The type of argument `i` as `declarer` writes it, when it names an alias.
    fn declared_text(&self, declarer: Declarer, i: usize) -> Option<Rc<str>> {
        match declarer {
            Declarer::None => None,
            Declarer::Function(f) => self.signatures[f.0].param_shown.get(i)?.clone(),
            Declarer::Fields(owner) => self.owner_info(owner).fields.get(i)?.shown.clone(),
            Declarer::Variant(id, variant) => {
                self.enums[id.0].variants[variant].shown.get(i)?.clone()
            }
        }
    }

    /// `Bindings::unify`, reporting at `at` each parameter it binds to a type that no type
    /// parameter takes.
    pub(super) fn bind_param(
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
            call.origin.set_first(i, at, call.arg);
            let ty = call.bindings.found[i].expect("a fresh parameter should be bound");
            // The context rejected it, and the outer call reported why.
            if ty == *REJECTED {
                if call.rejected.is_none() {
                    call.rejected = Some(
                        failure(call).unwrap_or_else(|| self.proof(at)), // proof: quiet reader
                    );
                }
            } else if ty == Type::NEVER {
                call.failed = Some(self.error(DiagnosticKind::TypeArgument {
                    ty: self.type_name(ty),
                    at,
                }));
            }
        }
        if call.early.is_some() {
            // `check_early` leaves `fresh` as it is.
            let fresh = std::mem::take(&mut call.bindings.fresh);
            self.check_early(call, &fresh, at);
            call.bindings.fresh = fresh;
        }
    }

    /// Whether `want`, the context of `call`, is known enough to report a result that cannot meet
    /// it. It holds no open variable, or `call` is a safe call. No context that is not optional
    /// meets the optional value of a safe call, whatever its open parts become. Or `call` is the
    /// late argument of an outer call (`Body::late_argument`), and each open part is an `Any`:
    /// then the result clashed with a known part, as `main` reports (`Two<_, bool>` and
    /// `Two<_, string>`).
    fn known_context(&self, call: &Inference<'_>, want: Type) -> bool {
        let want = self.table.resolve(want);
        !want.holds_infer()
            || self.body.safe_call == Some(call.at)
                && !matches!(want.kind(), TypeKind::Optional(_) | TypeKind::Infer(_))
            || self.body.late_argument == Some(call.at)
                && !matches!(want.kind(), TypeKind::Infer(_))
                && self.table.default_open(want) == want
    }

    /// The failed `call`, whose result could not meet its known context and left its parameter
    /// `param` open. The code that uses the value reports the mismatch with the result type, as
    /// for a callee that is not generic; the call's E0050 waits in `Body::misfits` until that
    /// report cancels it (see `Misfit`). An open parameter shows as a hole.
    fn misfit(&mut self, call: &Inference<'_>, sig: &GenericSig<'_>, param: usize) -> Poisoned {
        let with: Vec<Type> = (call.bindings.found.iter())
            .map(|&ty| {
                let ty = self.table.resolve(listed(ty));
                if ty.holds_infer() { *HOLE } else { ty }
            })
            .collect();
        let ret = sig.ret.map_or(*HOLE, |ret| ret.subst(&with));
        // The result of a safe call is optional (see `Checker::member`).
        let ty = if self.body.safe_call == Some(call.at) {
            super::as_optional(ret)
        } else {
            ret
        };
        let entry = self.body.misfits.len();
        let in_result = sig.ret.is_some_and(|ret| names_outside_assoc(ret, param));
        let d = OpenParam::of(sig, param).kind(in_result).into();
        self.body.misfits.push(Some(d));
        let reported = self.proof(call.at); // proof: held misfit
        Poisoned {
            found: Some(super::Misfit { ty, entry }),
            ..Poisoned::untyped(reported)
        }
    }

    /// Reports the call of `sig` that binds nothing to its parameter at `param`.
    fn cannot_infer(&mut self, sig: &GenericSig<'_>, param: usize) -> Reported {
        let in_result = sig.ret.is_some_and(|ret| names_outside_assoc(ret, param));
        let kind = OpenParam::of(sig, param).kind(in_result);
        self.error(kind)
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
        for (key, ..) in self.body.ahead.drain(mark..) {
            self.body.ahead_at.remove(&key);
        }
        result
    }

    /// `rank_value`, for a span not ranked yet.
    #[expect(
        clippy::too_many_lines,
        reason = "the match names each expression kind, so a new kind must choose its rank"
    )]
    #[deny(clippy::wildcard_enum_match_arm)]
    fn rank_walk(&mut self, e: &ast::Expr, fit: &mut bool) -> Ranked {
        use ast::ExprKind as K;
        match &e.kind {
            _ if self.bare_none_written(e) => Ranked {
                optional: true,
                holds_optional: true,
                ..Ranked::WAITS
            },
            _ if self.int_literal(e) => Ranked::LITERAL,
            K::ArrayLit(elements) => {
                let found = combine(elements.iter().map(|e| self.rank_value(e, fit)));
                Ranked {
                    optional: false,
                    nested: false,
                    scalar: false,
                    ..found
                }
            }
            K::ArrayFill { value, .. } => Ranked {
                optional: false,
                nested: false,
                scalar: false,
                leads: true,
                ..combine([self.rank_value(value, fit)]).or_literal_if_waiting()
            },
            K::Binary { op, lhs, rhs, .. }
                if super::expr::operator(*op).is_some_and(|(_, ty)| ty == Type::I64) =>
            {
                Ranked {
                    leads: true,
                    ..combine(
                        [&**lhs, &**rhs]
                            .into_iter()
                            .map(|e| self.rank_value(e, fit)),
                    )
                    .or_literal_if_waiting()
                }
            }
            K::Unary {
                op: ast::UnaryOp::Neg,
                operand,
            } => Ranked {
                leads: true,
                ..combine([self.rank_value(operand, fit)]).or_literal_if_waiting()
            },
            K::Field {
                base,
                name,
                safe: false,
            } => {
                // A unit variant through an alias holds what the alias builds: `BE.E`.
                match self
                    .generic_variant(base, name)
                    .filter(|v| v.fields.is_empty())
                {
                    Some(v) => Ranked {
                        holds_optional: v.built.holds_optional(),
                        ..Ranked::WAITS
                    },
                    None => Ranked::CONCRETE,
                }
            }
            // A user item cannot be named `panic` (E0034), and `call` tries the prelude first.
            K::Call { callee, .. } if callee.name.as_str() == "panic" => Ranked::NEVER,
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
                let ty = self.value_ahead(scrutinee);
                let form = LetValue::Match {
                    ty: ty.filter(|&ty| matchable(ty)),
                };
                combine_branches(arms.iter().map(|arm| {
                    let bind =
                        |c: &mut Self| Some(c.bind_pattern(None, &form, &arm.pattern, Some(arm)));
                    if let K::Block(block) = &arm.body.kind {
                        self.block_rank(block, fit, bind)
                    } else {
                        Some(self.tail_rank(arm_key(arm), None, &arm.body, fit, bind))
                    }
                }))
            }
            K::Block(block) => self
                .block_rank(block, fit, |_| None)
                .unwrap_or(Ranked::NEVER),
            K::Loop { breaks, .. } => Ranked {
                never: !breaks,
                ..Ranked::CONCRETE
            },
            K::Todo { .. } => Ranked::NEVER,
            K::Int { .. }
            | K::Float(_)
            | K::Bool(_)
            | K::Char(_)
            | K::Str(_)
            | K::Interpolate(_)
            | K::Name(_)
            | K::Applied { .. }
            | K::SelfValue
            | K::NoneLit
            | K::Field { safe: true, .. }
            | K::Unary { .. }
            | K::Binary { .. }
            | K::Compare { .. }
            | K::Coalesce { .. }
            | K::Index { .. }
            | K::Range(_)
            | K::Is { .. }
            | K::If {
                else_block: None, ..
            }
            | K::IfLet {
                else_block: None, ..
            }
            | K::Return { .. }
            | K::Break { .. }
            | K::Continue { .. } => Ranked::CONCRETE,
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
            ret_optional,
            places,
            muts,
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
                let ranked = if has_place(muts, i) && is_place(&arg.value) {
                    Ranked::CONCRETE
                } else {
                    self.rank_value(&arg.value, fit)
                };
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
        found.holds_optional = holds || found.optional || ret_optional && found.rank == Rank::Waits;
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
        // The receiver of a call that may take `mut self` is such a place too.
        // shortcut: a plain generic method that shares a `mut self` name is not ranked, and a
        // literal argument of such a method takes its default type (E0032); upgrade by testing
        // the receiver's declared type when such a call needs the context.
        if let K::MethodCall {
            receiver,
            method,
            safe,
            ..
        } = &e.kind
            && self.parks_receiver(receiver, method.name, *safe)
        {
            if method.name.as_str() != "push" {
                self.unranked_mut_self.insert(method.name);
            }
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
                #[cfg(test)]
                super::reached("answered_by_several");
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
                let shape = if let Some(v) = variant {
                    // Mixed labels are reported at the declaration.
                    let shaped = !v.fields.is_empty()
                        && (v.labels.iter().all(Option::is_some)
                            || v.labels.iter().all(Option::is_none))
                        && fits(v.fields.iter().copied().zip(v.labels.iter().copied()), args);
                    let open = if shaped {
                        open_params(v.fields.iter().copied(), &v.names, 0)
                    } else {
                        Vec::new()
                    };
                    CallShape {
                        generic: true,
                        shaped,
                        open: !open.is_empty(),
                        ret_optional: v.built.holds_optional(),
                        places: carried_by_construction(v.fields.iter().copied(), v.names.len()),
                        ..CallShape::default()
                    }
                } else if !safe
                    && method.name == Symbol::SOME
                    && (option_written(receiver) == Some(false)
                        || matches!(receiver.kind, ast::ExprKind::Name(n) if self.names_option(n)))
                {
                    // `Option.Some` is `Some`, as `generic_callee` takes it.
                    CallShape {
                        generic: true,
                        shaped: matches!(&args[..], [arg] if arg.label.is_none()),
                        open: false,
                        makes: Makes::some(),
                        ret_optional: false,
                        places: Places::FIRST,
                        muts: 0,
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
            } if types.is_empty() => {
                let shape = self.generic_callee(callee.name, args);
                Some((shape, args))
            }
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
        // An argument is not part of the chain that an index ends.
        let indexed = std::mem::take(&mut self.body.indexed);
        let ((checked, form, _), jumped) =
            self.jumps(|c| c.let_value(Unwrap::IfLet, pattern, value));
        self.body.indexed = indexed;
        let index = self.keep((value.span, false), None, jumped);
        let then = self.block_rank(then_block, fit, |c| {
            Some(c.bind_pattern(Some(Unwrap::IfLet), &form, pattern, None))
        });
        self.body.ahead[index].1 = Some(Ahead::Let(checked, form));
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
            Some(StmtKind::Expr(tail)) if tail.is_jump() => None,
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
        // Neither a pattern's guard nor a statement is part of the chain that an index ends.
        let indexed = std::mem::take(&mut self.body.indexed);
        let mut so_far = BlockSoFar::default();
        let (bound, jumped) = self.jumps(|c| {
            let bound = bind(c);
            if let Some(block) = block {
                c.block_statements(block, Expectation::None, &mut so_far, block.stmts.len() - 1);
            }
            bound
        });
        self.body.indexed = indexed;
        // Its place, held before the value's ranking keeps work of its own.
        let index = self.keep((key, true), None, jumped);
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
    /// with its place in `body.ahead`. If the work made an invalid jump, the flag is set again.
    pub(super) fn kept(&mut self, span: Span, block: bool) -> Option<(usize, Ahead)> {
        let index = *self.body.ahead_at.get(&(span, block))?;
        #[cfg(test)]
        INSPECTED.set(INSPECTED.get() + 1);
        let (_, work, jumped) = &mut self.body.ahead[index];
        let work = work.take()?;
        // The work did its jump when ranking ran it, and the check that uses it must see that.
        // `|=`, not `=`: the flag of the window may be set already.
        self.body.invalid_jump |= *jumped;
        Some((index, work))
    }

    /// Keeps `work` in `body.ahead` for `key`, a span and whether it is a block's, and returns
    /// its place there. `jumped`: whether the work made a jump outside a loop (see `jumps`).
    fn keep(&mut self, key: (Span, bool), work: Option<Ahead>, jumped: bool) -> usize {
        let index = self.body.ahead.len();
        self.body.ahead.push((key, work, jumped));
        self.body.ahead_at.insert(key, index);
        index
    }

    /// The variant `base.member` of a generic enum that `base` names without type arguments, as
    /// a call of it binds the enum's type parameters. Through an alias, they are the alias's
    /// parameters, and the payload has the types that its arguments for the enum's make.
    pub(super) fn generic_variant(
        &mut self,
        base: &ast::Expr,
        member: &ast::Ident,
    ) -> Option<VariantShape> {
        let ast::ExprKind::Name(name) = base.kind else {
            return None;
        };
        let Some(Declared::Known(id)) = self.names.enum_id(name) else {
            let Via::To(through) = self.through(name) else {
                return None;
            };
            if !self.alias_visible(name) {
                return None;
            }
            let TypeKind::Enum(id, list) = through.ty.kind() else {
                return None;
            };
            let info = &self.enums[id.0];
            let &variant = info.index.get(&member.name)?;
            let v = &info.variants[variant];
            return (!through.params.is_empty()).then(|| VariantShape {
                built: through.ty,
                names: through.params.clone(),
                fields: (v.fields.iter())
                    .map(|ty| ty.and_then(|ty| self.subst(ty, list.get())))
                    .collect(),
                labels: v.labels.clone(),
            });
        };
        let info = &self.enums[id.0];
        let &variant = info.index.get(&member.name)?;
        (!info.type_params.is_empty()).then(|| VariantShape {
            built: Type::intern(TypeKind::Enum(id, own_args(&info.type_params))),
            names: info.type_params.clone(),
            fields: info.variants[variant].fields.clone(),
            labels: info.variants[variant].labels.clone(),
        })
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
            TypeKind::Class(..) | TypeKind::Struct(..) | TypeKind::Enum(..) => {}
            TypeKind::Param(..) | TypeKind::Assoc(..) | TypeKind::Any(..) => {
                return self.bound_shape(ty, method, args);
            }
            _ => return CallShape::default(),
        }
        // The method the call will find, as `find_method` finds it, without its reports.
        let func = match self.applicable_method(ty, method.name) {
            Some(Declared::Known(func)) => func,
            Some(Declared::Poisoned(_)) => return CallShape::default(),
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
        let open = open_params_of_call(sig.params.iter().copied(), ret, &sig.type_params, owner);
        let mut with = self.owner_args(func, ty).get().to_vec();
        with.extend(own_params(&sig.type_params, owner));
        let ret_optional =
            (sig.ret.and_then(|ret| self.subst(ret, &with))).is_some_and(Type::holds_optional);
        CallShape {
            generic: sig.own > 0,
            shaped,
            open: !open.is_empty(),
            makes: Makes::of(sig.ret, sig.params.iter().copied()),
            ret_optional,
            places: carried(
                sig.ret,
                sig.params.iter().copied(),
                sig.type_params.len(),
                owner,
            ),
            ..CallShape::default()
        }
    }

    /// Whether `e` is a method call that several methods answer: only its context picks one, so
    /// it cannot stay open for a join.
    fn picks_by_context(&mut self, e: &ast::Expr) -> bool {
        matches!(&e.kind, ast::ExprKind::MethodCall { receiver, method, safe, .. }
            if self.answered_by_several(receiver, method, *safe))
    }

    /// Whether the value of `e` is a call that holds, in an argument, a method call that several
    /// methods answer. Only a type from outside picks that method (`id(s.name()) == "a"`). The
    /// walk goes through each call between and each block that is only its value. A receiver
    /// that is a place is no such call. A method call by itself is not one: its receiver is not
    /// checked ahead for each comparison. A block with a statement is not looked into: its
    /// receiver can read a binding of the block, which is not in scope ahead of the block's check.
    pub(super) fn waits_for_pick(&mut self, e: &ast::Expr) -> bool {
        match &lone_value(e).kind {
            ast::ExprKind::Call { args, .. } => args.iter().any(|arg| {
                let value = lone_value(&arg.value);
                match &value.kind {
                    ast::ExprKind::MethodCall {
                        receiver,
                        method,
                        safe,
                        ..
                    } => {
                        !self.parks_receiver(receiver, method.name, *safe)
                            && self.picks_by_context(value)
                    }
                    _ => self.waits_for_pick(value),
                }
            }),
            _ => false,
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
            TypeKind::Param(..) | TypeKind::Assoc(..) | TypeKind::Any(..) => {
                self.required(ty, method.name).len() > 1
            }
            TypeKind::Class(..) | TypeKind::Struct(..) | TypeKind::Enum(..) => {
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
                let open = open_params_of_call(params, ret, &names, owner);
                let mut with = vec![receiver];
                with.extend_from_slice(one.args.get());
                with.extend(own_params(&names, owner));
                let ret_optional = (requirement.ret.and_then(|ret| self.subst(ret, &with)))
                    .is_some_and(Type::holds_optional);
                CallShape {
                    generic: !requirement.own.is_empty(),
                    shaped,
                    open: !open.is_empty(),
                    makes: Makes::of(requirement.ret, requirement.params.iter().copied()),
                    ret_optional,
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
            _ => self.value(e, Expectation::None),
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
        // An argument is not part of the chain that an index ends.
        let indexed = std::mem::take(&mut self.body.indexed);
        let (checked, jumped) = self.jumps(|c| c.value(e, Expectation::None));
        self.body.indexed = indexed;
        let ty = type_of(&checked);
        self.keep((e.span, false), Some(Ahead::Receiver(checked)), jumped);
        ty
    }

    /// The shape of a call of `name` with `args` that calls a generic function, constructs a
    /// generic class, or builds `Some`, `Ok` or `Err`, or calls `dbg`.
    fn generic_callee(&mut self, name: Symbol, args: &[ast::Arg]) -> CallShape {
        // `dbg<T>(x: T) -> T`: its type is its argument's, so it waits when that argument waits;
        // its parameter names `T`, so it is not `open`. One argument is the shape, with or
        // without a label: the label is E0055, and the argument still waits.
        // It comes first, as `call` runs the prelude's call before it looks up a user's item. A
        // user's item named `dbg` is rejected, and `call` then poisons the call.
        if name.as_str() == "dbg" {
            return CallShape {
                generic: true,
                shaped: args.len() == 1,
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
        let holder = match (self.names.struct_id(name), self.names.class(name)) {
            (Some(Declared::Known(id)), _) => Some(FieldOwner::Struct(id)),
            (_, Some(Declared::Known(class))) => Some(FieldOwner::Class(class)),
            _ => None,
        };
        match (holder, self.names.function(name)) {
            (Some(owner), _) => {
                let info = self.owner_info(owner);
                let shaped = fits(info.fields.iter().map(|f| (f.ty, Some(f.name.name))), args);
                let open = if shaped {
                    open_params(info.fields.iter().map(|f| f.ty), &info.type_params, 0)
                } else {
                    Vec::new()
                };
                CallShape {
                    generic: !info.type_params.is_empty(),
                    shaped,
                    open: !open.is_empty(),
                    places: carried_by_construction(
                        info.fields.iter().map(|f| f.ty),
                        info.type_params.len(),
                    ),
                    ..CallShape::default()
                }
            }
            (None, Some(Declared::Known(func))) => {
                let sig = &self.signatures[func.0];
                let shaped =
                    sig.ret.is_some() && fits(sig.params.iter().map(|&ty| (ty, None)), args);
                let ret = sig.ret.filter(|_| shaped);
                let open =
                    open_params_of_call(sig.params.iter().copied(), ret, &sig.type_params, 0);
                CallShape {
                    generic: !sig.type_params.is_empty(),
                    shaped,
                    open: !open.is_empty(),
                    makes: Makes::of(sig.ret, sig.params.iter().copied()),
                    ret_optional: sig.ret.is_some_and(Type::holds_optional),
                    places: carried(
                        sig.ret,
                        sig.params.iter().copied(),
                        sig.type_params.len(),
                        0,
                    ),
                    muts: mut_places(&sig.muts),
                }
            }
            // `Some<T>(x: T) -> T?`, and the prelude's `Ok` and `Err`, which no user item can
            // take the name of.
            (None, None) if name == Symbol::SOME => CallShape {
                generic: true,
                shaped: matches!(args, [arg] if arg.label.is_none()),
                open: false,
                makes: Makes::some(),
                ret_optional: false,
                places: Places::FIRST,
                muts: 0,
            },
            // An alias of a generic class, whose parameters its uses bind.
            (None, None) if let Via::To(through) = self.through(name) => {
                self.alias_callee(&through, args)
            }
            (None, None) => self
                .result_variant(name)
                .map_or_else(CallShape::default, |(id, v)| {
                    let info = &self.enums[id.0];
                    let shaped = matches!(args, [arg] if arg.label.is_none());
                    let fields = || info.variants[v].fields.iter().copied();
                    let open = if shaped {
                        open_params(fields(), &info.type_params, 0)
                    } else {
                        Vec::new()
                    };
                    CallShape {
                        generic: true,
                        shaped,
                        open: !open.is_empty(),
                        places: carried_by_construction(fields(), info.type_params.len()),
                        ..CallShape::default()
                    }
                }),
            _ => CallShape::default(),
        }
    }

    /// The shape of a call of `through`, the alias of a class that its arguments construct: the
    /// class's fields, with the types that the alias's arguments for its parameters make them.
    fn alias_callee(&self, through: &Through, args: &[ast::Arg]) -> CallShape {
        let TypeKind::Class(class, list) = through.ty.kind() else {
            return CallShape::default();
        };
        let fields: Vec<(Option<Type>, Option<Symbol>)> = (self.classes[class.0].fields.iter())
            .map(|f| (f.ty, Some(f.name.name)))
            .map(|(ty, name)| (ty.and_then(|ty| self.subst(ty, list.get())), name))
            .collect();
        let shaped = fits(fields.iter().copied(), args);
        let open = if shaped {
            open_params(fields.iter().map(|f| f.0), &through.params, 0)
        } else {
            Vec::new()
        };
        CallShape {
            generic: !through.params.is_empty(),
            shaped,
            open: !open.is_empty(),
            places: carried_by_construction(fields.iter().map(|f| f.0), through.params.len()),
            ret_optional: through.ty.holds_optional(),
            ..CallShape::default()
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
    /// parameters are `declared`. `Err` with the token of the report when they do not fit
    /// `declared`, or one is `Error` or cannot be a type argument. A list after a name that takes
    /// none is reported, and the name stands without it.
    pub(super) fn type_args(
        &mut self,
        name: &ast::Ident,
        declared: &[Symbol],
        written: &[ast::TypeExpr],
        angles: Span,
    ) -> Result<TypeArgs, Reported> {
        self.type_args_of(name, declared, written, angles, true)
    }

    /// Resolves the type arguments `written` after a poisoned name, so that their own errors stay.
    /// The name gives no count and no bound to check them against: a repair may change both.
    pub(super) fn resolve_args(&mut self, written: &[ast::TypeExpr]) {
        for arg in written {
            let _ = self.type_argument(arg);
        }
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
    ) -> Result<TypeArgs, Reported> {
        let form = match (declared.len(), written.len()) {
            (0, 0) => return Ok(TypeArgs::NONE),
            (0, _) => {
                self.not_generic(name.name, angles);
                return Ok(TypeArgs::NONE);
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
                    .collect::<Result<Vec<_>, Reported>>()
                    .map(|args| TypeArgs::intern(&args));
            }
        };
        let at = match form {
            ArgumentsForm::Missing { .. } => name.span,
            _ => angles,
        };
        Err(self.error(DiagnosticKind::TypeArguments {
            name: name.name.to_string(),
            at,
            form,
        }))
    }

    /// Reports the type arguments in `angles` after `name`, which takes none: the fix removes
    /// them.
    pub(super) fn not_generic(&mut self, name: Symbol, angles: Span) {
        let safe = !self.comment_in(&[angles.range()]);
        self.error(DiagnosticKind::TypeArguments {
            name: name.to_string(),
            at: angles,
            form: ArgumentsForm::NotGeneric { safe },
        });
    }

    /// The type of the type argument `arg`: `Err`, reported, for `never`, which no type
    /// parameter takes, and for a type that is `Error`.
    fn type_argument(&mut self, arg: &ast::TypeExpr) -> Result<Type, Reported> {
        if let ast::TypeKind::Name(name) = arg.kind
            && name.as_str() == "never"
            && self.body.type_param(name).is_none()
            && self.names.class(name).is_none()
            && self.names.struct_id(name).is_none()
            && self.names.enum_id(name).is_none()
            && self.names.alias(name).is_none()
        {
            return Err(self.error(DiagnosticKind::TypeArgument {
                ty: arg.to_string(),
                at: arg.span,
            }));
        }
        // `resolve` reported the error of the written type, or its declaration did, but it gives
        // no token.
        self.resolve(arg).ok_or_else(|| self.proof(arg.span)) // proof: declaration
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
        self.error(DiagnosticKind::MainSignature {
            at: angles,
            form: MainForm::TypeParams { remove },
        });
    }

    /// Reports `op` on the operand at `at`, of type `ty`, which names a type parameter that lacks
    /// the operator.
    pub(super) fn param_lacks(&mut self, at: Span, ty: Type, op: impl fmt::Display) -> Reported {
        let d = self.lacks(at, ty, op);
        self.diagnostics.error(d)
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
            && self.names.class(n).is_none()
            && self.names.struct_id(n).is_none()
            && self.names.enum_id(n).is_none()
            && self.names.alias(n).is_none()
            && self.body.type_param(n).is_none()
    }

    /// `misread` with the comparison `compare` its help writes, if any.
    pub(super) fn misread_as(&mut self, name: Span, compare: Option<String>) -> Poisoned {
        let reported = self.error(DiagnosticKind::TypeArguments {
            name: self.source.slice(name.start()..name.end()).to_string(),
            at: name,
            form: ArgumentsForm::Value { compare },
        });
        self.body.misread.insert(name.start(), reported);
        Poisoned::untyped(reported)
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
            && let Err(failed) = checked
            && self.names_no_type(first)
            && self.names_no_type(last)
        {
            self.body.misread.insert(span.start(), failed.reported());
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
            self.names.class(n),
            self.names.function(n),
            self.names.enum_id(n),
            self.names.struct_id(n),
        ) {
            (Some(Declared::Known(id)), ..) => Some(self.classes[id.0].type_params.len()),
            (_, Some(Declared::Known(id)), ..) => Some(self.signatures[id.0].type_params.len()),
            (.., Some(Declared::Known(id)), _) => Some(self.enums[id.0].type_params.len()),
            (.., Some(Declared::Known(id))) => Some(self.structs[id.0].type_params.len()),
            _ => None,
        };
        match params {
            Some(0) => self.not_generic(n, angles),
            None if (PRELUDE.contains(&n.as_str()) || built_in_type(n.as_str()).is_some())
                && !self.poisoned_use(n) =>
            {
                self.not_generic(n, angles);
            }
            // A local that hides a poisoned name is no value.
            None if self.lookup(n).is_some_and(|b| b.hides_name) => {}
            None if self.lookup(n).is_some()
                || matches!(self.names.constant(n), Some(Declared::Known(_))) =>
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
        #[cfg(test)]
        super::reached("path_method");
        let name = match &receiver.kind {
            ast::ExprKind::Name(name) => *name,
            ast::ExprKind::Applied { name, .. } => name.name,
            _ => return false,
        };
        let angles = matches!(receiver.kind, ast::ExprKind::Applied { .. });
        match self.meaning(name, names::Site::new(names::Position::PathCall, angles)) {
            // A poisoned name that no local hides is no path: its own error is reported.
            Resolved::Poisoned(..) => return false,
            // No item can take the name of the optional type, whose members are its variants.
            Resolved::OptionType => return true,
            Resolved::Item(ItemId::Enum(id)) => return self.enum_path_method(id, method),
            // An alias is the type that it names, unless a local takes the name.
            Resolved::Item(ItemId::Alias(_)) => {
                if let Via::To(through) = self.through(name) {
                    match through.ty.kind() {
                        TypeKind::Enum(id, _) => return self.enum_path_method(id, method),
                        TypeKind::Optional(_) => return true,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        (self.built_in_of(name).is_some() || matches!(name.as_str(), "math" | "Math"))
            && self.lookup(name).is_none()
    }

    /// Whether `.method` after the enum `id`'s name is a path: a variant, or a method whose type
    /// arguments wait for the member's report, which says it is a method.
    fn enum_path_method(&self, id: tir::EnumId, method: &ast::Ident) -> bool {
        let info = &self.enums[id.0];
        match info.methods.get(&method.name) {
            Some(&Declared::Known(func)) => {
                self.signatures[func.0].type_params.len() == info.type_params.len()
            }
            _ => true,
        }
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
        // The members of an optional are those of its payload.
        let ty = innermost(type_of(receiver)?);
        let field = match ty.kind() {
            TypeKind::Class(id, _) => self.classes[id.0].field(method.name).is_some(),
            TypeKind::Struct(id, _) => self.structs[id.0].field(method.name).is_some(),
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
    ) -> Reported {
        let text = self.source;
        // An alias that takes no parameter has no type arguments to move onto it.
        let takes_none =
            matches!(base.kind, ast::ExprKind::Name(name) if self.alias_takes_none(name));
        let instead = (!takes_none && types.iter().all(|ty| !self.names_no_type(ty))).then(|| {
            format!(
                "{}{}.{}",
                text.text(base.span),
                text.type_text(angles),
                variant.name
            )
        });
        self.error(DiagnosticKind::TypeArguments {
            name: text.type_text(base.span.to(variant.span)).into_owned(),
            at: angles,
            form: ArgumentsForm::Variant { instead },
        })
    }
}

/// The part of `ty` at the first place of each type parameter in `place`, as `part_at` finds it
/// for one parameter. A place that is optional where `ty` is not adds nothing: there
/// the call's value wraps, and `required_join` decides.
fn parts_at(place: Type, ty: Type, parts: &mut FxHashMap<Symbol, Type>) {
    let mut seen = FxHashSet::default();
    walk_parts(place, ty, parts, &mut seen);
}

/// `parts_at` below the top. `seen` holds each pair walked, so shared parts are walked once.
fn walk_parts(
    place: Type,
    ty: Type,
    parts: &mut FxHashMap<Symbol, Type>,
    seen: &mut FxHashSet<(Type, Type)>,
) {
    #[cfg(test)]
    super::join::PART_STEPS.set(super::join::PART_STEPS.get() + 1);
    if !place.has_param() || !seen.insert((place, ty)) {
        return;
    }
    crate::conformance::ensure_stack(|| {
        let lists = match (place.kind(), ty.kind()) {
            (TypeKind::Param(name, _), _) => {
                parts.entry(name).or_insert(ty);
                return;
            }
            (TypeKind::Array(ours), TypeKind::Array(theirs))
            | (TypeKind::Optional(ours), TypeKind::Optional(theirs)) => {
                return walk_parts(ours, theirs, parts, seen);
            }
            (TypeKind::Class(a, ours), TypeKind::Class(b, theirs)) if a == b => (ours, theirs),
            (TypeKind::Struct(a, ours), TypeKind::Struct(b, theirs)) if a == b => (ours, theirs),
            (TypeKind::Enum(a, ours), TypeKind::Enum(b, theirs)) if a == b => (ours, theirs),
            (TypeKind::Any(a, ours), TypeKind::Any(b, theirs)) if a == b => (ours, theirs),
            _ => return,
        };
        for (&ours, &theirs) in lists.0.get().iter().zip(lists.1.get()) {
            walk_parts(ours, theirs, parts, seen);
        }
    });
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
        let s = Type::intern(TypeKind::Param(Symbol::intern("S"), 0));
        assert_eq!(bindings.known(s), None);
        // It does not hide a parameter that is bound beside it.
        let two = Type::array(s);
        assert!(bindings.unify(two, Type::array(Type::STRING)));
        assert_eq!(bindings.known(s), Some(Type::STRING));
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

    /// An associated type of a parameter that is not bound is the hole as a whole. It is not known
    /// while its base or an argument of its interface is unbound. `printed` shows the hole.
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
        assert_eq!(bindings.known(item), None);
        assert_eq!(bindings.printed(item), *HOLE);
        assert!(bindings.unify(s, Type::STRING));
        // The base is bound, and the argument is not: the type waits, and a hole stands for it.
        assert_eq!(bindings.known(with_arg), None);
        assert!(bindings.unify(t, Type::I64));
        let string_item = Type::intern(TypeKind::Assoc(
            Type::STRING,
            InterfaceId(0),
            TypeArgs::intern(&[Type::I64]),
            0,
        ));
        assert_eq!(bindings.known(with_arg), Some(string_item));
    }

    #[test]
    fn rollback_should_undo_the_open_calls_that_its_snapshot_added_or_dropped() {
        // GIVEN an open call at `kept`, and a snapshot that drops it and adds one at `added`
        let entry = || OpenEntry {
            ret: Type::I64,
            marked: Type::I64,
            types: Vec::new(),
            params: Vec::new(),
            bounds: Vec::new(),
            members: Vec::new(),
            conflict: None,
        };
        let call = |at| {
            let kind = tir::ExprKind::Struct {
                id: tir::StructId(0),
                args: Vec::new(),
            };
            tir::Expr::new(kind, Type::I64, at)
        };
        let (kept, added) = (Span::new(0, 1), Span::new(2, 3));
        let mut table = super::super::infer::Table::default();
        table.add_open(kept, entry());
        let snapshot = table.snapshot();
        table.forget_open(kept);
        table.add_open(added, entry());
        // WHEN the snapshot rolls back
        table.rollback(snapshot);
        // THEN the call at `kept` is open again, and the call at `added` is not
        assert!(table.open_call(&call(kept)).is_some());
        assert!(table.open_call(&call(added)).is_none());
    }
}
