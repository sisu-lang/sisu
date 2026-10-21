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

/// An edge between type parameters, from an instantiation at `site` inside the declaration
/// `holder`.
struct Edge {
    from: usize,
    to: usize,
    growing: bool,
    holder: usize,
    site: Span,
    instance: Instance,
}

struct Graph {
    /// By declaration, its first node: the functions', then the classes', then the enums'; then
    /// the number of nodes.
    first: Vec<usize>,
    functions: usize,
    classes: usize,
    edges: Vec<Edge>,
}

impl Graph {
    /// The edges into the parameters of the declaration `target` from those of `holder` that
    /// each of `args`, its arguments, mentions.
    fn args(
        &mut self,
        holder: usize,
        args: &[Type],
        target: usize,
        site: Span,
        instance: Instance,
    ) {
        for (q, &arg) in args.iter().enumerate() {
            mentions(arg, &mut |p| {
                self.edges.push(Edge {
                    from: self.first[holder] + p,
                    to: self.first[target] + q,
                    growing: !matches!(arg.kind(), TypeKind::Param(_, i) if i as usize == p),
                    holder,
                    site,
                    instance,
                });
            });
        }
    }

    /// The edges of each generic class and enum in `ty`, written at `site` inside `holder`.
    fn types(&mut self, holder: usize, ty: Type, site: Span) {
        if !ty.has_param() {
            return;
        }
        let (target, args) = match ty.kind() {
            TypeKind::Array(inner) | TypeKind::Optional(inner) => {
                return self.types(holder, inner, site);
            }
            TypeKind::Class(id, args) => (self.functions + id.0, args),
            TypeKind::Enum(id, args) => (self.functions + self.classes + id.0, args),
            _ => return,
        };
        self.args(holder, args.get(), target, site, Instance::Type(ty));
        for &arg in args.get() {
            self.types(holder, arg, site);
        }
    }
}

/// Calls `found` with the index of each type parameter `ty` mentions.
fn mentions(ty: Type, found: &mut impl FnMut(usize)) {
    match ty.kind() {
        TypeKind::Param(_, i) => found(i as usize),
        TypeKind::Array(inner) | TypeKind::Optional(inner) => mentions(inner, found),
        TypeKind::Class(_, args) | TypeKind::Enum(_, args) => {
            for &arg in args.get() {
                mentions(arg, found);
            }
        }
        _ => {}
    }
}

impl Checker<'_> {
    /// Reports each strongly connected component of the instantiation graph (see the module
    /// doc) that holds a growing edge, once, at its first growing edge in source order, with a
    /// note at each other edge of a shortest way back. `functions` are the lowered bodies, by
    /// `FuncId`; `unbuilt`, by class and then enum, marks the declarations no value can be built
    /// of, whose error stands for their cycle.
    pub(super) fn instantiation_cycles(
        &mut self,
        functions: &[Option<tir::Function>],
        unbuilt: &[bool],
    ) {
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
        let nodes = graph.first[graph.first.len() - 1];
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
        for g in growing.into_iter().flatten() {
            let edge = &graph.edges[g];
            let back = way_back(&graph, &out, &component, edge.to, edge.from);
            // A call that gives two parameters their arguments is one site, shown once.
            let mut shown = FxHashSet::from_iter([edge.site]);
            let through = back
                .into_iter()
                .map(|i| &graph.edges[i])
                .filter(|e| shown.insert(e.site))
                .map(|e| (e.site, self.instance_name(functions, e.instance)))
                .collect();
            let name = match edge.holder.checked_sub(declared) {
                None => self.signatures[edge.holder].name.clone(),
                Some(c) if c < self.classes.len() => self.classes[c].name.name.to_string(),
                Some(c) => self.enums[c - self.classes.len()].name.name.to_string(),
            };
            self.report(DiagnosticKind::InstantiationCycle {
                name,
                at: edge.site,
                instance: self.instance_name(functions, edge.instance),
                through,
            });
        }
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

/// The edges of a shortest path from node `from` to node `to`, which are in one component.
fn way_back(
    graph: &Graph,
    out: &[Vec<usize>],
    component: &[usize],
    from: usize,
    to: usize,
) -> Vec<usize> {
    // Each node reached, by the edge that reached it first.
    let mut reached = FxHashMap::default();
    let mut queue = VecDeque::from([from]);
    while let Some(n) = queue.pop_front() {
        if n == to {
            break;
        }
        for &i in &out[n] {
            let m = graph.edges[i].to;
            if component[m] == component[from] && !reached.contains_key(&m) {
                reached.insert(m, i);
                queue.push_back(m);
            }
        }
    }
    let mut path = Vec::new();
    let mut n = to;
    while n != from {
        let i = reached[&n];
        path.push(i);
        n = graph.edges[i].from;
    }
    path.reverse();
    path
}
