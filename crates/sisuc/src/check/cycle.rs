//! Instantiation cycles: a generic declaration that instantiates itself, directly or through
//! others, with a type argument that grows, which `mono` would instantiate without end.
//!
//! The graph's nodes are the type parameters of every declaration; a method is a declaration of
//! its own, its owner's parameters first. An instantiation inside a declaration that gives the
//! argument `A` to the parameter `Q` adds an edge `P → Q` for each parameter `P` of the
//! declaration that `A` mentions, growing when `A` is not `P` itself. The edges come from the
//! calls in function bodies, and from the classes and enums that field and payload types name.
//! A class or enum named in a body's locals or expressions is instantiated too, but its edges
//! run into a class's or an enum's parameters, whose own edges never lead back into a function's,
//! so they close no cycle and are left out; nor does a class's or an enum's parameter lead into
//! its methods', which `mono` instantiates only when a call reaches them.
//!
//! Each edge has a weight: the depth at which the argument nests the parameter (0 for the
//! parameter itself, 1 for `[P]`), so that a cycle whose weights sum above zero is a type argument
//! that grows with each turn. An edge into a type's node from one of its parts weighs 1, and the
//! edge from that node into a parameter weighs 0, so a longest path from a parameter to a
//! parameter adds up the greatest depth.
//!
//! A call of a requirement with type parameters of its own reaches every method that meets it,
//! so each such parameter is a hub: an argument of the call is an argument of the hub, and the
//! hub passes it, at no cost, to the same parameter of each method of that requirement.
//!
//! A call through a bound reaches every conformance of the interface, so each interface has a
//! hub too. The receiver of the call, which nests the caller's parameter `P` at depth `d`, is an
//! edge `P → H` of weight `d`; the hub passes it to each parameter `Q` of each method of each
//! conformance, at weight `-k`, where `k` is the greatest depth of `Q` in the conformance's
//! target, since the receiver is that target with the types that stand for its parameters. So
//! the edges grow with the program, and a path through the hub stands for an edge from `P` to
//! `Q` for each call and conformance, which a pair of edges for each would make a product.
//!
//! The weights of a hub's edges are negative, and no known algorithm finds a cycle of positive
//! weight in a graph with negative weights in linear time. This is an exception to
//! `CODING_STANDARDS.md`'s "A pass is linear in the program", agreed by the user on 2026-10-25:
//! the search is Bellman–Ford on each strongly connected component with a negative weight,
//! which takes at most the nodes times the edges of the component, and its passes stop at the
//! first that changes nothing. The nodes and the edges are linear in the program, but with hubs
//! a component can hold many functions. A component with no negative weight needs no search:
//! any cycle through a positive edge is a cycle of positive weight. `check/tests` pins the cost
//! of the worst input.

use std::collections::VecDeque;

use rustc_hash::{FxHashMap, FxHashSet};

use super::Checker;
use crate::ast::angled;
use crate::diagnostic::{DiagnosticKind, Hop};
use crate::graph::components;
use crate::source::Span;
use crate::tir::{self, Type, TypeArgs, TypeKind};
use crate::types::{write_args, write_method};

