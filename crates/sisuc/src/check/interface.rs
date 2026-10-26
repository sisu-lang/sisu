//! Interface declarations: their parameters, requirements and inheritance, the bounds that name
//! them, and the closure of the interfaces a bound inherits.

use std::collections::VecDeque;

use rustc_hash::{FxHashMap, FxHashSet};

use super::{Checker, Declared, Definition};
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
            self.interface_signature(interface, InterfaceId(i));
        }
        self.inheritance_cycles(&interfaces);
        self.inherited_requirements(&interfaces);
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
            let mut names = self.type_params(&interface.type_params, &requirement.type_params);
            names.insert(0, params[0]);
            self.body.set_type_params(&names);
            for param in &requirement.params {
                self.resolve(&param.ty);
            }
            if let Some(ret) = &requirement.ret {
                self.resolve(ret);
            }
            self.bounds_of(&requirement.type_params);
        }
        self.body.set_type_params(&[]);
        self.body.self_type = None;
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
        // Not an interface: `resolve` reports a name that is unknown or misused, and the rest is
        // a type that is no interface.
        let resolved = self.resolve(ty)?;
        let kind = match resolved.kind() {
            TypeKind::Class(..) => NotInterface::Class,
            TypeKind::Enum(..) => NotInterface::Enum,
            TypeKind::Param(..) => NotInterface::TypeParam,
            _ => NotInterface::BuiltIn,
        };
        self.report(DiagnosticKind::NotAnInterface {
            written: ty.to_string(),
            at: ty.span,
            kind,
        });
        None
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
    fn inheritance_cycles(&mut self, interfaces: &[&ast::Interface]) {
        let edges: Vec<Vec<usize>> = self
            .interfaces
            .iter()
            .map(|info| info.inherits.iter().map(|b| b.interface.0).collect())
            .collect();
        let component = components(&edges);
        let mut size = vec![0; edges.len()];
        for &k in &component {
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
            let through = cycle_through(&edges, &component, i)
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
    fn inherited_requirements(&mut self, interfaces: &[&ast::Interface]) {
        let mut closure = Vec::new();
        let mut owners: FxHashMap<Symbol, (InterfaceId, Span)> = FxHashMap::default();
        for (i, interface) in interfaces.iter().enumerate() {
            let id = InterfaceId(i);
            let info = &self.interfaces[i];
            closure.clear();
            let receiver = Type::intern(TypeKind::Param(info.type_params[0], 0));
            self.interfaces_of(receiver, &info.inherits, &mut closure);
            owners.clear();
            for &(inherited, _) in closure.iter().filter(|(inherited, _)| *inherited != id) {
                for requirement in &self.interfaces[inherited.0].requirements {
                    owners
                        .entry(requirement.name)
                        .or_insert((inherited, requirement.span));
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
    /// bound's closure is computed per use, so a use costs the closure's size.
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
