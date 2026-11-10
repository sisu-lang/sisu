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

use rustc_hash::FxHashSet;

use super::expr::{bare_none, none_literal};
use super::generic::{Site, type_meets};
use super::interface::{Bound, Slot};
use super::reports::Reported;
use super::{
    Body, Checked, Checker, Declared, DiagnosticKind, Expectation, Poisoned, UnknownForm,
    built_in_type, own_args, tail, value_span,
};
use crate::ast;
use crate::diagnostic::{Definition, NestingForm};
use crate::graph::{components, cycle_through};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{InterfaceId, Type, TypeArgs, TypeKind};
use crate::types::SharedMemo;

/// The deepest type a use of an alias may build, as the parser's limit for a written type.
const MAX_DEPTH: u32 = 256;

/// How many distinct types the resolution of one written type may add to the memo.
const MAX_ENTRIES: usize = 100_000;
/// The most entries that the memo may hold in a whole check, whatever the written types add. The
/// limit of one written type bounds a type, and this bounds the program: a program of many uses,
/// each under `MAX_ENTRIES` with its own arguments, would otherwise build and keep all of them.
/// It is a first value, and GitHub issue #322 tunes it.
pub(super) const MAX_CHECK_ENTRIES: usize = 1_000_000;

/// Which limit of the memo a substitution passed.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Passed {
    /// The limit of one written type, `MAX_ENTRIES`.
    Type,
    /// The limit of the whole check, `MAX_CHECK_ENTRIES`.
    Check,
}