#[cfg(test)]
thread_local! {
    /// How many edges the last instantiation graph on this thread held.
    pub(super) static EDGES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many edges Bellman–Ford has relaxed on this thread.
    pub(super) static BELLMAN_FORD_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// An instantiation, which a diagnostic writes as the program does.
#[derive(Clone, Copy)]
enum Instance {
    /// A call of the function, with its type arguments, a method's owner's first.
    Call(tir::FuncId, TypeArgs),
    /// A generic class or enum, as a type.
    Type(Type),
    /// A call of a requirement of an interface, by the requirement's place, with its own type
    /// arguments.
    Required(tir::InterfaceId, usize, TypeArgs),
}

/// An edge of the graph, from an instantiation at `site` inside the declaration `holder`; or,
/// with no `instance`, into a type's node from the node of one of its parts.
struct Edge {
    from: usize,
    to: usize,
    /// The depth at which the part of an argument that the edge starts at nests the parameter,
    /// added over a path: 1 into a type's node from a part, 0 from the node to a parameter, and
    /// minus the depth of a conformance's parameter out of an interface's hub.
    weight: i64,
    /// Whether the argument is not a parameter itself.
    growing: bool,
    /// Whether the edge runs from a hub to a parameter of a method that meets what it stands for.
    out: bool,
    holder: usize,
    site: Span,
    /// What a diagnostic underlines: `site`, or the instance inside it. Only `site` picks and
    /// merges cycles.
    mark: Span,
    instance: Option<Instance>,
}

struct Graph {
    /// By declaration, its first node: the functions', then the classes', then the enums'; then
    /// the number of parameter nodes.
    first: Vec<usize>,
    functions: usize,
    classes: usize,
    edges: Vec<Edge>,
    /// By declaration and type, the node of a type argument that mentions a parameter, after
    /// the parameters' nodes. Its parts' edges into it stand for "mentions": a parameter reaches
    /// the node of each type that mentions it, so an argument needs one edge however many
    /// parameters it mentions, and each type of a declaration is looked at once.
    typed: FxHashMap<(usize, Type), usize>,
    /// The types `types` has walked, with the site each was written at.
    walked: FxHashSet<(Span, Type)>,
}

impl Graph {
    /// The edges into the parameters of the declaration `target` from `args`, its arguments,
    /// inside `holder`: growing unless the argument is a parameter itself.
    fn args(
        &mut self,
        holder: usize,
        args: &[Type],
        target: usize,
        site: Span,
        mark: Span,
        instance: Instance,
    ) {
        for (q, &arg) in args.iter().enumerate().filter(|(_, arg)| arg.has_param()) {
            let from = self.node(holder, arg);
            self.edges.push(Edge {
                from,
                to: self.first[target] + q,
                weight: 0,
                growing: !matches!(arg.kind(), TypeKind::Param(..)),
                out: false,
                holder,
                site,
                mark,
                instance: Some(instance),
            });
        }
    }

    /// An edge from the hub node `from` to the parameter node `to` of the method `method`.
    fn out(&mut self, from: usize, to: usize, weight: i64, method: usize) {
        self.edges.push(Edge {
            from,
            to,
            weight,
            growing: false,
            out: true,
            holder: method,
            site: Span::new(0, 0),
            mark: Span::new(0, 0),
            instance: None,
        });
    }

    /// The node of `ty`, which mentions a parameter, inside `holder`: a parameter's own, or one
    /// made, with its parts' after them, by a walk that keeps its own stack, as a type can nest
    /// deeper than the program's stack allows.
    fn node(&mut self, holder: usize, ty: Type) -> usize {
        let mut stack = vec![(ty, false)];
        while let Some((t, parts_made)) = stack.pop() {
            if self.made(holder, t).is_some() {
                continue;
            }
            if !parts_made {
                stack.push((t, true));
                stack.extend(parts(t).map(|part| (part, false)));
                continue;
            }
            let n = self.first[self.first.len() - 1] + self.typed.len();
            self.typed.insert((holder, t), n);
            for part in parts(t) {
                let from = self
                    .made(holder, part)
                    .expect("a type's parts should be made before it");
                self.edges.push(Edge {
                    from,
                    to: n,
                    weight: 1,
                    growing: false,
                    out: false,
                    holder,
                    site: Span::new(0, 0),
                    mark: Span::new(0, 0),
                    instance: None,
                });
            }
        }
        self.made(holder, ty)
            .expect("the walk should make the type it starts at")
    }

    /// The node of `ty` inside `holder`, if it has one yet.
    fn made(&self, holder: usize, ty: Type) -> Option<usize> {
        match ty.kind() {
            TypeKind::Param(_, i) => Some(self.first[holder] + i as usize),
            _ => self.typed.get(&(holder, ty)).copied(),
        }
    }

    /// The edges of each generic class and enum in `ty`, written at `site` inside `holder`; a
    /// type shared inside `ty` is walked once. An edge sits at the span `inner` gives its
    /// instance, else at `site`.
    fn types(&mut self, holder: usize, ty: Type, site: Span, inner: &[(Type, Span)]) {
        if !ty.has_param() {
            return;
        }
        let mut spans = FxHashMap::default();
        for &(t, span) in inner {
            spans.entry(t).or_insert(span);
        }
        let mut stack = vec![ty];
        while let Some(ty) = stack.pop() {
            if !ty.has_param() || !self.walked.insert((site, ty)) {
                continue;
            }
            let (target, args) = match ty.kind() {
                TypeKind::Array(inner) | TypeKind::Optional(inner) => {
                    stack.push(inner);
                    continue;
                }
                TypeKind::Class(id, args) => (self.functions + id.0, args),
                TypeKind::Enum(id, args) => (self.functions + self.classes + id.0, args),
                _ => continue,
            };
            let mark = spans.get(&ty).copied().unwrap_or(site);
            self.args(holder, args.get(), target, site, mark, Instance::Type(ty));
            stack.extend(args.get());
        }
    }
}

/// The parts of `ty` that mention a parameter. It looks into the kinds `TypeKind::has_param`
/// does, so a type other than a parameter that mentions one has a part that does.
fn parts(ty: Type) -> impl Iterator<Item = Type> {
    let (inner, args) = match ty.kind() {
        TypeKind::Array(inner) | TypeKind::Optional(inner) => (Some(inner), &[][..]),
        TypeKind::Class(_, args) | TypeKind::Enum(_, args) => (None, args.get()),
        _ => (None, &[][..]),
    };
    inner
        .into_iter()
        .chain(args.iter().copied())
        .filter(|part| part.has_param())
}

impl Checker<'_> {
    /// Reports each strongly connected component of the instantiation graph (see the module
    /// doc) that holds a cycle of positive weight, once, at the first edge of positive weight of
    /// the cycle it finds, in source order, with a note at each other instantiation on the cycle.
    /// `functions` are the lowered bodies, by `FuncId`; `unbuilt`, by class and then enum, marks
    /// the declarations that have their own error for no value being buildable, which stands for
    /// their cycle.
    pub(super) fn instantiation_cycles(
        &mut self,
        functions: &[Option<tir::Function>],
        unbuilt: &[bool],
    ) {
        let graph = self.instantiation_graph(functions, unbuilt);
        #[cfg(test)]
        EDGES.set(graph.edges.len());
        let nodes = graph.first[graph.first.len() - 1] + graph.typed.len();
        let mut out = vec![Vec::new(); nodes];
        for (i, edge) in graph.edges.iter().enumerate() {
            out[edge.from].push(i);
        }
        let targets: Vec<Vec<usize>> = out
            .iter()
            .map(|edges| edges.iter().map(|&i| graph.edges[i].to).collect())
            .collect();
        let component = components(&targets);
        // By component, whether an edge of negative weight is in it, and its first growing edge
        // in source order.
        let mut negative = vec![false; nodes];
        let mut growing: Vec<Option<usize>> = vec![None; nodes];
        for (i, edge) in graph.edges.iter().enumerate() {
            let k = component[edge.from];
            if component[edge.to] != k {
                continue;
            }
            negative[k] |= edge.weight < 0;
            if edge.growing
                && growing[k].is_none_or(|g| edge.site.start() < graph.edges[g].site.start())
            {
                growing[k] = Some(i);
            }
        }
        // Components that grow at one site, as `f([x], [y])` does for each parameter, are
        // reported once.
        let mut reported = FxHashSet::default();
        let mut cycles = Vec::new();
        for (k, &first) in growing.iter().enumerate() {
            if negative[k] {
                if let Some(cycle) = positive_cycle(&graph, &out, &component, k) {
                    cycles.push((None, cycle));
                }
            } else if let Some(g) = first {
                // Every weight is at least 0, so the cycle through a growing edge is positive:
                // it goes back by the instantiations that cost least.
                let edge = &graph.edges[g];
                let mut cycle = vec![g];
                cycle.extend(way_back(&graph, &out, &component, edge.to, edge.from));
                cycles.push((Some(g), cycle));
            }
        }
        for (label, cycle) in cycles {
            self.report_cycle(functions, &graph, label, &cycle, &mut reported);
        }
    }

    /// Reports the cycle of positive weight `cycle`, whose edges follow each other. It is labeled
    /// at `label`, an edge of it, or else at its first step of positive weight in source order. A
    /// cycle is reported once for each site.
    fn report_cycle(
        &mut self,
        functions: &[Option<tir::Function>],
        graph: &Graph,
        label: Option<usize>,
        cycle: &[usize],
        reported: &mut FxHashSet<Span>,
    ) {
        let edges = &graph.edges;
        // The cycle starts after an edge that no part leads into, so that each step is the parts
        // of an argument and then the edge that gives it.
        let start = cycle
            .iter()
            .position(|&i| edges[i].instance.is_some() || edges[i].out)
            .map_or(0, |i| i + 1);
        let cycle: Vec<usize> = cycle[start..]
            .iter()
            .chain(&cycle[..start])
            .copied()
            .collect();
        let label = label.unwrap_or_else(|| {
            // The weight of each step: the parts that lead to the edge, then the edge itself.
            let mut weight = 0;
            let mut best: Option<usize> = None;
            for &i in &cycle {
                weight += edges[i].weight;
                if edges[i].instance.is_none() && !edges[i].out {
                    continue;
                }
                if weight > 0 && best.is_none_or(|b| edges[i].site.start() < edges[b].site.start())
                {
                    best = Some(i);
                }
                weight = 0;
            }
            best.expect("a cycle of positive weight should hold a step of positive weight")
        });
        let edge = &edges[label];
        if !reported.insert(edge.site) {
            return;
        }
        let at = cycle
            .iter()
            .position(|&i| i == label)
            .expect("the label should be an edge of the cycle");
        // What follows the label around the cycle.
        let after: Vec<usize> = cycle[at + 1..]
            .iter()
            .chain(&cycle[..at])
            .copied()
            .collect();
        let (mut through, mut reach) = (Vec::new(), None);
        // A call that gives two parameters their arguments is one site, shown once.
        let mut shown = FxHashSet::from_iter([edge.site]);
        let mut k = 0;
        // A label at a call through a hub is followed by the edge out of the hub.
        if matches!(edge.instance, Some(Instance::Required(..)))
            && after.first().is_some_and(|&i| edges[i].out)
        {
            k = 1;
        }
        if let Some(Instance::Required(interface, ..)) = edge.instance {
            reach = Some(interface);
        }
        while let Some(&i) = after.get(k) {
            k += 1;
            let e = &edges[i];
            let Some(instance) = e.instance else {
                continue;
            };
            if !shown.insert(e.site) {
                continue;
            }
            let hop = match (instance, after.get(k).map(|&j| &edges[j])) {
                // A call through a hub may run any method that the hub reaches: the one the
                // cycle takes.
                (Instance::Required(interface, ..), Some(out)) if out.out => {
                    k += 1;
                    reach.get_or_insert(interface);
                    Hop::Calls {
                        method: self.method_text(functions, out.holder),
                        interface: self.interfaces[interface.0].name.name.to_string(),
                    }
                }
                _ => Hop::Instantiates(self.instance_name(functions, instance)),
            };
            through.push((e.mark, hop));
        }
        let declared = graph.functions;
        let name = match edge.holder.checked_sub(declared) {
            None => self.signatures[edge.holder].name.clone(),
            Some(c) if c < self.classes.len() => self.classes[c].name.name.to_string(),
            Some(c) => self.enums[c - self.classes.len()].name.name.to_string(),
        };
        self.report(DiagnosticKind::InstantiationCycle {
            name,
            at: edge.mark,
            instance: self.instance_name(
                functions,
                edge.instance
                    .expect("a labeled edge should be an instantiation"),
            ),
            through,
            reach: reach.map(|interface| self.interfaces[interface.0].name.name.to_string()),
        });
    }

    /// The instantiation graph of the program: see `instantiation_cycles`.
    fn instantiation_graph(&self, functions: &[Option<tir::Function>], unbuilt: &[bool]) -> Graph {
        // Each own type parameter of a requirement is a hub, a node of a declaration of its own:
        // every call of the requirement passes its argument to the hub, and the hub passes it to
        // the same parameter of each method that meets the requirement. So is each interface, for
        // the receiver of a call through a bound.
        let mut hubs = FxHashMap::default();
        let mut declarations = self.signatures.len() + self.classes.len() + self.enums.len();
        for (i, info) in self.interfaces.iter().enumerate() {
            for (m, requirement) in info.requirements.iter().enumerate() {
                for j in 0..requirement.own.len() {
                    hubs.insert((i, m, j), declarations);
                    declarations += 1;
                }
            }
        }
        let interface_hubs = declarations;
        declarations += self.interfaces.len();
        let counts = self.signatures.iter().map(|s| s.type_params.len());
        let counts = counts.chain(self.classes.iter().map(|c| c.type_params.len()));
        let counts = counts.chain(self.enums.iter().map(|e| e.type_params.len()));
        let counts = counts.chain(std::iter::repeat_n(
            1,
            declarations - interface_hubs + hubs.len(),
        ));
        let mut first = vec![0];
        for n in counts {
            first.push(first[first.len() - 1] + n);
        }
        let declared = self.signatures.len();
        let mut graph = Graph {
            first,
            functions: declared,
            classes: self.classes.len(),
            edges: Vec::new(),
            typed: FxHashMap::default(),
            walked: FxHashSet::default(),
        };
        let mut exprs = Vec::new();
        for (holder, function) in functions.iter().enumerate() {
            let Some(function) = function.as_ref().filter(|f| !f.type_params.is_empty()) else {
                continue;
            };
            exprs.clear();
            function.body.exprs(&mut exprs);
            for e in &exprs {
                match e.kind {
                    tir::ExprKind::Call { func, types, .. } => graph.args(
                        holder,
                        types.get(),
                        func.0,
                        e.span,
                        e.span,
                        Instance::Call(func, types),
                    ),
                    tir::ExprKind::InterfaceCall {
                        interface,
                        iargs,
                        method,
                        types,
                        ..
                    } => {
                        let method = method as usize;
                        let instance = Instance::Required(interface, method, types);
                        // The receiver goes to the hub of the interface, `Self` first among
                        // its arguments.
                        let hub = interface_hubs + interface.0;
                        graph.args(holder, &iargs.get()[..1], hub, e.span, e.span, instance);
                        for (j, arg) in types.get().iter().enumerate() {
                            let hub = hubs[&(interface.0, method, j)];
                            graph.args(holder, &[*arg], hub, e.span, e.span, instance);
                        }
                    }
                    _ => {}
                }
            }
        }
        self.hub_edges(&mut graph, &hubs, interface_hubs);
        for (c, class) in self.classes.iter().enumerate() {
            if unbuilt[c] {
                continue;
            }
            for field in &class.fields {
                if let Some(ty) = field.ty {
                    graph.types(declared + c, ty, field.ty_span, &field.inner);
                }
            }
        }
        for (e, info) in self.enums.iter().enumerate() {
            if unbuilt[self.classes.len() + e] {
                continue;
            }
            for variant in &info.variants {
                let written = variant
                    .fields
                    .iter()
                    .zip(&variant.spans)
                    .zip(&variant.inner);
                for ((ty, &span), inner) in written {
                    if let Some(ty) = *ty {
                        graph.types(declared + self.classes.len() + e, ty, span, inner);
                    }
                }
            }
        }
        graph
    }

    /// The edges from each hub to the parameters of the methods that meet what it stands for. The
    /// hub of a requirement's own parameter passes it to the parameter of the same place in each
    /// method that meets the requirement, at no cost. The hub of an interface passes the
    /// receiver to each parameter of each method of each conformance, less the depth that the
    /// parameter has in the conformance's target.
    fn hub_edges(
        &self,
        graph: &mut Graph,
        hubs: &FxHashMap<(usize, usize, usize), usize>,
        interface_hubs: usize,
    ) {
        for (interface, conformance) in self.conformances.iter() {
            let requirements = &self.interfaces[interface.0].requirements;
            // A conformance that lacks a method has its own error.
            if conformance.methods.len() != requirements.len() {
                continue;
            }
            let depths = param_depths(conformance.target, conformance.type_params);
            let hub = graph.first[interface_hubs + interface.0];
            for (m, (requirement, &method)) in
                requirements.iter().zip(&conformance.methods).enumerate()
            {
                let sig = &self.signatures[method.0];
                let owned = sig.type_params.len() - sig.own;
                let params = graph.first[method.0];
                for (q, &depth) in depths.iter().enumerate().take(owned) {
                    graph.out(hub, params + q, -depth, method.0);
                }
                for j in 0..requirement.own.len() {
                    let own = graph.first[hubs[&(interface.0, m, j)]];
                    graph.out(own, params + owned + j, 0, method.0);
                }
            }
        }
    }

    /// The method `holder`, as the program writes it with its owner's and its own type
    /// parameters: `Box<T>.run`, `Ring.wrap<U>`.
    fn method_text(&self, functions: &[Option<tir::Function>], holder: usize) -> String {
        let function = functions[holder]
            .as_ref()
            .expect("a method on a cycle should have a body, which gives its edge on the cycle");
        let (Some(method), Some(owner)) = (&function.method, function.owner) else {
            unreachable!("a hub reaches methods only");
        };
        let mut out = String::new();
        write_method(&mut out, owner, None, method, |head| self.head(head));
        let own = &function.type_params[function.owner_params..];
        out.push_str(&angled(own).to_string());
        out
    }

    /// `instance` as the program writes it: `f<[T]>`, `C<[T]>.m<U>`, or a type.
    fn instance_name(&self, functions: &[Option<tir::Function>], instance: Instance) -> String {
        let (func, types) = match instance {
            Instance::Type(ty) => return self.type_name(ty),
            Instance::Required(interface, method, types) => {
                let info = &self.interfaces[interface.0];
                let name = info.requirements[method].name.name;
                let mut out = format!("{}.{name}", info.name.name);
                write_args(&mut out, types.get(), |head| self.head(head));
                return out;
            }
            Instance::Call(func, types) => (func, types),
        };
        let function = functions[func.0]
            .as_ref()
            .expect("a callee on a cycle should have a body, which gives its edge on the cycle");
        let mut out = String::new();
        let own = if let (Some(method), Some(owner)) = (&function.method, function.owner) {
            let head = |head| self.head(head);
            write_method(&mut out, owner.subst(types.get()), None, method, head);
            &types.get()[function.owner_params..]
        } else {
            out.push_str(&self.signatures[func.0].name);
            types.get()
        };
        write_args(&mut out, own, |head| self.head(head));
        out
    }
}

