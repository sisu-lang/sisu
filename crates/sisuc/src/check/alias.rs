//! Type aliases: `type Grid = [[char?]]` and `type Pairs<T> = [[T]]`. An alias is another name for
//! a type, so the checker resolves it and never stores it: a use is `Type::subst` of the alias's
//! body, and no later stage sees an alias.
//!
//! Every alias resolves in a pass before the signatures, each after the aliases that its body
//! names, so a use finds it resolved. The aliases are a graph, and a strongly connected component
//! of more than one alias, or an alias that writes itself, is a cycle. The body has a type
//! parameter by place, with no name, so an alias
//! that is applied to its own parameters in order is its body, and every alias shares one memo of
//! what it has substituted.
//! Two limits bound what a use builds: the depth of a type, and the number of distinct types that
//! one written type adds to the memo.

use std::rc::Rc;

use rustc_hash::FxHashMap;

use super::expr::{bare_none, dbg_argument, none_literal, rank};
use super::generic::{Site, type_meets};
use super::interface::{Bound, Slot};
use super::{
    Body, Checked, Checker, Declared, DiagnosticKind, Poisoned, UnknownForm, built_in_type,
    own_args, tail, value_span,
};
use crate::ast;
use crate::graph::{components, cycle_through};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{InterfaceId, Type, TypeArgs, TypeKind};
use crate::types::SharedMemo;

/// The deepest type a use of an alias may build, as the parser's limit for a written type.
const MAX_DEPTH: u32 = 256;

/// How many distinct types the resolution of one written type may add to the memo.
const MAX_ENTRIES: usize = 100_000;

#[cfg(test)]
thread_local! {
    /// How many aliases the walk over them has entered on this thread.
    pub(super) static VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The most aliases that the walk may enter on this thread: the one that passes it panics, so a
    /// test of a walk that would not end fails at once, as a stack overflow does not.
    pub(super) static VISIT_BUDGET: std::cell::Cell<usize> = const { std::cell::Cell::new(usize::MAX) };
}

/// Counts one alias that the walk enters, which only a test reads.
#[cfg_attr(
    not(test),
    expect(clippy::missing_const_for_fn, reason = "counts only in tests")
)]
fn count_visit() {
    #[cfg(test)]
    {
        VISITS.set(VISITS.get() + 1);
        assert!(
            VISITS.get() <= VISIT_BUDGET.get(),
            "alias resolution should stay within its budget of {} bodies",
            VISIT_BUDGET.get()
        );
    }
}

/// What a use of an alias checks again on the type it names: the check that the body cannot make,
/// as it names a parameter of the alias.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum UseCheck {
    /// An applied class or enum whose declaration has bounds.
    Applied(Type),
    /// A type that conforms to an interface, written at the span, which `T.Item` needs of `T`.
    Conforms(Type, InterfaceId, Span),
}

/// An alias that is resolved: `None` for the type is `Error`.
#[derive(Clone)]
pub(super) struct Resolved {
    /// The type that the alias names, with the parameter at place `i` as `Param("", i)`.
    pub(super) body: Type,
    /// The distinct checks of the body that name a parameter, which each use makes.
    pub(super) checks: Rc<[UseCheck]>,
}

#[derive(Clone)]
enum State {
    Unresolved,
    InProgress,
    Done(Option<Resolved>),
}

/// An alias the program declares, with its name, its parameters and its body as written.
pub(super) struct AliasInfo {
    pub(super) name: ast::Ident,
    /// The names of the parameters, as written.
    pub(super) params: Vec<Symbol>,
    ty: ast::TypeExpr,
    state: State,
    /// Whether the alias is on a cycle: `Error` whatever its body gives.
    failed: bool,
}

impl AliasInfo {
    pub(super) fn new(alias: &ast::AliasDecl) -> Self {
        Self {
            name: alias.name.clone(),
            params: alias.params.iter().map(|p| p.name).collect(),
            ty: alias.ty.clone(),
            state: State::Unresolved,
            failed: false,
        }
    }
}

