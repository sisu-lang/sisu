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

use std::collections::VecDeque;

use rustc_hash::{FxHashMap, FxHashSet};

use super::Checker;
use crate::diagnostic::DiagnosticKind;
use crate::graph::components;
use crate::source::Span;
use crate::tir::{self, Type, TypeArgs, TypeKind};

/// An instantiation, which a diagnostic writes as the program does.
#[derive(Clone, Copy)]
enum Instance {
    /// A call of the function, with its type arguments, a method's owner's first.
    Call(tir::FuncId, TypeArgs),
    /// A generic class or enum, as a type.
    Type(Type),
}

/// An edge of the graph, from an instantiation at `site` inside the declaration `holder`; or,
/// with no `instance`, into a type's node from the node of one of its parts.
struct Edge {
    from: usize,
    to: usize,
    growing: bool,
    holder: usize,
    site: Span,
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
        instance: Instance,
    ) {
        for (q, &arg) in args.iter().enumerate().filter(|(_, arg)| arg.has_param()) {
            let from = self.node(holder, arg);
            self.edges.push(Edge {
                from,
                to: self.first[target] + q,
                growing: !matches!(arg.kind(), TypeKind::Param(..)),
                holder,
                site,
                instance: Some(instance),
            });
        }
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
                    growing: false,
                    holder,
                    site: Span::new(0, 0),
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
    /// type shared inside `ty` is walked once.
    fn types(&mut self, holder: usize, ty: Type, site: Span) {
        if !ty.has_param() {
            return;
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
            self.args(holder, args.get(), target, site, Instance::Type(ty));
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
    /// doc) that holds a growing edge, once, at its first growing edge in source order, with a
    /// note at each other edge of a shortest way back. `functions` are the lowered bodies, by
    /// `FuncId`; `unbuilt`, by class and then enum, marks the declarations that have their own
    /// error for no value being buildable, which stands for their cycle.
    pub(super) fn instantiation_cycles(
        &mut self,
        functions: &[Option<tir::Function>],
        unbuilt: &[bool],
    ) {
        let graph = self.instantiation_graph(functions, unbuilt);
        let declared = graph.functions;
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
        // By component, its first growing edge in source order.
        let mut growing: Vec<Option<usize>> = vec![None; nodes];
        for (i, edge) in graph.edges.iter().enumerate() {
            let k = component[edge.from];
            if edge.growing
                && component[edge.to] == k
                && growing[k].is_none_or(|g| edge.site.start() < graph.edges[g].site.start())
            {
                growing[k] = Some(i);
            }
        }
        // Components that grow at one site, as `f([x], [y])` does for each parameter, are
        // reported once.
        let mut reported = FxHashSet::default();
        for g in growing.into_iter().flatten() {
            let edge = &graph.edges[g];
            if !reported.insert(edge.site) {
                continue;
            }
            let back = way_back(&graph, &out, &component, edge.to, edge.from);
            // A call that gives two parameters their arguments is one site, shown once.
            let mut shown = FxHashSet::from_iter([edge.site]);
            let through = back
                .into_iter()
                .map(|i| &graph.edges[i])
                .filter_map(|e| {
                    e.instance
                        .filter(|_| shown.insert(e.site))
                        .map(|i| (e.site, i))
                })
                .map(|(site, instance)| (site, self.instance_name(functions, instance)))
                .collect();
            let name = match edge.holder.checked_sub(declared) {
                None => self.signatures[edge.holder].name.clone(),
                Some(c) if c < self.classes.len() => self.classes[c].name.name.to_string(),
                Some(c) => self.enums[c - self.classes.len()].name.name.to_string(),
            };
            self.report(DiagnosticKind::InstantiationCycle {
                name,
                at: edge.site,
                instance: self.instance_name(
                    functions,
                    edge.instance
                        .expect("a growing edge should be an instantiation"),
                ),
                through,
            });
        }
    }

    /// The instantiation graph of the program: see `instantiation_cycles`.
    fn instantiation_graph(&self, functions: &[Option<tir::Function>], unbuilt: &[bool]) -> Graph {
        let counts = self.signatures.iter().map(|s| s.type_params.len());
        let counts = counts.chain(self.classes.iter().map(|c| c.type_params.len()));
        let counts = counts.chain(self.enums.iter().map(|e| e.type_params.len()));
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
                if let tir::ExprKind::Call { func, types, .. } = e.kind {
                    graph.args(
                        holder,
                        types.get(),
                        func.0,
                        e.span,
                        Instance::Call(func, types),
                    );
                }
            }
        }
        for (c, class) in self.classes.iter().enumerate() {
            if unbuilt[c] {
                continue;
            }
            for field in &class.fields {
                if let Some(ty) = field.ty {
                    graph.types(declared + c, ty, field.ty_span);
                }
            }
        }
        for (e, info) in self.enums.iter().enumerate() {
            if unbuilt[self.classes.len() + e] {
                continue;
            }
            for variant in &info.variants {
                for (ty, &span) in variant.fields.iter().zip(&variant.spans) {
                    if let Some(ty) = *ty {
                        graph.types(declared + self.classes.len() + e, ty, span);
                    }
                }
            }
        }
        graph
    }

    /// `instance` as the program writes it: `f<[T]>`, `C<[T]>.m<U>`, or a type.
    fn instance_name(&self, functions: &[Option<tir::Function>], instance: Instance) -> String {
        let (func, types) = match instance {
            Instance::Type(ty) => return self.type_name(ty),
            Instance::Call(func, types) => (func, types),
        };
        let written = |list: &[Type]| TypeArgs::intern(list).written(|ty| self.type_name(ty));
        let owner = functions[func.0]
            .as_ref()
            .expect("a callee on a cycle should have a body, which gives its edge on the cycle")
            .owner_params;
        let (owner_args, args) = types.get().split_at(owner);
        let name = &self.signatures[func.0].name;
        match name.split_once('.') {
            Some((owner, method)) => {
                format!("{owner}{}.{method}{}", written(owner_args), written(args))
            }
            None => format!("{name}{}", written(args)),
        }
    }
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
    fn shared_type_arguments_are_walked_once() {
        let mut ty = param(0);
        for _ in 0..20 {
            ty = pair(ty, ty);
        }
        let site = Span::new(0, 1);
        let mut g = graph(1);
        g.args(0, &[ty], 0, site, Instance::Type(ty));
        // A node per `Pair`, an edge from each of its two arguments, and the call's edge.
        assert_eq!(g.typed.len(), 20);
        assert_eq!(g.edges.len(), 2 * 20 + 1);
        // Written as a field's type: each level's `Pair` gives an edge into each of its two
        // parameters, once, from the nodes of the 19 levels below the top.
        let mut g = graph(1);
        g.types(0, ty, site);
        assert_eq!(g.edges.len(), 2 * 20 + 2 * 19);
    }

    /// An argument that mentions `n` parameters, `Pair<…Pair<T0, T1>…, Tn>`, costs a node and
    /// two edges per `Pair` once, and one edge at each further site: never a copy of what each
    /// part mentions.
    #[test]
    fn arguments_mentioning_many_parameters_stay_linear() {
        for n in [16_u32, 256] {
            let mut ty = param(0);
            for i in 1..=n {
                ty = pair(ty, param(i));
            }
            let mut g = graph(n as usize + 1);
            for site in 0..10 {
                g.args(0, &[ty], 0, Span::new(site, site + 1), Instance::Type(ty));
            }
            assert_eq!(g.typed.len(), n as usize);
            assert_eq!(g.edges.len(), 2 * n as usize + 10);
        }
    }

    /// A type nested deeper than a 64 KiB stack allows recursing into is walked with a stack
    /// of its own, as an argument and as a field's type.
    #[test]
    fn deep_types_are_walked_without_recursion() {
        let levels = 10_000;
        std::thread::Builder::new()
            .stack_size(64 * 1024)
            .spawn(move || {
                let mut ty = param(0);
                for _ in 0..levels {
                    ty = Type::intern(TypeKind::Array(ty));
                }
                let mut g = graph(1);
                g.args(0, &[ty], 0, Span::new(0, 1), Instance::Type(ty));
                assert_eq!(g.typed.len(), levels);
                let mut ty = param(0);
                for _ in 0..levels {
                    ty = pair(ty, param(0));
                }
                let mut g = graph(1);
                g.types(0, ty, Span::new(0, 1));
                assert_eq!(g.walked.len(), levels + 1);
            })
            .expect("the thread should start")
            .join()
            .expect("the walk should not overflow the stack");
    }

    /// A parameter has no parts: its node is its own, never one made of parts.
    #[test]
    fn a_parameter_has_no_parts() {
        assert_eq!(parts(param(0)).count(), 0);
    }
}
