//! `mut` arguments: the place, its type, the overlap of two places, and the call's desugaring
//! (ADR 0043).

use std::borrow::Borrow;
use std::fmt;
use std::hash::{Hash, Hasher};

use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet};

use super::expr::{fits, local};
use super::generic::mark;
use super::place::{Chain, Change, CheckedPlace, Path, Root, build, place_root};
use super::{Checked, Checker, Poisoned, UNKNOWN, block_expr, type_of};
use crate::ast;
use crate::diagnostic::{
    ChainForm, ChainLabel, DiagnosticKind, Immutable, MissingFix, NotMutForm, NotPlace, SliceOf,
};
use crate::parser::{is_place, is_range_target};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, ExprKind, LocalId, Type, TypeArgs, TypeKind};

#[cfg(test)]
thread_local! {
    /// How many steps of a `mut` place's path the overlap check has hashed or compared on this
    /// thread: the work of `twice`.
    pub(super) static MUT_KEY_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Counts one step that `twice` hashes or compares.
fn tick() {
    #[cfg(test)]
    MUT_KEY_STEPS.set(MUT_KEY_STEPS.get() + 1);
}

/// Whether the argument `arg` is a `mut` argument that the call checks as a place. A labeled one
/// is E0055 already, and is checked as a value.
pub(super) fn marked(arg: &ast::Arg) -> bool {
    arg.mut_keyword.is_some() && arg.label.is_none()
}

/// The `mut` flags and the parameter names that a call of `args` keeps of its callee's, for its
/// `MutSite`. Only a callee with a `mut` parameter, or a call with a `mut` argument, pays for
/// a site: the others get nothing.
pub(super) fn site_of(
    muts: &[bool],
    names: &[ast::Ident],
    args: &[ast::Arg],
) -> (Vec<bool>, Vec<ast::Ident>) {
    let has_mut = muts.contains(&true);
    if !has_mut && !args.iter().any(|arg| arg.mut_keyword.is_some()) {
        return (Vec::new(), Vec::new());
    }
    (
        if has_mut { muts.to_vec() } else { Vec::new() },
        names.to_vec(),
    )
}

/// Whether `e` is a literal that takes its type from its context: `[]`, `None`, an array of them,
/// or `Some` of one.
fn context_literal(e: &ast::Expr) -> bool {
    match &e.kind {
        ast::ExprKind::NoneLit => true,
        ast::ExprKind::ArrayLit(elements) => elements.iter().all(context_literal),
        ast::ExprKind::Call { callee, args, .. } => {
            callee.name == Symbol::SOME && args.iter().all(|a| context_literal(&a.value))
        }
        _ => false,
    }
}

/// What a call path knows of its callee, for the reports about its `mut` arguments.
pub(super) struct MutSite<'a> {
    /// The callee as an error names it.
    pub(super) callee: &'a str,
    /// Whether each parameter is `mut`.
    pub(super) muts: &'a [bool],
    /// The name of each parameter. A site that has a `mut` parameter, or a call with a `mut`
    /// argument, always has them all (`site_of`), so a `mut` position has its name.
    pub(super) names: &'a [ast::Ident],
}

/// A `mut` argument that checked, until its call takes it (`Checker::take_mut_parts`).
pub(super) struct MutPart {
    path: Path,
    /// The local that stands for the argument in the call.
    temp: LocalId,
    ty: Type,
    /// The place as written.
    span: Span,
    /// The name at the root of the place as written, when it is a name.
    root: Option<Symbol>,
}

/// The types of the place of a `mut` argument, for E0170.
pub(super) struct PlaceShape {
    /// The type that the place stores.
    write: Type,
    /// The type that the place reads as: a `?.` of its root field makes it an optional.
    read: Type,
    chain: Option<Chain>,
}

/// The `mut` arguments of a call that checked.
#[derive(Default)]
pub(super) struct MutParts {
    /// The parts of the arguments that have `mut`, each with its key, in argument order.
    marked: Vec<(MutPart, MutPlaceKey)>,
    /// What the check of each argument at a `mut` parameter that has no `mut` found, by its place
    /// among the arguments, in argument order. A value that is no place is not here.
    unmarked: Vec<(usize, MarkCheck)>,
}

/// What `marks` keeps of a place that an argument names.
pub(super) struct Kept {
    key: MutPlaceKey,
    /// The type that the place stores, when it is known.
    ty: Option<Type>,
    /// Whether the place goes through `?.`.
    chain: bool,
}

/// What the one check of an argument at a `mut` parameter, with no `mut` on it, found.
pub(super) enum MarkCheck {
    /// A place that can change.
    Place(Kept),
    /// Why a place that the argument names cannot be passed `mut`, and what is kept of it when it
    /// has a path.
    NotPlace(NotPlace, Option<Kept>),
    /// A place with an error in it, which reported itself.
    Failed,
}

/// A callee as the reports about the `mut` markers of its arguments name it.
#[derive(Clone, Copy)]
pub(super) enum Callee<'a> {
    /// A function or method of the program or the prelude: the parameter names it has.
    Written(&'a MutSite<'a>),
    /// A built-in, by the name that the call writes: `print`, `xs.push`.
    Unnamed(&'a dyn fmt::Display),
    /// A constructor, written `shown`: a class's, or a variant's when `class` is false. Its
    /// arguments carry a label when `labeled`.
    Construct {
        shown: &'a dyn fmt::Display,
        class: bool,
        labeled: bool,
    },
}

/// The root of a `mut` place. `Temp` is a root that is no binding: a call's result.
#[derive(Clone, Copy, Debug)]
pub(super) enum RootKey {
    Local(LocalId),
    Temp,
}