/// What the checker keeps of the aliases.
#[derive(Default)]
pub(super) struct Aliases {
    /// The first alias of each name, and every broken one.
    pub(super) names: FxHashMap<Symbol, Declared<usize>>,
    /// Every alias in source order, so a second alias of a name has a body of its own.
    pub(super) infos: Vec<AliasInfo>,
    /// What every substitution of an alias body has built, for the whole check.
    memo: SharedMemo,
    /// How many entries the memo held when the outermost written type that resolves began.
    base: Option<usize>,
    /// Whether a use passed `MAX_ENTRIES` in the outermost written type.
    too_large: bool,
    /// How many uses of an alias have resolved.
    pub(super) uses: usize,
}

/// The type parameter of an alias body at place `i`: it has no name.
fn body_param(i: usize) -> Type {
    let i = u32::try_from(i).expect("an alias should have fewer than 2^32 parameters");
    Type::intern(TypeKind::Param(Symbol::intern(""), i))
}

/// What a name in a value position stands for when it is an alias: its parameters as written,
/// which a use binds as a call binds the parameters of a generic callee, and the type that it
/// names over them.
pub(super) struct Through {
    pub(super) params: Vec<Symbol>,
    pub(super) ty: Type,
    /// The checks that the alias's uses make once `params` are bound: none when the use wrote
    /// the type arguments, which made them.
    pub(super) checks: Rc<[UseCheck]>,
}

/// A name in a value position, and whether an alias took it.
pub(super) enum Via {
    /// No alias has the name.
    Not,
    /// An alias has it, and the alias is `Error`: its own report says why.
    Error,
    To(Through),
}

/// The type of a field as `resolve_field` finds it.
pub(super) struct FieldType {
    /// `None` is `Error`.
    pub(super) ty: Option<Type>,
    /// The class and enum types written in it, each with its span.
    pub(super) inner: Vec<(Type, Span)>,
    /// The type as the field writes it, when it names an alias.
    pub(super) shown: Option<Rc<str>>,
}