/// The greatest depth at which each of the first `count` parameters nests in `ty`, a
/// conformance's target: 1 for a parameter that is an argument of the class or enum itself.
fn param_depths(ty: Type, count: usize) -> Vec<i64> {
    let mut depths = vec![0; count];
    let mut stack = vec![(ty, 0)];
    while let Some((ty, depth)) = stack.pop() {
        match ty.kind() {
            TypeKind::Param(_, i) => {
                let slot = &mut depths[i as usize];
                *slot = (*slot).max(depth);
            }
            TypeKind::Array(inner) | TypeKind::Optional(inner) => stack.push((inner, depth + 1)),
            TypeKind::Class(_, args) | TypeKind::Enum(_, args) => {
                stack.extend(
                    args.get()
                        .iter()
                        .filter(|a| a.has_param())
                        .map(|&a| (a, depth + 1)),
                );
            }
            _ => {}
        }
    }
    depths
}

/// A cycle of positive weight among the edges inside the component `k`, as the edges in order,
/// if there is one: Bellman–Ford, the longest way to each node from all of them at once. The
/// weights add up around a cycle that keeps making a way longer, and a pass that makes none
/// ends the search. A node whose way a pass still lengthens after one pass for each node has a
/// way that goes around such a cycle.
///
/// This is an exception to `CODING_STANDARDS.md`'s "A pass is linear in the program" (user,
/// 2026-10-25). The hubs' edges have negative weights, and no known algorithm finds a cycle of
/// positive weight in a graph with negative weights in linear time. The nodes and the edges are
/// linear in the program and the passes stop at the first that changes nothing, but a component
/// can hold many functions through a hub, and a cycle costs the nodes plus one passes of the
/// component's edges. A component with no negative weight is not searched (see the module doc).
fn positive_cycle(
    graph: &Graph,
    out: &[Vec<usize>],
    component: &[usize],
    k: usize,
) -> Option<Vec<usize>> {
    let nodes: Vec<usize> = (0..out.len()).filter(|&n| component[n] == k).collect();
    // The place of each node of the component among them.
    let place: FxHashMap<usize, usize> = nodes.iter().enumerate().map(|(p, &n)| (n, p)).collect();
    // Each edge inside the component, as the edge and the places of its nodes.
    let edges: Vec<(usize, usize, usize)> = nodes
        .iter()
        .flat_map(|&n| out[n].iter().copied())
        .filter(|&i| component[graph.edges[i].to] == k)
        .map(|i| (i, place[&graph.edges[i].from], place[&graph.edges[i].to]))
        .collect();
    let mut length = vec![0_i64; nodes.len()];
    // The edge that gave each node its longest way so far.
    let mut last: Vec<Option<usize>> = vec![None; nodes.len()];
    let mut changed = None;
    for _ in 0..=nodes.len() {
        changed = None;
        for &(i, from, to) in &edges {
            #[cfg(test)]
            BELLMAN_FORD_STEPS.set(BELLMAN_FORD_STEPS.get() + 1);
            let longer = length[from] + graph.edges[i].weight;
            if longer > length[to] {
                length[to] = longer;
                last[to] = Some(i);
                changed = Some(to);
            }
        }
        changed?;
    }
    // The way to `n` has as many edges as the passes made it longer, so walking back as many
    // steps as there are nodes is inside the cycle.
    let back = |n: usize| {
        let i = last[n].expect("a node whose way was lengthened should have an edge into it");
        (i, place[&graph.edges[i].from])
    };
    let mut n = changed.expect("the last pass should have lengthened a way");
    for _ in 0..nodes.len() {
        n = back(n).1;
    }
    let start = n;
    let mut cycle = Vec::new();
    loop {
        let (i, from) = back(n);
        cycle.push(i);
        n = from;
        if n == start {
            break;
        }
    }
    cycle.reverse();
    Some(cycle)
}