/// One step of a `mut` place's path: a field, or an index. `Unknown` is an index that a call can
/// change, or one that is not a literal or a local.
#[derive(Clone, Copy, Debug)]
pub(super) enum Step {
    Field(usize),
    Literal(i64),
    /// A `let` local or a plain parameter, which no argument can change.
    Local(LocalId),
    Unknown,
}

// `twice` gives a `Temp` root and an `Unknown` step a fresh id and never looks them up, so these
// stay reflexive. Each hash and each compare is one counted step.
impl Hash for RootKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        tick();
        match self {
            RootKey::Local(id) => id.hash(state),
            RootKey::Temp => state.write_u8(0),
        }
    }
}

impl PartialEq for RootKey {
    fn eq(&self, other: &Self) -> bool {
        tick();
        match (self, other) {
            (RootKey::Local(a), RootKey::Local(b)) => a == b,
            (RootKey::Temp, RootKey::Temp) => true,
            _ => false,
        }
    }
}

impl Eq for RootKey {}

impl Hash for Step {
    fn hash<H: Hasher>(&self, state: &mut H) {
        tick();
        std::mem::discriminant(self).hash(state);
        match self {
            Step::Field(index) => index.hash(state),
            Step::Literal(n) => n.hash(state),
            Step::Local(id) => id.hash(state),
            Step::Unknown => {}
        }
    }
}

impl PartialEq for Step {
    fn eq(&self, other: &Self) -> bool {
        tick();
        match (self, other) {
            (Step::Field(a), Step::Field(b)) => a == b,
            (Step::Literal(a), Step::Literal(b)) => a == b,
            (Step::Local(a), Step::Local(b)) => a == b,
            (Step::Unknown, Step::Unknown) => true,
            _ => false,
        }
    }
}

impl Eq for Step {}

/// The place of a `mut` argument as the overlap check sees it: its root and each step of the
/// path.
pub(super) struct MutPlaceKey {
    root: RootKey,
    steps: Vec<Step>,
    /// The length of the longest proper prefix whose value is a class object: the root is
    /// length 0, and -1 when no proper prefix is one. A place that holds another stops at a
    /// class reference, so the key `a` holds the key `b` only when `a` is a proper prefix of `b`
    /// and `a`'s length is greater than `b`'s `class`.
    class: isize,
}

/// How two `mut` places relate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Overlap {
    /// The same place.
    Same,
    /// One holds the other.
    Inside,
}

/// The first earlier key that each key meets, as `(earlier, later, how)`: one at most for each
/// later key.
///
/// The work is linear in the length of the paths: each prefix gets an id from a map that is keyed
/// by the id of its parent and its step, so a path of length L costs L lookups.
pub(super) fn twice<K: Borrow<MutPlaceKey>>(keys: &[K]) -> Vec<(usize, usize, Overlap)> {
    // Sized up front, so that growing a map never hashes its keys again.
    let total: usize = keys.iter().map(|k| k.borrow().steps.len()).sum();
    let mut roots: FxHashMap<RootKey, usize> =
        FxHashMap::with_capacity_and_hasher(keys.len(), FxBuildHasher);
    let mut steps: FxHashMap<(usize, Step), usize> =
        FxHashMap::with_capacity_and_hasher(total, FxBuildHasher);
    // The first key of each full id, and of each proper prefix that its key holds.
    let mut whole: FxHashMap<usize, usize> = FxHashMap::default();
    let mut held: FxHashMap<usize, usize> = FxHashMap::default();
    let mut next = 0;
    let mut fresh = || {
        next += 1;
        next
    };
    let mut found = Vec::new();
    for (later, key) in keys.iter().enumerate() {
        let key = key.borrow();
        let mut id = match key.root {
            RootKey::Temp => fresh(),
            root @ RootKey::Local(_) => *roots.entry(root).or_insert_with(&mut fresh),
        };
        let mut prefixes = vec![id];
        for &step in &key.steps {
            id = match step {
                Step::Unknown => fresh(),
                step => *steps.entry((id, step)).or_insert_with(&mut fresh),
            };
            prefixes.push(id);
        }
        let length = key.steps.len();
        // The prefixes that this key holds, and that a class reference does not cut off.
        let holds = usize::try_from(key.class + 1).expect("a class boundary should be at least -1");
        let inside = prefixes.get(holds..length).unwrap_or_default();
        let earlier = [
            whole.get(&id).map(|&i| (i, Overlap::Same)),
            held.get(&id).map(|&i| (i, Overlap::Inside)),
        ]
        .into_iter()
        .flatten()
        .chain(
            inside
                .iter()
                .filter_map(|p| whole.get(p).map(|&i| (i, Overlap::Inside))),
        )
        .min_by_key(|&(i, _)| i);
        if let Some((first, how)) = earlier {
            found.push((first, later, how));
        }
        whole.entry(id).or_insert(later);
        for &p in inside {
            held.entry(p).or_insert(later);
        }
    }
    found
}

impl PlaceShape {
    /// Whether the place stores `ty`.
    pub(super) fn writes(&self, ty: Type) -> bool {
        self.write == ty
    }

    /// Whether the place goes through `?.`.
    pub(super) fn has_chain(&self) -> bool {
        self.chain.is_some()
    }
}

impl CheckedPlace {
    /// The types of the place, which stores `write`.
    fn shape(&mut self, write: Type) -> PlaceShape {
        PlaceShape {
            write,
            read: self.value_ty().unwrap_or(write),
            chain: self.chain.take(),
        }
    }
}

