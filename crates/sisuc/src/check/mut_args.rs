//! `mut` arguments: the place, its type, the overlap of two places, and the call's desugaring
//! (ADR 0043).

use std::hash::{Hash, Hasher};

use rustc_hash::{FxBuildHasher, FxHashMap};

use super::expr::local;
use super::generic::mark;
use super::place::{Chain, Change, CheckedPlace, Path, Root, build};
use super::{Checked, Checker, Poisoned, UNKNOWN, block_expr, type_of};
use crate::ast;
use crate::diagnostic::{ChainForm, ChainLabel, DiagnosticKind, NotPlace, SliceOf};
use crate::parser::{is_place, is_range_target};
use crate::source::Span;
use crate::tir::{self, ExprKind, LocalId, Type, TypeKind};

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

/// What a call path knows of its callee, for the reports about its `mut` arguments.
pub(super) struct MutSite<'a> {
    /// The callee as an error names it.
    pub(super) callee: &'a str,
    /// Whether each parameter is `mut`.
    pub(super) muts: &'a [bool],
    /// The name of each parameter.
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
}

/// The types of the place of a `mut` argument, for E0170.
pub(super) struct PlaceShape {
    /// The type that the place stores.
    write: Type,
    /// The type that the place reads as: a `?.` of its root field makes it an optional.
    read: Type,
    chain: Option<Chain>,
}

/// The parts of the `mut` arguments of a call that checked, each with its key, in argument order.
pub(super) type MutParts = Vec<(MutPart, MutPlaceKey)>;

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
pub(super) fn twice(keys: &[MutPlaceKey]) -> Vec<(usize, usize, Overlap)> {
    // Sized up front, so that growing a map never hashes its keys again.
    let total: usize = keys.iter().map(|k| k.steps.len()).sum();
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
    /// The place that the `mut` argument `arg` names: a `let mut` local or `mut` parameter, a
    /// `mut` field of a class object, or an element of one. Why it is none otherwise, with no
    /// report: the caller reports E0169. The range index of a slice comes first. The place
    /// stamps its root binding as written, so a local that is passed `mut` counts as assigned.
    /// A `?.` along the path is a place (ADR 0037), and `chain` records it.
    pub(super) fn mut_place(&mut self, arg: &ast::Expr) -> Result<CheckedPlace, NotPlace> {
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
            return Err(NotPlace::Range(slice));
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
            return Err(NotPlace::SelfValue { class });
        }
        if !is_place(arg) {
            // An expression with an error has its error, and no place to report.
            return match self.checked_alone(arg) {
                (_, true) => Ok(CheckedPlace::failed()),
                (_, false) => Err(NotPlace::Temporary),
            };
        }
        let mut place = self.place(arg.span, arg, Change::MutArg);
        if let Some(cause) = place.refused.take() {
            return Err(NotPlace::Immutable(cause));
        }
        if place.guard.is_some() {
            place.chain = Some(Chain {
                name: self.chain_head(arg),
            });
        }
        Ok(place)
    }

    /// The type of the expression `arg`, checked on its own for its own mistakes, and whether
    /// it reported an error.
    fn checked_alone(&mut self, arg: &ast::Expr) -> (Option<Type>, bool) {
        let errors = self.diagnostics.errors;
        let ty = type_of(&self.expr(arg, Some(UNKNOWN)));
        (ty, self.diagnostics.errors > errors)
    }

    /// The text before the first `?.` of the place `e`, when it is on one line.
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
                ast::ExprKind::Index { base, .. } => e = base,
                _ => {
                    return self
                        .one_line(head.span)
                        .then(|| self.source.text(head.span).to_string());
                }
            }
        }
    }

    /// How many `?.` the place `e` goes through.
    fn chain_steps(mut e: &ast::Expr) -> usize {
        let mut steps = 0;
        loop {
            match &e.kind {
                ast::ExprKind::Field { base, safe, .. } => {
                    steps += usize::from(*safe);
                    e = base;
                }
                ast::ExprKind::Index { base, .. } => e = base,
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
            Err(cause) => {
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
        parts.push((
            MutPart {
                path,
                temp,
                ty,
                span,
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

    /// Reports each `mut` argument of a call whose place is the place of an earlier one, or
    /// holds it, or is held by it (E0171). Gives the parts, in argument order.
    pub(super) fn mut_overlaps(&mut self, taken: MutParts) -> Vec<MutPart> {
        let (parts, keys): (Vec<MutPart>, Vec<MutPlaceKey>) = taken.into_iter().unzip();
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

    /// The base of a place, a chain of fields and indexes: a root that is no binding is hidden,
    /// then each index from the root outwards. No object and no element is read yet.
    fn bind_base(&mut self, e: tir::Expr, lets: &mut Vec<tir::Stmt>) -> tir::Expr {
        if matches!(e.kind, ExprKind::Local(_)) {
            return e;
        }
        let (ty, span) = (e.ty, e.span);
        match e.kind {
            ExprKind::Field { base, index } => {
                let base = Box::new(self.bind_base(*base, lets));
                tir::Expr::new(ExprKind::Field { base, index }, ty, span)
            }
            ExprKind::Index { base, index } => {
                let base = Box::new(self.bind_base(*base, lets));
                let index = Box::new(self.hide(*index, lets));
                tir::Expr::new(ExprKind::Index { base, index }, ty, span)
            }
            kind => self.hide(tir::Expr::new(kind, ty, span), lets),
        }
    }

    /// The call `call`, of a callee that has the `mut` parameters whose arguments are `parts`,
    /// as a `MutCall` in a block that binds the values of step 1, in written order: each
    /// argument that is no `mut` argument, and the root and the indexes of each `mut` argument.
    /// `call` reads the temp of each part where its argument is. The block ends in the call.
    ///
    /// Every copy-in is the read of its place and every store-back is `place = temp`.
    pub(super) fn mut_call(&mut self, mut call: tir::Expr, parts: Vec<MutPart>) -> tir::Expr {
        let (ty, span) = (call.ty, call.span);
        let (ExprKind::Call { args, .. } | ExprKind::InterfaceCall { args, .. }) = &mut call.kind
        else {
            unreachable!("a call with `mut` arguments should be a call of a function or a method")
        };
        let mut lets = Vec::new();
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
            let mut bound = Vec::new();
            if matches!(path.root, Root::Field(_)) {
                let base = operands
                    .next()
                    .expect("a field root's base should be the first operand of its path");
                bound.push(self.bind_base(base, &mut lets));
            }
            for index in operands {
                bound.push(self.hide(index, &mut lets));
            }
            let place = build(path.root, bound);
            let read = self.read(place.clone(), at);
            back.push(tir::Stmt::Assign {
                place,
                value: local(temp, part_ty, at),
            });
            muts.push(tir::MutArg {
                keep: Vec::new(),
                temp,
                read,
            });
            rebound.push(arg);
        }
        *args = rebound;
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
