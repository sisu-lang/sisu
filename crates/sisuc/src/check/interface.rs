//! Interface declarations: their parameters, requirements and inheritance, the bounds that name
//! them, and the closure of the interfaces a bound inherits.

use std::collections::VecDeque;

use rustc_hash::{FxHashMap, FxHashSet};

use super::extend::holder_of;
use super::{BindingKind, Body, Checker, Declared, Definition};
use crate::ast;
use crate::conformance::find;
use crate::diagnostic::{AddBound, DiagnosticKind, NotConformForm, NotInterface, WideKind};
use crate::graph::components;
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{ClassId, EnumId, FuncId, InterfaceId, Type, TypeArgs, TypeKind};

/// The most interfaces that one bound, or one list of interfaces a type conforms to, may give
/// with the interfaces they inherit. An interface that inherits another twice, with different
/// arguments, doubles the number at each level of inheritance, so the walk stops here (E0157).
pub(super) const BOUND_CLOSURE_LIMIT: usize = 256;

pub(super) struct InterfaceInfo {
    pub(super) name: ast::Ident,
    /// `Self`, then the interface's own: `Self` is `Param(Self, 0)`.
    pub(super) type_params: Vec<Symbol>,
    /// The interfaces it inherits, each with arguments that are a parameter of this interface or
    /// name none. An inheritance that fails to resolve is not in the list.
    pub(super) inherits: Vec<Bound>,
    /// Where each of `inherits` is written.
    pub(super) inherits_at: Vec<Span>,
    /// The first requirement of each name, in declaration order.
    pub(super) requirements: Vec<RequirementInfo>,
    /// The bounds of each of `type_params`: `Self` conforms to the interface itself.
    pub(super) bounds: Vec<Vec<Bound>>,
}

/// A requirement's signature, over the interface's parameters (`Self` first) and then its own.
pub(super) struct RequirementInfo {
    pub(super) name: ast::Ident,
    /// `fn` through the return type.
    pub(super) span: Span,
    /// Its own type parameters.
    pub(super) own: Vec<Symbol>,
    /// The bounds of each of `own`.
    pub(super) bounds: Vec<Vec<Bound>>,
    /// Without `self`; `None` is `Error`.
    pub(super) params: Vec<Option<Type>>,
    /// `None` is `Error`; no return type is `unit`.
    pub(super) ret: Option<Type>,
}

/// A bound `T: I<A, ...>`, a conformance or an inheritance: the interface and its arguments,
/// without `Self`.
#[derive(Clone, Copy)]
pub(super) struct Bound {
    pub(super) interface: InterfaceId,
    pub(super) args: TypeArgs,
    /// Where the bound is written.
    pub(super) at: Span,
}

/// A requirement that a type parameter's bounds give: the interface that declares it, with its
/// arguments (without `Self`), and the requirement's place among the interface's.
#[derive(Clone, Copy)]
pub(super) struct Required {
    pub(super) interface: InterfaceId,
    pub(super) args: TypeArgs,
    pub(super) method: usize,
}

/// A bound check that waits for every conformance: the applied class or enum type `ty`, each of
/// whose arguments is written at the span of `ats`, in the scope `scope`.
pub(super) struct PendingBound {
    ty: Type,
    ats: Vec<Span>,
    scope: usize,
    /// Where the type that holds `ty` is kept, if it is kept: a type that fails the check is
    /// unknown there.
    place: Option<Slot>,
}

/// Where a resolved type is kept: the type of a parameter or a result of a signature, of a field,
/// of a payload of a variant, or of a requirement's parameter or result.
#[derive(Clone, Copy)]
pub(super) enum Slot {
    Param(FuncId, usize),
    Ret(FuncId),
    Field(ClassId, usize),
    Payload(EnumId, usize, usize),
    RequirementParam(InterfaceId, usize, usize),
    RequirementRet(InterfaceId, usize),
}

/// The type parameters in scope where a pending check was queued, and their bounds.
pub(super) struct Scope {
    params: Vec<Symbol>,
    bounds: Vec<Vec<Bound>>,
    self_type: Option<Type>,
}

