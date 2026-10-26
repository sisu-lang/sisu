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
//! An associated type `P.Item` as an argument is a type of its own in the graph, and an edge into
//! it from `P` weighs the growth of `Item`: the greatest, over the conformances that choose it,
//! of how much deeper the choice nests the types that stand for the conformance's parameters than
//! the conformance's target does. `Wrap<T>`'s `type Item = Wrap<Wrap<T>>` grows by 1, and
//! `type Item = T` by -1. A choice that names a parameter through another associated type adds
//! that type's growth, which is a longest path over the choices, and unbounded on a cycle of
//! positive weight. A choice that names no parameter gives a growth that is absent: `P.Item` is
//! then a type that no argument changes, and the argument adds no edge.
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
    /// How many nodes the search for a cycle has read to gather the nodes of the components.
    pub(super) static GATHER_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The nodes and the edges of the component that the last search for a cycle read.
    pub(super) static SEARCHED: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

/// The weight of an edge into an associated type whose growth is unbounded: more than any path
/// of the graph can take back.
const UNBOUNDED: i64 = 1 << 32;

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
    /// Whether the edge runs into an associated type from its base, so that its weight is a
    /// growth, which may not be above 0.
    assoc: bool,
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
    /// By interface, where its associated types start in `growth`.
    assoc_first: Vec<usize>,
    /// The growth of each associated type of each interface (`growths`); `None` when absent.
    growth: Vec<Option<i64>>,
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
                assoc: false,
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
            assoc: false,
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
                stack.extend(self.parts(t).into_iter().map(|(part, _)| (part, false)));
                continue;
            }
            let n = self.first[self.first.len() - 1] + self.typed.len();
            self.typed.insert((holder, t), n);
            for (part, weight) in self.parts(t) {
                let from = self
                    .made(holder, part)
                    .expect("a type's parts should be made before it");
                self.edges.push(Edge {
                    from,
                    to: n,
                    weight,
                    growing: false,
                    out: false,
                    assoc: matches!(t.kind(), TypeKind::Assoc(..)),
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

    /// The parts of `ty` that mention a parameter, each with the weight of the edge from its node
    /// into `ty`'s: 1 for the type inside an array or an optional and for an argument of a class
    /// or an enum, and the growth of the associated type for its base and the arguments of its
    /// interface. An associated type with no growth has no part.
    fn parts(&self, ty: Type) -> Vec<(Type, i64)> {
        let (inner, args, weight) = match ty.kind() {
            TypeKind::Array(inner) | TypeKind::Optional(inner) => (Some(inner), &[][..], 1),
            TypeKind::Class(_, args) | TypeKind::Enum(_, args) => (None, args.get(), 1),
            TypeKind::Assoc(base, interface, args, k) => {
                let Some(growth) = self.growth[self.assoc_first[interface.0] + k as usize] else {
                    return Vec::new();
                };
                (Some(base), args.get(), growth)
            }
            _ => (None, &[][..], 1),
        };
        (inner.into_iter().chain(args.iter().copied()))
            .filter(|part| part.has_param())
            .map(|part| (part, weight))
            .collect()
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
                TypeKind::Assoc(base, _, args, _) => {
                    stack.push(base);
                    stack.extend(args.get());
                    continue;
                }
                _ => continue,
            };
            let mark = spans.get(&ty).copied().unwrap_or(site);
            self.args(holder, args.get(), target, site, mark, Instance::Type(ty));
            stack.extend(args.get());
        }
    }
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
            // A growth that is not above 0 breaks the rule of a component with no negative
            // weight, that a cycle through a growing edge is a cycle of positive weight.
            negative[k] |= edge.weight < 0 || (edge.assoc && edge.weight == 0);
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
        // The nodes of each component with a negative weight, gathered in one pass.
        let mut members: Vec<Vec<usize>> = vec![Vec::new(); nodes];
        for (n, &k) in component.iter().enumerate() {
            #[cfg(test)]
            GATHER_STEPS.set(GATHER_STEPS.get() + 1);
            if negative[k] {
                members[k].push(n);
            }
        }
        for (k, &first) in growing.iter().enumerate() {
            if negative[k] {
                if let Some(cycle) = positive_cycle(&graph, &out, &component, k, &members[k]) {
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
            // The weight of each step: the parts that lead to the edge, then the edge itself. The
            // first of the steps of positive weight in source order, the earlier on the cycle
            // when two start at one place.
            let mut weight = 0;
            let mut positive = Vec::new();
            for &i in &cycle {
                weight += edges[i].weight;
                if edges[i].instance.is_none() && !edges[i].out {
                    continue;
                }
                if weight > 0 {
                    positive.push(i);
                }
                weight = 0;
            }
            (positive.into_iter())
                .min_by_key(|&i| edges[i].site.start())
                .expect("a cycle of positive weight should hold a step of positive weight")
        });
        let edge = &edges[label];
        if !reported.insert(edge.site) {
            return;
        }
        // What follows the label around the cycle.
        let at = cycle
            .iter()
            .position(|&i| i == label)
            .expect("the label should be an edge of the cycle");
        let mut cycle = cycle;
        cycle.rotate_left(at);
        let (mut through, mut reach) = (Vec::new(), None);
        // A call that gives two parameters their arguments is one site, shown once.
        let mut shown = FxHashSet::from_iter([edge.site]);
        if let Some(Instance::Required(interface, ..)) = edge.instance {
            reach = Some(interface);
        }
        // A call through a hub, until the edge out of the hub says which method it may run.
        let mut calling: Option<(Span, tir::InterfaceId)> = None;
        for &i in &cycle[1..] {
            let e = &edges[i];
            if e.out {
                if let Some((mark, interface)) = calling.take() {
                    through.push((
                        mark,
                        Hop::Calls {
                            // The method that the cycle takes, of all the hub reaches.
                            method: self.method_text(functions, e.holder),
                            interface: self.interfaces[interface.0].name.name.to_string(),
                        },
                    ));
                }
                continue;
            }
            let Some(instance) = e.instance else {
                continue;
            };
            if !shown.insert(e.site) {
                continue;
            }
            if let Instance::Required(interface, ..) = instance {
                reach.get_or_insert(interface);
                calling = Some((e.mark, interface));
            } else {
                through.push((
                    e.mark,
                    Hop::Instantiates(self.instance_name(functions, instance)),
                ));
            }
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
        // The hubs of the interfaces follow, one each.
        let interface_hubs = declarations;
        let counts = self.signatures.iter().map(|s| s.type_params.len());
        let counts = counts.chain(self.classes.iter().map(|c| c.type_params.len()));
        let counts = counts.chain(self.enums.iter().map(|e| e.type_params.len()));
        let counts = counts.chain(std::iter::repeat_n(1, hubs.len() + self.interfaces.len()));
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
            assoc_first: Vec::new(),
            growth: Vec::new(),
        };
        (graph.assoc_first, graph.growth) = self.growths();
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

    /// Where the associated types of each interface start in the second list, and the growth of
    /// each (see the module doc), `None` when absent. A longest path over the choices: the choice
    /// of the associated type `a` names a parameter of its conformance at some depth, which gives
    /// `a` a growth outright, and names `Q.b` at some depth, which gives `a` the growth of `b`
    /// less the depth of `Q` in the target, and plus the depth of the name. Relaxing as many
    /// times as there are associated types settles a path; one that still grows after that is on
    /// a cycle of positive weight, and so is every growth that reaches it: `UNBOUNDED`.
    ///
    /// shortcut: a choice that names `Q.b.c` counts the growth of `c` and not of `b`. To add them,
    /// give each associated type a node for each base it is written through.
    fn growths(&self) -> (Vec<usize>, Vec<Option<i64>>) {
        let mut first = Vec::with_capacity(self.interfaces.len());
        let mut count = 0;
        for info in &self.interfaces {
            first.push(count);
            count += info.assoc.len();
        }
        let mut growth: Vec<Option<i64>> = vec![None; count];
        if count == 0 {
            return (first, growth);
        }
        // The associated types a choice names through a parameter, by the type that chooses.
        let mut edges: Vec<(usize, usize, i64)> = Vec::new();
        for (interface, conformance) in self.conformances.iter() {
            let target = param_depths(conformance.target, conformance.type_params);
            for (k, chosen) in conformance.assoc.iter().enumerate() {
                let Some(chosen) = *chosen else { continue };
                let at = first[interface.0] + k;
                let (depths, uses) = depths_and_assoc(chosen, conformance.type_params);
                for (q, depth) in depths.iter().enumerate() {
                    let outright = depth.map(|depth| depth - target[q]);
                    growth[at] = growth[at].max(outright);
                }
                for used in uses {
                    let named = first[used.interface.0] + used.index;
                    edges.push((at, named, used.depth - target[used.param]));
                }
            }
        }
        let relax = |growth: &mut Vec<Option<i64>>| {
            let mut changed = Vec::new();
            for &(from, to, weight) in &edges {
                let Some(through) = growth[to].map(|g| g + weight) else {
                    continue;
                };
                if growth[from].is_none_or(|g| through > g) {
                    growth[from] = Some(through);
                    changed.push(from);
                }
            }
            changed
        };
        for _ in 0..=count {
            if relax(&mut growth).is_empty() {
                return (first, growth);
            }
        }
        // Still growing: a cycle of positive weight. A type that a pass lengthens after that is on
        // the cycle or reaches it, since a way that goes through no cycle is settled by now, so
        // each such type is unbounded.
        let mut unbounded = vec![false; count];
        for _ in 0..=count {
            for from in relax(&mut growth) {
                unbounded[from] = true;
            }
        }
        for (g, _) in growth.iter_mut().zip(unbounded).filter(|&(_, u)| u) {
            *g = Some(UNBOUNDED);
        }
        (first, growth)
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
    let (depths, _) = depths_and_assoc(ty, count);
    depths.into_iter().map(|depth| depth.unwrap_or(0)).collect()
}

/// An associated type of a parameter in a type: the place of the parameter that its base names,
/// the associated type, and the depth at which it stands.
struct AssocUse {
    param: usize,
    interface: tir::InterfaceId,
    index: usize,
    depth: i64,
}

/// `param_depths`, with `None` for a parameter that `ty` does not name, and each associated type
/// of a parameter that `ty` names. The arguments of an associated type's interface stand at its
/// depth.
fn depths_and_assoc(ty: Type, count: usize) -> (Vec<Option<i64>>, Vec<AssocUse>) {
    let mut depths = vec![None; count];
    let mut uses = Vec::new();
    let mut stack = vec![(ty, 0)];
    while let Some((ty, depth)) = stack.pop() {
        if !ty.has_param() {
            continue;
        }
        match ty.kind() {
            TypeKind::Param(_, i) => {
                let slot = &mut depths[i as usize];
                *slot = (*slot).max(Some(depth));
            }
            TypeKind::Array(inner) | TypeKind::Optional(inner) => stack.push((inner, depth + 1)),
            TypeKind::Class(_, args) | TypeKind::Enum(_, args) => {
                stack.extend(args.get().iter().map(|&a| (a, depth + 1)));
            }
            TypeKind::Assoc(base, interface, args, k) => {
                let mut root = base;
                while let TypeKind::Assoc(inner, ..) = root.kind() {
                    root = inner;
                }
                let TypeKind::Param(_, q) = root.kind() else {
                    unreachable!(
                        "an associated type of a type parameter should be written through one"
                    );
                };
                uses.push(AssocUse {
                    param: q as usize,
                    interface,
                    index: k as usize,
                    depth,
                });
                stack.extend(args.get().iter().map(|&a| (a, depth)));
            }
            _ => unreachable!("only a part that names a parameter is walked"),
        }
    }
    (depths, uses)
}

/// A cycle of positive weight among the edges inside the component `k`, whose nodes are `nodes`,
/// as the edges in order, if there is one: Bellman–Ford, the longest way to each node from all of
/// them at once. The weights add up around a cycle that keeps making a way longer, and a pass
/// that makes none ends the search. A node whose way a pass still lengthens after one pass for
/// each node has a way that goes around such a cycle.
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
    nodes: &[usize],
) -> Option<Vec<usize>> {
    // The place of each node of the component among them.
    let place: FxHashMap<usize, usize> = nodes.iter().enumerate().map(|(p, &n)| (n, p)).collect();
    // Each edge inside the component, as the edge and the places of its nodes.
    let edges: Vec<(usize, usize, usize)> = nodes
        .iter()
        .flat_map(|&n| out[n].iter().copied())
        .filter(|&i| component[graph.edges[i].to] == k)
        .map(|i| (i, place[&graph.edges[i].from], place[&graph.edges[i].to]))
        .collect();
    #[cfg(test)]
    SEARCHED.set((nodes.len(), edges.len()));
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
            assoc_first: Vec::new(),
            growth: Vec::new(),
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
        assert_eq!(graph(1).parts(param(0)), Vec::new());
    }
}