impl Checker<'_> {
    /// Stamps the root of each `mut` argument of `args` as written, for every call: a `mut`
    /// argument counts as a write whatever the call then gets wrong. A call that fails to resolve
    /// checks its arguments as values, so this runs before it, and W0002 does not warn on the
    /// local as the cascade of the call's error.
    pub(super) fn stamp_marked(&mut self, args: &[ast::Arg]) {
        for arg in args.iter().filter(|arg| arg.mut_keyword.is_some()) {
            self.stamp_push(&arg.value);
        }
    }

    /// The place that the `mut` argument `arg` names: a `let mut` local or `mut` parameter, a
    /// `mut` field of a class object, or an element of one. Why it is none otherwise, with no
    /// report: the caller reports E0169. The range index of a slice comes first. The place
    /// stamps its root binding as written, so a local that is passed `mut` counts as assigned.
    /// A `?.` along the path is a place (ADR 0037), and `chain` records it. A place that cannot
    /// change gives its key too, when it has a path.
    pub(super) fn mut_place(
        &mut self,
        arg: &ast::Expr,
    ) -> Result<CheckedPlace, (NotPlace, Option<Kept>)> {
        if is_range_target(arg) {
            // As a `push` onto a range does: the bound alone is reported, with no warning that
            // the root is never reassigned.
            self.stamp_push(arg);
            let (ty, failed) = self.checked_alone(arg);
            if failed {
                return Ok(CheckedPlace::failed());
            }
            let slice = if ty == Some(Type::STRING) {
                SliceOf::String
            } else {
                SliceOf::Array
            };
            return Err((NotPlace::Range(slice), None));
        }
        if matches!(arg.kind, ast::ExprKind::SelfValue) {
            // Outside a method there is no `self` to pass: E0041 says so.
            if self.body.self_type.is_none() {
                self.checked_alone(arg);
                return Ok(CheckedPlace::failed());
            }
            let class = matches!(
                self.body.self_type.map(Type::kind),
                Some(TypeKind::Class(..))
            );
            return Err((NotPlace::SelfValue { class }, None));
        }
        let root = place_root(arg);
        if let ast::ExprKind::Name(name) = root.kind
            && self.lookup(name).is_none()
            && !self.constants.contains_key(&name)
            && self.is_function(name)
        {
            // A function, or a place with a function at its root, is a value: E0105 says so.
            self.checked_alone(arg);
            return Ok(CheckedPlace::failed());
        }
        if !is_place(arg) {
            // An expression with an error has its error, and no place to report.
            return match self.checked_alone(arg) {
                (_, true) => Ok(CheckedPlace::failed()),
                (_, false) => Err((NotPlace::Temporary, None)),
            };
        }
        let mut place = self.place(arg.span, arg, Change::MutArg);
        if let Some(cause) = place.refused.take() {
            let kept = place.path.as_ref().map(|path| Kept {
                key: self.mut_key(path),
                ty: place.ty,
                chain: place.guard.is_some(),
            });
            return Err((NotPlace::Immutable(cause), kept));
        }
        if place.guard.is_some() {
            place.chain = Some(Chain {
                name: self.chain_head(arg),
            });
        }
        Ok(place)
    }

    /// The type of the expression `arg`, checked on its own for its own mistakes, and whether
    /// it failed.
    ///
    /// It failed when it reported an error. It failed too when its type is unknown, an error was
    /// reported before it, and it is no context literal: its type comes from a local that failed.
    /// A context literal (`[]`, `None`, `Some` and arrays of them) has no type and no error, so it
    /// never failed. With no earlier error, an unknown type is no failure either, so E0169 is
    /// reported.
    // shortcut: a temporary of another form that takes its type from its context, such as
    // `if c { [] } else { [] }` or `{ [] }`, counts as failed beside an earlier error and hides
    // its E0169; upgrade if such a form must report E0169 there.
    fn checked_alone(&mut self, arg: &ast::Expr) -> (Option<Type>, bool) {
        let errors = self.diagnostics.errors;
        let ty = type_of(&self.expr(arg, Some(UNKNOWN)));
        let failed =
            self.diagnostics.errors > errors || ty.is_none() && errors > 0 && !context_literal(arg);
        (ty, failed)
    }

    /// The text before the first `?.` of the place `e`, when it is on one line.
    /// An index above a `?.` is E0062 before this runs, so the walk stops at the first index.
    fn chain_head(&self, mut e: &ast::Expr) -> Option<String> {
        let mut head = e;
        loop {
            match &e.kind {
                ast::ExprKind::Field { base, safe, .. } => {
                    if *safe {
                        head = base;
                    }
                    e = base;
                }
                _ => {
                    return self
                        .one_line(head.span)
                        .then(|| self.source.text(head.span).to_string());
                }
            }
        }
    }

    /// How many `?.` the place `e` goes through.
    /// An index above a `?.` is E0062 before this runs, so the walk stops at the first index.
    fn chain_steps(mut e: &ast::Expr) -> usize {
        let mut steps = 0;
        loop {
            match &e.kind {
                ast::ExprKind::Field { base, safe, .. } => {
                    steps += usize::from(*safe);
                    e = base;
                }
                _ => return steps,
            }
        }
    }

    /// The root binding, or the field, whose declaration E0169 names for the argument `e`.
    fn refused_name(&self, mut e: &ast::Expr) -> String {
        while let ast::ExprKind::Index { base, .. } = &e.kind {
            e = base;
        }
        match &e.kind {
            ast::ExprKind::Name(name) => name.to_string(),
            ast::ExprKind::Field { name, .. } => name.name.to_string(),
            _ => self.source.text(e.span).to_string(),
        }
    }

    /// The place of the argument `arg`, which is passed `mut`, with E0169 reported when it is
    /// none. `None` when it is none, or a part of it failed.
    fn mut_place_reported(&mut self, arg: &ast::Expr) -> Option<CheckedPlace> {
        match self.mut_place(arg) {
            // A part of it failed, and reported itself.
            Ok(place) => place.path.is_some().then_some(place),
            Err((cause, _)) => {
                let name = self.refused_name(arg);
                self.report(DiagnosticKind::MutArgumentNotPlace {
                    at: arg.span,
                    name,
                    cause,
                });
                None
            }
        }
    }

    /// Reports E0170 for the `mut` argument `arg` at place `at`, whose place has the types of
    /// `shape` and whose parameter has the type `want` once inference is done. `declared_as` is
    /// the parameter's type as it is declared.
    pub(super) fn report_mut_type(
        &mut self,
        arg: &ast::Expr,
        shape: &PlaceShape,
        (site, at): (&MutSite<'_>, usize),
        (want, declared_as): (Type, Type),
    ) {
        let name = site.names.get(at).map(|n| n.name.to_string());
        let declared = site
            .names
            .get(at)
            .map(|n| n.span)
            .filter(|span| !span.in_prelude());
        // A type that still names a parameter says nothing for a `?.` place, which no parameter
        // type fits: the place's own write type stands for the parameter's.
        let want = if shape.chain.is_some() && (want.has_param() || want.holds_hole()) {
            shape.write
        } else {
            want
        };
        let chain = shape.chain.as_ref().map(|chain| {
            let label = if want == shape.write && want == shape.read {
                ChainLabel::NoPlace {
                    head: chain.name.clone(),
                }
            } else if want == shape.write {
                ChainLabel::WriteType
            } else if want == shape.read {
                ChainLabel::WritesNone {
                    field: match &arg.kind {
                        ast::ExprKind::Field { name, .. } => name.name.to_string(),
                        _ => String::new(),
                    },
                }
            } else {
                ChainLabel::Other
            };
            let shown = self
                .one_line(arg.span)
                .then(|| self.source.text(arg.span).to_string());
            let help = chain.name.as_ref().filter(|_| {
                shown.is_some()
                    && Self::chain_steps(arg) == 1
                    && matches!(label, ChainLabel::WriteType | ChainLabel::NoPlace { .. })
            });
            let help = help.map(|head| {
                format!("unwrap `{head}` with `if let`, and pass the field of the unwrapped object")
            });
            ChainForm { shown, label, help }
        });
        let found = shape.read;
        let promotes = chain.is_none()
            && !matches!(found.kind(), TypeKind::Optional(_))
            && matches!(want.kind(), TypeKind::Optional(_))
            && want.without_optionals() == found;
        let d = DiagnosticKind::MutArgumentType {
            at: arg.span,
            arg: self.source.text(arg.span).to_string(),
            name,
            position: at + 1,
            callee: site.callee.to_string(),
            found: self.type_name(found),
            want: self.type_name(want),
            declared,
            declared_as: self.type_name(declared_as),
            promotes,
            chain,
        };
        self.report(d);
    }

    /// The temp that stands for the `mut` argument `arg` in its call, and the part that the call
    /// takes with it, which goes to `parts`. The temp is the argument's value in the call.
    pub(super) fn mut_slot(
        &mut self,
        arg: &ast::Arg,
        place: CheckedPlace,
        parts: &mut MutParts,
    ) -> tir::Expr {
        let ty = place
            .ty
            .expect("a place that a `mut` argument takes should have a type");
        let path = place
            .path
            .expect("a place that a `mut` argument takes should have a path");
        let key = self.mut_key(&path);
        let temp = self.fresh(ty);
        let span = arg.value.span;
        let root = match place_root(&arg.value).kind {
            ast::ExprKind::Name(name) => Some(name),
            _ => None,
        };
        parts.marked.push((
            MutPart {
                path,
                temp,
                ty,
                span,
                root,
            },
            key,
        ));
        local(temp, ty, span)
    }

    /// The `mut` argument `arg` at place `at` of a call that `site` describes, whose parameter
    /// has type `want`: its place, with exactly that type. Reports E0169 or E0170 when it fails.
    pub(super) fn mut_arg(
        &mut self,
        arg: &ast::Arg,
        at: usize,
        want: Option<Type>,
        (site, parts): (&MutSite<'_>, &mut MutParts),
    ) -> Checked {
        let Some(mut place) = self.mut_place_reported(&arg.value) else {
            return Err(Poisoned::error());
        };
        let (Some(want), Some(have)) = (want, place.ty) else {
            return Err(Poisoned::error());
        };
        if place.chain.is_some() || want != have {
            let shape = place.shape(have);
            self.report_mut_type(&arg.value, &shape, (site, at), (want, want));
            return mark((Err(Poisoned::error()), true));
        }
        Ok(self.mut_slot(arg, place, parts))
    }

    /// `mut_arg` for a generic call: the place's type binds the parameters that the parameter's
    /// type `pattern` names, before any other argument does. Whether the place has exactly the
    /// parameter's type once inference is done is `Checker::mut_final`'s to say, as the
    /// unification accepts a payload where the parameter is an optional, and leaves an
    /// associated type to its conformance.
    pub(super) fn infer_mut_arg(
        &mut self,
        call: &mut super::generic::Inference<'_>,
        arg: &ast::Arg,
        at: usize,
        (pattern, site): (Option<Type>, &MutSite<'_>),
        parts: &mut MutParts,
    ) -> Checked {
        let Some(mut place) = self.mut_place_reported(&arg.value) else {
            return Err(Poisoned::error());
        };
        let (Some(pattern), Some(have)) = (pattern, place.ty) else {
            return Err(Poisoned::error());
        };
        let shape = place.shape(have);
        if shape.chain.is_some() {
            call.mut_shapes.push((at, shape));
            return mark((Err(Poisoned::error()), true));
        }
        if !self.bind_param(call, pattern, have, arg.value.span) {
            let want = self.printed(&call.bindings, pattern);
            self.report_mut_type(&arg.value, &shape, (site, at), (want, pattern));
            return mark((Err(Poisoned::error()), true));
        }
        call.mut_shapes.push((at, shape));
        Ok(self.mut_slot(arg, place, parts))
    }

    /// The argument `arg`, at the `mut` parameter `at`, when it has no `mut` and names a place.
    /// Its one check, with no report, goes to `parts` for `marks`.
    ///
    /// With `bind`, a generic call and the parameter's type, the place binds the type parameters
    /// that the type names, as `mut` would. No other argument then fails against a type that the
    /// place has not bound. A type that does not fit is for E0166 to say.
    ///
    /// The result is the failed argument, or `None` for a value, which the call checks as it
    /// checks any other.
    pub(super) fn unmarked_place(
        &mut self,
        arg: &ast::Arg,
        at: usize,
        parts: &mut MutParts,
        bind: Option<(&mut super::generic::Inference<'_>, Option<Type>)>,
    ) -> Option<Checked> {
        if arg.label.is_some() || !is_place(&arg.value) {
            return None;
        }
        let check = match self.mut_place(&arg.value) {
            Ok(place) => match place.path.as_ref().map(|path| self.mut_key(path)) {
                Some(key) => MarkCheck::Place(Kept {
                    key,
                    ty: place.ty,
                    chain: place.chain.is_some(),
                }),
                None => MarkCheck::Failed,
            },
            Err((cause, kept)) => MarkCheck::NotPlace(cause, kept),
        };
        // A `let` local or a field that cannot change binds as a place does: with `mut`, it would.
        let kept = match &check {
            MarkCheck::Place(kept) | MarkCheck::NotPlace(NotPlace::Immutable(_), Some(kept)) => {
                Some(kept)
            }
            _ => None,
        };
        if let (
            Some((call, Some(pattern))),
            Some(Kept {
                ty: Some(have),
                chain: false,
                ..
            }),
        ) = (bind, kept)
        {
            let _ = self.bind_param(call, pattern, *have, arg.value.span);
        }
        parts.unmarked.push((at, check));
        // The call keeps its result type: the result meets its context or is E0032 as usual.
        Some(Err(Poisoned::error()))
    }

    /// The argument `arg` at the `mut` parameter `at` of a call that `site` describes, whose type
    /// is `want`: checked as a place, with `mut` (`mut_arg`) or without it (`unmarked_place`).
    /// `None` is a value with no `mut`, which the call checks as it checks any other.
    pub(super) fn mut_argument(
        &mut self,
        arg: &ast::Arg,
        at: usize,
        want: Option<Type>,
        (site, parts): (&MutSite<'_>, &mut MutParts),
    ) -> Option<Checked> {
        if marked(arg) {
            Some(self.mut_arg(arg, at, want, (site, parts)))
        } else {
            self.unmarked_place(arg, at, parts, None)
        }
    }

    /// Reports each argument of `args` that has `mut`, and whose parameter is not `mut` or does
    /// not exist (E0167): `callee` says how the call names it. A label on a function's argument
    /// is E0055 already, and is not checked for its marker.
    pub(super) fn refuse_mut(&mut self, args: &[ast::Arg], callee: Callee<'_>) {
        for (i, arg) in args.iter().enumerate() {
            let Some(at) = arg.mut_keyword else { continue };
            if arg.label.is_some() && !matches!(callee, Callee::Construct { labeled: true, .. }) {
                continue;
            }
            let form = match &callee {
                Callee::Written(site) if site.muts.get(i) == Some(&true) => continue,
                Callee::Written(site) => match site.names.get(i) {
                    Some(name) => NotMutForm::Written {
                        name: name.name.to_string(),
                        callee: site.callee.to_string(),
                        declared: Some(name.span).filter(|span| !span.in_prelude()),
                    },
                    None => NotMutForm::Unnamed {
                        position: i + 1,
                        callee: site.callee.to_string(),
                    },
                },
                Callee::Unnamed(name) => NotMutForm::Unnamed {
                    position: i + 1,
                    callee: name.to_string(),
                },
                Callee::Construct {
                    shown, class: true, ..
                } => NotMutForm::Class {
                    shown: shown.to_string(),
                },
                Callee::Construct {
                    shown,
                    class: false,
                    ..
                } => NotMutForm::Variant {
                    shown: shown.to_string(),
                },
            };
            // The fix removes `mut` and the spaces and tabs after it, up to the next token.
            let next = (arg.label.as_ref())
                .map(|label| label.span.start())
                .filter(|&start| start >= at.end())
                .unwrap_or_else(|| arg.value.span.start());
            let gap = self.source.text(Span::new(at.end(), next));
            let fix_end = at.end() + gap.len() - gap.trim_start_matches([' ', '\t']).len();
            self.report(DiagnosticKind::MutMarkerNotMut { at, fix_end, form });
        }
    }

    /// Reports each argument of `args` that has `mut` where `site` has no `mut` parameter
    /// (E0167), and each that has none where it has one (E0166). `parts` holds the check of each
    /// argument of the second kind that names a place, and `params` are the types of the
    /// parameters, which `list` makes the types they have once the call is checked (`None` when
    /// it failed). `result` is the type that the call's context expects and the type of its
    /// result: a fix or a help is offered only when the result meets its context. It checks
    /// nothing itself.
    pub(super) fn marks(
        &mut self,
        args: &[ast::Arg],
        parts: &mut MutParts,
        site: &MutSite<'_>,
        (params, list): (&[Option<Type>], Option<TypeArgs>),
        result: (Option<Type>, Option<Type>),
    ) {
        self.refuse_mut(args, Callee::Written(site));
        if !site.muts.contains(&true) {
            return;
        }
        let wants = self.final_params(params, list, parts);
        let meets = self.result_meets(result);
        let checks = std::mem::take(&mut parts.unmarked);
        // A place that `mut` would pass twice with another gets no fix: both fixes together are
        // E0171. `twice` names the later key of a pair and the first one that it meets; the
        // reverse order names the other keys.
        let mut keys: Vec<&MutPlaceKey> = parts.marked.iter().map(|(_, key)| key).collect();
        let at: Vec<Option<usize>> = checks
            .iter()
            .map(|(_, check)| match check {
                MarkCheck::Place(Kept { key, .. })
                | MarkCheck::NotPlace(_, Some(Kept { key, .. })) => {
                    keys.push(key);
                    Some(keys.len() - 1)
                }
                _ => None,
            })
            .collect();
        let mut related = vec![false; keys.len()];
        if keys.len() > 1 && !checks.is_empty() {
            for (first, later, _) in twice(&keys) {
                related[first] = true;
                related[later] = true;
            }
            keys.reverse();
            let end = keys.len() - 1;
            for (first, later, _) in twice(&keys) {
                related[end - first] = true;
                related[end - later] = true;
            }
        }
        let mut checks = checks.into_iter().zip(at).peekable();
        for (i, arg) in args.iter().enumerate() {
            if !site.muts[i] || arg.mut_keyword.is_some() || arg.label.is_some() {
                continue;
            }
            let found = checks.next_if(|((place, _), _)| *place == i);
            let related = found
                .as_ref()
                .and_then(|(_, key)| *key)
                .is_some_and(|key| related[key]);
            let want = wants.get(i).copied().flatten();
            let check = found.map(|((_, check), _)| check);
            let fix = self.missing_fix(arg, check, (related || !meets, want));
            let param = site
                .names
                .get(i)
                .expect("a site that has a `mut` parameter should name every parameter");
            self.report(DiagnosticKind::MutMarkerMissing {
                at: arg.value.span,
                name: param.name.to_string(),
                callee: site.callee.to_string(),
                declared: Some(param.span).filter(|span| !span.in_prelude()),
                fix,
            });
        }
    }

    /// Whether the result of a call, of type `got`, meets the type `want` that its context
    /// expects. A context that is unknown or holds a hole expects nothing.
    fn result_meets(&mut self, (want, got): (Option<Type>, Option<Type>)) -> bool {
        let (Some(want), Some(got)) = (want, got) else {
            return true;
        };
        want == UNKNOWN || want.holds_hole() || fits(want, got) || self.converts_to_any(got, want)
    }

    /// What the report of the missing `mut` of `arg` offers. `check` is the check of the argument,
    /// `related` whether another `mut` place of the call meets it or the result misses its
    /// context, and `want` the parameter's type. A fix or a help is offered only when following
    /// it gives a valid call.
    fn missing_fix(
        &self,
        arg: &ast::Arg,
        check: Option<MarkCheck>,
        (related, want): (bool, Option<Type>),
    ) -> MissingFix {
        let fits = |kept: &Kept| !related && !kept.chain && kept.ty.is_some() && kept.ty == want;
        match check {
            // A place of a call that failed before it checked its arguments: nothing is known.
            None if is_place(&arg.value) => MissingFix::NoFix,
            Some(MarkCheck::Place(kept)) if fits(&kept) => MissingFix::Add,
            Some(MarkCheck::Failed | MarkCheck::Place(_)) => MissingFix::NoFix,
            Some(MarkCheck::NotPlace(NotPlace::Immutable(cause), kept)) => {
                let fits = kept.as_ref().is_some_and(fits);
                match cause {
                    Immutable::Let { .. } | Immutable::Field { .. } if !fits => MissingFix::NoFix,
                    Immutable::Let { .. } => {
                        let name = self.refused_name(&arg.value);
                        let shown = arg.value.span;
                        let arg = if self.one_line(shown) {
                            self.source.text(shown).to_string()
                        } else {
                            name.clone()
                        };
                        MissingFix::Let { name, arg }
                    }
                    Immutable::Field { .. } => MissingFix::Field {
                        name: self.refused_name(&arg.value),
                    },
                    _ => MissingFix::Copy,
                }
            }
            None | Some(MarkCheck::NotPlace(..)) => MissingFix::Copy,
        }
    }

    /// The type of each of `params` once the call that gave `list` is checked: only when an
    /// argument at a `mut` parameter has no `mut`, which `marks` compares with its place.
    pub(super) fn final_params(
        &self,
        params: &[Option<Type>],
        list: Option<TypeArgs>,
        parts: &MutParts,
    ) -> Vec<Option<Type>> {
        let Some(list) = list.filter(|_| !parts.unmarked.is_empty()) else {
            return Vec::new();
        };
        params
            .iter()
            .map(|param| {
                let ty = self.subst((*param)?, list.get())?;
                self.normalize(ty)
            })
            .collect()
    }

    /// Reports each `mut` argument of a call whose place is the place of an earlier one, or
    /// holds it, or is held by it (E0171). Gives the parts, in argument order.
    pub(super) fn mut_overlaps(&mut self, taken: MutParts) -> Vec<MutPart> {
        let (parts, keys): (Vec<MutPart>, Vec<MutPlaceKey>) = taken.marked.into_iter().unzip();
        for (first, second, how) in twice(&keys) {
            let (a, b) = (&parts[first], &parts[second]);
            let (shown, inside) = match how {
                Overlap::Same => (self.source.text(b.span).to_string(), None),
                // The longer path is the one inside.
                Overlap::Inside => {
                    let (inner, outer) = if keys[first].steps.len() > keys[second].steps.len() {
                        (a, b)
                    } else {
                        (b, a)
                    };
                    (
                        self.source.text(inner.span).to_string(),
                        Some(self.source.text(outer.span).to_string()),
                    )
                }
            };
            self.report(DiagnosticKind::MutPlaceTwice {
                first: a.span,
                second: b.span,
                shown,
                inside,
            });
        }
        parts
    }

    /// The key of the place `path`.
    fn mut_key(&self, path: &Path) -> MutPlaceKey {
        let mut operands = path.operands.iter();
        let (root, mut steps) = match path.root {
            Root::Local(id) => (RootKey::Local(id), Vec::new()),
            Root::Field(index) => {
                let base = operands
                    .next()
                    .expect("a field root's base should be the first operand of its path");
                let (root, mut steps) = self.read_key(base);
                steps.push(Step::Field(index));
                (root, steps)
            }
        };
        steps.extend(operands.map(|index| self.index_step(index)));
        // Every field step is a field of a class object, so the prefix before it is one.
        let class = steps
            .iter()
            .rposition(|step| matches!(step, Step::Field(_)))
            .map_or(-1, |at| {
                isize::try_from(at).expect("a path should fit in `isize`")
            });
        MutPlaceKey { root, steps, class }
    }

    /// The root and the steps of the read `e`, a chain of fields and indexes.
    fn read_key(&self, mut e: &tir::Expr) -> (RootKey, Vec<Step>) {
        let mut steps = Vec::new();
        let root = loop {
            match &e.kind {
                ExprKind::Field { base, index } => {
                    steps.push(Step::Field(*index));
                    e = base;
                }
                ExprKind::Index { base, index } => {
                    steps.push(self.index_step(index));
                    e = base;
                }
                ExprKind::Local(id) => break RootKey::Local(*id),
                _ => break RootKey::Temp,
            }
        };
        steps.reverse();
        (root, steps)
    }

    /// The step of the index `e`: a literal, or a local that no argument can change.
    fn index_step(&self, e: &tir::Expr) -> Step {
        match &e.kind {
            ExprKind::Int(n) => Step::Literal(*n),
            ExprKind::Local(id) if !self.body.locals[id.0].mutable => Step::Local(*id),
            _ => Step::Unknown,
        }
    }

    /// `e`, or a hidden `let` of it, pushed to `lets`, and a read of that: the value that step 1
    /// of the call keeps. A literal and an immutable local stay as they are, which nothing can
    /// change.
    fn hide(&mut self, e: tir::Expr, lets: &mut Vec<tir::Stmt>) -> tir::Expr {
        let fixed = match &e.kind {
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Char(_)
            | ExprKind::Str(_)
            | ExprKind::None => true,
            ExprKind::Local(id) => !self.body.locals[id.0].mutable,
            _ => false,
        };
        if fixed {
            return e;
        }
        let (ty, span) = (e.ty, e.span);
        let temp = self.fresh(ty);
        lets.push(tir::Stmt::Let {
            local: temp,
            init: e,
        });
        local(temp, ty, span)
    }

    /// The base of a place, a chain of fields and indexes, from the root outwards. A root that is
    /// no binding is hidden in a `let`, and so is each index. Each class object along the chain
    /// that is no binding goes to `keep`. It is a fresh local with the read of the object. That
    /// read starts at the local of the object before it. No object and no element is read yet.
    /// The result is the base as the place reads it: the local of the last object, or the
    /// binding.
    ///
    /// A binding in `rebound` is kept as well, when it holds a class object. `rebound` holds the
    /// root of each `mut` argument of the call that starts at a binding. The store-back of such an
    /// argument can rebind the binding, and the store-back of this place must still write to the
    /// object that the copy-in read. Only a `mut` argument of the call itself can rebind a
    /// binding of the caller while the call runs. A call with no such argument pays for no keep.
    /// For `f(mut b.x, mut b)` the keep is not needed, because the store-back of `b.x` comes
    /// first. It costs one retain and one release, and the output is the same.
    fn bind_base(
        &mut self,
        e: tir::Expr,
        rebound: &FxHashSet<LocalId>,
        lets: &mut Vec<tir::Stmt>,
        keep: &mut Vec<(LocalId, tir::Expr)>,
    ) -> tir::Expr {
        if matches!(e.kind, ExprKind::Local(id) if !rebound.contains(&id)) {
            return e;
        }
        let (ty, span) = (e.ty, e.span);
        let e = match e.kind {
            ExprKind::Field { base, index } => {
                let base = Box::new(self.bind_base(*base, rebound, lets, keep));
                tir::Expr::new(ExprKind::Field { base, index }, ty, span)
            }
            ExprKind::Index { base, index } => {
                let base = Box::new(self.bind_base(*base, rebound, lets, keep));
                let index = Box::new(self.hide(*index, lets));
                tir::Expr::new(ExprKind::Index { base, index }, ty, span)
            }
            kind @ ExprKind::Local(_) => tir::Expr::new(kind, ty, span),
            kind => return self.hide(tir::Expr::new(kind, ty, span), lets),
        };
        if !matches!(ty.kind(), TypeKind::Class(..)) {
            return e;
        }
        let object = self.fresh(ty);
        keep.push((object, e));
        local(object, ty, span)
    }

    /// The test that every index of `place` is in range now, or `None` for a place with no index.
    /// The test of an index is `index < len(array)`. The tests join with `&&`, from the root
    /// outwards. So each array is read only after the indexes before it passed. Each array after
    /// the first is a `let` in the block of the test that reads it. So the path is read once. The
    /// block releases these arrays before the store-back. So the store-back finds them unique.
    fn indexes_in_range(&mut self, place: &tir::Place, at: Span) -> Option<tir::Expr> {
        let mut indexes = Vec::new();
        let mut root = place;
        while let tir::Place::Index { base, index } = root {
            indexes.push(index);
            root = base;
        }
        indexes.reverse();
        let first = self.read(root.clone(), at);
        // `arrays[k]` is the array that `indexes[k]` indexes: the root's, then one `let` each.
        let mut arrays = vec![first];
        let mut lets = Vec::new();
        for (k, index) in indexes
            .iter()
            .enumerate()
            .take(indexes.len().saturating_sub(1))
        {
            let TypeKind::Array(element) = arrays[k].ty.kind() else {
                panic!("an indexed place should hold an array, as `element` checked")
            };
            let array = self.fresh(element);
            let read = ExprKind::Index {
                base: Box::new(arrays[k].clone()),
                index: Box::new((*index).clone()),
            };
            lets.push(tir::Stmt::Let {
                local: array,
                init: tir::Expr::new(read, element, at),
            });
            arrays.push(local(array, element, at));
        }
        let mut tests: Vec<tir::Expr> = indexes
            .iter()
            .zip(arrays)
            .map(|(index, array)| {
                let len = tir::Expr::new(ExprKind::Len(Box::new(array)), Type::I64, at);
                let test = ExprKind::Compare {
                    operands: vec![(*index).clone(), len],
                    ops: vec![ast::CompareOp::Lt],
                };
                tir::Expr::new(test, Type::BOOL, at)
            })
            .collect();
        let mut guard = tests.pop()?;
        while let (Some(test), Some(array)) = (tests.pop(), lets.pop()) {
            let rest = block_expr(vec![array], Some(guard), Type::BOOL, at);
            let both = ExprKind::Binary {
                op: tir::BinaryOp::And,
                lhs: Box::new(test),
                rhs: Box::new(rest),
            };
            guard = tir::Expr::new(both, Type::BOOL, at);
        }
        Some(guard)
    }

    /// The call `call`, of a callee that has the `mut` parameters whose arguments are `parts`,
    /// as a `MutCall` in a block that binds the values of step 1, in written order: each
    /// argument that is no `mut` argument, and the root and the indexes of each `mut` argument.
    /// `call` reads the temp of each part where its argument is. The block ends in the call.
    ///
    /// The copy-in of a part keeps the class objects along its place, then reads the place from
    /// the last one. The store-back is `place = temp` on those objects, and only when every index
    /// of the place is in range then: a write to an element that is gone is lost.
    pub(super) fn mut_call(&mut self, mut call: tir::Expr, parts: Vec<MutPart>) -> tir::Expr {
        let (ty, span) = (call.ty, call.span);
        // The copy-in reads the place, and a result can carry that value out: `exchange` returns
        // it. So the call reads the root local. A call with no result keeps the rule that a
        // `mut` argument is a write (`stamp_marked`).
        if ty != Type::UNIT {
            for name in parts.iter().filter_map(|part| part.root) {
                if let Some(binding) = self.lookup(name) {
                    binding.read = true;
                }
            }
        }
        let (ExprKind::Call { args, .. } | ExprKind::InterfaceCall { args, .. }) = &mut call.kind
        else {
            unreachable!("a call with `mut` arguments should be a call of a function or a method")
        };
        let mut lets = Vec::new();
        // The bindings that a store-back of this call can rebind: the roots of the `mut`
        // arguments that start at a binding. A part with operands starts at an array local, which
        // is no class object, so `bind_base` keeps nothing for it.
        let rebound_roots: FxHashSet<LocalId> = parts
            .iter()
            .filter_map(|part| match part.path.root {
                Root::Local(id) => Some(id),
                Root::Field(_) => None,
            })
            .collect();
        let mut parts = parts.into_iter().peekable();
        let (mut muts, mut back) = (Vec::new(), Vec::new());
        let mut rebound = Vec::with_capacity(args.len());
        for arg in std::mem::take(args) {
            let own = matches!(arg.kind, ExprKind::Local(id) if parts.peek().is_some_and(|p| p.temp == id));
            let Some(part) = parts.next_if(|_| own) else {
                rebound.push(self.hide(arg, &mut lets));
                continue;
            };
            let MutPart {
                path,
                temp,
                ty: part_ty,
                span: at,
                ..
            } = part;
            let mut operands = path.operands.into_iter();
            let (mut bound, mut keep) = (Vec::new(), Vec::new());
            if matches!(path.root, Root::Field(_)) {
                let base = operands
                    .next()
                    .expect("a field root's base should be the first operand of its path");
                bound.push(self.bind_base(base, &rebound_roots, &mut lets, &mut keep));
            }
            for index in operands {
                bound.push(self.hide(index, &mut lets));
            }
            let place = build(path.root, bound);
            let read = self.read(place.clone(), at);
            let in_range = self.indexes_in_range(&place, at);
            let store = tir::Stmt::Assign {
                place,
                value: local(temp, part_ty, at),
            };
            back.push(match in_range {
                None => store,
                Some(cond) => {
                    let then_block = tir::Block {
                        stmts: vec![store],
                        value: None,
                        ty: Type::UNIT,
                    };
                    let guarded = ExprKind::If {
                        cond: Box::new(cond),
                        then_block,
                        else_block: None,
                    };
                    tir::Stmt::Expr(tir::Expr::new(guarded, Type::UNIT, at))
                }
            });
            muts.push(tir::MutArg { keep, temp, read });
            rebound.push(arg);
        }
        *args = rebound;
        // An argument that can leave is in a hidden `let` now, so the call itself cannot leave.
        call.refresh_exits();
        let back = tir::Block {
            stmts: back,
            value: None,
            ty: Type::UNIT,
        };
        let call = tir::Expr::new(
            ExprKind::MutCall {
                call: Box::new(call),
                muts,
                back,
            },
            ty,
            span,
        );
        if lets.is_empty() {
            call
        } else {
            block_expr(lets, Some(call), ty, span)
        }
    }
}
