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
//! The operators `==` and `<` and `contains` call a written `eq` or `less` through a helper that
//! is not a call in the typed tree, so each one in a generic body adds an edge, by the type of its
//! operand. An array or an optional passes its element or its contents, which the generated rule
//! does at no cost. An enum with no written conformance to the interface passes its arguments to
//! the enum's generated `Eq` node, a declaration of its own that has the enum's parameters and an
//! edge for each payload type, chosen by the same rule. Any other type goes to the interface's
//! hub, as the receiver of a call through a bound does. An enum with only conditional
//! conformances takes both edges. A `print` and each part of an interpolation are the same for
//! `Display`: the operand type chooses the edges, and each enum has a generated `Display` node
//! beside its generated `Eq` node.
//!
//! Each parameter of a function, class, enum or generated node has a second node, for its value
//! with its outer array and optional levels taken off, since the generated rule takes those off
//! before any written method runs. An argument feeds a parameter's first node as above, and its
//! second node from the argument with its outer levels taken off, at weight 0: from the second
//! node of the parameter when that is one parameter under arrays and optionals only, else from the
//! node of the argument without those levels. An operand that is a parameter under arrays and
//! optionals starts its edge to the hub at that parameter's second node. A hub's edges feed first
//! nodes only. Two nodes for each parameter keep the graph linear.
//!
//! An associated type `P.Item` as an argument is a type of its own in the graph, and an edge into
//! it from `P` weighs the growth of `Item`: the greatest, over the conformances that choose it,
//! of how much deeper the choice nests the types that stand for the conformance's parameters than
//! the conformance's target does. `Wrap<T>`'s `type Item = Wrap<Wrap<T>>` grows by 1, and
//! `type Item = T` by -1. A choice that names a parameter through other associated types adds
//! the growth of each one in the chain, `T.A.B` adding that of `A` and of `B`. The growths are
//! the least fixed point of these sums, and unbounded on a cycle of positive weight among the
//! names, whether or not a direct parameter starts the cycle (see `Checker::growths`). A choice
//! that names no parameter gives a growth that is absent: `P.Item` is then a type that no
//! argument changes, and the argument adds no edge.
//!
//! A field or payload type that holds an associated type `P.Item` holds the type that each
//! conformance chooses for `Item`. Each choice is a declaration of its own, with the conformance's
//! parameters, as a hub is. The base `P` feeds a hub of the choices of `Item` at no cost, and the
//! hub feeds each parameter of each choice at the weight of minus the greatest depth of the
//! parameter in the conformance's target, so that a use and a choice add one edge each, not one
//! for each pair. The type chosen gives the edges that a field type gives. The choice
//! `type Item = E<W<W<T>>>` of `W<T>` gives the enum `E<T: I>` that holds `T.Item` an edge back to
//! itself of weight 1. A use `P.Link.Item` whose base is an associated type means only the
//! choices of `Item` made by the classes and enums that the choices of `Link` name, since `P.Link`
//! is one of those types (`Assocs::heads`). Such a use feeds the gate of each of those choices
//! instead of the hub: a node of its own for each choice, which the hub feeds too, and which feeds
//! the choice's parameters. A use reads at most `MATCHING_CHOICES` heads and choices; above that,
//! or when a choice of `Link` names a parameter, an associated type or any other type, the use
//! feeds the hub. The graph does not follow the head from one turn of a cycle to the next. A
//! link of the base whose associated type has unbounded growth for the
//! arguments of its use adds no weight: the edge is made from the base without that link, and the
//! other links keep their growth. The link may be a cycle of names alone, which instantiates no
//! class or enum, and its weight there would report a program that has its own error. It may also
//! stand for another type of the place, which no conformance with a growing choice stands for; a
//! growth through such a link is left to the query for a conformance, which stops when more than
//! `CONFORMS_DEPTH_LIMIT` class or enum types are open (E0140).
//!
//! An interface with type parameters has choices for some arguments, and a use `P.Item` has
//! arguments too. A choice made for arguments that hold no parameter is not meant by a use with
//! other arguments that hold none (a conformance to `S<bool>` is not one to `S<i64>`), so it gives
//! that use no edge and no growth. Each such list of arguments that a conformance has is a slot of
//! its own, beside one for the lists that hold a parameter and the associated type itself; a slot
//! of closed arguments counts the choices of the slot for the lists with a parameter, which may
//! be meant for any, and the associated type itself counts all of them (`Assocs`).
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
//!
//! The growths of the associated types take part in that exception too: they are found by passes
//! over the choices' names, at most three times the number of associated types and slots plus 1
//! of them, each pass reading every name once.

use std::collections::VecDeque;

use rustc_hash::{FxHashMap, FxHashSet};

use super::{Checker, Lowered};
use crate::ast::angled;
use crate::diagnostic::{DiagnosticKind, Hop, Spelled};
use crate::graph::components;
use crate::source::Span;
use crate::tir::{self, Type, TypeArgs, TypeKind, head_of};
use crate::types::{Head, write_args, write_method};