#[cfg(test)]
thread_local! {
    /// The most entries the memo has held on this thread, read after each substitution.
    pub(super) static MEMO_PEAK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
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
    /// An applied class or enum whose declaration has bounds, or an `any` type whose interface
    /// has bounds on its arguments.
    Applied(Type),
    /// A type that conforms to an interface, which `T.Item` needs of `T`: `T.Item` is written at
    /// `at`, with `T` at `base`.
    Conforms {
        ty: Type,
        interface: InterfaceId,
        at: Span,
        base: Span,
    },
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
    /// Every alias in source order, so a second alias of a name has a body of its own.
    pub(super) infos: Vec<AliasInfo>,
    /// For each alias, the aliases whose bodies name it.
    users: Vec<Vec<usize>>,
    /// The alias whose body is being resolved.
    resolving: Option<usize>,
    /// What every substitution of an alias body has built, for the whole check.
    memo: SharedMemo,
    /// How many entries the memo held when the outermost written type that resolves began.
    base: Option<usize>,
    /// The limit that a use passed, which is reported at the end of the outermost written type.
    /// A limit that `through` passed waits for the next use that reports one.
    too_large: Option<Passed>,
    /// Whether the memo passed `MAX_CHECK_ENTRIES`: every later substitution fails, with no
    /// report of its own, so each later use of an alias is `Error`.
    exhausted: bool,
    /// How many uses of an alias have resolved.
    pub(super) uses: usize,
    /// The interfaces that an alias body names as `S.Name`. An alias has no owner, so no use
    /// records them, and `Checker::reach` marks them as used.
    pub(super) named_interfaces: Vec<InterfaceId>,
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
        let named = |name: Symbol| match self.names.alias(name) {
            Some(Declared::Known(id)) if !params.contains(&name) => Some((
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
            ast::TypeKind::Array(element) | ast::TypeKind::Any(element) => {
                self.alias_refs(element, params, out);
            }
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
        let mut users = vec![Vec::new(); count];
        for (user, out) in edges.iter().enumerate() {
            for &named in out {
                users[named].push(user);
            }
        }
        self.aliases.users = users;
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
        let outer = self.aliases.resolving.replace(id);
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
        self.aliases.resolving = outer;
        body.map(|body| Resolved {
            body,
            checks: state.alias.unwrap_or_default().into(),
        })
    }

    /// The names that make a cycle when the body being resolved writes them: the alias itself,
    /// and each alias that names it, also through other aliases. Empty outside an alias body.
    pub(super) fn cyclic_names(&self) -> FxHashSet<Symbol> {
        let mut names = FxHashSet::default();
        let Some(id) = self.aliases.resolving else {
            return names;
        };
        let mut seen = vec![false; self.aliases.infos.len()];
        seen[id] = true;
        let mut stack = vec![id];
        while let Some(alias) = stack.pop() {
            names.insert(self.aliases.infos[alias].name.name);
            for &user in &self.aliases.users[alias] {
                if !std::mem::replace(&mut seen[user], true) {
                    stack.push(user);
                }
            }
        }
        names
    }

    /// The aliases that a suggestion for an unknown type may name: not one that reaches the body
    /// being resolved, as the fix would make a cycle.
    pub(super) fn alias_candidates(&self) -> impl Iterator<Item = (&str, Definition)> {
        let cyclic = self.cyclic_names();
        (self.names.alias_names())
            .filter(move |name| !cyclic.contains(name))
            .map(|name| (name.as_str(), Definition::Alias))
    }

    /// Makes the alias `id` `Error`, as an alias is whose bounds fail where it is declared. So is
    /// every alias that names it, also through other aliases. Each is made once.
    pub(super) fn unknown_alias(&mut self, id: usize) {
        let Aliases { infos, users, .. } = &mut self.aliases;
        // An unknown alias passed this on to its users when it became unknown. A second failed
        // bound of the same alias must not scan them again.
        if matches!(infos[id].state, State::Done(None)) {
            return;
        }
        infos[id].state = State::Done(None);
        let mut stack = vec![id];
        while let Some(alias) = stack.pop() {
            for &user in &users[alias] {
                #[cfg(test)]
                super::ALIAS_USER_STEPS.set(super::ALIAS_USER_STEPS.get() + 1);
                if matches!(infos[user].state, State::Done(Some(_))) {
                    infos[user].state = State::Done(None);
                    stack.push(user);
                }
            }
        }
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
        self.error(DiagnosticKind::AliasCycle {
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
        let id = match self.names.alias(name) {
            None => return Via::Not,
            Some(Declared::Known(id)) => id,
            Some(Declared::Poisoned(_)) => return Via::Error,
        };
        let Some(resolved) = self.alias_body(id) else {
            return Via::Error;
        };
        let params = self.aliases.infos[id].params.clone();
        // The parameters take their written names, as a message prints them.
        let ty = self.substitute(resolved.body, own_args(&params));
        // The body was within the limit of a written type when it resolved, so a substitution of
        // its own parameters stays within it. It can pass the limit of the whole check: then the
        // alias is `Error`, and the next use that reports a limit reports that one.
        if self.aliases.too_large == Some(Passed::Type) {
            self.aliases.too_large = None;
        }
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
        let Ok(list) = self.type_args_of(name, &through.params, written, angles, false) else {
            return Via::Error;
        };
        let at = name.span.to(angles);
        let outermost = self.begin_written();
        let built = self.substitute(through.ty, list);
        if outermost {
            self.aliases.base = None;
        }
        let Some(built) = built else {
            self.report_too_large(at);
            return Via::Error;
        };
        if self.use_checks(&through.checks, list, at).is_err() {
            return Via::Error;
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
        let Declared::Known(id) = self.names.alias(name)? else {
            return None;
        };
        match &self.aliases.infos[id].state {
            State::Done(Some(resolved)) => Some(resolved.body),
            _ => None,
        }
    }

    /// Whether `name` is an alias that takes no type parameter.
    pub(super) fn alias_takes_none(&self, name: Symbol) -> bool {
        matches!(self.names.alias(name), Some(Declared::Known(id)) if self.aliases.infos[id].params.is_empty())
    }

    /// Whether `e` is the empty optional: `None`, `Option.None`, `Option<T>.None`, or the same
    /// through an alias of an optional type, which no local can hide.
    pub(super) fn none_written(&self, e: &ast::Expr) -> bool {
        none_literal(e) || self.alias_none(e).is_some()
    }

    /// `Some(bare)` when `e` is `A.None` or `A<T>.None` for an alias `A` of an optional type:
    /// `bare` when the alias takes parameters and `e` writes none, so the context
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
        let Some(Declared::Known(id)) = self.names.alias(*alias) else {
            return None;
        };
        let State::Done(Some(resolved)) = &self.aliases.infos[id].state else {
            return None;
        };
        // A local cannot hide an alias of an optional type (it is reported).
        (name.name == Symbol::NONE && matches!(resolved.body.kind(), TypeKind::Optional(_)))
            .then(|| bare && !self.aliases.infos[id].params.is_empty())
    }

    /// `bare_none` of `e`, which also holds for the `A.None` of an alias with parameters, which
    /// waits for its context as `Option.None` does.
    pub(super) fn bare_none_written(&self, e: &ast::Expr) -> bool {
        bare_none(e) || self.alias_none(e) == Some(true)
    }

    /// Whether a path that starts with the alias `name` reads the alias: no local has the name, or
    /// the alias names an enum or an optional type, which a local cannot hide (it is reported).
    pub(super) fn alias_visible(&mut self, name: Symbol) -> bool {
        self.lookup(name).is_none() || self.alias_unhideable(name)
    }

    /// Whether `name` is an alias of an enum, whose variants a path of the name writes.
    pub(super) fn alias_names_enum(&self, name: Symbol) -> bool {
        self.alias_type(name)
            .is_some_and(|ty| matches!(ty.kind(), TypeKind::Enum(..)))
    }

    /// Whether `name` is an alias of an optional type, whose `Some` and `None` a path writes.
    pub(super) fn alias_names_option(&self, name: Symbol) -> bool {
        self.alias_type(name)
            .is_some_and(|ty| matches!(ty.kind(), TypeKind::Optional(..)))
    }

    /// Whether a local cannot hide the alias `name`, as none can hide what it names: an enum, and
    /// `Option`.
    pub(super) fn alias_unhideable(&self, name: Symbol) -> bool {
        self.alias_names_enum(name) || self.alias_names_option(name)
    }

    /// Whether `name` is an alias that is `Error`: a broken one, or one that has failed. Its own
    /// error stands, so a use of it reports nothing more.
    pub(super) fn alias_error(&self, name: Symbol) -> bool {
        match self.names.alias(name) {
            Some(Declared::Poisoned(_)) => true,
            Some(Declared::Known(id)) => matches!(self.aliases.infos[id].state, State::Done(None)),
            None => false,
        }
    }

    /// Whether the path base `base` names a built-in type: its own name, or an alias of one,
    /// which may write type arguments after it that the alias takes.
    pub(super) fn names_built_in(&self, base: &ast::Expr) -> bool {
        match &base.kind {
            ast::ExprKind::Name(name) => self.built_in_of(*name).is_some(),
            ast::ExprKind::Applied { name, .. } => {
                self.names.alias(name.name).is_some() && self.built_in_of(name.name).is_some()
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
        let (resolved, list) = (resolved?, list.ok()?);
        let built = self.substitute(resolved.body, list)?;
        // The checks come before the replacement of an associated type, which poisons a type that
        // does not conform without a report of its own. A type that fails one is `Error`, as a
        // class that fails its bound is.
        self.use_checks(&resolved.checks, list, ty.bare).ok()?;
        // The body of the alias was resolved once, with no owner, so its `any` types name their
        // interfaces for each use in a body. The type arguments record their own where they are
        // written.
        self.record_anys(resolved.body, ty.span);
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
        if self.aliases.exhausted {
            return None;
        }
        let base = self.aliases.base.unwrap_or(self.aliases.memo.len());
        let per_type = base + MAX_ENTRIES;
        let built = ty.subst_shared(
            list,
            &mut self.aliases.memo,
            per_type.min(MAX_CHECK_ENTRIES),
        );
        #[cfg(test)]
        MEMO_PEAK.set(MEMO_PEAK.get().max(self.aliases.memo.len()));
        if built.is_none() {
            // The limit of the type comes first when both are passed together.
            let passed = if per_type <= MAX_CHECK_ENTRIES {
                Passed::Type
            } else {
                self.aliases.exhausted = true;
                Passed::Check
            };
            self.aliases.too_large = Some(passed);
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
    /// an alias body, one that still names a parameter waits for the use of that alias. `Ok`
    /// when they all hold; else the token of a failed check, or of a substitution that passed
    /// the limit.
    pub(super) fn use_checks(
        &mut self,
        checks: &[UseCheck],
        list: TypeArgs,
        at: Span,
    ) -> Result<(), Reported> {
        let mut held = Ok(());
        for &check in checks {
            let (of, applied) = match check {
                UseCheck::Applied(ty) => (ty, None),
                UseCheck::Conforms {
                    ty,
                    interface,
                    at,
                    base,
                } => (ty, Some((interface, at, base))),
            };
            let Some(ty) = self.substitute(of, list) else {
                return Err(self.unbuilt_check(at));
            };
            let check = match applied {
                None => UseCheck::Applied(ty),
                Some((interface, at, base)) => UseCheck::Conforms {
                    ty,
                    interface,
                    at,
                    base,
                },
            };
            let made = self.make_check(check, at);
            held = held.and(made);
        }
        held
    }

    /// Makes `check` where the type is written at `at`; in an alias body, a type that names a
    /// parameter is kept for the uses of the alias instead. `Ok` when it holds, or waits; else the
    /// token of a report.
    pub(super) fn make_check(&mut self, check: UseCheck, at: Span) -> Result<(), Reported> {
        let (ty, count) = match check {
            UseCheck::Applied(ty) => (ty, ty_args(ty)),
            UseCheck::Conforms { ty, .. } => (ty, 0),
        };
        if let Some(kept) = &mut self.body.alias
            && ty.has_param()
        {
            if !kept.contains(&check) {
                kept.push(check);
            }
            #[cfg(test)]
            super::reached(match check {
                UseCheck::Conforms { .. } => "make_check keeps a conformance",
                UseCheck::Applied(ty) if matches!(ty.kind(), TypeKind::Any(..)) => {
                    "make_check keeps an any"
                }
                UseCheck::Applied(_) => "make_check keeps an applied type",
            });
            return Ok(());
        }
        match check {
            UseCheck::Applied(ty) => match ty.kind() {
                TypeKind::Any(interface, args) => {
                    let bound = Bound {
                        interface,
                        args,
                        at,
                    };
                    self.check_arguments(bound, ty, vec![at; count]);
                    Ok(())
                }
                _ => self.check_applied(ty, vec![at; count]),
            },
            UseCheck::Conforms {
                ty,
                interface,
                at: bound_at,
                base,
            } => {
                let bound = Bound {
                    interface,
                    args: TypeArgs::NONE,
                    at: bound_at,
                };
                self.check_conforms(at, ty, bound, base)
            }
        }
    }

    /// Whether `name` is `Option`, or an alias of an optional type.
    pub(super) fn names_option(&mut self, name: Symbol) -> bool {
        name == Symbol::OPTION
            || matches!(self.through(name), Via::To(through) if matches!(through.ty.kind(), TypeKind::Optional(_)))
    }

    /// Whether `name`, a qualifier in a pattern at a position of the optional type `ty`, is
    /// `Option`, or an alias of that type, whose fixed arguments hold: the alias's own parameters
    /// are the only types that `ty` may leave open.
    pub(super) fn names_option_at(&mut self, name: Symbol, ty: Type) -> bool {
        name == Symbol::OPTION
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
                Some(self.failed_alias(site.name.span, call.map_or(&[], |(args, _)| args)))
            }
            Via::To(through) => match through.ty.kind() {
                // An enum with no parameter binds none of the alias's, which is reported once the
                // variant checks; a generic enum binds them in `enum_path`.
                TypeKind::Enum(id, _) if self.enums[id.0].type_params.is_empty() => {
                    let checked = self.enum_path(span, id, member, call, site, Some(&through));
                    if checked.is_ok() && !through.params.is_empty() {
                        let reported = self.report_unbound_alias(
                            site,
                            &through,
                            Some((member, call.is_some())),
                        );
                        return Some(Err(Poisoned::untyped(reported)));
                    }
                    Some(checked)
                }
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

    /// Reports the parameters of `through`, the alias that `site` writes with no type argument,
    /// as unbound (E0050). A value that holds no part of the alias's type, a conversion, a
    /// constant of `f64` or a variant of an enum that takes no parameter, binds none, and the
    /// context cannot, so the first is the one reported, with the form that the value has: the
    /// call of a conversion, or `Alias<...>.member` with `(...)` when a call follows. The caller
    /// asks only for a value that otherwise checks, so a mistake of the member or of the
    /// arguments comes first. A context that failed does not hide this report: no context can
    /// bind these parameters.
    pub(super) fn report_unbound_alias(
        &mut self,
        site: Site<'_>,
        through: &Through,
        member: Option<(&ast::Ident, bool)>,
    ) -> Reported {
        let name = site.name;
        let dots = vec!["..."; through.params.len()].join(", ");
        let (callee, example, at) = match member {
            Some((member, call)) => (
                format!("{}.{}", name.name, member.name),
                format!(
                    "{}<{dots}>.{}{}",
                    name.name,
                    member.name,
                    if call { "(...)" } else { "" }
                ),
                name.span.to(member.span),
            ),
            None => (
                name.name.to_string(),
                format!("{}<{dots}>(...)", name.name),
                name.span,
            ),
        };
        self.error(DiagnosticKind::CannotInferParam {
            param: through.params[0].to_string(),
            callee,
            example,
            at,
            in_result: false,
        })
    }

    /// A use at `at`, with the arguments `args`, of an alias that is `Error`.
    pub(super) fn failed_alias(&mut self, at: Span, args: &[ast::Arg]) -> Checked {
        self.stray(args);
        Err(Poisoned::untyped(self.failed_alias_token(at)))
    }

    /// The token of a use at `at` of an alias that is `Error`. Its own report says why, and no
    /// list of the checker holds the token of that report. A use in no written type reports the
    /// limit of the memo that it passed.
    pub(super) fn failed_alias_token(&mut self, at: Span) -> Reported {
        self.memo_report(at).unwrap_or_else(|| self.proof(at)) // proof: declaration
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
        // The value is `Option`'s, but a message names the alias, as the user wrote it.
        #[cfg(test)]
        super::reached("optional_alias_path");
        let site = if through.params.is_empty() {
            // A hinted value stays hinted.
            let expected = match site.expected {
                Expectation::Hint { siblings, .. } => Expectation::Hint {
                    want: through.ty,
                    siblings,
                },
                _ => Expectation::Required(through.ty),
            };
            Site {
                written: &[],
                expected,
                ..site
            }
        } else {
            site
        };
        self.option_path(span, site, member, call, Some(through))
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
        match declaring[..] {
            // A generic interface needs arguments that no bound writes: no one names it.
            [(interface, k)] if self.interfaces[interface.0].type_params.len() == 1 => {
                let k = u32::try_from(k).expect("an interface should have fewer than 2^32 types");
                self.aliases.named_interfaces.push(interface);
                // A type that does not conform is reported, and the associated type stays.
                let _ = self.make_check(
                    UseCheck::Conforms {
                        ty: base_ty,
                        interface,
                        at: ty.bare,
                        base: base.span,
                    },
                    ty.bare,
                );
                Some(Type::intern(TypeKind::Assoc(
                    base_ty,
                    interface,
                    TypeArgs::NONE,
                    k,
                )))
            }
            [] | [_] => {
                self.error(DiagnosticKind::UnknownType {
                    name: name.name.to_string(),
                    at: name.span,
                    form: UnknownForm::NoAssociated {
                        base: self.source.type_text(base.span).into_owned(),
                        add: None,
                    },
                });
                None
            }
            _ => {
                self.error(DiagnosticKind::Ambiguous {
                    method: name.name.to_string(),
                    receiver: self.source.type_text(base.span).into_owned(),
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
            self.error(DiagnosticKind::NestingTooDeep {
                at: ty.bare,
                limit: MAX_DEPTH as usize,
                form: NestingForm::Alias(self.source.type_text(ty.bare).into_owned()),
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
        if self.aliases.too_large.is_none() {
            return resolved;
        }
        self.report_too_large(ty.bare);
        None
    }

    /// The token of a check of an alias use at `at` whose type the memo did not build. A written
    /// type holds the report of the limit until it is done (`end_written`). A use in a value
    /// position is in no written type, so it reports the limit here.
    fn unbuilt_check(&mut self, at: Span) -> Reported {
        // `aliases.too_large` holds the report until the written type is done.
        self.memo_report(at).unwrap_or_else(|| self.proof(at)) // proof: held misfit
    }

    /// The token of the report of a limit of the memo, for a use at `at`. When `at` is in no
    /// written type, it reports the limit that a use passed. A written type holds that report
    /// until it is done.
    fn memo_report(&mut self, at: Span) -> Option<Reported> {
        if self.aliases.base.is_none() {
            self.report_too_large(at)
        } else {
            None
        }
    }

    /// Reports the written type at `at`, when a use passed a limit of the memo in it: once for
    /// the type, and once for the check, as every later substitution fails with no report.
    /// Gives the token of the report, when it makes one.
    fn report_too_large(&mut self, at: Span) -> Option<Reported> {
        let passed = self.aliases.too_large.take()?;
        let reported = self.error(DiagnosticKind::AliasTooLarge {
            ty: self.source.type_text(at).into_owned(),
            at,
            limit: match passed {
                Passed::Type => MAX_ENTRIES,
                Passed::Check => MAX_CHECK_ENTRIES,
            },
            check: passed == Passed::Check,
        });
        Some(reported)
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
        let known = |checker: &Self, ty: Option<Type>| ty.and_then(|ty| checker.normalize(ty));
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
        TypeKind::Class(_, args) | TypeKind::Enum(_, args) | TypeKind::Any(_, args) => {
            args.get().len()
        }
        _ => unreachable!("only an applied class, enum or `any` type has bounds to check"),
    }
}