impl Checker<'_> {
    /// The aliases that the type `ty` writes and the parameters `params` do not hide, each with
    /// where the name stands, in source order.
    fn alias_refs(&self, ty: &ast::TypeExpr, params: &[Symbol], out: &mut Vec<(usize, Span)>) {
        let named = |name: Symbol| match self.aliases.names.get(&name) {
            Some(&Declared::Known(id)) if !params.contains(&name) => Some((
                id,
                Span::new(ty.span.start(), ty.span.start() + name.as_str().len()),
            )),
            _ => None,
        };
        match &ty.kind {
            ast::TypeKind::Name(name) => out.extend(named(*name)),
            ast::TypeKind::Applied(applied) => {
                out.extend(named(applied.name));
                for arg in &applied.args {
                    self.alias_refs(arg, params, out);
                }
            }
            ast::TypeKind::Array(element) => self.alias_refs(element, params, out),
            ast::TypeKind::Member { base, .. } => self.alias_refs(base, params, out),
            ast::TypeKind::SelfType => {}
        }
    }

    /// Resolves every alias, each after the aliases that its body names, so that a use finds its
    /// alias resolved and a chain of any length takes no recursion. The aliases and the aliases
    /// that each writes are a graph, whose strongly connected components give both the order and
    /// the cycles. A cyclic component is reported once, and its aliases are `Error`. An alias that
    /// no signature names is resolved too, so its errors are reported.
    pub(super) fn resolve_aliases(&mut self) {
        let count = self.aliases.infos.len();
        let refs: Vec<Vec<(usize, Span)>> = (0..count)
            .map(|i| {
                let info = &self.aliases.infos[i];
                let mut out = Vec::new();
                self.alias_refs(&info.ty, &info.params, &mut out);
                out
            })
            .collect();
        let edges: Vec<Vec<usize>> = (refs.iter())
            .map(|out| out.iter().map(|&(to, _)| to).collect())
            .collect();
        let component = components(&edges);
        let mut size = vec![0; count];
        for &c in &component {
            size[c] += 1;
        }
        let mut reported = vec![false; count];
        for i in 0..count {
            if size[component[i]] == 1 && !edges[i].contains(&i) {
                continue;
            }
            // The first alias of a component in source order carries its report.
            if !std::mem::replace(&mut reported[component[i]], true) {
                self.alias_cycle(&refs, (&edges, &component), i);
            }
            self.aliases.infos[i].failed = true;
            self.aliases.infos[i].state = State::Done(None);
        }
        // A component's number is above that of every component it reaches: its aliases last.
        let mut order: Vec<usize> = (0..count).collect();
        order.sort_by_key(|&i| component[i]);
        for i in order {
            count_visit();
            if self.aliases.infos[i].failed {
                // On a cycle: `Error` already, but the body may hold other errors.
                self.resolve_body(i);
            } else {
                self.alias_body(i);
            }
        }
    }

    /// The alias `id`, resolved; `None` is `Error`. The walk of `resolve_aliases` resolves it
    /// before any use, so a use finds it done. An alias that is in progress gives `Error`.
    fn alias_body(&mut self, id: usize) -> Option<Resolved> {
        match &self.aliases.infos[id].state {
            State::Done(done) => return done.clone(),
            State::InProgress => return None,
            State::Unresolved => {}
        }
        self.aliases.infos[id].state = State::InProgress;
        let done = self.resolve_body(id);
        self.aliases.infos[id].state = State::Done(done.clone());
        done
    }

    /// The body of the alias `id`, resolved: its type, and the checks that each use makes.
    fn resolve_body(&mut self, id: usize) -> Option<Resolved> {
        let (params, ty) = {
            let info = &self.aliases.infos[id];
            (info.params.clone(), info.ty.clone())
        };
        // The body is a written type of its own, with the parameters of the alias in scope and
        // no other part of the declaration that names it.
        let written = self.written.take();
        let base = self.aliases.base.take();
        let from = self.queued();
        let fresh = Body {
            alias: Some(Vec::new()),
            ..Body::default()
        };
        let (body, state) = self.with_body(fresh, |c| {
            c.body.set_type_params(&params);
            c.resolve(&ty)
        });
        self.keep_at(from..self.queued(), Slot::Alias(id));
        self.written = written;
        self.aliases.base = base;
        body.map(|body| Resolved {
            body,
            checks: state.alias.unwrap_or_default().into(),
        })
    }

    /// Makes the alias `id` `Error`, as an alias is whose bounds fail where it is declared.
    pub(super) fn unknown_alias(&mut self, id: usize) {
        self.aliases.infos[id].state = State::Done(None);
    }

    /// Reports the cycle of the component of the alias `first`, the first of the component in
    /// source order: a shortest cycle through it, which names each alias with the reference that
    /// leads to the next. `refs` holds each alias's references in order, and `graph` its edges
    /// and its components.
    fn alias_cycle(
        &mut self,
        refs: &[Vec<(usize, Span)>],
        graph: (&[Vec<usize>], &[usize]),
        first: usize,
    ) {
        let mut cycle = vec![first];
        cycle.extend(cycle_through(graph.0, graph.1, first));
        let name = |k: usize| self.aliases.infos[cycle[k % cycle.len()]].name.clone();
        let steps = (0..cycle.len())
            .map(|k| {
                let (from, to) = (cycle[k], cycle[(k + 1) % cycle.len()]);
                let (_, at) = *refs[from]
                    .iter()
                    .find(|&&(next, _)| next == to)
                    .expect("an alias on a cycle should write the next one");
                (name(k).name.to_string(), name(k + 1).name.to_string(), at)
            })
            .collect();
        let through = (1..cycle.len()).map(|k| name(k).name.to_string()).collect();
        self.report(DiagnosticKind::AliasCycle {
            name: name(0).name.to_string(),
            at: name(0).span,
            through,
            steps,
        });
    }

    /// What `name` stands for as an alias, where a value's name is a class, an enum, an optional or
    /// a built-in type: a constructor, a variant, a conversion or a member of the type that the
    /// alias names. The caller reads the name as a type's only after a local has not taken it.
    pub(super) fn through(&mut self, name: Symbol) -> Via {
        let id = match self.aliases.names.get(&name) {
            None => return Via::Not,
            Some(&Declared::Known(id)) => id,
            Some(Declared::Broken) => return Via::Error,
        };
        let Some(resolved) = self.alias_body(id) else {
            return Via::Error;
        };
        let params = self.aliases.infos[id].params.clone();
        // The parameters take their written names, as a message prints them.
        let ty = self.substitute(resolved.body, own_args(&params));
        // The body was within the limits when it resolved, so a substitution of its own
        // parameters stays within them; if not, the alias is `Error`, and the use reports nothing.
        self.aliases.too_large = false;
        match ty {
            Some(ty) => Via::To(Through {
                params,
                ty,
                checks: resolved.checks,
            }),
            None => Via::Error,
        }
    }

    /// `through` for a name that a site writes with the type arguments `written` in `angles`: the
    /// alias is applied as a written type is, so the type arguments are resolved and counted, the
    /// checks of the alias are made, and the limits hold. The type that it gives is complete: it
    /// has no parameter left to bind.
    pub(super) fn through_site(
        &mut self,
        name: &ast::Ident,
        written: &[ast::TypeExpr],
        angles: Span,
    ) -> Via {
        let through = match self.through(name.name) {
            Via::To(through) => through,
            other => return other,
        };
        if written.is_empty() {
            return Via::To(through);
        }
        self.aliases.uses += 1;
        let Some(list) = self.type_args_of(name, &through.params, written, angles, false) else {
            return Via::Error;
        };
        let at = name.span.to(angles);
        let outermost = self.begin_written();
        let built = self.substitute(through.ty, list);
        if outermost {
            self.aliases.base = None;
        }
        let Some(built) = built else {
            if std::mem::take(&mut self.aliases.too_large) {
                self.report(DiagnosticKind::AliasTooLarge {
                    ty: self.source.text(at).to_string(),
                    at,
                    limit: MAX_ENTRIES,
                });
            }
            return Via::Error;
        };
        match self.use_checks(&through.checks, list, at) {
            Some(true) => {}
            _ => return Via::Error,
        }
        match self.settle(built) {
            Some(ty) => Via::To(Through {
                params: Vec::new(),
                ty,
                checks: Rc::from([]),
            }),
            None => Via::Error,
        }
    }

    /// The type that the alias `name` names, if it is resolved and not `Error`; a parameter shows
    /// as `Param("", i)`. The reads that cannot change the checker use it, and ask only for a
    /// built-in type, which no parameter is in.
    pub(super) fn alias_type(&self, name: Symbol) -> Option<Type> {
        let &Declared::Known(id) = self.aliases.names.get(&name)? else {
            return None;
        };
        match &self.aliases.infos[id].state {
            State::Done(Some(resolved)) => Some(resolved.body),
            _ => None,
        }
    }

    /// Whether `name` is an alias that takes no type parameter.
    pub(super) fn alias_takes_none(&self, name: Symbol) -> bool {
        matches!(self.aliases.names.get(&name), Some(&Declared::Known(id)) if self.aliases.infos[id].params.is_empty())
    }

    /// Whether `e` is the empty optional: `None`, `Option.None`, `Option<T>.None`, or the same
    /// through an alias of an optional type that no local hides.
    pub(super) fn none_written(&self, e: &ast::Expr) -> bool {
        none_literal(e) || self.alias_none(e).is_some()
    }

    /// `Some(bare)` when `e` is `A.None` or `A<T>.None` for an alias `A` of an optional type that
    /// no local hides: `bare` when the alias takes parameters and `e` writes none, so the context
    /// fixes the type, as it does for `Option.None`.
    pub(super) fn alias_none(&self, e: &ast::Expr) -> Option<bool> {
        let ast::ExprKind::Field {
            base,
            name,
            safe: false,
        } = &e.kind
        else {
            return None;
        };
        let (alias, bare) = match &base.kind {
            ast::ExprKind::Name(alias) => (alias, true),
            ast::ExprKind::Applied { name, .. } => (&name.name, false),
            _ => return None,
        };
        let Some(&Declared::Known(id)) = self.aliases.names.get(alias) else {
            return None;
        };
        let State::Done(Some(resolved)) = &self.aliases.infos[id].state else {
            return None;
        };
        (name.name.as_str() == "None"
            && matches!(resolved.body.kind(), TypeKind::Optional(_))
            && !self.body.scopes.iter().any(|s| s.contains_key(alias)))
        .then(|| bare && !self.aliases.infos[id].params.is_empty())
    }

    /// `rank` of `e`, where the empty optional of an alias with parameters waits for its context
    /// as `Option.None` does.
    pub(super) fn rank(&self, e: &ast::Expr) -> u8 {
        if let Some(argument) = dbg_argument(e) {
            return self.rank(argument);
        }
        if self.alias_none(e) == Some(true) {
            return 2;
        }
        rank(e)
    }

    /// `bare_none` of `e`, which also holds for `A.None` as `rank` says.
    pub(super) fn bare_none_written(&self, e: &ast::Expr) -> bool {
        bare_none(e) || self.alias_none(e) == Some(true)
    }

    /// Whether a path that starts with the alias `name` reads the alias: no local has the name, or
    /// the alias names an enum, which a local cannot hide (it is reported).
    pub(super) fn alias_visible(&mut self, name: Symbol) -> bool {
        self.lookup(name).is_none() || self.alias_names_enum(name)
    }

    /// Whether `name` is an alias of an enum, whose variants a path of the name writes.
    pub(super) fn alias_names_enum(&self, name: Symbol) -> bool {
        self.alias_type(name)
            .is_some_and(|ty| matches!(ty.kind(), TypeKind::Enum(..)))
    }

    /// Whether `name` is an alias that is `Error`: a broken one, or one that has failed. Its own
    /// error stands, so a use of it reports nothing more.
    pub(super) fn alias_error(&self, name: Symbol) -> bool {
        match self.aliases.names.get(&name) {
            Some(Declared::Broken) => true,
            Some(&Declared::Known(id)) => matches!(self.aliases.infos[id].state, State::Done(None)),
            None => false,
        }
    }

    /// Whether the path base `base` names a built-in type: its own name, or an alias of one,
    /// which may write type arguments after it that the alias takes.
    pub(super) fn names_built_in(&self, base: &ast::Expr) -> bool {
        match &base.kind {
            ast::ExprKind::Name(name) => self.built_in_of(*name).is_some(),
            ast::ExprKind::Applied { name, .. } => {
                self.aliases.names.contains_key(&name.name) && self.built_in_of(name.name).is_some()
            }
            _ => false,
        }
    }

    /// The built-in type that `name` names: a built-in type's own name, or an alias of one.
    pub(super) fn built_in_of(&self, name: Symbol) -> Option<Type> {
        built_in_type(name.as_str()).or_else(|| {
            self.alias_type(name).filter(|ty| {
                matches!(
                    *ty,
                    Type::I64 | Type::U8 | Type::F64 | Type::BOOL | Type::CHAR | Type::STRING
                )
            })
        })
    }

    /// The type that the alias `id` names, written as `ty` with its type arguments `args` in
    /// `angles`: its body with each parameter replaced. `None`, reported, when the arguments do not
    /// fit or the alias is `Error`.
    pub(super) fn alias(
        &mut self,
        id: usize,
        ty: &ast::TypeExpr,
        name: Symbol,
        args: &[ast::TypeExpr],
        angles: Span,
    ) -> Option<Type> {
        let ident = ast::Ident {
            name,
            span: Span::new(ty.span.start(), ty.span.start() + name.as_str().len()),
        };
        self.aliases.uses += 1;
        let resolved = self.alias_body(id);
        let params = self.aliases.infos[id].params.clone();
        let list = self.type_args_of(&ident, &params, args, angles, false);
        let (resolved, list) = (resolved?, list?);
        let built = self.substitute(resolved.body, list)?;
        // The checks come before the replacement of an associated type, which poisons a type that
        // does not conform without a report of its own. A type that fails one is `Error`, as a
        // class that fails its bound is.
        if !self.use_checks(&resolved.checks, list, ty.bare)? {
            return None;
        }
        self.settle(built)
    }

    /// `ty` with the type arguments `list` for its parameters, through the memo that every use
    /// shares; `None` when that adds too many entries. An alias applied to its own parameters in
    /// order is its body, whatever it is applied to: nothing is walked.
    fn substitute(&mut self, ty: Type, list: TypeArgs) -> Option<Type> {
        let own: Vec<Type> = (0..list.get().len()).map(body_param).collect();
        if list == TypeArgs::intern(&own) {
            return Some(ty);
        }
        let base = self.aliases.base.unwrap_or(self.aliases.memo.len());
        let built = ty.subst_shared(list, &mut self.aliases.memo, base + MAX_ENTRIES);
        if built.is_none() {
            self.aliases.too_large = true;
        }
        built
    }

    /// `built`, a use of an alias, with each associated type of a known type replaced, once every
    /// conformance is known; before, there is no answer, and the type stays as it is.
    fn settle(&self, built: Type) -> Option<Type> {
        if self.pending_bounds.is_none() {
            return self.normalize(built);
        }
        Some(built)
    }

    /// Makes the checks of a use of an alias with the type arguments `list`, written at `at`. In
    /// an alias body, one that still names a parameter waits for the use of that alias. Whether
    /// they all hold; `None` when a substitution passed the limit.
    pub(super) fn use_checks(
        &mut self,
        checks: &[UseCheck],
        list: TypeArgs,
        at: Span,
    ) -> Option<bool> {
        let mut held = true;
        for &check in checks {
            let (of, applied) = match check {
                UseCheck::Applied(ty) => (ty, None),
                UseCheck::Conforms(ty, interface, bound) => (ty, Some((interface, bound))),
            };
            let ty = self.substitute(of, list)?;
            let check = match applied {
                None => UseCheck::Applied(ty),
                Some((interface, bound)) => UseCheck::Conforms(ty, interface, bound),
            };
            held &= self.make_check(check, at);
        }
        Some(held)
    }

    /// Makes `check` where the type is written at `at`; in an alias body, a type that names a
    /// parameter is kept for the uses of the alias instead. Whether it holds, or waits.
    pub(super) fn make_check(&mut self, check: UseCheck, at: Span) -> bool {
        let (ty, count) = match check {
            UseCheck::Applied(ty) => (ty, ty_args(ty)),
            UseCheck::Conforms(ty, ..) => (ty, 0),
        };
        if let Some(kept) = &mut self.body.alias
            && ty.has_param()
        {
            if !kept.contains(&check) {
                kept.push(check);
            }
            return true;
        }
        match check {
            UseCheck::Applied(ty) => self.check_applied(ty, vec![at; count]),
            UseCheck::Conforms(ty, interface, bound) => {
                let bound = Bound {
                    interface,
                    args: TypeArgs::NONE,
                    at: bound,
                };
                self.check_conforms(at, ty, bound)
            }
        }
    }

    /// Whether `name` is `Option`, or an alias of an optional type.
    pub(super) fn names_option(&mut self, name: Symbol) -> bool {
        name.as_str() == "Option"
            || matches!(self.through(name), Via::To(through) if matches!(through.ty.kind(), TypeKind::Optional(_)))
    }

    /// Whether `name`, a qualifier in a pattern at a position of the optional type `ty`, is
    /// `Option`, or an alias of that type, whose fixed arguments hold: the alias's own parameters
    /// are the only types that `ty` may leave open.
    pub(super) fn names_option_at(&mut self, name: Symbol, ty: Type) -> bool {
        name.as_str() == "Option"
            || matches!(self.through(name), Via::To(through)
                if matches!(through.ty.kind(), TypeKind::Optional(_))
                    && type_meets(through.params.len(), through.ty, ty))
    }

    /// `A.member`, or `A.member(args)` when `call` gives the arguments and the span of their
    /// parentheses, where `site` writes the alias `A`: the variant of the enum that it names, or of
    /// `Option` for an optional. `None` when `A` is no alias, or names a type that has no
    /// variant.
    pub(super) fn alias_path(
        &mut self,
        span: Span,
        site: Site<'_>,
        member: &ast::Ident,
        call: Option<(&[ast::Arg], Span)>,
    ) -> Option<Checked> {
        match self.through_site(site.name, site.written, site.angles) {
            Via::Not => None,
            Via::Error => {
                self.stray(call.map_or(&[], |(args, _)| args));
                Some(Err(Poisoned::error()))
            }
            Via::To(through) => match through.ty.kind() {
                TypeKind::Enum(id, _) => {
                    Some(self.enum_path(span, id, member, call, site, Some(&through)))
                }
                TypeKind::Optional(_) => {
                    Some(self.optional_alias_path(span, site, &through, member, call))
                }
                _ => None,
            },
        }
    }

    /// `A.None` and `A.Some(x)`, where the alias `A` names an optional type: the paths of `Option`.
    /// The type that the alias names is the context of the value when it takes no parameter, or
    /// when the type arguments written after it fixed it. Else the value binds the parameters, as
    /// a call of a generic callee does.
    pub(super) fn optional_alias_path(
        &mut self,
        span: Span,
        site: Site<'_>,
        through: &Through,
        member: &ast::Ident,
        call: Option<(&[ast::Arg], Span)>,
    ) -> Checked {
        if !through.params.is_empty() {
            return self.option_path(span, site, member, call, Some(through));
        }
        // The value is `Option`'s, so a message names `Option`, not the alias.
        let option = ast::Ident {
            name: Symbol::intern("Option"),
            span: site.name.span,
        };
        let site = Site {
            name: &option,
            written: &[],
            expected: Some(through.ty),
            ..site
        };
        self.option_path(span, site, member, call, None)
    }

    /// The type `base.name` in an alias body, where `base`, a parameter of the alias, takes no
    /// bound: the one interface that declares an associated type `name`, which each use checks
    /// that its argument conforms to. `None`, reported, when no interface declares one, or two do.
    pub(super) fn alias_assoc(
        &mut self,
        ty: &ast::TypeExpr,
        base: &ast::TypeExpr,
        base_ty: Type,
        name: &ast::Ident,
    ) -> Option<Type> {
        let declaring = self
            .declaring_assoc
            .get(&name.name)
            .cloned()
            .unwrap_or_default();
        let written = self.source.text(base.span).to_string();
        match declaring[..] {
            // A generic interface needs arguments that no bound writes: no one names it.
            [(interface, k)] if self.interfaces[interface.0].type_params.len() == 1 => {
                let k = u32::try_from(k).expect("an interface should have fewer than 2^32 types");
                self.make_check(UseCheck::Conforms(base_ty, interface, ty.bare), ty.bare);
                Some(Type::intern(TypeKind::Assoc(
                    base_ty,
                    interface,
                    TypeArgs::NONE,
                    k,
                )))
            }
            [] | [_] => {
                self.report(DiagnosticKind::UnknownType {
                    name: name.name.to_string(),
                    at: name.span,
                    form: UnknownForm::NoAssociated {
                        base: written,
                        add: None,
                    },
                });
                None
            }
            _ => {
                self.report(DiagnosticKind::Ambiguous {
                    method: name.name.to_string(),
                    receiver: written,
                    interfaces: (declaring.iter())
                        .map(|&(interface, _)| self.bound_text(interface, TypeArgs::NONE))
                        .collect(),
                    at: name.span,
                    annotate: None,
                });
                None
            }
        }
    }

    /// `ty`, a written type that has been resolved, as `resolved`: the limits of aliases apply. A
    /// type that a use of an alias made, and that nests more than `MAX_DEPTH` levels, is reported
    /// at the written type, and is `Error`. `uses` is how many uses had resolved before it began.
    pub(super) fn alias_limits(
        &mut self,
        ty: &ast::TypeExpr,
        resolved: Option<Type>,
        uses: usize,
    ) -> Option<Type> {
        let resolved = resolved?;
        if self.aliases.uses > uses && resolved.depth() > MAX_DEPTH {
            self.report(DiagnosticKind::NestingTooDeep {
                at: ty.bare,
                limit: MAX_DEPTH as usize,
                alias: Some(self.source.text(ty.bare).to_string()),
            });
            return None;
        }
        Some(resolved)
    }

    /// Starts to count the entries of the memo that the written type being resolved adds, unless
    /// a written type around it counts them. Whether this type is the outermost.
    pub(super) fn begin_written(&mut self) -> bool {
        let outermost = self.aliases.base.is_none();
        if outermost {
            self.aliases.base = Some(self.aliases.memo.len());
        }
        outermost
    }

    /// Ends the count that `begin_written` started: a written type that added more than
    /// `MAX_ENTRIES` entries is reported at `ty`, and is `Error`.
    pub(super) fn end_written(
        &mut self,
        ty: &ast::TypeExpr,
        resolved: Option<Type>,
    ) -> Option<Type> {
        self.aliases.base = None;
        if !std::mem::take(&mut self.aliases.too_large) {
            return resolved;
        }
        self.report(DiagnosticKind::AliasTooLarge {
            ty: self.source.text(ty.bare).to_string(),
            at: ty.bare,
            limit: MAX_ENTRIES,
        });
        None
    }

    /// The type written as `ty`, when the types it resolved to followed a use of an alias, which
    /// `uses` says: a mismatch with a declared type shows the type as the declaration writes it.
    pub(super) fn shown_text(&self, ty: &ast::TypeExpr, uses: usize) -> Option<Rc<str>> {
        (self.aliases.uses > uses).then(|| Rc::from(ty.to_string()))
    }

    /// Says that the expression at `at`, which is checked against `want`, has the type that its
    /// declaration writes as `text`, so a mismatch at `at` shows `text`.
    pub(super) fn show(&mut self, at: Span, want: Type, text: &Rc<str>) {
        self.body.shown.insert(at, (want, Rc::clone(text)));
    }

    /// How a mismatch at `at` names `want`: as the declaration of what is wanted writes it, when
    /// it names an alias, else as the resolved type.
    pub(super) fn want_name(&self, at: Span, want: Type) -> String {
        match self.body.shown.get(&at) {
            Some((ty, text)) if *ty == want => text.to_string(),
            _ => self.type_name(want),
        }
    }

    /// `resolve_written` for the type of a field, and the type as the field writes it, when the
    /// type names an alias and the class takes no type parameter, `plain`.
    pub(super) fn resolve_field(&mut self, ty: &ast::TypeExpr, plain: bool) -> FieldType {
        let uses = self.aliases.uses;
        let (resolved, inner) = self.resolve_written(ty);
        FieldType {
            ty: resolved,
            inner,
            shown: self.shown_text(ty, uses).filter(|_| plain),
        }
    }

    /// Says that the value of `body`, the block of a function that returns `want`, has the return
    /// type as the signature writes it, when that names an alias.
    pub(super) fn show_value(&mut self, body: &ast::Block, want: Option<Type>) {
        let (Some(want), Some(text)) = (want, self.body.ret_shown.clone()) else {
            return;
        };
        self.show(value_span(body), want, &text);
        if let Some(value) = tail(body) {
            self.show(value.span, want, &text);
        }
    }

    /// Replaces the associated types of known types in the types that the declarations keep, as
    /// `normalize` does where a body uses a type. A use of an alias such as `type X<T> = T.Item`
    /// puts a class under an associated type when a signature or a field names `X<Ints>`, before
    /// any conformance is known to say which type that is. Without an alias, no declaration holds
    /// one, so the pass does nothing.
    pub(super) fn normalize_declarations(&mut self) {
        if self.aliases.infos.is_empty() {
            return;
        }
        let known = |checker: &Self, ty: Option<Type>| match ty {
            Some(ty) if ty.has_assoc() => checker.normalize(ty),
            ty => ty,
        };
        for i in 0..self.signatures.len() {
            let params: Vec<_> = (self.signatures[i].params.iter())
                .map(|&ty| known(self, ty))
                .collect();
            self.signatures[i].ret = known(self, self.signatures[i].ret);
            self.signatures[i].params = params;
        }
        for i in 0..self.classes.len() {
            for k in 0..self.classes[i].fields.len() {
                self.classes[i].fields[k].ty = known(self, self.classes[i].fields[k].ty);
            }
        }
        for i in 0..self.enums.len() {
            for v in 0..self.enums[i].variants.len() {
                let fields: Vec<_> = (self.enums[i].variants[v].fields.iter())
                    .map(|&ty| known(self, ty))
                    .collect();
                self.enums[i].variants[v].fields = fields;
            }
        }
    }
}

/// How many type arguments the applied class or enum `ty` has.
fn ty_args(ty: Type) -> usize {
    match ty.kind() {
        TypeKind::Class(_, args) | TypeKind::Enum(_, args) => args.get().len(),
        _ => unreachable!("only an applied class or enum has bounds to check"),
    }
}