#[cfg(test)]
thread_local! {
    /// How many edges the last instantiation graph on this thread held.
    pub(super) static EDGES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many edges Bellman–Ford has relaxed on this thread.
    pub(super) static BELLMAN_FORD_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many nodes the search for a cycle has read to gather the nodes of the components.
    pub(super) static GATHER_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many array and optional levels `Graph::core` has taken off a type on this thread.
    pub(super) static CORE_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many edges the graph of the marks of growing declarations held on this thread, 0 when
    /// it was not built.
    pub(super) static MARK_EDGES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The nodes and the edges of the component that the last search for a cycle read.
    pub(super) static SEARCHED: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

/// The weight of an edge into an associated type whose growth is unbounded: more than any path
/// of the graph can take back.
const UNBOUNDED: i64 = 1 << 32;

/// The most heads and choices that one use of an associated type reads to find the choices that it
/// can mean (`Graph::matching`). Above it, the use feeds the hub of all the choices, so the edges
/// of the graph stay in proportion to the program.
const MATCHING_CHOICES: usize = 64;

/// The interface that `instance` calls through, if it calls through one.
fn calls_through(instance: Instance) -> Option<tir::InterfaceId> {
    match instance {
        Instance::Required(interface, ..) | Instance::Operator(interface, _) => Some(interface),
        Instance::Call(..) | Instance::Type(..) => None,
    }
}

/// An instantiation, which a diagnostic writes as the program does.
#[derive(Clone, Copy)]
enum Instance {
    /// A call of the function, with its type arguments, a method's owner's first. With a
    /// `Spelling`, the program wrote an operator and the typed tree lowered it to this call.
    Call(tir::FuncId, TypeArgs, Option<Spelling>),
    /// A generic class or enum, as a type. With a `Spelling`, the program wrote an operator or a
    /// method call that reaches the type's generated node.
    Type(Type, Option<Spelling>),
    /// A call of a requirement of an interface, by the requirement's place, with its own type
    /// arguments.
    Required(tir::InterfaceId, usize, TypeArgs),
    /// A call through a language interface that the program spells as an operator, an
    /// interpolation or a generated `==` or `to_string`, and not as a call of the method.
    Operator(tir::InterfaceId, Written),
}

/// A language interface and how the program spells a call through it.
type Spelling = (tir::InterfaceId, Written);

/// How the program spells a call through a language interface that is not a call of its method.
/// A call of the method through a bound, as `x.eq(y)` writes it, is a `Required` instance.
#[derive(Clone, Copy)]
enum Written {
    /// An operator or built-in function, as written: `==`, `<`, `contains` or `print`.
    Op(&'static str),
    /// A part of an interpolation.
    Interpolation,
    /// The generated `==` or `to_string` of an enum, for the payload type given.
    Generated(Type),
    /// A call of the method, as `x.eq(y)` or `x.to_string()` writes it on a type whose conformance
    /// the compiler writes. The typed tree lowers it to the node of the operator.
    Method,
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
    /// Whether the edge runs out of a hub: to a parameter of a method that meets what it stands
    /// for, or of a choice of an associated type, or to a hub of the choices.
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

/// A choice of an associated type. It is a declaration of its own in the graph, with the
/// conformance's parameters, as a hub is: the base of the associated type feeds them, and the
/// type chosen gives the edges.
struct Choice {
    /// The class or enum that the conformance has as its target, which a diagnostic names.
    head: Head,
    /// The type chosen, and where it is written.
    ty: Type,
    at: Span,
    /// The greatest depth of each parameter of the conformance in its target (`param_depths`).
    depths: Vec<i64>,
    /// The slot of the choice (`Assocs::slot`).
    slot: usize,
    /// Whether the choice is made for interface arguments that hold a parameter: any use of its
    /// place may mean it.
    loose: bool,
}

/// The associated types of the interfaces, as the graph reads them. A place is an associated
/// type of an interface, `first` plus its index. A choice is made for the interface with some
/// arguments, and a use `P.Item` is of the interface with some arguments too. A choice made for
/// arguments that hold no parameter is not meant by a use with other such arguments (a
/// conformance to `S<bool>` is not one to `S<i64>`), so each place has a slot for each such list
/// of arguments that a conformance has, and one for the lists that hold a parameter (`slots`).
/// The slots follow the places in `growth` and `hub_at`.
#[derive(Default)]
struct Assocs {
    /// By interface, where its associated types start in `growth`.
    first: Vec<usize>,
    /// How many places there are: the slots of `made` follow them.
    places: usize,
    /// The growth of each place and slot (`growths`); `None` when absent. A place has the
    /// greatest growth of its slots.
    growth: Vec<Option<i64>>,
    /// By place and the arguments of a conformance's interface if they hold no parameter, its
    /// slot.
    slots: FxHashMap<(usize, Option<TypeArgs>), usize>,
    /// The place and the arguments of each slot, in the order made: slot `places + i` is `made[i]`.
    made: Vec<(usize, Option<TypeArgs>)>,
    /// The choices of the associated types that name a parameter of their conformance.
    choices: Vec<Choice>,
    /// The hubs of the choices: see `Hub`.
    hubs: Vec<Hub>,
    /// By place and slot, the hub that a use feeds, if a choice can be meant.
    hub_at: Vec<Option<usize>>,
    /// By place, the classes and enums that its choices name as the type chosen, or `None` when a
    /// choice names any other type (a parameter, an associated type, an array) or is missing.
    heads: Vec<Option<Vec<Head>>>,
    /// By place and head, the choices that a conformance of the head makes for the place.
    by_head: FxHashMap<(usize, Head), Vec<usize>>,
}

/// A node that stands for a set of choices, or for hubs that do, so that the uses of an
/// associated type and its choices make a sum of edges, not a product: a use feeds the hub, and
/// the hub feeds each choice.
#[derive(Default)]
struct Hub {
    /// The choices that the hub feeds.
    choices: Vec<usize>,
    /// The hubs that the hub feeds, at no cost.
    hubs: Vec<usize>,
}

/// Whether `args`, the arguments of an interface, hold no parameter and no associated type. A use
/// with one such list never means a conformance with another list.
fn closed(args: TypeArgs) -> bool {
    !args.has_assoc() && !args.get().iter().any(|arg| arg.has_param())
}

impl Assocs {
    /// The slot of the uses of the place `place` with the interface arguments `args`: the one
    /// for those arguments if a conformance has them, else the one for arguments with a
    /// parameter, and the place itself when `args` hold a parameter, since any choice may be
    /// meant then. `None` when no choice can be meant.
    fn slot(&self, place: usize, args: TypeArgs) -> Option<usize> {
        if !closed(args) {
            return Some(place);
        }
        let key = |key| self.slots.get(&(place, key)).copied();
        key(Some(args)).or_else(|| key(None))
    }

    /// The growth of the place `place` for the uses with the interface arguments `args`.
    fn growth_of(&self, place: usize, args: TypeArgs) -> Option<i64> {
        self.growth[self.slot(place, args)?]
    }

    /// The hub that the uses of the place `place` with the interface arguments `args` feed.
    fn hub(&self, place: usize, args: TypeArgs) -> Option<usize> {
        self.hub_at.get(self.slot(place, args)?).copied().flatten()
    }
}

struct Graph {
    /// By declaration, its first node: the functions', then the classes', then the enums'; then
    /// the number of parameter nodes.
    first: Vec<usize>,
    functions: usize,
    classes: usize,
    enums: usize,
    /// Where the declaration of the hub of each interface starts: the interface's index added.
    interface_hubs: usize,
    /// The declaration of the generated `Eq` node of the first enum; the others follow, and then
    /// the generated `Display` nodes in the same order.
    eq_first: usize,
    /// The declaration of the generated `Display` node of the first enum.
    display_first: usize,
    /// The interface whose operators go to the generated `Display` nodes.
    display: tir::InterfaceId,
    /// By interface and enum, whether a written conformance has the enum as its target, and
    /// whether one of them has no bound on its parameters.
    records: FxHashMap<(tir::InterfaceId, tir::EnumId), (bool, bool)>,
    edges: Vec<Edge>,
    /// By declaration and type, the node of a type argument that mentions a parameter, after
    /// the parameters' first and second nodes. Its parts' edges into it stand for "mentions": a
    /// parameter reaches the node of each type that mentions it, so an argument needs one edge
    /// however many parameters it mentions, and each type of a declaration is looked at once.
    typed: FxHashMap<(usize, Type), usize>,
    /// By array or optional type, the type with its outer levels taken off (`core`).
    cores: FxHashMap<Type, Type>,
    /// The types `types` has walked, with the site each was written at.
    walked: FxHashSet<(Span, Type)>,
    assocs: Assocs,
    /// The declaration of the first choice; the others follow.
    choices_first: usize,
}

impl Graph {
    /// The number of first nodes, which the second nodes and then the nodes of types follow.
    fn firsts(&self) -> usize {
        self.first[self.first.len() - 1]
    }

    /// Whether a parameter of the declaration `decl` has a second node: all but a hub's.
    fn has_second(&self, decl: usize) -> bool {
        decl < self.functions + self.classes + self.enums || decl >= self.eq_first
    }

    /// The second node of parameter `i` of the declaration `decl`.
    fn second(&self, decl: usize, i: usize) -> usize {
        self.firsts() + self.first[decl] + i
    }

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
            if !self.has_second(target) {
                continue;
            }
            // The second node takes the argument with its outer levels off.
            let core = self.core(arg);
            let from = match core.kind() {
                TypeKind::Param(_, i) => self.second(holder, i as usize),
                _ => self.node(holder, core),
            };
            self.edges.push(Edge {
                from,
                to: self.second(target, q),
                weight: 0,
                growing: false,
                out: false,
                assoc: false,
                holder,
                site,
                mark,
                instance: Some(instance),
            });
        }
    }

    /// `ty` with every outer array and optional level taken off. Each level is read once for the
    /// program, however many sites share the type: the levels read are kept with their core, so
    /// a type that holds a type read before stops at it.
    fn core(&mut self, ty: Type) -> Type {
        let mut levels = Vec::new();
        let mut core = ty;
        while let TypeKind::Array(inner) | TypeKind::Optional(inner) = core.kind() {
            if let Some(&known) = self.cores.get(&core) {
                core = known;
                break;
            }
            #[cfg(test)]
            CORE_STEPS.set(CORE_STEPS.get() + 1);
            levels.push(core);
            core = inner;
        }
        for level in levels {
            self.cores.insert(level, core);
        }
        core
    }

    /// An edge into the hub declaration `hub` from the node `from`, for the operator or call at
    /// `site` inside `holder`: it hands on an operand of the receiver's type.
    fn receive(
        &mut self,
        holder: usize,
        from: usize,
        hub: usize,
        (site, mark): (Span, Span),
        instance: Instance,
    ) {
        self.edges.push(Edge {
            from,
            to: self.first[hub],
            weight: 0,
            growing: false,
            out: false,
            assoc: false,
            holder,
            site,
            mark,
            instance: Some(instance),
        });
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

    /// The edges of an operator of the language interface `interface` on an operand of type `ty`
    /// inside `holder`, at `site`: see the module doc. An operand that names no parameter adds
    /// none. A diagnostic underlines `mark` and spells the call as `written`, or as a call of the
    /// method when there is none.
    fn operator(
        &mut self,
        holder: usize,
        ty: Type,
        interface: tir::InterfaceId,
        (site, mark): (Span, Span),
        written: Option<Written>,
    ) {
        let ty = self.core(ty);
        if !ty.has_param() {
            return;
        }
        let instance = written.map_or(
            Instance::Required(interface, 0, TypeArgs::NONE),
            |written| Instance::Operator(interface, written),
        );
        // A generated node keeps the type as its label: only the program's own spelling replaces
        // it.
        let spelling = match written {
            Some(Written::Generated(_)) => None,
            written => Some((interface, written.unwrap_or(Written::Method))),
        };
        let hub = self.interface_hubs + interface.0;
        match ty.kind() {
            TypeKind::Param(_, i) => {
                let from = self.second(holder, i as usize);
                self.receive(holder, from, hub, (site, mark), instance);
            }
            TypeKind::Enum(id, args) => {
                let (listed, unconditional) = self
                    .records
                    .get(&(interface, id))
                    .copied()
                    .unwrap_or_default();
                if !listed || !unconditional {
                    let first = if interface == self.display {
                        self.display_first
                    } else {
                        self.eq_first
                    };
                    let target = first + id.0;
                    let instance = Instance::Type(ty, spelling);
                    self.args(holder, args.get(), target, site, mark, instance);
                }
                if listed {
                    let from = self.node(holder, ty);
                    self.receive(holder, from, hub, (site, mark), instance);
                }
            }
            _ => {
                let from = self.node(holder, ty);
                self.receive(holder, from, hub, (site, mark), instance);
            }
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
                stack.extend(self.parts(t).into_iter().map(|(part, _)| (part, false)));
                continue;
            }
            let n = 2 * self.firsts() + self.typed.len();
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
                let place = self.assocs.first[interface.0] + k as usize;
                let Some(growth) = self.assocs.growth_of(place, args) else {
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

    /// The edges of the associated type `ty` of `base`, written at `site` inside `holder`, whose
    /// choices the hub `hub` stands for. It is what each choice chooses, with the choice's
    /// parameters standing for parts of the base: the base feeds the choices (`Hub`), which feed
    /// each parameter of the conformance, less the greatest depth of the parameter in the
    /// target, the way a hub feeds the methods of a conformance. The type chosen gives the edges
    /// on from there (`Choice`). The base feeds the gate of each choice that it can mean when
    /// `matching` knows them, else the hub of them all.
    fn choose(&mut self, holder: usize, ty: Type, base: Type, hub: usize, site: Span) {
        let targets: Vec<usize> = match self.matching(ty, base) {
            Some(choices) => (choices.iter())
                .map(|&c| self.first[self.gates_first() + c])
                .collect(),
            None => vec![self.first[self.hubs_first() + hub]],
        };
        if targets.is_empty() {
            return;
        }
        let from = self.node(holder, self.without_unbounded(base));
        for to in targets {
            self.edges.push(Edge {
                from,
                to,
                weight: 0,
                growing: false,
                out: false,
                assoc: false,
                holder,
                site,
                mark: site,
                instance: Some(Instance::Type(ty, None)),
            });
        }
    }

    /// The choices of the associated type `ty` that a use with the base `base` can mean, when the
    /// base is itself an associated type `P.Link`: `P.Link` is a type that a choice of `Link`
    /// names, so a choice of `ty` is meant only if a conformance of that type's class or enum
    /// makes it. `None` when the base is no associated type, when a choice of `Link` names any
    /// other type, or when more than `MATCHING_CHOICES` heads and choices must be read: the use
    /// then feeds the hub of all the choices.
    fn matching(&self, ty: Type, base: Type) -> Option<Vec<usize>> {
        let (TypeKind::Assoc(_, interface, args, k), TypeKind::Assoc(_, link, _, l)) =
            (ty.kind(), base.kind())
        else {
            return None;
        };
        let place = self.assocs.first[interface.0] + k as usize;
        let slot = (self.assocs.slot(place, args))
            .expect("a use whose place has a hub should have a slot");
        let heads = self.assocs.heads[self.assocs.first[link.0] + l as usize].as_ref()?;
        // One read for each head, and one for each choice that a head makes.
        let mut read = heads.len();
        let mut found = Vec::new();
        for &head in heads {
            let made = (self.assocs.by_head.get(&(place, head))).map_or(&[][..], Vec::as_slice);
            read += made.len();
            // shortcut: above K matching choices a use feeds the whole hub;
            // raise K if a real program hits it.
            if read > MATCHING_CHOICES {
                return None;
            }
            // An open use means any choice of the place, a closed one the choices of its slot
            // and those for arguments that hold a parameter.
            found.extend(made.iter().filter(|&&c| {
                let choice = &self.assocs.choices[c];
                slot == place || choice.slot == slot || choice.loose
            }));
        }
        Some(found)
    }

    /// `base` without the links of its chain that have unbounded growth for the arguments of
    /// their use, which pass their base at no cost; the type parameter at the end of the chain
    /// stays. Such a link may be a cycle of names alone, which instantiates no class or enum
    /// (`normalize` ends it where the type is used), so the weight of its growth would stand for
    /// no instance. The other links keep their growth. The link may also stand for another type
    /// of the place than the one that grows; a growth through that type that the graph misses is
    /// left to the query for a conformance, which stops when more than `CONFORMS_DEPTH_LIMIT`
    /// class or enum types are open (E0140).
    fn without_unbounded(&self, base: Type) -> Type {
        let mut kept = Vec::new();
        let mut inner = base;
        while let TypeKind::Assoc(next, interface, args, k) = inner.kind() {
            let place = self.assocs.first[interface.0] + k as usize;
            if self.assocs.growth_of(place, args) != Some(UNBOUNDED) {
                kept.push((interface, args, k));
            }
            inner = next;
        }
        for &(interface, args, k) in kept.iter().rev() {
            inner = Type::intern(TypeKind::Assoc(inner, interface, args, k));
        }
        inner
    }

    /// The declaration that the headline of the cycle `cycle` names, whose edge `label` is the
    /// label. The label of a choice stands at the type that the choice names, whose holder is the
    /// choice. The cycle holds the declaration that holds the associated type: it is the holder of
    /// the last edge before the label that is no part of a choice.
    fn headline(&self, cycle: &[usize], label: usize) -> usize {
        let holder = self.edges[label].holder;
        if holder < self.choices_first {
            return holder;
        }
        (cycle.iter().rev())
            .map(|&i| self.edges[i].holder)
            .find(|&holder| holder < self.choices_first)
            .unwrap_or(holder)
    }

    /// The declaration of the first hub of the choices; the others follow the choices.
    fn hubs_first(&self) -> usize {
        self.choices_first + self.assocs.choices.len()
    }

    /// The declaration of the gate of the first choice; the others follow the hubs of the choices.
    fn gates_first(&self) -> usize {
        self.hubs_first() + self.assocs.hubs.len()
    }

    /// The edges out of each hub of the choices: to the gate of each choice it stands for, at no
    /// cost, and to the hubs it reaches; and out of each gate: to each parameter of its choice,
    /// minus the greatest depth of the parameter in the conformance's target. A use that can mean
    /// only some choices of a hub feeds their gates (`Graph::choose`).
    fn feed(&mut self) {
        for (h, hub) in self.assocs.hubs.iter().enumerate() {
            let from = self.first[self.hubs_first() + h];
            for &c in &hub.choices {
                let to = self.first[self.gates_first() + c];
                self.edges
                    .push(Self::hub_edge(from, to, 0, self.choices_first + c));
            }
            for &other in &hub.hubs {
                let to = self.first[self.hubs_first() + other];
                self.edges
                    .push(Self::hub_edge(from, to, 0, self.hubs_first() + other));
            }
        }
        for (c, choice) in self.assocs.choices.iter().enumerate() {
            let from = self.first[self.gates_first() + c];
            let first = self.first[self.choices_first + c];
            for (q, depth) in choice.depths.iter().enumerate() {
                self.edges.push(Self::hub_edge(
                    from,
                    first + q,
                    -depth,
                    self.choices_first + c,
                ));
            }
        }
    }

    /// An edge out of a hub of the choices, which no instantiation writes.
    fn hub_edge(from: usize, to: usize, weight: i64, holder: usize) -> Edge {
        Edge {
            from,
            to,
            weight,
            growing: false,
            out: true,
            assoc: false,
            holder,
            site: Span::new(0, 0),
            mark: Span::new(0, 0),
            instance: None,
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
                TypeKind::Assoc(base, interface, args, k) => {
                    let place = self.assocs.first[interface.0] + k as usize;
                    if let Some(hub) = self.assocs.hub(place, args) {
                        self.choose(holder, ty, base, hub, site);
                    }
                    stack.push(base);
                    stack.extend(args.get());
                    continue;
                }
                _ => continue,
            };
            let mark = spans.get(&ty).copied().unwrap_or(site);
            self.args(
                holder,
                args.get(),
                target,
                site,
                mark,
                Instance::Type(ty, None),
            );
            stack.extend(args.get());
        }
    }
}

/// The strongly connected components of an instantiation graph, and what each holds.
struct Strands {
    /// By node, the edges that start at it.
    out: Vec<Vec<usize>>,
    /// By node, its component.
    component: Vec<usize>,
    /// By component, whether an edge of negative weight is in it.
    negative: Vec<bool>,
    /// By component, its first growing edge in source order.
    growing: Vec<Option<usize>>,
    /// By component with a negative weight, its nodes, gathered in one pass.
    members: Vec<Vec<usize>>,
}

impl Strands {
    fn of(graph: &Graph) -> Strands {
        let nodes = 2 * graph.firsts() + graph.typed.len();
        let mut out = vec![Vec::new(); nodes];
        for (i, edge) in graph.edges.iter().enumerate() {
            out[edge.from].push(i);
        }
        let targets: Vec<Vec<usize>> = out
            .iter()
            .map(|edges| edges.iter().map(|&i| graph.edges[i].to).collect())
            .collect();
        let component = components(&targets);
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
        // The nodes of each component with a negative weight, gathered in one pass.
        let mut members: Vec<Vec<usize>> = vec![Vec::new(); nodes];
        for (n, &k) in component.iter().enumerate() {
            #[cfg(test)]
            GATHER_STEPS.set(GATHER_STEPS.get() + 1);
            if negative[k] {
                members[k].push(n);
            }
        }
        Strands {
            out,
            component,
            negative,
            growing,
            members,
        }
    }

    /// Whether the component `k` holds a cycle of positive weight: a growing edge when no weight
    /// is negative, else the search that `instantiation_cycles` makes.
    fn grows(&self, graph: &Graph, k: usize) -> bool {
        if self.negative[k] {
            positive_cycle(graph, &self.out, &self.component, k, &self.members[k]).is_some()
        } else {
            self.growing[k].is_some()
        }
    }
}

/// The declarations that the types `fields` name, as nodes (the classes, then the enums, then the
/// choices of `assocs`, then its hubs), and among them those that a type gives an argument which
/// mentions a parameter and is not one. An associated type names the hub of its choices (the nodes
/// after the choices), and gives it a changed argument when its base is not a parameter itself:
/// the base is a type of its own then, which nests the parameter at the growth of the associated
/// types in it.
fn named(
    stack: &mut Vec<Type>,
    (classes, enums): (usize, usize),
    assocs: &Assocs,
    fields: impl Iterator<Item = Type>,
) -> (Vec<usize>, Vec<usize>) {
    let (mut names, mut changed) = (Vec::new(), Vec::new());
    let declarations = classes + enums;
    stack.extend(fields);
    while let Some(ty) = stack.pop() {
        let (node, args) = match ty.kind() {
            TypeKind::Array(inner) | TypeKind::Optional(inner) => {
                stack.push(inner);
                continue;
            }
            TypeKind::Class(id, args) => (id.0, args),
            TypeKind::Enum(id, args) => (classes + id.0, args),
            TypeKind::Assoc(base, interface, args, k) => {
                let place = assocs.first[interface.0] + k as usize;
                if let Some(hub) = assocs.hub(place, args) {
                    let hub = declarations + assocs.choices.len() + hub;
                    names.push(hub);
                    if !matches!(base.kind(), TypeKind::Param(..)) {
                        changed.push(hub);
                    }
                }
                stack.push(base);
                stack.extend(args.get());
                continue;
            }
            _ => continue,
        };
        names.push(node);
        let args = args.get();
        if args
            .iter()
            .any(|arg| arg.has_param() && !matches!(arg.kind(), TypeKind::Param(..)))
        {
            changed.push(node);
        }
        stack.extend(args);
    }
    (names, changed)
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
        let Strands {
            out,
            component,
            negative,
            growing,
            members,
        } = Strands::of(&graph);
        // Components that grow at one site, as `f([x], [y])` does for each parameter, are
        // reported once.
        let mut reported = FxHashSet::default();
        let mut cycles = Vec::new();
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
        let label_instance = edge
            .instance
            .expect("a labeled edge should be an instantiation");
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
        let choice = self.choice_use(edges, &cycle);
        let label_spelled = self.spell(functions, graph, edge.holder, label_instance);
        // A call that gives two parameters their arguments is one site, shown once.
        let mut shown = FxHashSet::from_iter([edge.site]);
        if let Some(interface) = calls_through(label_instance) {
            reach = Some((interface, label_spelled.clone()));
        }
        // A call through a hub, until the edge out of the hub says which method it may run.
        let mut calling: Option<(Span, tir::InterfaceId, Option<Spelled>)> = None;
        for &i in &cycle[1..] {
            let e = &edges[i];
            if e.out {
                if let Some((mark, interface, by)) = calling.take() {
                    through.push((
                        mark,
                        Hop::Calls {
                            by,
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
            if let Some(interface) = calls_through(instance) {
                let spelled = self.spell(functions, graph, e.holder, instance);
                let by = matches!(instance, Instance::Operator(..)).then(|| spelled.clone());
                reach.get_or_insert((interface, spelled));
                calling = Some((e.mark, interface, by));
            } else {
                through.push((
                    e.mark,
                    Hop::Instantiates(self.instance_name(functions, instance)),
                ));
            }
        }
        let holder = graph.headline(&cycle, label);
        self.report(DiagnosticKind::InstantiationCycle {
            name: self.node_owner_name(graph, holder),
            at: edge.mark,
            instance: label_spelled,
            through,
            reach: reach
                .map(|(interface, by)| (self.interfaces[interface.0].name.name.to_string(), by)),
            choice,
        });
    }

    /// The first use of an associated type on the cycle, as written: its edge runs into the
    /// choices.
    fn choice_use(&self, edges: &[Edge], cycle: &[usize]) -> Option<String> {
        (cycle.iter())
            .find_map(|&i| match edges[i].instance {
                Some(Instance::Type(ty, _)) if matches!(ty.kind(), TypeKind::Assoc(..)) => Some(ty),
                _ => None,
            })
            .map(|ty| self.type_name(ty))
    }

    /// `instance`, at an edge of the declaration `holder`, as the program spells it.
    fn spell(
        &self,
        functions: &[Option<tir::Function>],
        graph: &Graph,
        holder: usize,
        instance: Instance,
    ) -> Spelled {
        let spelling = match instance {
            Instance::Operator(interface, written) => Some((interface, written)),
            Instance::Call(.., spelling) | Instance::Type(_, spelling) => spelling,
            Instance::Required(..) => None,
        };
        let Some((interface, written)) = spelling else {
            return Spelled::Instance(self.instance_name(functions, instance));
        };
        match written {
            Written::Method => Spelled::Instance(
                self.instance_name(functions, Instance::Required(interface, 0, TypeArgs::NONE)),
            ),
            Written::Op(op) => Spelled::Operator(op.to_string()),
            Written::Interpolation => Spelled::Interpolation,
            Written::Generated(operand) => Spelled::Generated {
                display: interface == self.lang.display,
                enum_name: self.node_owner_name(graph, holder),
                operand: self.type_name(operand),
            },
        }
    }

    /// The name of the declaration `holder` of the graph, which a diagnostic gives. A generated
    /// node stands for its enum, and a choice for the target of its conformance.
    fn node_owner_name(&self, graph: &Graph, holder: usize) -> String {
        match holder.checked_sub(graph.functions) {
            _ if holder >= graph.choices_first => self
                .head(graph.assocs.choices[holder - graph.choices_first].head)
                .to_string(),
            _ if holder >= graph.display_first => self.enums[holder - graph.display_first]
                .name
                .name
                .to_string(),
            _ if holder >= graph.eq_first => {
                self.enums[holder - graph.eq_first].name.name.to_string()
            }
            None => self.signatures[holder].name.clone(),
            Some(c) if c < self.classes.len() => self.classes[c].name.name.to_string(),
            Some(c) => self.enums[c - self.classes.len()].name.name.to_string(),
        }
    }

    /// The graph of the program's declarations with no edges yet: the nodes of every function,
    /// class and enum, of each hub, and the associated types' growths. Also the hubs: of each own
    /// type parameter of a requirement, by interface, requirement and parameter, and where those
    /// of the interfaces start.
    fn graph_frame(&self) -> (Graph, FxHashMap<(usize, usize, usize), usize>, usize) {
        // Each own type parameter of a requirement is a hub, a node of a declaration of its own:
        // every call of the requirement passes its argument to the hub, and the hub passes it to
        // the same parameter of each method that meets the requirement. So is each interface, for
        // the receiver of a call through a bound.
        let assocs = self.assocs(self.holds_assoc);
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
        // The hubs of the interfaces follow, one each, and then the generated `Eq` node of each
        // enum, which has the enum's parameters, and its generated `Display` node.
        let interface_hubs = declarations;
        let eq_first = interface_hubs + self.interfaces.len();
        let display_first = eq_first + self.enums.len();
        let counts = self.signatures.iter().map(|s| s.type_params.len());
        let counts = counts.chain(self.classes.iter().map(|c| c.type_params.len()));
        let counts = counts.chain(self.enums.iter().map(|e| e.type_params.len()));
        let counts = counts.chain(std::iter::repeat_n(1, hubs.len() + self.interfaces.len()));
        let counts = counts.chain(self.enums.iter().map(|e| e.type_params.len()));
        let counts = counts.chain(self.enums.iter().map(|e| e.type_params.len()));
        let counts = counts.chain(assocs.choices.iter().map(|c| c.depths.len()));
        let counts = counts.chain(assocs.hubs.iter().map(|_| 1));
        let counts = counts.chain(assocs.choices.iter().map(|_| 1));
        let mut first = vec![0];
        for n in counts {
            first.push(first[first.len() - 1] + n);
        }
        let declared = self.signatures.len();
        let graph = Graph {
            first,
            functions: declared,
            classes: self.classes.len(),
            enums: self.enums.len(),
            interface_hubs,
            eq_first,
            display_first,
            display: self.lang.display,
            records: FxHashMap::default(),
            edges: Vec::new(),
            typed: FxHashMap::default(),
            cores: FxHashMap::default(),
            walked: FxHashSet::default(),
            assocs,
            choices_first: display_first + self.enums.len(),
        };
        (graph, hubs, interface_hubs)
    }

    /// A graph of the classes, the enums and the choices of `assocs` alone: no function, hub or
    /// generated node, so its size follows the declarations that `declaration_edges` reads.
    fn declaration_frame(&self, assocs: Assocs) -> Graph {
        let counts = (self.classes.iter().map(|c| c.type_params.len()))
            .chain(self.enums.iter().map(|e| e.type_params.len()))
            .chain(assocs.choices.iter().map(|c| c.depths.len()))
            .chain(assocs.hubs.iter().map(|_| 1))
            .chain(assocs.choices.iter().map(|_| 1));
        let mut first = vec![0];
        for n in counts {
            first.push(first[first.len() - 1] + n);
        }
        let end = self.classes.len() + self.enums.len();
        Graph {
            first,
            functions: 0,
            classes: self.classes.len(),
            enums: self.enums.len(),
            interface_hubs: end,
            eq_first: end,
            display_first: end,
            display: self.lang.display,
            records: FxHashMap::default(),
            edges: Vec::new(),
            typed: FxHashMap::default(),
            cores: FxHashMap::default(),
            walked: FxHashSet::default(),
            assocs,
            choices_first: end,
        }
    }

    /// The instantiation graph of the program: see `instantiation_cycles`.
    fn instantiation_graph(&self, functions: &[Option<tir::Function>], unbuilt: &[bool]) -> Graph {
        let (mut graph, hubs, interface_hubs) = self.graph_frame();
        graph.records = self.written_records();
        let mut exprs = Vec::new();
        for (holder, function) in functions.iter().enumerate() {
            let Some(function) = function.as_ref().filter(|f| !f.type_params.is_empty()) else {
                continue;
            };
            exprs.clear();
            function.body.exprs(&mut exprs);
            for e in &exprs {
                match e.kind {
                    tir::ExprKind::Call { func, types, .. } => {
                        // An operator that the checker lowered to a call is labeled as written.
                        let spelling = match self.spellings.get(&e.span) {
                            Some(&Lowered::Operator(interface, op)) => {
                                Some((interface, Written::Op(op)))
                            }
                            _ => None,
                        };
                        let instance = Instance::Call(func, types, spelling);
                        graph.args(holder, types.get(), func.0, e.span, e.span, instance);
                    }
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
                        // its arguments. A language interface's receiver may stand for an array
                        // or an optional, which its generated conformance takes off first.
                        let hub = interface_hubs + interface.0;
                        if [self.lang.eq, self.lang.ord, self.lang.display].contains(&interface) {
                            graph.operator(
                                holder,
                                iargs.get()[0],
                                interface,
                                (e.span, e.span),
                                None,
                            );
                        } else {
                            graph.args(holder, &iargs.get()[..1], hub, e.span, e.span, instance);
                        }
                        for (j, arg) in types.get().iter().enumerate() {
                            let hub = hubs[&(interface.0, method, j)];
                            graph.args(holder, &[*arg], hub, e.span, e.span, instance);
                        }
                    }
                    _ => self.operator_edges(&mut graph, holder, e),
                }
            }
        }
        self.hub_edges(&mut graph, &hubs, interface_hubs);
        self.generated_edges(&mut graph);
        self.declaration_edges(&mut graph, unbuilt);
        graph
    }

    /// The edges of the operators, `print` and interpolations in the expression `e` inside the
    /// function `holder`: see the module doc.
    fn operator_edges(&self, graph: &mut Graph, holder: usize, e: &tir::Expr) {
        // A method call that the checker lowered to the node of an operator is spelled as the
        // call: `Eq.eq` or `Display.to_string`, over the whole call.
        let method = self.spellings.get(&e.span) == Some(&Lowered::Method);
        let written = |written| (!method).then_some(written);
        match e.kind {
            // `x == None` tests presence and calls nothing.
            tir::ExprKind::Equal {
                ref lhs,
                ref rhs,
                negated,
            } if !matches!(lhs.kind, tir::ExprKind::None)
                && !matches!(rhs.kind, tir::ExprKind::None) =>
            {
                graph.operator(
                    holder,
                    lhs.ty,
                    self.lang.eq,
                    (e.span, e.span),
                    written(Written::Op(if negated { "!=" } else { "==" })),
                );
            }
            tir::ExprKind::Compare {
                ref operands,
                ref ops,
            } => {
                graph.operator(
                    holder,
                    operands[0].ty,
                    self.lang.ord,
                    (e.span, e.span),
                    written(Written::Op(ops[0].text())),
                );
            }
            tir::ExprKind::Print {
                ref operand,
                stream,
            } => {
                let name = match stream {
                    tir::Stream::Stdout => "print",
                    tir::Stream::Stderr => "eprint",
                };
                graph.operator(
                    holder,
                    operand.ty,
                    self.lang.display,
                    (e.span, e.span),
                    Some(Written::Op(name)),
                );
            }
            tir::ExprKind::Interpolate { ref parts, .. } => {
                for part in parts {
                    let mark = if method { e.span } else { part.span };
                    graph.operator(
                        holder,
                        part.ty,
                        self.lang.display,
                        (e.span, mark),
                        written(Written::Interpolation),
                    );
                }
            }
            tir::ExprKind::Contains { ref base, .. } => {
                let element = (base.ty.array_element()).expect("`contains` should take an array");
                graph.operator(
                    holder,
                    element,
                    self.lang.eq,
                    (e.span, e.span),
                    Some(Written::Op("contains")),
                );
            }
            _ => {}
        }
    }

    /// By interface and enum, whether a written conformance targets the enum, and whether one of
    /// them has no bound: one pass over the conformances.
    pub(super) fn written_records(
        &self,
    ) -> FxHashMap<(tir::InterfaceId, tir::EnumId), (bool, bool)> {
        let mut records: FxHashMap<_, (bool, bool)> = FxHashMap::default();
        for (interface, conformance) in self.conformances.iter() {
            if let TypeKind::Enum(id, _) = conformance.target.kind() {
                let entry = records.entry((interface, id)).or_default();
                entry.0 = true;
                entry.1 |= conformance.bounds.iter().all(Vec::is_empty);
            }
        }
        records
    }

    /// The edges of the generated `Eq` and `Display` nodes of each enum: one for each payload
    /// type, chosen as an operand is. An enum that grows, or that no value can be built of, has
    /// none, as its declaration has none: its own error stands for its cycle.
    fn generated_edges(&self, graph: &mut Graph) {
        for (e, info) in self.enums.iter().enumerate() {
            if self.marked[e] {
                continue;
            }
            for (holder, interface) in [
                (graph.eq_first + e, self.lang.eq),
                (graph.display_first + e, self.lang.display),
            ] {
                for variant in &info.variants {
                    for (ty, &span) in variant.fields.iter().zip(&variant.spans) {
                        if let Some(ty) = *ty {
                            graph.operator(
                                holder,
                                ty,
                                interface,
                                (span, span),
                                Some(Written::Generated(ty)),
                            );
                        }
                    }
                }
            }
        }
    }

    /// By class and then enum, whether the declaration is on a cycle that grows its type
    /// arguments (E0140): a declaration with a parameter in a component of the declarations'
    /// graph that holds a cycle of positive weight (`Strands::grows`). The graph is the
    /// declarations' alone, `declaration_edges` over `Strands`, with no hub. It needs the choices
    /// of the conformances for the growth of an associated type, so it runs once they are
    /// recorded, before any query. With no associated type, no weight is negative. Only the
    /// declarations that `growable_declarations` names give edges, and the graph is not built
    /// when no declaration with a parameter is one of them.
    pub(super) fn growing_declarations(&self) -> Vec<bool> {
        let assocs = if self.holds_assoc {
            self.assocs(true)
        } else {
            Assocs::default()
        };
        let growable = self.growable_declarations(&assocs);
        let counts: Vec<usize> = (self.classes.iter().map(|c| c.type_params.len()))
            .chain(self.enums.iter().map(|e| e.type_params.len()))
            .collect();
        if !(counts.iter().zip(&growable)).any(|(&n, &on)| on && n > 0) {
            return vec![false; counts.len()];
        }
        let mut graph = self.declaration_frame(assocs);
        let off: Vec<bool> = growable.iter().map(|&on| !on).collect();
        self.declaration_edges(&mut graph, &off);
        #[cfg(test)]
        MARK_EDGES.set(graph.edges.len());
        let strands = Strands::of(&graph);
        // Each component is searched once, whatever number of declarations its nodes belong to.
        let mut decided: FxHashMap<usize, bool> = FxHashMap::default();
        (counts.iter().enumerate())
            .map(|(d, &n)| {
                let first = graph.first[graph.functions + d];
                (first..first + n).any(|node| {
                    let k = strands.component[node];
                    *decided.entry(k).or_insert_with(|| strands.grows(&graph, k))
                })
            })
            .collect()
    }

    /// By class and then enum, whether the declaration is on a cycle of declarations that name
    /// each other in a field or a payload type (at any depth of an argument, array or optional)
    /// in which a type gives an argument that mentions a parameter and is not one: a type
    /// argument can grow along such a cycle only. A cycle that passes each parameter on as it is
    /// has weight 0. An associated type names the type of each choice of it, a declaration of its
    /// own, which names in turn. Each field and payload type is read once, with its arguments.
    fn growable_declarations(&self, assocs: &Assocs) -> Vec<bool> {
        let mut stack = Vec::new();
        let declared = (self.classes.len(), self.enums.len());
        let nodes =
            self.classes.len() + self.enums.len() + assocs.choices.len() + assocs.hubs.len();
        let mut names: Vec<Vec<usize>> = Vec::with_capacity(nodes);
        let mut changed: Vec<Vec<usize>> = Vec::with_capacity(nodes);
        let mut read = |(named, passed): (Vec<usize>, Vec<usize>)| {
            names.push(named);
            changed.push(passed);
        };
        for class in &self.classes {
            read(named(
                &mut stack,
                declared,
                assocs,
                class.fields.iter().filter_map(|f| f.ty),
            ));
        }
        for info in &self.enums {
            let payloads = (info.variants.iter()).flat_map(|v| v.fields.iter().copied().flatten());
            read(named(&mut stack, declared, assocs, payloads));
        }
        for choice in &assocs.choices {
            read(named(
                &mut stack,
                declared,
                assocs,
                std::iter::once(choice.ty),
            ));
        }
        let after = declared.0 + declared.1 + assocs.choices.len();
        for hub in &assocs.hubs {
            let fed = (hub.choices.iter())
                .map(|&c| declared.0 + declared.1 + c)
                .chain(hub.hubs.iter().map(|&h| after + h));
            read((fed.collect(), Vec::new()));
        }
        let component = components(&names);
        let mut growable = vec![false; names.len()];
        for (from, passed) in changed.iter().enumerate() {
            for &to in passed
                .iter()
                .filter(|&&to| component[to] == component[from])
            {
                growable[component[to]] = true;
            }
        }
        (component.iter().take(declared.0 + declared.1))
            .map(|&k| growable[k])
            .collect()
    }

    /// The edges that the field types of each class and the payload types of each enum give,
    /// except for a declaration in `unbuilt`, by class and then enum, whose own error stands for
    /// its cycle, or that `growable_declarations` leaves out (`growing_declarations`); and those
    /// that the type of each choice gives. They read the declarations alone, so the marks of
    /// growing declarations use them with no function.
    fn declaration_edges(&self, graph: &mut Graph, unbuilt: &[bool]) {
        let declared = graph.functions;
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
        for c in 0..graph.assocs.choices.len() {
            let (ty, at) = (graph.assocs.choices[c].ty, graph.assocs.choices[c].at);
            graph.types(graph.choices_first + c, ty, at, &[]);
        }
        graph.feed();
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

    /// Whether the type of a field of a class or a payload of an enum holds an associated type.
    pub(super) fn field_assoc(&self) -> bool {
        let fields = (self.classes.iter()).flat_map(|c| c.fields.iter().filter_map(|f| f.ty));
        let payloads = (self.enums.iter()).flat_map(|e| {
            e.variants
                .iter()
                .flat_map(|v| v.fields.iter().copied().flatten())
        });
        fields.chain(payloads).any(Type::has_assoc)
    }

    /// The associated types of the interfaces: their growths and, with `choices`, the choices that
    /// name a parameter of their conformance, which are placed as the growths are.
    fn assocs(&self, choices: bool) -> Assocs {
        let mut assocs = self.growths();
        assocs.hub_at = vec![None; assocs.growth.len()];
        if !choices || assocs.growth.is_empty() {
            return assocs;
        }
        // By place, the hubs of its slots that hold a choice, for the hub of the place to reach.
        let mut own: Vec<Vec<usize>> = vec![Vec::new(); assocs.places];
        assocs.heads = vec![Some(Vec::new()); assocs.places];
        // The place of each conformance among those of its interface.
        let mut seen = vec![0; self.interfaces.len()];
        for (interface, conformance) in self.conformances.iter() {
            let index = seen[interface.0];
            seen[interface.0] += 1;
            let first = assocs.first[interface.0];
            for k in 0..self.interfaces[interface.0].assoc.len() {
                let place = first + k;
                // A choice that the conformance does not make leaves the heads of the place
                // unknown.
                let chosen = conformance.assoc.get(k).copied().flatten();
                match chosen.and_then(head_of) {
                    Some(head) => {
                        if let Some(heads) = &mut assocs.heads[place] {
                            heads.push(head);
                        }
                    }
                    None => assocs.heads[place] = None,
                }
                // A choice that names no parameter passes nothing on: no argument changes it.
                let Some(ty) = chosen.filter(|ty| ty.has_param()) else {
                    continue;
                };
                let key = closed(conformance.args).then_some(conformance.args);
                let slot = assocs.slots[&(place, key)];
                let hub = *assocs.hub_at[slot].get_or_insert_with(|| {
                    assocs.hubs.push(Hub::default());
                    own[place].push(assocs.hubs.len() - 1);
                    assocs.hubs.len() - 1
                });
                let head = head_of(conformance.target)
                    .expect("the target of a conformance should be a class or an enum type");
                assocs.hubs[hub].choices.push(assocs.choices.len());
                assocs
                    .by_head
                    .entry((place, head))
                    .or_default()
                    .push(assocs.choices.len());
                assocs.choices.push(Choice {
                    head,
                    ty,
                    at: self.choice_ats[&(interface, index, k)],
                    depths: param_depths(conformance.target, conformance.type_params),
                    slot,
                    loose: key.is_none(),
                });
            }
        }
        // A head that many conformances name is one head.
        for heads in assocs.heads.iter_mut().flatten() {
            let mut known = FxHashSet::default();
            heads.retain(|&head| known.insert(head));
        }
        for i in 0..assocs.made.len() {
            let (place, key) = assocs.made[i];
            let slot = assocs.places + i;
            let loose = (assocs.slots.get(&(place, None))).and_then(|&loose| assocs.hub_at[loose]);
            // A slot of closed arguments also reaches the choices made for arguments that hold a
            // parameter, which may be meant for any.
            if let (Some(hub), Some(_), Some(loose)) = (assocs.hub_at[slot], key, loose) {
                assocs.hubs[hub].hubs.push(loose);
            }
        }
        // A use with a parameter in its interface arguments may mean any choice of the place.
        for (place, hubs) in own.into_iter().enumerate() {
            assocs.hub_at[place] = match hubs[..] {
                [] => None,
                [only] => Some(only),
                _ => {
                    assocs.hubs.push(Hub {
                        choices: Vec::new(),
                        hubs,
                    });
                    Some(assocs.hubs.len() - 1)
                }
            };
        }
        assocs
    }

    /// Where the associated types of each interface start in the second list, and the growth of
    /// each (see the module doc), `None` when absent. The choice of the associated type `a` names
    /// a parameter of its conformance at some depth, which gives `a` a growth outright, and names
    /// `Q.b.c` at some depth, which gives `a` the growth of `b` and of `c` (each step of the
    /// chain) less the depth of `Q` in the target, and plus the depth of the name. The growth is
    /// the greatest of these over the choices: a fixed point, since a choice may name `a` itself.
    ///
    /// One that grows without end is `UNBOUNDED`, and so is every growth that names it. A cycle
    /// of positive weight among the names grows without end whether or not an outright growth
    /// starts it: `type A = W<W<T.A>>` names a type that is built by the same choice, so each
    /// base that it applies to is deeper than the last. A name that is absent counts as 0 in
    /// that search, which finds such a cycle and nothing else (`cyclic`).
    fn growths(&self) -> Assocs {
        let mut first = Vec::with_capacity(self.interfaces.len());
        let mut count = 0;
        for info in &self.interfaces {
            first.push(count);
            count += info.assoc.len();
        }
        let mut assocs = Assocs {
            first,
            places: count,
            growth: vec![None; count],
            ..Assocs::default()
        };
        if count == 0 {
            return assocs;
        }
        let growth = &mut assocs.growth;
        // The associated types a choice names through a parameter, by the type that chooses.
        let mut rules: Vec<Rule> = Vec::new();
        for (interface, conformance) in self.conformances.iter() {
            let target = param_depths(conformance.target, conformance.type_params);
            for (k, chosen) in conformance.assoc.iter().enumerate() {
                // A choice that names no parameter has no growth, and no slot of its own: a use
                // of its arguments falls back to the slot for arguments with a parameter.
                let Some(chosen) = chosen.filter(|ty| ty.has_param()) else {
                    continue;
                };
                let place = assocs.first[interface.0] + k;
                let key = closed(conformance.args).then_some(conformance.args);
                let slot = *assocs.slots.entry((place, key)).or_insert_with(|| {
                    assocs.made.push((place, key));
                    growth.push(None);
                    growth.len() - 1
                });
                let (depths, uses) = depths_and_assoc(chosen, conformance.type_params);
                for (q, depth) in depths.iter().enumerate() {
                    let outright = depth.map(|depth| depth - target[q]);
                    growth[slot] = growth[slot].max(outright);
                }
                for used in uses {
                    rules.push(Rule {
                        at: slot,
                        names: (used.chain.iter())
                            .map(|&(interface, index)| assocs.first[interface.0] + index)
                            .collect(),
                        weight: used.depth - target[used.param],
                    });
                }
            }
        }
        // A place has the greatest growth of its slots, and a slot of closed arguments at least
        // that of the slot of the arguments that hold a parameter.
        for (i, &(place, key)) in assocs.made.iter().enumerate() {
            let slot = count + i;
            rules.push(Rule {
                at: place,
                names: vec![slot],
                weight: 0,
            });
            if let (Some(_), Some(&loose)) = (key, assocs.slots.get(&(place, None))) {
                rules.push(Rule {
                    at: slot,
                    names: vec![loose],
                    weight: 0,
                });
            }
        }
        if rules.is_empty() {
            return assocs;
        }
        let growth = &mut assocs.growth;
        let unbounded = cyclic(growth, &rules);
        for (g, _) in growth.iter_mut().zip(&unbounded).filter(|&(_, &u)| u) {
            *g = Some(UNBOUNDED);
        }
        // A rule passes a growth on from a slot to its place, and from a name to a slot: at most
        // two steps for each place along a path.
        for _ in 0..=growth.len() {
            if relax(growth, &rules, &unbounded).is_empty() {
                break;
            }
        }
        debug_assert!(
            relax(growth, &rules, &unbounded).is_empty(),
            "a growth that no cycle of positive weight reaches should settle in as many passes as there are associated types and slots"
        );
        assocs
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
            Instance::Type(ty, _) => return self.type_name(ty),
            Instance::Required(interface, method, types) => {
                let info = &self.interfaces[interface.0];
                let name = info.requirements[method].name.name;
                let mut out = format!("{}.{name}", info.name.name);
                write_args(&mut out, types.get(), |head| self.head(head));
                return out;
            }
            Instance::Operator(..) => unreachable!("an operator should be spelled by `spell`"),
            Instance::Call(func, types, _) => (func, types),
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
/// the associated types written after it from the first (`Q.b.c` is `b`, then `c`), each by its
/// interface and place, and the depth at which the type stands.
struct AssocUse {
    param: usize,
    chain: Vec<(tir::InterfaceId, usize)>,
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
            TypeKind::Assoc(..) => {
                let (mut root, mut chain) = (ty, Vec::new());
                while let TypeKind::Assoc(inner, interface, args, k) = root.kind() {
                    chain.push((interface, k as usize));
                    stack.extend(args.get().iter().map(|&a| (a, depth)));
                    root = inner;
                }
                let TypeKind::Param(_, q) = root.kind() else {
                    unreachable!(
                        "an associated type of a type parameter should be written through one"
                    );
                };
                chain.reverse();
                uses.push(AssocUse {
                    param: q as usize,
                    chain,
                    depth,
                });
            }
            _ => unreachable!("only a part that names a parameter is walked"),
        }
    }
    (depths, uses)
}

/// A name of associated types in a choice: the growth of `at` is at least `weight` more than the
/// growths of `names` together.
struct Rule {
    at: usize,
    names: Vec<usize>,
    weight: i64,
}

/// One pass over `rules`: raises the growth of each associated type that a rule gives a greater
/// one, unless it is `fixed`. A rule applies when every type it names has a growth. Returns the
/// associated types raised.
fn relax(growth: &mut [Option<i64>], rules: &[Rule], fixed: &[bool]) -> Vec<usize> {
    let mut raised = Vec::new();
    for rule in rules.iter().filter(|rule| !fixed[rule.at]) {
        // A sum that passes `UNBOUNDED` is `UNBOUNDED`: the growth does not end, and the number
        // stays one that the weights of the graph can add to.
        let named = (rule.names.iter()).try_fold(0_i64, |sum, &n| {
            Some(sum.saturating_add(growth[n]?).min(UNBOUNDED))
        });
        let Some(through) = named.map(|named| (named + rule.weight).min(UNBOUNDED)) else {
            continue;
        };
        if growth[rule.at].is_none_or(|g| through > g) {
            growth[rule.at] = Some(through);
            raised.push(rule.at);
        }
    }
    raised
}

/// Which associated types have a growth that does not end: those that a cycle of positive weight
/// among the rules raises again and again, and those that name one while it is raised. `growth`
/// holds the growths outright. Every associated type starts at the growth it has, or at 0, so a
/// cycle counts whether or not an outright growth reaches it.
///
/// A growth that is bounded settles in as many passes as there are associated types: the
/// greatest of its rules has no associated type twice along any path of names, since a path
/// that repeats one adds a weight above 0, which is unbounded, or at most 0, which the path
/// without the repeat matches. So a type that a pass raises after that is on a cycle or names
/// one. A type that names an unbounded one and is not raised in these passes takes its growth
/// from it when the growths are found, as a rule gives it.
fn cyclic(growth: &[Option<i64>], rules: &[Rule]) -> Vec<bool> {
    let count = growth.len();
    let mut ghost: Vec<Option<i64>> = growth.iter().map(|g| Some(g.unwrap_or(0))).collect();
    let none = vec![false; count];
    let mut unbounded = vec![false; count];
    let settled = (0..=count).any(|_| relax(&mut ghost, rules, &none).is_empty());
    if !settled {
        // Still growing. A cycle makes some type on it grow in every turn around, and a turn
        // takes at most as many passes as there are associated types.
        for _ in 0..=count {
            for raised in relax(&mut ghost, rules, &none) {
                unbounded[raised] = true;
            }
        }
    }
    // A growth that reached `UNBOUNDED` has no more room to grow, and did not end.
    for (unbounded, growth) in unbounded.iter_mut().zip(&ghost) {
        *unbounded |= growth.is_some_and(|growth| growth >= UNBOUNDED);
    }
    unbounded
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
            enums: 0,
            interface_hubs: 2,
            eq_first: 2,
            display_first: 2,
            display: tir::InterfaceId(usize::MAX),
            records: FxHashMap::default(),
            edges: Vec::new(),
            typed: FxHashMap::default(),
            cores: FxHashMap::default(),
            walked: FxHashSet::default(),
            assocs: Assocs::default(),
            choices_first: 0,
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
        g.args(0, &[ty], 0, site, site, Instance::Type(ty, None));
        // A node per `Pair`, an edge from each of its two arguments, and the call's two edges:
        // into the parameter's first node and into its second.
        assert_eq!(g.typed.len(), 20);
        assert_eq!(g.edges.len(), 2 * 20 + 2);
        // Written as a field's type: each level's `Pair` gives two edges into each of its two
        // parameters, once (the first node and the second), and the 19 nodes below the top have
        // two edges each from their parts.
        let mut g = graph(1);
        g.types(0, ty, site, &[]);
        assert_eq!(g.edges.len(), 2 * 19 + 2 * 2 * 20);
    }

    /// An argument that mentions `n` parameters, `Pair<…Pair<T0, T1>…, Tn>`, costs a node and
    /// two edges per `Pair` once, and two edges at each further site (the parameter's first node
    /// and its second): never a copy of what each part mentions.
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
                g.args(0, &[ty], 0, at, at, Instance::Type(ty, None));
            }
            assert_eq!(g.typed.len(), n as usize);
            assert_eq!(g.edges.len(), 2 * n as usize + 2 * 10);
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
                    Instance::Type(ty, None),
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

    /// The outer levels of a type that many sites share are taken off once for the type, however
    /// many argument edges and operators read it: `n` sites on a type nested `n` deep read `n`
    /// levels, not `n * n`. A type that holds a shorter one reads only the levels it adds.
    #[test]
    fn the_outer_levels_of_a_shared_type_should_be_taken_off_once() {
        let steps = |n: usize| {
            let mut ty = param(0);
            for _ in 0..n {
                ty = Type::intern(TypeKind::Array(ty));
            }
            let mut g = graph(1);
            CORE_STEPS.set(0);
            for site in 0..n {
                let at = Span::new(site, site + 1);
                g.args(0, &[ty], 0, at, at, Instance::Type(ty, None));
                g.operator(
                    0,
                    ty,
                    tir::InterfaceId(0),
                    (at, at),
                    Some(Written::Op("==")),
                );
            }
            CORE_STEPS.get()
        };
        assert_eq!(steps(20), 20);
        crate::mir::tests::assert_near_linear(steps(20), steps(40), steps(60));
        // Each type of a ladder holds the one before it.
        let mut g = graph(1);
        let mut ty = param(0);
        CORE_STEPS.set(0);
        for site in 0..30 {
            ty = Type::intern(TypeKind::Array(ty));
            let at = Span::new(site, site + 1);
            g.operator(
                0,
                ty,
                tir::InterfaceId(0),
                (at, at),
                Some(Written::Op("==")),
            );
        }
        assert_eq!(CORE_STEPS.get(), 30);
    }

    /// A growth that doubles in each pass, with as many passes as 100 associated types need, passes
    /// `UNBOUNDED` long before they end: it is unbounded, and no sum overflows.
    #[test]
    fn a_growth_that_doubles_should_be_unbounded_without_overflow() {
        let mut growth = vec![None; 100];
        growth[0] = Some(1);
        let rules = [Rule {
            at: 0,
            names: vec![0, 0],
            weight: 0,
        }];
        let unbounded = cyclic(&growth, &rules);
        assert!(unbounded[0]);
        assert!(!unbounded[1..].contains(&true));
    }

    /// A parameter has no parts: its node is its own, never one made of parts.
    #[test]
    fn a_parameter_should_have_no_parts() {
        assert_eq!(graph(1).parts(param(0)), Vec::new());
    }

    /// A graph of a function, two enums, the hub of an interface and a generated `Eq` node for
    /// each enum, each with one parameter.
    fn graph_with_enums() -> Graph {
        Graph {
            first: vec![0, 1, 2, 3, 4, 5, 6],
            functions: 1,
            classes: 0,
            enums: 2,
            interface_hubs: 3,
            eq_first: 4,
            display_first: 6,
            display: tir::InterfaceId(usize::MAX),
            records: FxHashMap::default(),
            edges: Vec::new(),
            typed: FxHashMap::default(),
            cores: FxHashMap::default(),
            walked: FxHashSet::default(),
            assocs: Assocs::default(),
            choices_first: 0,
        }
    }

    /// The nodes after the first nodes are the second nodes, one for each parameter slot in the
    /// order of the first, and then the nodes of types.
    #[test]
    fn second_nodes_should_follow_the_first_nodes_and_precede_the_types() {
        let mut g = graph(3);
        // The first nodes are 0 to 4 (a function of three parameters, a class of two).
        assert_eq!(g.firsts(), 5);
        assert_eq!(g.second(0, 0), 5);
        assert_eq!(g.second(0, 2), 7);
        assert_eq!(g.second(1, 1), 9);
        let array = Type::intern(TypeKind::Array(param(0)));
        assert_eq!(g.node(0, array), 10);
    }

    /// An argument that is a parameter, under any arrays and optionals, feeds the second node of
    /// the parameter it gives from its own second node; any other argument, from its own node.
    #[test]
    fn an_argument_should_feed_the_second_node_from_its_core() {
        let site = Span::new(0, 1);
        let mut g = graph(2);
        let array = Type::intern(TypeKind::Array(Type::intern(TypeKind::Optional(param(1)))));
        g.args(0, &[array], 0, site, site, Instance::Type(array, None));
        // Two edges for the nodes of the optional and the array, then the first node from the
        // argument's own node, and the second from the parameter's second.
        assert_eq!(g.edges.len(), 4);
        let second = &g.edges[3];
        assert_eq!((second.from, second.to), (g.second(0, 1), g.second(0, 0)));
        assert_eq!(second.weight, 0);
    }

    /// An enum operand with no written conformance goes to its generated node alone; one with an
    /// unconditional conformance, to the hub alone; one with only conditional conformances, to
    /// both.
    #[test]
    fn an_enum_operand_should_choose_its_edges_by_its_conformances() {
        let site = Span::new(0, 1);
        let interface = tir::InterfaceId(0);
        let ty = Type::intern(TypeKind::Enum(
            tir::EnumId(1),
            TypeArgs::intern(&[param(0)]),
        ));
        // Each row: what the records say, whether the hub and the generated node get an edge.
        for (row, records, hub, generated) in [
            ("no conformance", None, false, true),
            ("an unconditional one", Some((true, true)), true, false),
            ("only conditional ones", Some((true, false)), true, true),
        ] {
            let mut g = graph_with_enums();
            if let Some(records) = records {
                g.records.insert((interface, tir::EnumId(1)), records);
            }
            g.operator(0, ty, interface, (site, site), Some(Written::Op("==")));
            let into = |node: usize| g.edges.iter().any(|edge| edge.to == node);
            // Parameter 0 of the hub is node 3; of the generated node of enum 1, node 5.
            assert_eq!(into(3), hub, "{row}: the hub");
            assert_eq!(into(5), generated, "{row}: the generated node");
        }
    }
}
