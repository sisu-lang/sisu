//! Interface declarations: their parameters, requirements and inheritance, the bounds that name
//! them, and the closure of the interfaces a bound inherits.

use std::collections::VecDeque;

use rustc_hash::{FxHashMap, FxHashSet};

use super::{BindingKind, Body, Checker, Declared, Definition};
use crate::ast;
use crate::diagnostic::{DiagnosticKind, NotInterface};
use crate::graph::components;
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{InterfaceId, Type, TypeArgs, TypeKind};

pub(super) struct InterfaceInfo {
    pub(super) name: ast::Ident,
    /// `Self`, then the interface's own: `Self` is `Param(Self, 0)`.
    pub(super) type_params: Vec<Symbol>,
    /// The interfaces it inherits, each with arguments that are a parameter of this interface or
    /// name none. An inheritance that fails to resolve is not in the list.
    pub(super) inherits: Vec<Bound>,
    /// The name of each requirement, in declaration order.
    pub(super) requirements: Vec<ast::Ident>,
}

/// A bound `T: I<A, ...>`, a conformance or an inheritance: the interface and its arguments,
/// without `Self`.
#[derive(Clone, Copy)]
pub(super) struct Bound {
    pub(super) interface: InterfaceId,
    pub(super) args: TypeArgs,
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
    }

    /// Resolves the bounds, inheritance and requirements of `interface`, whose id is `id`.
    fn interface_signature(&mut self, interface: &ast::Interface, id: InterfaceId) {
        let params = self.interfaces[id.0].type_params.clone();
        self.body.set_type_params(&params);
        self.body.self_type = Some(Type::intern(TypeKind::Param(params[0], 0)));
        self.bounds_of(&interface.type_params);
        let inherits = interface
            .inherits
            .iter()
            .filter_map(|ty| self.inherited(ty, interface))
            .collect();
        self.interfaces[id.0].inherits = inherits;
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
            if self.first_definition(
                &mut first,
                Definition::Method,
                Some(owner),
                &requirement.name,
            ) {
                self.interfaces[id.0]
                    .requirements
                    .push(requirement.name.clone());
            }
            self.own_type_params(&mut taken, &requirement.type_params);
            let own: Vec<Symbol> = (requirement.type_params.iter())
                .map(|param| param.name.name)
                .collect();
            self.body.push_type_params(&own);
            // A requirement has no body: its parameters are checked as a function's are.
            self.body.scopes.push(FxHashMap::default());
            for param in &requirement.params {
                let ty = self.resolve(&param.ty);
                self.declare(&param.name, ty, BindingKind::Param, None);
            }
            self.pop_scope();
            if let Some(ret) = &requirement.ret {
                self.resolve(ret);
            }
            self.bounds_of(&requirement.type_params);
            self.body.truncate_type_params(scope);
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

    /// Resolves the bounds of `params`, which are in scope, and reports each that names no
    /// interface. A later task keeps them.
    pub(super) fn bounds_of(&mut self, params: &[ast::TypeParam]) {
        for ty in params.iter().flat_map(|param| &param.bounds) {
            self.bound(ty);
        }
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
            let args = self.type_args(&ident, &declared, args, angles)?;
            return Some(Bound {
                interface: id,
                args,
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
                            .entry(requirement.name)
                            .or_insert((InterfaceId(inherited), requirement.span));
                    }
                }
            }
            let mut reports = Vec::new();
            for requirement in &self.interfaces[i].requirements {
                if let Some(&(owner, first)) = owners.get(&requirement.name) {
                    reports.push(DiagnosticKind::InheritedRequirement {
                        name: requirement.name.to_string(),
                        interface: interface.name.name.to_string(),
                        owner: self.interfaces[owner.0].name.name.to_string(),
                        at: requirement.span,
                        first,
                    });
                }
            }
            for report in reports {
                self.report(report);
            }
        }
    }

    /// Appends to `out` the interfaces of `bounds` and every interface they inherit, each
    /// `(interface, arguments)` once, where `receiver` is the type that meets the bounds. The
    /// arguments of an inherited interface are copied from the bound's, or from `receiver` for
    /// `Self`, or written without a parameter, so the closure never grows an argument.
    ///
    /// The third recorded exception to "A pass is linear in the program" (user, 2026-10-25): a
    /// bound's closure is computed per use, so a use costs the closure's size. The closure of
    /// arguments may double at each level of inheritance, as the spec says ("Inheritance"): an
    /// interface that inherits one twice, once with a parameter and once with a type, has the
    /// two as two entries. Task 4 reads the closure; the check for E0034 does not, as it needs
    /// no arguments.
    #[cfg_attr(not(test), expect(dead_code, reason = "Task 4 calls it for a bound"))]
    pub(super) fn interfaces_of(
        &self,
        receiver: Type,
        bounds: &[Bound],
        out: &mut Vec<(InterfaceId, TypeArgs)>,
    ) {
        let mut at = out.len();
        let mut seen: FxHashSet<(InterfaceId, TypeArgs)> = out.iter().copied().collect();
        for bound in bounds {
            if seen.insert((bound.interface, bound.args)) {
                out.push((bound.interface, bound.args));
            }
        }
        // The arguments of the interface being read, led by `Self`.
        let mut with = Vec::new();
        while let Some(&(interface, args)) = out.get(at) {
            at += 1;
            with.clear();
            with.push(receiver);
            with.extend_from_slice(args.get());
            for inherited in &self.interfaces[interface.0].inherits {
                let args = inherited.args.subst(&with);
                if seen.insert((inherited.interface, args)) {
                    out.push((inherited.interface, args));
                }
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