impl Checker<'_> {
    /// Pass 1b, after every name is known: resolves each interface's bounds, inheritance and
    /// requirement signatures, then reports its inheritance cycles and the requirements that it
    /// declares again. `items` holds the interfaces in `InterfaceId` order.
    pub(super) fn interface_signatures(&mut self, items: &[&ast::Item]) {
        let interfaces: Vec<&ast::Interface> = items
            .iter()
            .filter_map(|item| match item {
                ast::Item::Interface(interface) => Some(interface),
                _ => None,
            })
            .collect();
        for (i, interface) in interfaces.iter().enumerate() {
            // A fresh body: the parameters of a requirement are bound in it, and so are dropped.
            self.with_body(Body::default(), |checker| {
                checker.interface_signature(interface, InterfaceId(i));
            });
        }
        let edges: Vec<Vec<usize>> = self
            .interfaces
            .iter()
            .map(|info| info.inherits.iter().map(|b| b.interface.0).collect())
            .collect();
        let component = components(&edges);
        self.inheritance_cycles(&interfaces, &edges, &component);
        self.inherited_requirements(&interfaces, &edges, &component);
        for (i, info) in self.interfaces.iter().enumerate() {
            for requirement in &info.requirements {
                (self.declaring.entry(requirement.name.name).or_default()).push(InterfaceId(i));
            }
        }
    }

    /// Resolves the bounds, inheritance and requirements of `interface`, whose id is `id`.
    fn interface_signature(&mut self, interface: &ast::Interface, id: InterfaceId) {
        let params = self.interfaces[id.0].type_params.clone();
        self.body.set_type_params(&params);
        self.body.self_type = Some(Type::intern(TypeKind::Param(params[0], 0)));
        // `Self` conforms to the interface itself, with its own parameters as the arguments.
        let own: Vec<Type> = (1..)
            .zip(&params[1..])
            .map(|(i, &name)| Type::intern(TypeKind::Param(name, i)))
            .collect();
        let this = Bound {
            interface: id,
            args: TypeArgs::intern(&own),
            at: interface.name.span,
        };
        self.interfaces[id.0].bounds =
            self.declare_bounds(0, vec![vec![this]], &interface.type_params);
        let (inherits, inherits_at) = interface
            .inherits
            .iter()
            .filter_map(|ty| Some((self.inherited(ty, interface)?, ty.span)))
            .unzip();
        self.interfaces[id.0].inherits = inherits;
        self.interfaces[id.0].inherits_at = inherits_at;
        // Task 7 takes associated types.
        for assoc in &interface.assoc {
            self.unsupported_at(assoc.name.span);
        }
        let mut first = FxHashMap::default();
        // The names of the interface's parameters, and then the requirement's own: the scope of
        // the interface is set up once, and a requirement adds and removes only its own.
        let mut taken = FxHashMap::default();
        for param in &interface.type_params {
            taken
                .entry(param.name.name)
                .or_insert((param.name.span, Definition::Type));
        }
        let scope = self.body.type_params.len();
        for requirement in &interface.methods {
            let owner = interface.name.name.as_str();
            let first_of_name = self.first_definition(
                &mut first,
                Definition::Method,
                Some(owner),
                &requirement.name,
            );
            self.own_type_params(&mut taken, &requirement.type_params);
            let own: Vec<Symbol> = (requirement.type_params.iter())
                .map(|param| param.name.name)
                .collect();
            self.body.push_type_params(&own);
            // The bounds first, so that a type in the signature meets them.
            let bounds = self.declare_bounds(scope, Vec::new(), &requirement.type_params);
            // A requirement has no body: its parameters are checked as a function's are.
            self.body.scopes.push(FxHashMap::default());
            let mut params = Vec::new();
            // The checks that each parameter's type queues, then the result's.
            let mut queued = Vec::new();
            for param in &requirement.params {
                let from = self.queued();
                let ty = self.resolve(&param.ty);
                queued.push(from..self.queued());
                params.push(ty);
                self.declare(&param.name, ty, BindingKind::Param, None);
            }
            self.pop_scope();
            let from = self.queued();
            let ret = requirement
                .ret
                .as_ref()
                .map_or(Some(Type::UNIT), |ret| self.resolve(ret));
            let ret_queued = from..self.queued();
            self.body.truncate_type_params(scope);
            if first_of_name {
                let r = self.interfaces[id.0].requirements.len();
                for (i, queued) in queued.into_iter().enumerate() {
                    self.keep_at(queued, Slot::RequirementParam(id, r, i));
                }
                self.keep_at(ret_queued, Slot::RequirementRet(id, r));
                self.interfaces[id.0].requirements.push(RequirementInfo {
                    name: requirement.name.clone(),
                    span: requirement.span,
                    own,
                    bounds,
                    params,
                    ret,
                });
            }
            for param in &requirement.type_params {
                if taken
                    .get(&param.name.name)
                    .is_some_and(|&(span, _)| span == param.name.span)
                {
                    taken.remove(&param.name.name);
                }
            }
        }
    }

    /// Resolves the bounds of `params`, which are in scope, and puts them in scope after `head`,
    /// the bounds of the parameters from the place `first` on that come before `params`. Gives
    /// all of them.
    ///
    /// A check of an applied type that a bound names waits for every conformance. It takes the
    /// scope when it is queued, which has no bound of `params` yet, so it takes the scope that has
    /// them all once they are in place.
    pub(super) fn declare_bounds(
        &mut self,
        first: usize,
        head: Vec<Vec<Bound>>,
        params: &[ast::TypeParam],
    ) -> Vec<Vec<Bound>> {
        let queued = self.queued();
        let mut bounds = head;
        bounds.extend(self.bounds_of(params));
        self.body.set_bounds(first, bounds.clone());
        if self.queued() > queued {
            let scope = self.scope();
            for check in self.pending_bounds.iter_mut().flatten().skip(queued) {
                check.scope = scope;
            }
        }
        bounds
    }

    /// Resolves the bounds of `params`, which are in scope, and reports each that names no
    /// interface. Gives the bounds of each parameter that name one.
    fn bounds_of(&mut self, params: &[ast::TypeParam]) -> Vec<Vec<Bound>> {
        (params.iter())
            .map(|param| {
                (param.bounds.iter())
                    .filter_map(|ty| self.bound(ty))
                    .collect()
            })
            .collect()
    }

    /// The interface `ty` names with its arguments. `None`, reported, when it names no
    /// interface, or its arguments do not fit; `None` without a report for a broken name.
    pub(super) fn bound(&mut self, ty: &ast::TypeExpr) -> Option<Bound> {
        let named = match &ty.kind {
            ast::TypeKind::Name(name) if ty.optional == 0 => Some((*name, &[][..], ty.span)),
            ast::TypeKind::Applied(applied) if ty.optional == 0 => {
                Some((applied.name, &applied.args[..], applied.angles))
            }
            _ => None,
        };
        if let Some((name, args, angles)) = named
            && self.body.type_param(name).is_none()
            && let Some(&found) = self.interface_ids.get(&name)
        {
            let Declared::Known(id) = found else {
                return None;
            };
            let ident = ast::Ident {
                name,
                span: Span::new(ty.span.start(), ty.span.start() + name.as_str().len()),
            };
            let declared = self.interfaces[id.0].type_params[1..].to_vec();
            // An interface is copied by no one, so its arguments record no instantiation.
            let args = self.type_args_of(&ident, &declared, args, angles, false)?;
            return Some(Bound {
                interface: id,
                args,
                at: ty.bare,
            });
        }
        // An optional or an array of an interface is a type that is no interface. `resolve` would
        // say that the interface is no type, and advise a bound, which this already is.
        let shape = match &ty.kind {
            _ if ty.optional > 0 => Some(NotInterface::Optional),
            ast::TypeKind::Array(_) => Some(NotInterface::Array),
            _ => None,
        };
        if let Some(kind) = shape
            && self.names_interface(ty)
        {
            self.not_an_interface(ty, kind);
            return None;
        }
        // Not an interface: `resolve` reports a name that is unknown or misused, and the rest is
        // a type that is no interface.
        let resolved = self.resolve(ty)?;
        let kind = match resolved.kind() {
            TypeKind::Class(..) => NotInterface::Class,
            TypeKind::Enum(..) => NotInterface::Enum,
            TypeKind::Param(..) => NotInterface::TypeParam,
            TypeKind::Optional(..) => NotInterface::Optional,
            TypeKind::Array(..) => NotInterface::Array,
            _ => NotInterface::BuiltIn,
        };
        self.not_an_interface(ty, kind);
        None
    }

    /// Reports the written type `ty`, which is a `kind` and no interface, at its name.
    fn not_an_interface(&mut self, ty: &ast::TypeExpr, kind: NotInterface) {
        self.report(DiagnosticKind::NotAnInterface {
            written: ty.to_string(),
            at: ty.bare,
            kind,
        });
    }

    /// Whether `ty` is the name of a declared interface, with no `?` and no array around it.
    pub(super) fn is_interface_name(&self, ty: &ast::TypeExpr) -> bool {
        ty.optional == 0
            && matches!(ty.kind, ast::TypeKind::Name(_) | ast::TypeKind::Applied(_))
            && self.names_interface(ty)
    }

    /// Whether the element of `ty`, past its arrays and its `?`, names a declared interface.
    fn names_interface(&self, ty: &ast::TypeExpr) -> bool {
        let name = match &ty.kind {
            ast::TypeKind::Array(element) => return self.names_interface(element),
            ast::TypeKind::Name(name) => *name,
            ast::TypeKind::Applied(applied) => applied.name,
            _ => return false,
        };
        self.body.type_param(name).is_none()
            && matches!(self.interface_ids.get(&name), Some(Declared::Known(_)))
    }

    /// The interface that `ty`, in the inheritance list of `interface`, names. An argument that is
    /// neither a parameter of `interface` nor free of parameters is reported, and so the
    /// inheritance is dropped: the closure of a bound copies arguments and never builds one.
    fn inherited(&mut self, ty: &ast::TypeExpr, interface: &ast::Interface) -> Option<Bound> {
        let bound = self.bound(ty)?;
        let ast::TypeKind::Applied(applied) = &ty.kind else {
            return Some(bound);
        };
        let mut fits = true;
        for (written, &arg) in applied.args.iter().zip(bound.args.get()) {
            if !matches!(arg.kind(), TypeKind::Param(..)) && arg.has_param() {
                self.report(DiagnosticKind::InheritedArgument {
                    arg: written.to_string(),
                    interface: interface.name.name.to_string(),
                    at: written.span,
                });
                fits = false;
            }
        }
        fits.then_some(bound)
    }

    /// Reports each cycle of inheritance once, on the first interface of its component in source
    /// order, at that interface's inheritance list.
    fn inheritance_cycles(
        &mut self,
        interfaces: &[&ast::Interface],
        edges: &[Vec<usize>],
        component: &[usize],
    ) {
        let mut size = vec![0; edges.len()];
        for &k in component {
            size[k] += 1;
        }
        let mut reported = vec![false; edges.len()];
        for (i, interface) in interfaces.iter().enumerate() {
            let cyclic = size[component[i]] > 1 || edges[i].contains(&i);
            if !cyclic || std::mem::replace(&mut reported[component[i]], true) {
                continue;
            }
            // An interface on a cycle inherits at least the next one on it.
            let (first, last) = (
                &interface.inherits[0],
                &interface.inherits[interface.inherits.len() - 1],
            );
            let through = cycle_through(edges, component, i)
                .into_iter()
                .map(|at| self.interfaces[at].name.name.to_string())
                .collect();
            self.report(DiagnosticKind::InheritanceCycle {
                name: interface.name.name.to_string(),
                at: first.span.to(last.span),
                through,
            });
        }
    }

    /// Reports each requirement that an interface declares and an interface it inherits has too.
    /// An interface on a cycle does not count the others on it: E0150 has reported them.
    ///
    /// Part of the third recorded exception to "A pass is linear in the program" (user,
    /// 2026-10-25; ruling F9 (a)): each interface reads the requirements of every interface it
    /// inherits, N interfaces by n inherited, which is the size of the report it may give. The
    /// walk follows `InterfaceId`s only, because a requirement's name does not depend on the
    /// arguments of the inheritance. To cap it would redesign the lookup, and cost a later change.
    fn inherited_requirements(
        &mut self,
        interfaces: &[&ast::Interface],
        edges: &[Vec<usize>],
        component: &[usize],
    ) {
        let mut seen = FxHashSet::default();
        let mut queue = Vec::new();
        let mut owners: FxHashMap<Symbol, (InterfaceId, Span)> = FxHashMap::default();
        for (i, interface) in interfaces.iter().enumerate() {
            seen.clear();
            seen.insert(i);
            queue.clear();
            queue.push(i);
            owners.clear();
            // Nearest first, so that the owner named is the nearest that has the requirement.
            let mut at = 0;
            while let Some(&n) = queue.get(at) {
                at += 1;
                for &inherited in &edges[n] {
                    if !seen.insert(inherited) {
                        continue;
                    }
                    #[cfg(test)]
                    super::INHERIT_VISITS.set(super::INHERIT_VISITS.get() + 1);
                    queue.push(inherited);
                    if component[inherited] == component[i] {
                        continue;
                    }
                    for requirement in &self.interfaces[inherited].requirements {
                        owners
                            .entry(requirement.name.name)
                            .or_insert((InterfaceId(inherited), requirement.name.span));
                    }
                }
            }
            let mut reports = Vec::new();
            for requirement in &self.interfaces[i].requirements {
                if let Some(&(owner, first)) = owners.get(&requirement.name.name) {
                    reports.push(DiagnosticKind::InheritedRequirement {
                        name: requirement.name.name.to_string(),
                        interface: interface.name.name.to_string(),
                        owner: self.interfaces[owner.0].name.name.to_string(),
                        at: requirement.name.span,
                        first,
                    });
                }
            }
            for report in reports {
                self.report(report);
            }
        }
    }

    /// Whether the method `func` meets `requirement`: after `Self`, the interface's arguments
    /// and the method's own type parameters stand in for the requirement's (`base` holds the
    /// first two), it has the same parameter types and return type, and the same number of type
    /// parameters of its own, with the same bounds. A type that is `Error` matches any.
    pub(super) fn meets(&self, requirement: &RequirementInfo, func: FuncId, base: &[Type]) -> bool {
        let sig = &self.signatures[func.0];
        if requirement.own.len() != sig.own || requirement.params.len() != sig.params.len() {
            return false;
        }
        let owner = sig.type_params.len() - sig.own;
        let mut with = base.to_vec();
        for (i, &name) in (owner..).zip(&sig.type_params[owner..]) {
            let i = u32::try_from(i).expect("a declaration should have fewer than 2^32 parameters");
            with.push(Type::intern(TypeKind::Param(name, i)));
        }
        let differs = |want: Option<Type>, found: Option<Type>| matches!((want, found), (Some(want), Some(found)) if want.subst(&with) != found);
        if (requirement.params.iter().zip(&sig.params)).any(|(&want, &found)| differs(want, found))
            || differs(requirement.ret, sig.ret)
        {
            return false;
        }
        (requirement.bounds.iter().zip(&sig.bounds)).all(|(want, found)| {
            let want: Vec<_> = (want.iter())
                .map(|b| (b.interface, b.args.subst(&with)))
                .collect();
            let found: Vec<_> = found.iter().map(|b| (b.interface, b.args)).collect();
            want.iter().all(|b| found.contains(b)) && found.iter().all(|b| want.contains(b))
        })
    }

    /// Appends to `out` the interfaces of `bounds` and every interface they inherit, each
    /// `(interface, arguments)` once, where `receiver` is the type that meets the bounds. The
    /// arguments of an inherited interface are copied from the bound's, or from `receiver` for
    /// `Self`, or written without a parameter, so the closure never grows an argument. `seen` is
    /// a buffer for the walk, which the caller keeps from one closure to the next.
    ///
    /// The third recorded exception to "A pass is linear in the program" (user, 2026-10-25): a
    /// bound's closure is computed where it is declared and per body that has it, so a body costs
    /// the closure's size. The closure of arguments may double at each level of inheritance: an
    /// interface that inherits one twice, once with a parameter and once with a type, has the two
    /// as two entries. So the walk of each bound stops at `BOUND_CLOSURE_LIMIT` entries, and the
    /// caller reports E0157 at the bound.
    /// The check for E0034 does not read the closure, as it needs no arguments.
    ///
    /// `Err` holds the place in `bounds` of the bound whose interfaces went past the limit.
    pub(super) fn interfaces_of(
        &self,
        receiver: Type,
        bounds: &[Bound],
        out: &mut Vec<(InterfaceId, TypeArgs)>,
        seen: &mut FxHashSet<(InterfaceId, TypeArgs)>,
    ) -> Result<(), usize> {
        seen.clear();
        seen.extend(out.iter().copied());
        #[cfg(test)]
        super::CLOSURE_STEPS.set(super::CLOSURE_STEPS.get() + seen.len());
        self.extend_closure(receiver, bounds, out, seen, |_, _| {})
    }

    /// The interfaces that the type parameter at `place` in scope conforms to through its bounds,
    /// each with its arguments once, found when first asked for in a body.
    ///
    /// The third recorded exception to "A pass is linear in the program" (user, 2026-10-25; ruling
    /// F9 (b)): the closure is computed once for each body that has the bound, so a body costs
    /// the closure's size.
    pub(super) fn param_interfaces(&mut self, place: usize) -> &[(InterfaceId, TypeArgs)] {
        let body = &mut self.body;
        if body.conforming.len() <= place {
            body.conforming.resize(place + 1, None);
        }
        if self.body.conforming[place].is_none() {
            let mut out = Vec::new();
            let name = self.body.type_params[place];
            let receiver = Type::intern(TypeKind::Param(
                name,
                u32::try_from(place).expect("a declaration should have fewer than 2^32 parameters"),
            ));
            let bounds = self.body.bounds.get(place).map_or(&[][..], Vec::as_slice);
            let mut seen = std::mem::take(&mut self.closure_seen);
            let wide = self.interfaces_of(receiver, bounds, &mut out, &mut seen);
            self.closure_seen = seen;
            if let Err(k) = wide {
                self.too_wide(place, k);
            }
            self.body.conforming[place] = Some(out);
        }
        self.body.conforming[place]
            .as_deref()
            .expect("the closure of a type parameter should be found by now")
    }

    /// Reports the bound at place `k` among those of the type parameter at `place`, whose closure
    /// went past the limit: once for the bound, whatever bodies read it. The parameter is then
    /// unknown in this body: it conforms to every interface, and a method that its walk did not
    /// find draws no report.
    fn too_wide(&mut self, place: usize, k: usize) {
        self.body.wide.push(place);
        let bound = self.body.bounds[place][k];
        if self.wide_reported.insert(bound.at) {
            let text = self.source.file.text(bound.at);
            self.report(DiagnosticKind::BoundTooWide {
                kind: WideKind::Bound,
                written: format!("{}: {text}", self.body.type_params[place]),
                limit: BOUND_CLOSURE_LIMIT,
                at: bound.at,
            });
        }
    }

    /// Reports that the interface written at `at` in the list of `ty`, or in an `extend` block of
    /// it, gives more than `BOUND_CLOSURE_LIMIT` interfaces with those it inherits. The conformance
    /// is then unknown: `ty` has no report for what the walk did reach, as for a broken block.
    pub(super) fn too_wide_at(&mut self, ty: Type, at: Span) {
        self.report(DiagnosticKind::BoundTooWide {
            kind: WideKind::Conformance,
            written: format!("{}: {}", self.type_name(ty), self.source.file.text(at)),
            limit: BOUND_CLOSURE_LIMIT,
            at,
        });
        let holder = holder_of(ty).expect("a conformance's target should be a class or an enum");
        self.broken_extensions.insert(self.holder_name(holder));
    }

    /// The requirements named `name` that the bounds of the type parameter at `place` give,
    /// each once, in the order of the bounds. The first ask for a place in a body indexes by name
    /// the requirements of its interfaces, and the rest read the index: a call costs a lookup,
    /// not a walk of the bounds.
    ///
    /// The third recorded exception to "A pass is linear in the program" (user, 2026-10-25; ruling
    /// F9 (e)): the candidates of a name are `n` for `n` interfaces of the bounds that declare it,
    /// and a call keeps them while it checks its arguments.
    pub(super) fn required(&mut self, place: usize, name: Symbol) -> Vec<Required> {
        if !self.body.required.contains_key(&place) {
            let entries = self.param_interfaces(place).to_vec();
            let mut by_name: FxHashMap<Symbol, Vec<Required>> = FxHashMap::default();
            for (interface, args) in entries {
                let requirements = &self.interfaces[interface.0].requirements;
                for (method, requirement) in requirements.iter().enumerate() {
                    #[cfg(test)]
                    super::CLOSURE_STEPS.set(super::CLOSURE_STEPS.get() + 1);
                    (by_name.entry(requirement.name.name).or_default()).push(Required {
                        interface,
                        args,
                        method,
                    });
                }
            }
            self.body.required.insert(place, by_name);
        }
        (self.body.required[&place].get(&name))
            .cloned()
            .unwrap_or_default()
    }

    /// Whether `ty` conforms to `interface` with `args` (without `Self`): a type parameter
    /// through its bounds and what they inherit, a class or an enum through its conformance,
    /// when what the conformance's parameters stand for meets their bounds. A broken `extend`
    /// block may have given its target the conformance, so that holds.
    ///
    /// An answer is kept by the type, the interface and its arguments, so a bound that several
    /// conformances share costs one answer for each type, not one for each way to it: with
    /// `extend<T: A & B> Box<T>: A` and `: B`, `Box^n<Leaf>: A` costs steps linear in `n`. The
    /// answer for a type that holds a type parameter changes with the bounds, so it is kept with
    /// the body (`Body.conformed`).
    pub(super) fn conforms(&mut self, ty: Type, interface: InterfaceId, args: TypeArgs) -> bool {
        match ty.kind() {
            TypeKind::Param(_, place) => {
                let place = place as usize;
                self.param_interfaces(place).contains(&(interface, args))
                    || self.body.wide.contains(&place)
            }
            TypeKind::Class(..) | TypeKind::Enum(..) => {
                let broken = holder_of(ty).is_some_and(|holder| {
                    self.broken_extensions.contains(&self.holder_name(holder))
                });
                if broken {
                    return true;
                }
                let key = (ty, interface, args);
                let kept = if ty.has_param() {
                    self.body.conformed.get(&key)
                } else {
                    self.conformed.get(&key)
                };
                if let Some(&answer) = kept {
                    return answer;
                }
                let answer = self.conforms_by_conformance(ty, interface, args);
                if ty.has_param() {
                    self.body.conformed.insert(key, answer);
                } else {
                    self.conformed.insert(key, answer);
                }
                answer
            }
            _ => false,
        }
    }

    /// `conforms` for the class or enum type `ty`, asking for the conformance.
    fn conforms_by_conformance(
        &mut self,
        ty: Type,
        interface: InterfaceId,
        args: TypeArgs,
    ) -> bool {
        #[cfg(test)]
        super::CONFORMS_STEPS.set(super::CONFORMS_STEPS.get() + 1);
        let Some((k, stands)) = find(&self.conformances, interface, ty, args) else {
            return false;
        };
        let Some((_, bounds)) = self.conformance_bounds.get(&(interface, k)) else {
            return true;
        };
        let bounds = bounds.clone();
        for (i, bounds) in bounds.iter().enumerate() {
            for b in bounds {
                if !self.conforms(stands.get()[i], b.interface, b.args.subst(stands.get())) {
                    return false;
                }
            }
        }
        true
    }

    /// `interface` with `args`, as a program writes a bound.
    pub(super) fn bound_text(&self, interface: InterfaceId, args: TypeArgs) -> String {
        let name = self.interfaces[interface.0].name.name.as_str();
        let args: Vec<String> = args.get().iter().map(|&ty| self.type_name(ty)).collect();
        if args.is_empty() {
            name.to_string()
        } else {
            format!("{name}<{}>", args.join(", "))
        }
    }

    /// Checks that `ty` meets `bound`, whose arguments name the parameters that `with` gives
    /// types to. Reports it at `at` when not.
    pub(super) fn check_bound(&mut self, at: Span, ty: Type, bound: Bound, with: &[Type]) -> bool {
        let args = bound.args.subst(with);
        if self.conforms(ty, bound.interface, args) {
            return true;
        }
        let add = self.add_bound(ty, bound.interface);
        // A class or an enum may conform, when the types that stand for its parameters do.
        let needs = find(&self.conformances, bound.interface, ty, args)
            .map(|(k, stands)| self.failed_bound(bound.interface, k, stands));
        self.report(DiagnosticKind::NotConform {
            ty: self.type_name(ty),
            interface: self.bound_text(bound.interface, args),
            at,
            form: NotConformForm::Bound {
                // No label points into the prelude.
                bound: (!bound.at.in_prelude()).then_some(bound.at),
                add,
                needs,
            },
        });
        false
    }

    /// The bound that would make the type parameter `ty` conform to `interface`, written with the
    /// bounds it has: `None` unless `ty` is a type parameter in scope and `interface` takes no
    /// type parameters.
    pub(super) fn add_bound(&self, ty: Type, interface: InterfaceId) -> Option<AddBound> {
        let TypeKind::Param(name, place) = ty.kind() else {
            return None;
        };
        if self.body.type_param(name) != Some(place as usize)
            || self.interfaces[interface.0].type_params.len() != 1
        {
            return None;
        }
        let have = self.body.bounds.get(place as usize).into_iter().flatten();
        let mut bounds: Vec<String> = have.map(|b| self.bound_text(b.interface, b.args)).collect();
        bounds.push(self.bound_text(interface, TypeArgs::NONE));
        Some(AddBound {
            param: name.to_string(),
            bounds,
        })
    }

    /// Checks that each argument of the applied type `ty`, written at `ats`, meets the bounds
    /// that its class or enum declares. When every conformance is not yet known, the check waits.
    /// Whether it holds, or waits.
    pub(super) fn check_applied(&mut self, ty: Type, ats: Vec<Span>) -> bool {
        let scope = self.pending_bounds.is_some().then(|| self.scope());
        if let (Some(scope), Some(pending)) = (scope, &mut self.pending_bounds) {
            pending.push(PendingBound {
                ty,
                ats,
                scope,
                place: None,
            });
            return true;
        }
        self.check_applied_now(ty, &ats)
    }

    fn check_applied_now(&mut self, ty: Type, ats: &[Span]) -> bool {
        let (bounds, args) = match ty.kind() {
            TypeKind::Class(id, args) => (self.classes[id.0].bounds.clone(), args),
            TypeKind::Enum(id, args) => (self.enums[id.0].bounds.clone(), args),
            _ => unreachable!("only an applied class or enum should wait for its bounds"),
        };
        let mut holds = true;
        for (k, bounds) in bounds.iter().enumerate() {
            for &bound in bounds {
                holds &= self.check_bound(ats[k], args.get()[k], bound, args.get());
            }
        }
        holds
    }

    /// The place among `scopes` of the type parameters now in scope, with their bounds: made on
    /// the first ask, until they change.
    fn scope(&mut self) -> usize {
        if let Some(at) = self.body.scope {
            return at;
        }
        let at = self.scopes.len();
        self.scopes.push(Scope {
            params: self.body.type_params.clone(),
            bounds: self.body.bounds.clone(),
            self_type: self.body.self_type,
        });
        self.body.scope = Some(at);
        at
    }

    /// How many bound checks wait for every conformance.
    pub(super) fn queued(&self) -> usize {
        self.pending_bounds.as_ref().map_or(0, Vec::len)
    }

    /// Says that the checks queued in `queued` are of a type kept at `place`.
    pub(super) fn keep_at(&mut self, queued: std::ops::Range<usize>, place: Slot) {
        let waiting = self.pending_bounds.iter_mut();
        for check in waiting.flat_map(|pending| &mut pending[queued.clone()]) {
            check.place = Some(place);
        }
    }

    /// Runs the bound checks that waited for every conformance, in the order they were queued,
    /// each in the scope it was queued in, and makes unknown the type kept at the place of each
    /// that fails. Later types are checked as they are resolved.
    ///
    /// The checks of one scope stand together in the queue, as a scope is made again when it
    /// changes, so each scope is set up once for all its checks.
    pub(super) fn run_pending_bounds(&mut self) {
        let pending = self.pending_bounds.take().unwrap_or_default();
        let scopes = std::mem::take(&mut self.scopes);
        for group in pending.chunk_by(|a, b| a.scope == b.scope) {
            let scope = &scopes[group[0].scope];
            let (failed, _) = self.with_body(Body::default(), |checker| {
                checker.body.set_type_params(&scope.params);
                checker.body.set_bounds(0, scope.bounds.clone());
                checker.body.self_type = scope.self_type;
                let mut failed = Vec::new();
                for p in group {
                    if !checker.check_applied_now(p.ty, &p.ats) {
                        failed.extend(p.place);
                    }
                }
                failed
            });
            for place in failed {
                self.make_unknown(place);
            }
        }
    }

    /// Makes the type kept at `place` unknown, as a type that fails its bound is where it is
    /// written.
    fn make_unknown(&mut self, place: Slot) {
        match place {
            Slot::Param(f, i) => self.signatures[f.0].params[i] = None,
            Slot::Ret(f) => self.signatures[f.0].ret = None,
            Slot::Field(c, i) => self.classes[c.0].fields[i].ty = None,
            Slot::Payload(e, v, i) => self.enums[e.0].variants[v].fields[i] = None,
            Slot::RequirementParam(interface, r, i) => {
                self.interfaces[interface.0].requirements[r].params[i] = None;
            }
            Slot::RequirementRet(interface, r) => {
                self.interfaces[interface.0].requirements[r].ret = None;
            }
        }
    }

    /// The interfaces that `interface` inherits, each with where it writes the inheritance. The
    /// closures read an inheritance list through here, so that a test counts the steps.
    fn inheritances(&self, interface: InterfaceId) -> impl Iterator<Item = (&Bound, Span)> {
        let info = &self.interfaces[interface.0];
        (info.inherits.iter().zip(info.inherits_at.iter().copied())).inspect(|_| {
            #[cfg(test)]
            super::CLOSURE_STEPS.set(super::CLOSURE_STEPS.get() + 1);
        })
    }

    /// `interfaces_of`, where `seen` holds the entries of `out` already, so that a caller who adds
    /// the closures of several bounds one after the other builds the set once. For each interface
    /// that it adds because another inherits it, `reached` gets the place of that other in `out`
    /// and where it writes the inheritance. The bounds themselves are not reported to it.
    ///
    /// The limit holds for each of `bounds` on its own: the closure of one is walked alone and then
    /// merged, so the work follows the sum of the closures, which are at most the limit each, and
    /// a bound is not reported for the interfaces of the bounds before it. The entries of `out`
    /// must be closed: the interfaces that an entry of `seen` inherits are in `seen` too.
    pub(super) fn extend_closure(
        &self,
        receiver: Type,
        bounds: &[Bound],
        out: &mut Vec<(InterfaceId, TypeArgs)>,
        seen: &mut FxHashSet<(InterfaceId, TypeArgs)>,
        mut reached: impl FnMut(usize, Span),
    ) -> Result<(), usize> {
        // The closure of the bound being read, then for each entry of it after the first the place
        // of the entry that inherits it and where that writes the inheritance, and for each entry
        // the place in `out` that it has once merged, if it is new.
        let mut own: Vec<(InterfaceId, TypeArgs)> = Vec::new();
        let mut inherited_by: Vec<(usize, Span)> = Vec::new();
        let mut alone = FxHashSet::default();
        let mut placed: Vec<Option<usize>> = Vec::new();
        // The arguments of the interface being read, led by `Self`.
        let mut with = Vec::new();
        for (k, bound) in bounds.iter().enumerate() {
            #[cfg(test)]
            super::CLOSURE_STEPS.set(super::CLOSURE_STEPS.get() + 1);
            own.clear();
            inherited_by.clear();
            alone.clear();
            own.push((bound.interface, bound.args));
            alone.insert((bound.interface, bound.args));
            for parent in 0.. {
                let Some(&(interface, args)) = own.get(parent) else {
                    break;
                };
                with.clear();
                with.push(receiver);
                with.extend_from_slice(args.get());
                for (inherited, written) in self.inheritances(interface) {
                    let args = inherited.args.subst(&with);
                    if alone.insert((inherited.interface, args)) {
                        own.push((inherited.interface, args));
                        inherited_by.push((parent, written));
                        if own.len() > BOUND_CLOSURE_LIMIT {
                            return Err(k);
                        }
                    }
                }
            }
            placed.clear();
            for (i, &entry) in own.iter().enumerate() {
                if !seen.insert(entry) {
                    placed.push(None);
                    continue;
                }
                if let Some(&(parent, written)) = i.checked_sub(1).map(|i| &inherited_by[i]) {
                    let parent = placed[parent].expect(
                        "the interface that inherits a new one should be new: the closure of an old one is in `out`",
                    );
                    reached(parent, written);
                }
                placed.push(Some(out.len()));
                out.push(entry);
            }
        }
        Ok(())
    }

    /// Pass 2c, after every signature: reads the interfaces of each bound where it is declared,
    /// so that a bound that gives more than the limit is reported (E0157) whether a body asks for
    /// its interfaces or not. A body that has the bound reads them again, and the bound is
    /// reported once.
    ///
    /// The third recorded exception to "A pass is linear in the program" (user, 2026-10-25; ruling
    /// F9 (b)): a closure is at most the limit, and each bound of a declaration is read once more.
    pub(super) fn read_declared_bounds(&mut self) {
        for f in 0..self.signatures.len() {
            let sig = &self.signatures[f];
            if sig.bounds.iter().all(Vec::is_empty) {
                continue;
            }
            let owner = sig.type_params.len() - sig.own;
            let mut bounds = vec![Vec::new(); owner];
            bounds.extend(sig.bounds.iter().cloned());
            let params = sig.type_params.clone();
            self.read_bounds(&params, bounds, owner);
        }
        for c in 0..self.classes.len() {
            let info = &self.classes[c];
            if info.bounded {
                let (params, bounds) = (info.type_params.clone(), info.bounds.clone());
                self.read_bounds(&params, bounds, 0);
            }
        }
        for e in 0..self.enums.len() {
            let info = &self.enums[e];
            if info.bounded {
                let (params, bounds) = (info.type_params.clone(), info.bounds.clone());
                self.read_bounds(&params, bounds, 0);
            }
        }
        for e in 0..self.extensions.len() {
            let ext = &self.extensions[e];
            if ext.bounds.iter().any(|bounds| !bounds.is_empty()) {
                let (params, bounds) = (ext.type_params.clone(), ext.bounds.clone());
                self.read_bounds(&params, bounds, 0);
            }
        }
        for i in 0..self.interfaces.len() {
            let info = &self.interfaces[i];
            let (params, bounds) = (info.type_params.clone(), info.bounds.clone());
            let requirements: Vec<_> = (info.requirements.iter())
                .filter(|r| r.bounds.iter().any(|bounds| !bounds.is_empty()))
                .map(|r| (r.own.clone(), r.bounds.clone()))
                .collect();
            let bounded = bounds.iter().skip(1).any(|bounds| !bounds.is_empty());
            if !bounded && requirements.is_empty() {
                continue;
            }
            self.with_body(Body::default(), |checker| {
                checker.body.set_type_params(&params);
                checker.body.set_bounds(0, bounds);
                // `Self` is first, and conforms to the interface itself.
                checker.read_places(1);
                // The scope of the interface is set up once, and a requirement adds its own.
                for (own, bounds) in requirements {
                    checker.body.push_type_params(&own);
                    checker.body.set_bounds(params.len(), bounds);
                    checker.read_places(params.len());
                    checker.body.truncate_type_params(params.len());
                }
            });
        }
    }

    /// Reads the bounds of the type parameters `params` from the place `from` on, which have the
    /// `bounds` by place.
    fn read_bounds(&mut self, params: &[Symbol], bounds: Vec<Vec<Bound>>, from: usize) {
        self.with_body(Body::default(), |checker| {
            checker.body.set_type_params(params);
            checker.body.set_bounds(0, bounds);
            checker.read_places(from);
        });
    }

    /// Finds the interfaces of each type parameter in scope from the place `from` on that has a
    /// bound.
    fn read_places(&mut self, from: usize) {
        for place in from..self.body.bounds.len() {
            if !self.body.bounds[place].is_empty() {
                self.param_interfaces(place);
            }
        }
    }
}

/// The interfaces on a shortest cycle through `start`, in order, without `start`; empty for an
/// interface that inherits itself. `start` lies on a cycle of its component in `edges`.
fn cycle_through(edges: &[Vec<usize>], component: &[usize], start: usize) -> Vec<usize> {
    let mut parent: FxHashMap<usize, usize> = FxHashMap::default();
    let mut queue = VecDeque::from([start]);
    // `start` is reached again through the cycle, so it is not seen at first.
    while let Some(n) = queue.pop_front() {
        for &m in &edges[n] {
            if component[m] == component[start] && !parent.contains_key(&m) {
                parent.insert(m, n);
                queue.push_back(m);
            }
        }
        if parent.contains_key(&start) {
            break;
        }
    }
    let mut path = Vec::new();
    let mut at = parent[&start];
    while at != start {
        path.push(at);
        at = parent[&at];
    }
    path.reverse();
    path
}