/// The edges of a shortest path from node `from` to node `to`, which are in one component, by
/// instantiations: an edge into a type's node from one of its parts costs nothing, so a way
/// through a deeper type argument is no longer for it (a 0-1 breadth-first search). Every edge
/// into a node costs the same, nothing into a type's node and one into a parameter, so the first
/// way a node is reached is a cheapest one.
fn way_back(
    graph: &Graph,
    out: &[Vec<usize>],
    component: &[usize],
    from: usize,
    to: usize,
) -> Vec<usize> {
    // Each node reached, by the edge that first reached it: the queue's order makes it a
    // cheapest.
    let mut reached: FxHashMap<usize, Option<usize>> = FxHashMap::default();
    reached.insert(from, None);
    let mut queue = VecDeque::from([from]);
    while let Some(n) = queue.pop_front() {
        if n == to {
            break;
        }
        for &i in &out[n] {
            let edge = &graph.edges[i];
            let m = edge.to;
            if component[m] == component[from] && !reached.contains_key(&m) {
                reached.insert(m, Some(i));
                if edge.instance.is_none() {
                    queue.push_front(m);
                } else {
                    queue.push_back(m);
                }
            }
        }
    }
    let mut path = Vec::new();
    let mut n = to;
    while let Some(i) = reached[&n] {
        path.push(i);
        n = graph.edges[i].from;
    }
    path.reverse();
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbol::Symbol;
    use crate::tir::ClassId;

    fn param(i: u32) -> Type {
        Type::intern(TypeKind::Param(Symbol::intern("T"), i))
    }

    fn pair(a: Type, b: Type) -> Type {
        Type::intern(TypeKind::Class(ClassId(0), TypeArgs::intern(&[a, b])))
    }

    /// A function with `params` type parameters and a class `Pair<A, B>`.
    fn graph(params: usize) -> Graph {
        Graph {
            first: vec![0, params, params + 2],
            functions: 1,
            classes: 1,
            edges: Vec::new(),
            typed: FxHashMap::default(),
            walked: FxHashSet::default(),
        }
    }

    /// A type that shares its arguments, `2^20` paths to `T` in 21 types, is looked at once per
    /// type, not per path.
    #[test]
    fn shared_type_arguments_should_be_walked_once() {
        let mut ty = param(0);
        for _ in 0..20 {
            ty = pair(ty, ty);
        }
        let site = Span::new(0, 1);
        let mut g = graph(1);
        g.args(0, &[ty], 0, site, site, Instance::Type(ty));
        // A node per `Pair`, an edge from each of its two arguments, and the call's edge.
        assert_eq!(g.typed.len(), 20);
        assert_eq!(g.edges.len(), 2 * 20 + 1);
        // Written as a field's type: each level's `Pair` gives an edge into each of its two
        // parameters, once, from the nodes of the 19 levels below the top.
        let mut g = graph(1);
        g.types(0, ty, site, &[]);
        assert_eq!(g.edges.len(), 2 * 20 + 2 * 19);
    }

    /// An argument that mentions `n` parameters, `Pair<…Pair<T0, T1>…, Tn>`, costs a node and
    /// two edges per `Pair` once, and one edge at each further site: never a copy of what each
    /// part mentions.
    #[test]
    fn arguments_mentioning_many_parameters_should_stay_linear() {
        for n in [16_u32, 256] {
            let mut ty = param(0);
            for i in 1..=n {
                ty = pair(ty, param(i));
            }
            let mut g = graph(n as usize + 1);
            for site in 0..10 {
                let at = Span::new(site, site + 1);
                g.args(0, &[ty], 0, at, at, Instance::Type(ty));
            }
            assert_eq!(g.typed.len(), n as usize);
            assert_eq!(g.edges.len(), 2 * n as usize + 10);
        }
    }

    /// A type nested deeper than a 64 KiB stack allows recursing into is walked with a stack
    /// of its own, as an argument and as a field's type.
    #[test]
    fn deep_types_should_be_walked_without_recursion() {
        let levels = 10_000;
        std::thread::Builder::new()
            .stack_size(64 * 1024)
            .spawn(move || {
                let mut ty = param(0);
                for _ in 0..levels {
                    ty = Type::intern(TypeKind::Array(ty));
                }
                let mut g = graph(1);
                g.args(
                    0,
                    &[ty],
                    0,
                    Span::new(0, 1),
                    Span::new(0, 1),
                    Instance::Type(ty),
                );
                assert_eq!(g.typed.len(), levels);
                let mut ty = param(0);
                for _ in 0..levels {
                    ty = pair(ty, param(0));
                }
                let mut g = graph(1);
                g.types(0, ty, Span::new(0, 1), &[]);
                assert_eq!(g.walked.len(), levels + 1);
            })
            .expect("the thread should start")
            .join()
            .expect("the walk should not overflow the stack");
    }

    /// A parameter has no parts: its node is its own, never one made of parts.
    #[test]
    fn a_parameter_should_have_no_parts() {
        assert_eq!(parts(param(0)).count(), 0);
    }
}
