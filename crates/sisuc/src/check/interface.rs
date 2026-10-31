//! Interface declarations: their parameters, requirements and inheritance, the bounds that name
//! them, and the closure of the interfaces a bound inherits.

use std::collections::VecDeque;
use std::rc::Rc;

use rustc_hash::{FxHashMap, FxHashSet};

use super::extend::holder_of;
use super::{
    BindingKind, Body, Checker, Choice, Choices, Declared, Definition, Holder, LeafAnswers, LeafKey,
};
use crate::ast;
use crate::conformance::{CHOICE_DEPTH_LIMIT, Entered, Generated, ensure_stack, find, generated};
use crate::diagnostic::{
    AddBound, BlockNeed, BoundSite, Definition as Kind, DiagnosticKind, NoOperatorForm,
    NotConformForm, NotInterface, PreludeBound, UnknownForm, WideKind,
};
use crate::graph::components;
use crate::lang::Dispatch;
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{ClassId, EnumId, FuncId, InterfaceId, Type, TypeArgs, TypeKind};

/// The most interfaces that one bound, or one list of interfaces a type conforms to, may give
/// with the interfaces they inherit. An interface that inherits another twice, with different
/// arguments, doubles the number at each level of inheritance, so the walk stops here (E0157).
pub(super) const BOUND_CLOSURE_LIMIT: usize = 256;

/// The state of one root walk of `Checker::leaf`: Tarjan's search for strongly connected
/// components. A type is on `stack` from the walk that reads it until its component closes;
/// `index` gives its place there. `low` is the least place that the open walk has cut against.
struct LeafWalk {
    index: FxHashMap<LeafKey, usize>,
    stack: Vec<LeafKey>,
    low: usize,
}

impl LeafWalk {
    fn new() -> Self {
        Self {
            index: FxHashMap::default(),
            stack: Vec::new(),
            low: usize::MAX,
        }
    }

    /// Puts `key` on the stack, and gives its place there.
    fn enter(&mut self, key: LeafKey) -> usize {
        let place = self.stack.len();
        self.stack.push(key);
        self.index.insert(key, place);
        place
    }
}

/// How many class or enum keys a query for a conformance may hold open, one inside the other. A
/// declaration that instantiates itself with a longer type argument each time, through a path that
/// the graph of instantiations does not see (`check/cycle.rs`), makes a query that has no end.
/// The query stops when more than this many are open, and reports E0140. The stack of the open
/// keys is on the heap, and the walk grows the call stack as it needs (`ensure_stack`), so the
/// value follows the memory that a query may use, not the call stack: 2^18 keys take about 40 MB,
/// and `mono` makes the same number of instances for a program that nests that deep, which is far
/// above any program.
pub(super) const CONFORMS_DEPTH_LIMIT: usize = 1 << 18;

/// Whether `part`, a part that `failing_leaf` found, is a type parameter or an associated type:
/// it fails for want of a bound, which the program can add.
fn is_parameter(part: Type) -> bool {
    matches!(part.kind(), TypeKind::Param(..) | TypeKind::Assoc(..))
}

/// `CONFORMS_DEPTH_LIMIT`; a test may lower it (`CONFORMS_DEPTH`).
fn depth_limit() -> usize {
    #[cfg(test)]
    return super::CONFORMS_DEPTH.get();
    #[cfg(not(test))]
    CONFORMS_DEPTH_LIMIT
}

/// How deep a type that `probe_associated` follows may nest its arguments: as many levels as a
/// walk of `normalize` follows choices, so the probe finds what `mono` would meet.
const PROBE_NESTING: usize = CHOICE_DEPTH_LIMIT;

/// How many associated types one bound's probe may normalize. Choices that each grow the type in
/// their own way make `2^n` types in `n` steps, so the nesting limit does not bound the work
/// alone. A chain of `PROBE_NESTING` levels with up to eight associated types in each fits.
const PROBE_BUDGET: usize = 8 * PROBE_NESTING;

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
    /// The first associated type of each name, in declaration order.
    pub(super) assoc: Vec<AssocInfo>,
}

/// An associated type of an interface.
pub(super) struct AssocInfo {
    pub(super) name: ast::Ident,
    /// The interfaces it conforms to, over the interface's parameters (`Self` first).
    pub(super) bounds: Vec<Bound>,
}

/// A requirement's signature, over the interface's parameters (`Self` first) and then its own.
pub(super) struct RequirementInfo {
    pub(super) name: ast::Ident,
    /// `fn` through the return type.
    pub(super) span: Span,
    /// Its own type parameters.
    pub(super) own: Vec<Symbol>,
    /// Where each of `own` is written.
    pub(super) own_at: Vec<Span>,
    /// The bounds of each of `own`.
    pub(super) bounds: Vec<Vec<Bound>>,
    /// Without `self`; `None` is `Error`.
    pub(super) params: Vec<Option<Type>>,
    /// Where the type of each of `params` is written.
    pub(super) params_at: Vec<Span>,
    /// `None` is `Error`; no return type is `unit`.
    pub(super) ret: Option<Type>,
    /// Where the return type is written, if it is.
    pub(super) ret_at: Option<Span>,
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

/// A bound check that waits for every conformance, in the scope `scope`.
pub(super) struct PendingBound {
    check: Check,
    scope: usize,
    /// Where the type that holds `ty` is kept, if it is kept: a type that fails the check is
    /// unknown there.
    place: Option<Slot>,
}

/// What a pending bound check reads.
enum Check {
    /// The applied class or enum type `ty`, each of whose arguments is written at the span of
    /// `ats`: each argument meets the bounds of its parameter.
    Applied { ty: Type, ats: Vec<Span> },
    /// The arguments of `bound`, each written at the span of `ats`, for the type `this` that
    /// has the interface: each argument meets the bounds of the interface's parameter.
    Arguments {
        bound: Bound,
        this: Type,
        ats: Vec<Span>,
    },
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
#[derive(Clone)]
pub(super) struct Scope {
    params: Vec<Symbol>,
    bounds: Vec<Vec<Bound>>,
    self_type: Option<Type>,
}

/// The pending bound checks while `run_pending_bounds` runs them. A query for a generated
/// conformance reads the payload types of an enum, and a payload that fails its check is unknown,
/// so the checks that keep a payload run when a query first reads it (`settle_payload`), or in
/// queue order, whichever comes first: the answer of a query does not depend on the order of the
/// declarations.
pub(super) struct Waiting {
    /// Each check not run yet; a check that runs is taken out.
    checks: Vec<Option<PendingBound>>,
    /// The scope of each check.
    scope_of: Vec<usize>,
    scopes: Vec<Scope>,
    /// The checks that keep each payload type, by their place in `checks`, until a query reads it.
    payloads: FxHashMap<(EnumId, usize, usize), Vec<usize>>,
}

/// How an operator runs for an operand type (`Checker::operation`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Operation {
    /// The operand type has the interface, and the operator is lowered this way.
    Run(Dispatch),
    /// The type has the interface only through the poison of a broken block: no code is made.
    Poison,
    /// The type has no such interface.
    Missing,
}

/// How a language interface holds for a type (`Checker::lang_path`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LangPath {
    /// A written conformance, the one at this place among the interface's, whose methods all exist.
    Written(usize),
    /// The compiler writes it.
    Generated,
    /// It holds only because a broken block may have given it, or because a conformance lacks
    /// methods and has its own error: no code is made from it.
    PoisonOnly,
    /// The type has no such conformance.
    None,
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
        self.interfaces_resolved = true;
    }

    /// Records the associated types of `interface`, whose id is `id` and whose parameters are
    /// `own` as arguments, and puts them in scope as bare names over `Self`. Gives the ones that
    /// are first of their name.
    fn declare_assoc<'a>(
        &mut self,
        interface: &'a ast::Interface,
        id: InterfaceId,
        own: TypeArgs,
    ) -> Vec<&'a ast::AssocType> {
        let this_type = Type::intern(TypeKind::Param(self.interfaces[id.0].type_params[0], 0));
        let mut named = FxHashMap::default();
        let mut declared = Vec::new();
        for assoc in &interface.assoc {
            let owner = Some(interface.name.name.as_str());
            if self.first_definition(&mut named, Kind::Type, owner, &assoc.name) {
                declared.push(assoc);
            }
        }
        self.body.assoc = Rc::new(Choices::new(
            (declared.iter().enumerate())
                .map(|(k, assoc)| {
                    let k =
                        u32::try_from(k).expect("an interface should have fewer than 2^32 types");
                    let ty = Type::intern(TypeKind::Assoc(this_type, id, own, k));
                    Choice {
                        name: assoc.name.clone(),
                        ty: Some(ty),
                        at: assoc.name.span,
                    }
                })
                .collect(),
        ));
        self.interfaces[id.0].assoc = (declared.iter())
            .map(|assoc| AssocInfo {
                name: assoc.name.clone(),
                bounds: Vec::new(),
            })
            .collect();
        for (k, assoc) in declared.iter().enumerate() {
            (self.declaring_assoc.entry(assoc.name.name).or_default()).push((id, k));
            self.assoc_at.insert((id, assoc.name.name), k);
        }
        declared
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
        // The associated types come first: a bare name stands for one in the interface.
        let declared = self.declare_assoc(interface, id, this.args);
        let (inherits, inherits_at) = interface
            .inherits
            .iter()
            .filter_map(|ty| Some((self.inherited(ty, interface)?, ty.span)))
            .unzip();
        self.interfaces[id.0].inherits = inherits;
        self.interfaces[id.0].inherits_at = inherits_at;
        for (k, assoc) in declared.iter().enumerate() {
            let ty = self.body.assoc.at(k).ty;
            let bounds = (assoc.bounds.iter())
                .filter_map(|bound| self.bound(bound, ty))
                .collect();
            self.interfaces[id.0].assoc[k].bounds = bounds;
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
                    own_at: requirement
                        .type_params
                        .iter()
                        .map(|p| p.name.span)
                        .collect(),
                    bounds,
                    params,
                    params_at: requirement.params.iter().map(|p| p.ty.span).collect(),
                    ret,
                    ret_at: requirement.ret.as_ref().map(|ret| ret.span),
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
    /// A bound that names an associated type in its arguments, as `T: Convert<S.Item>`, needs the
    /// bounds of the parameter it is written through, which may come after it. So the bounds that
    /// name none are resolved first and put in scope, and then the others, each after the bounds
    /// it needs (see `resolve_member_bounds`). An associated type whose base has no bound that
    /// declares it is not found, and is reported once.
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
        let at = first + head.len();
        let mut found: Vec<Vec<Option<Bound>>> = (params.iter())
            .map(|param| vec![None; param.bounds.len()])
            .collect();
        let mut members = Vec::new();
        for (i, param) in params.iter().enumerate() {
            for (j, ty) in param.bounds.iter().enumerate() {
                if ty.names_member() {
                    members.push((i, j));
                } else {
                    found[i][j] = self.resolve_bound(at, params, i, j);
                }
            }
        }
        if !members.is_empty() {
            // The bounds so far are in scope, for the bounds that write an associated type.
            let so_far = head.iter().cloned().chain(
                found
                    .iter()
                    .map(|bounds| bounds.iter().flatten().copied().collect()),
            );
            self.body.set_bounds(first, so_far.collect());
            self.resolve_member_bounds(at, params, &mut found, &members);
        }
        let mut bounds = head;
        bounds.extend(
            found
                .into_iter()
                .map(|bounds| bounds.into_iter().flatten().collect()),
        );
        self.body.set_bounds(first, bounds.clone());
        self.rescope(queued);
        bounds
    }

    /// Puts the checks queued after the first `queued` in the scope now in place, which has the
    /// bounds that were not in scope when they were queued.
    pub(super) fn rescope(&mut self, queued: usize) {
        if self.queued() > queued {
            let scope = self.scope();
            for check in self.pending_bounds.iter_mut().flatten().skip(queued) {
                check.scope = scope;
            }
        }
    }

    /// The bound `j` of the parameter `i` of `params`, which start at the place `at`.
    fn resolve_bound(
        &mut self,
        at: usize,
        params: &[ast::TypeParam],
        i: usize,
        j: usize,
    ) -> Option<Bound> {
        let place =
            u32::try_from(at + i).expect("a declaration should have fewer than 2^32 parameters");
        let this = Some(Type::intern(TypeKind::Param(params[i].name.name, place)));
        self.bound(&params[i].bounds[j], this)
    }

    /// Resolves into `found` the bounds `members` of `params`, as (parameter, place) pairs, that
    /// name an associated type of a parameter in their arguments. `found` holds the others, and
    /// `at` is the place of the first parameter. The bounds that `found` holds are in scope.
    ///
    /// An associated type `S.Item` holds the arguments of the bound of `S` that declares `Item`,
    /// so that bound is resolved first. Which bounds declare `Item`, and which declare the same
    /// one, depends on the names of their interfaces and the text of their arguments and not on
    /// their types, so the order is known before any bound resolves, and it does not depend on
    /// the order of the parameters. A bound waits for each declaration of the name it writes that
    /// another bound gives, so the resolution finds the type ambiguous if two differ (E0156). A
    /// declaration that only the bound gives is no wait: then no other bound gives the type, and
    /// the resolution reports it. The bounds resolve in waves: each wave
    /// holds the bounds that wait for no one now, and its bounds go into scope together at its
    /// end. Each bound and each name is read a fixed number of times.
    ///
    /// The bounds left wait for each other. Each that is on a loop of bounds is reported at the
    /// associated type it writes (E0030), and the others that wait for it stay unknown, with no
    /// report of their own.
    fn resolve_member_bounds(
        &mut self,
        at: usize,
        params: &[ast::TypeParam],
        found: &mut [Vec<Option<Bound>>],
        members: &[(usize, usize)],
    ) {
        // The first place of each name, as `Body` finds a parameter.
        let place_of: FxHashMap<Symbol, usize> = (params.iter().enumerate().rev())
            .map(|(i, param)| (param.name.name, i))
            .collect();
        // For each bound, the associated types it writes, as the place of the parameter they go
        // through in the list and their name.
        let written: Vec<Vec<(usize, &ast::Ident)>> = members
            .iter()
            .map(|&(i, j)| {
                let mut roots = Vec::new();
                params[i].bounds[j].member_roots(&mut roots);
                (roots.into_iter())
                    .filter_map(|(root, name)| Some((*place_of.get(&root)?, name)))
                    .collect()
            })
            .collect();
        let mut waits = Waits::new(&written, params.len());
        self.find_declarers(params, found, members, &mut waits);
        let mut ready = waits.start();
        let mut seen = FxHashMap::default();
        while !ready.is_empty() {
            let mut wave = Vec::new();
            for &m in &ready {
                #[cfg(test)]
                super::BOUND_STEPS.set(super::BOUND_STEPS.get() + 1);
                let (i, j) = members[m];
                found[i][j] = self.resolve_bound(at, params, i, j);
                if let Some(bound) = found[i][j] {
                    wave.push((at + i, bound));
                }
            }
            self.add_bounds(&wave, &mut seen);
            ready = waits.resolved(&ready);
        }
        if waits.blocked.iter().all(|&blocked| blocked == 0) {
            return;
        }
        for (root, name) in waits.on_loops(&written) {
            self.report(DiagnosticKind::UnknownType {
                name: name.name.to_string(),
                at: name.span,
                form: UnknownForm::BoundsLoop {
                    base: params[root].name.name.to_string(),
                },
            });
        }
    }

    /// Fills `waits` with the declarations that the bounds of each parameter give to the keys
    /// that some bound writes through the parameter. `found` holds the bounds in scope. The
    /// names that an interface declares are read once for each interface, and each bound reads
    /// the shorter of that list and the keys of its parameter.
    fn find_declarers(
        &self,
        params: &[ast::TypeParam],
        found: &[Vec<Option<Bound>>],
        members: &[(usize, usize)],
        waits: &mut Waits,
    ) {
        let mut by_param: Vec<Vec<usize>> = vec![Vec::new(); params.len()];
        for (m, &(i, _)) in members.iter().enumerate() {
            by_param[i].push(m);
        }
        for (root, bounds) in by_param.iter().enumerate() {
            if !waits.is_root[root] {
                continue;
            }
            let scoped = (found[root].iter().enumerate()).filter_map(|(j, bound)| {
                Some((&params[root].bounds[j], bound.as_ref()?.interface, None))
            });
            let listed = bounds.iter().filter_map(|&m| {
                let (i, j) = members[m];
                let ty = &params[i].bounds[j];
                Some((ty, self.written_interface(ty)?, Some(m)))
            });
            for (ty, interface, giver) in scoped.chain(listed) {
                let Some(with) = self.written_arguments(ty, interface) else {
                    continue;
                };
                let (class, new) = waits.class(root, interface, with.clone(), giver);
                if !new {
                    continue;
                }
                let declared = self.names_of(interface);
                let keys = std::mem::take(&mut waits.keys_of[root]);
                let give = |waits: &mut Waits, key, name| {
                    for &at in declared.by_name.get(&name).into_iter().flatten() {
                        let declaration = declared.list[at];
                        let args = declaration.args.subst(&with);
                        waits.give(class, key, declaration.declarer, args);
                    }
                };
                if declared.wide {
                    // A name past the limit may be declared: the key waits for the bound, which
                    // is then reported, and the parameter is unknown for the bounds after it.
                    for &(name, key) in &keys {
                        #[cfg(test)]
                        super::BOUND_STEPS.set(super::BOUND_STEPS.get() + 1);
                        give(waits, key, name);
                        waits.give(class, key, interface, TypeArgs::intern(&with[1..]));
                    }
                } else if keys.len() <= declared.by_name.len() {
                    for &(name, key) in &keys {
                        #[cfg(test)]
                        super::BOUND_STEPS.set(super::BOUND_STEPS.get() + 1);
                        give(waits, key, name);
                    }
                } else {
                    for &name in declared.by_name.keys() {
                        #[cfg(test)]
                        super::BOUND_STEPS.set(super::BOUND_STEPS.get() + 1);
                        if let Some(&key) = waits.key_of.get(&(root, name)) {
                            give(waits, key, name);
                        }
                    }
                }
                waits.keys_of[root] = keys;
            }
        }
    }

    /// Adds each of `added`, a bound with the place of the type parameter that has it, to the
    /// bounds in scope. A closure that was found for the parameter before grows by the closure of
    /// the bound: `seen` keeps the entries of each such closure from one call to the next, so the
    /// work follows the closure of the bound and not the closure of the parameter. What was found
    /// from the bounds of the other parameters stays. The scope that was made stays too: whoever
    /// adds bounds in place makes it again when they are all in.
    fn add_bounds(
        &mut self,
        added: &[(usize, Bound)],
        seen: &mut FxHashMap<usize, FxHashSet<(InterfaceId, TypeArgs)>>,
    ) {
        self.body.required.clear();
        self.body.conformed.clear();
        self.body.leaves.clear();
        for &(place, bound) in added {
            self.body.bounds[place].push(bound);
            let Some(mut out) = self.body.conforming.get_mut(place).and_then(Option::take) else {
                continue;
            };
            let seen = seen
                .entry(place)
                .or_insert_with(|| out.iter().copied().collect());
            let name = self.body.type_params[place];
            let place32 =
                u32::try_from(place).expect("a declaration should have fewer than 2^32 parameters");
            let receiver = Type::intern(TypeKind::Param(name, place32));
            // A closure that goes past the limit is reported where `read_declared_bounds` reads the
            // bound, for each bound that is declared. The bounds that wait for this parameter wait
            // for the bound that is past the limit too (see `Waits`), so none reads the closure
            // before it is made again.
            let _ = self.extend_closure(receiver, &[bound], &mut out, seen, |_, _| {});
            self.body.conforming[place] = Some(out);
        }
    }

    /// The interface that `ty`, the bound of a parameter, names, if it names one that is declared.
    /// A bound that writes an associated type has arguments, so it is never a plain name.
    fn written_interface(&self, ty: &ast::TypeExpr) -> Option<InterfaceId> {
        let ast::TypeKind::Applied(applied) = &ty.kind else {
            return None;
        };
        let name = applied.name;
        if self.body.type_param(name).is_some() {
            return None;
        }
        match self.interface_ids.get(&name) {
            Some(&Declared::Known(id)) => Some(id),
            _ => None,
        }
    }

    /// The arguments that the bound `ty` of `interface` writes, led by the unit type for `Self`,
    /// each as a type parameter that its text names: two bounds that write the same text write
    /// the same type. `None` when their number is not the interface's.
    fn written_arguments(&self, ty: &ast::TypeExpr, interface: InterfaceId) -> Option<Vec<Type>> {
        let args = match &ty.kind {
            ast::TypeKind::Applied(applied) => &applied.args[..],
            _ => &[],
        };
        if args.len() + 1 != self.interfaces[interface.0].type_params.len() {
            return None;
        }
        let mut with = vec![Type::UNIT];
        with.extend(
            args.iter()
                .map(|arg| Type::intern(TypeKind::Param(Symbol::intern(&arg.to_string()), 0))),
        );
        Some(with)
    }

    /// The associated types that a bound of `interface` declares, with the ones that the interfaces
    /// it inherits declare. The arguments of a declaration are those of the interface that
    /// declares it, over the parameters of `interface`; `Self` stays the `Self` of `interface`,
    /// for the caller to replace with the type that has the bound. The walk stops at the closure
    /// limit, as the closure of a bound does (E0157): then `wide` is `true`, and the interfaces
    /// past the limit may declare any name.
    fn build_names(&self, interface: InterfaceId) -> Names {
        let params = &self.interfaces[interface.0].type_params;
        let this = Type::intern(TypeKind::Param(params[0], 0));
        let own: Vec<Type> = (1..)
            .zip(&params[1..])
            .map(|(i, &name)| Type::intern(TypeKind::Param(name, i)))
            .collect();
        let start = (interface, TypeArgs::intern(&own));
        let mut seen = FxHashSet::default();
        seen.insert(start);
        let mut walk = vec![start];
        let mut out = Vec::new();
        let mut with = Vec::new();
        let mut wide = false;
        let mut at = 0;
        while let Some(&(declarer, args)) = walk.get(at) {
            at += 1;
            #[cfg(test)]
            super::BOUND_STEPS.set(super::BOUND_STEPS.get() + 1);
            for (k, declared) in (0..).zip(&self.interfaces[declarer.0].assoc) {
                #[cfg(test)]
                super::DECLARED_BUILT.set(super::DECLARED_BUILT.get() + 1);
                out.push(Declaration {
                    name: declared.name.name,
                    declarer,
                    args,
                    k,
                });
            }
            with.clear();
            with.push(this);
            with.extend_from_slice(args.get());
            for (inherited, _) in self.inheritances(declarer) {
                let next = (inherited.interface, inherited.args.subst(&with));
                if walk.len() >= BOUND_CLOSURE_LIMIT {
                    wide |= !seen.contains(&next);
                } else if seen.insert(next) {
                    walk.push(next);
                }
            }
        }
        let mut by_name: FxHashMap<Symbol, Vec<usize>> = FxHashMap::default();
        for (at, declaration) in out.iter().enumerate() {
            by_name.entry(declaration.name).or_default().push(at);
        }
        Names {
            list: out,
            by_name,
            wide,
        }
    }

    /// What `build_names` finds for `interface`: built once for the program, as soon as every
    /// interface has its inheritance.
    fn names_of(&self, interface: InterfaceId) -> Rc<Names> {
        if let Some(names) = self.declared.borrow().get(&interface) {
            return Rc::clone(names);
        }
        let names = Rc::new(self.build_names(interface));
        if self.interfaces_resolved {
            self.declared
                .borrow_mut()
                .insert(interface, Rc::clone(&names));
        }
        names
    }

    /// The interface `ty` names with its arguments, for the type `this` that has it, if that is
    /// known. `None`, reported, when it names no interface, or its arguments do not fit; `None`
    /// without a report for a broken name. The arguments are checked against the bounds of the
    /// interface's parameters once every conformance is known.
    pub(super) fn bound(&mut self, ty: &ast::TypeExpr, this: Option<Type>) -> Option<Bound> {
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
            let written = args;
            let args = self.type_args_of(&ident, &declared, written, angles, false)?;
            let bound = Bound {
                interface: id,
                args,
                at: ty.bare,
            };
            if let (Some(this), false) = (this, written.is_empty()) {
                let ats = written.iter().map(|arg| arg.span).collect();
                self.check_arguments(bound, this, ats);
            }
            return Some(bound);
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
            at: ty.span,
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
        // A class or an enum that took a language interface's name is the type.
        self.body.type_param(name).is_none()
            && !self.class_ids.contains_key(&name)
            && !self.enum_ids.contains_key(&name)
            && matches!(self.interface_ids.get(&name), Some(Declared::Known(_)))
    }

    /// The interface that `ty`, in the inheritance list of `interface`, names. An argument that is
    /// neither a parameter of `interface` nor free of parameters is reported, and so the
    /// inheritance is dropped: the closure of a bound copies arguments and never builds one.
    fn inherited(&mut self, ty: &ast::TypeExpr, interface: &ast::Interface) -> Option<Bound> {
        let bound = self.bound(ty, self.body.self_type)?;
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
        let mut assoc_owners: FxHashMap<Symbol, (InterfaceId, Span)> = FxHashMap::default();
        for (i, interface) in interfaces.iter().enumerate() {
            seen.clear();
            seen.insert(i);
            queue.clear();
            queue.push(i);
            owners.clear();
            assoc_owners.clear();
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
                    for assoc in &self.interfaces[inherited].assoc {
                        assoc_owners
                            .entry(assoc.name.name)
                            .or_insert((InterfaceId(inherited), assoc.name.span));
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
                        assoc: false,
                    });
                }
            }
            for assoc in &self.interfaces[i].assoc {
                if let Some(&(owner, first)) = assoc_owners.get(&assoc.name.name) {
                    reports.push(DiagnosticKind::InheritedRequirement {
                        name: assoc.name.name.to_string(),
                        interface: interface.name.name.to_string(),
                        owner: self.interfaces[owner.0].name.name.to_string(),
                        at: assoc.name.span,
                        first,
                        assoc: true,
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
        // A requirement that names an associated type the conformance chooses no type for has its
        // own report, and meets any method.
        let differs = |want: Option<Type>, found: Option<Type>| match (want, found) {
            (Some(want), Some(found)) => self.subst(want, &with).is_some_and(|want| want != found),
            _ => false,
        };
        if (requirement.params.iter().zip(&sig.params)).any(|(&want, &found)| differs(want, found))
            || differs(requirement.ret, sig.ret)
        {
            return false;
        }
        (requirement.bounds.iter().zip(&sig.bounds)).all(|(want, found)| {
            let want: Vec<_> = (want.iter())
                .map(|b| (b.interface, self.subst_args(b.args, &with)))
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
            let text = self.source.text(bound.at);
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
            written: format!("{}: {}", self.type_name(ty), self.source.text(at)),
            limit: BOUND_CLOSURE_LIMIT,
            at,
        });
        let holder = holder_of(ty).expect("a conformance's target should be a class or an enum");
        self.broken_extensions.insert(self.holder_name(holder));
    }

    /// Reports once for the class or enum `holder` that a query of `conforms` went past
    /// `CONFORMS_DEPTH_LIMIT` keys deep for (E0140). The report stands at its name, as the walk
    /// has no use to point at. It names the interface and the type of the root of the query.
    fn too_deep(&mut self, holder: Holder) {
        if !self.too_deep.insert(holder) {
            return;
        }
        let name = match holder {
            Holder::Class(id) => &self.classes[id.0].name,
            Holder::Enum(id) => &self.enums[id.0].name,
            Holder::Array | Holder::I64 => unreachable!("a conforms query keys a class or an enum"),
        };
        let (holder, at) = (name.name.to_string(), name.span);
        let (root, interface, args) = self
            .open
            .root()
            .expect("a query that is too deep should have a root");
        self.report(DiagnosticKind::ConformanceTooDeep {
            interface: self.bound_text(interface, args),
            root: self.type_name(root),
            limit: depth_limit(),
            holder,
            at,
        });
    }

    /// The requirements named `name` that the bounds of the type parameter or associated type `ty`
    /// give, each once, in the order of the bounds. The first ask for a type in a body indexes by
    /// name the requirements of its interfaces, and the rest read the index: a call costs a
    /// lookup, not a walk of the bounds.
    ///
    /// The third recorded exception to "A pass is linear in the program" (user, 2026-10-25; ruling
    /// F9 (e)): the candidates of a name are `n` for `n` interfaces of the bounds that declare it,
    /// and a call keeps them while it checks its arguments.
    pub(super) fn required(&mut self, ty: Type, name: Symbol) -> Vec<Required> {
        if !self.body.required.contains_key(&ty) {
            let entries = self.interfaces_for(ty);
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
            self.body.required.insert(ty, by_name);
        }
        (self.body.required[&ty].get(&name))
            .cloned()
            .unwrap_or_default()
    }

    /// Whether `ty` conforms to `interface` with `args` (without `Self`): a type parameter
    /// through its bounds and what they inherit, a class or an enum through its written
    /// conformance, and when none covers it, through the generated rule of a language interface
    /// (`conformance::generated`). A broken `extend` block may have given its target the
    /// conformance, so that holds.
    ///
    /// `Display` comes first: it holds for every type but `unit` and `never`, and reads no written
    /// conformance and no bound. A written `Eq` or `Ord` that covers a type decides, and when its
    /// bounds fail, the type has no such interface: no fallback to the generated rule. So every
    /// query nested here is a conjunction.
    ///
    /// An answer is kept by the type, the interface and its arguments, so a bound that several
    /// conformances share costs one answer for each type, not one for each way to it: with
    /// `extend<T: A & B> Box<T>: A` and `: B`, `Box^n<Leaf>: A` costs steps linear in `n`. The
    /// answer for a type that holds a type parameter changes with the bounds, so it is kept with
    /// the body (`Body.conformed`).
    ///
    /// The fourth recorded exception to "A pass is linear in the program" (user, 2026-10-27;
    /// ruling F9): a query costs one step for each class or enum type that it forms (see
    /// `conformance::Open`). It decides each such type once: a type with no type parameter once
    /// in the program, and a type that names one once in each body. Each is a type that `mono`
    /// instantiates when the program uses it. The worst input is a branching chain of enums, each
    /// with two ways to the next level, which forms 2^n types at its last level. The search for
    /// the failing part on the error path (`failing_leaf`) keeps two caches split the same way
    /// (`leaves` and `Body.leaves`). It decides each type once in the same way, and it has the
    /// same bound (user, 2026-10-31).
    pub(super) fn conforms(&mut self, ty: Type, interface: InterfaceId, args: TypeArgs) -> bool {
        if interface == self.lang.display {
            return generated(self.lang, ty, interface) == Generated::Holds;
        }
        ensure_stack(|| match ty.kind() {
            TypeKind::Param(_, place) => {
                let place = place as usize;
                self.param_interfaces(place).contains(&(interface, args))
                    || self.body.wide.contains(&place)
            }
            TypeKind::Assoc(..) => self.assoc_conforms(ty, interface, args),
            TypeKind::Class(..) | TypeKind::Enum(..) => self.conforms_holder(ty, interface, args),
            _ => match generated(self.lang, ty, interface) {
                Generated::Holds => true,
                Generated::Element(part) => self.conforms(part, interface, args),
                Generated::Fails | Generated::Payloads(..) => false,
            },
        })
    }

    /// `conforms` for the class or enum `ty`. It decides the key (`ty`, `interface`, `args`) as
    /// `conformance::Open` says: a key whose query is open or pending holds, and any other key is
    /// expanded once.
    fn conforms_holder(&mut self, ty: Type, interface: InterfaceId, args: TypeArgs) -> bool {
        let broken = holder_of(ty)
            .is_some_and(|holder| self.broken_extensions.contains(&self.holder_name(holder)));
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
        match self.open.enter(key) {
            Entered::Open(number) | Entered::Pending(number) => {
                self.open.hit(number);
                return true;
            }
            Entered::New => {}
        }
        #[cfg(test)]
        {
            super::CONFORMS_STEPS.set(super::CONFORMS_STEPS.get() + 1);
            assert!(
                super::CONFORMS_STEPS.get() <= super::CONFORMS_BUDGET.get(),
                "conforms should stay within its budget of {} steps",
                super::CONFORMS_BUDGET.get()
            );
        }
        // A growing class and a marked enum have their own report, so a query that is too deep at
        // one gets no second report. It is expanded as before. For a marked enum, its written
        // conformance checks the types of its parameters, and with none it holds with no part
        // query. A class has no generated rule: only a written conformance reads its parts.
        let holder = holder_of(ty).expect("a key of this walk should be a class or an enum");
        let own = match holder {
            Holder::Class(id) => self.growing_classes[id.0],
            Holder::Enum(id) => self.marked[id.0],
            Holder::Array | Holder::I64 => unreachable!("a conforms query keys a class or an enum"),
        };
        let answer = if self.open.depth() > depth_limit() && !own {
            // The query holds, as it does for a marked enum: the report of `too_deep` is its only
            // diagnostic.
            self.too_deep(holder);
            true
        } else {
            self.conforms_expanded(ty, interface, args)
        };
        let mut done = self.open.leave(key, answer);
        if self.open.is_idle() {
            done.extend(self.open.settle());
        }
        for ((ty, interface, args), answer) in done {
            let kept = if ty.has_param() {
                &mut self.body.conformed
            } else {
                &mut self.conformed
            };
            kept.insert((ty, interface, args), answer);
        }
        answer
    }

    /// The answer for a key that `conforms_holder` has opened: the conformance that covers `ty`
    /// decides, with its bounds; when none does, the generated rule.
    fn conforms_expanded(&mut self, ty: Type, interface: InterfaceId, args: TypeArgs) -> bool {
        if let Some((k, stands)) = find(&self.conformances, interface, ty, args) {
            return self.record_holds(interface, k, stands);
        }
        match generated(self.lang, ty, interface) {
            Generated::Holds => true,
            Generated::Fails | Generated::Element(_) => false,
            Generated::Payloads(id, own) => self.payloads_conform(id, own, interface),
        }
    }

    /// Whether the conformance `k` of `interface` holds when its parameters stand for `stands`:
    /// each bound of each parameter holds for the type that stands for it.
    fn record_holds(&mut self, interface: InterfaceId, k: usize, stands: TypeArgs) -> bool {
        let Some((_, bounds)) = self.conformance_bounds.get(&(interface, k)) else {
            return true;
        };
        let bounds = bounds.clone();
        for (i, bounds) in bounds.iter().enumerate() {
            for b in bounds {
                let args = self.subst_args(b.args, stands.get());
                if !self.conforms(stands.get()[i], b.interface, args) {
                    return false;
                }
            }
        }
        true
    }

    /// Whether each payload type of each variant of the enum `id`, with `own` for its
    /// parameters, conforms to `interface`. An enum that grows, or that no value can be built of,
    /// holds with no part query: its own error is its only diagnostic. A payload that is an
    /// error, or whose associated type has no choice, has its report already, so it holds.
    fn payloads_conform(&mut self, id: EnumId, own: TypeArgs, interface: InterfaceId) -> bool {
        if self.marked[id.0] {
            return true;
        }
        for slot in self.payload_slots(id) {
            let Some(payload) = self.payload_type(id, slot) else {
                continue;
            };
            let Some(part) = self.subst(payload, own.get()) else {
                continue;
            };
            if !self.conforms(part, interface, TypeArgs::NONE) {
                return false;
            }
        }
        true
    }

    /// How the language interface `interface` holds for `ty`: through a written conformance, which
    /// is a call of its method; through the generated rule; through the poison of a broken block
    /// or a record that lacks methods, which has its own error; or not at all. A written `Eq` or
    /// `Ord` that covers `ty` decides, and when its bounds fail the type has none. `Display` is
    /// different: every type but `unit` and `never` has it, so when each written record fails its
    /// bounds, the path is the generated `Display`. Each record's bounds are asked as a root
    /// query of their own, so `conforms` reads none for `Display`.
    pub(super) fn lang_path(&mut self, ty: Type, interface: InterfaceId) -> LangPath {
        let args = TypeArgs::NONE;
        if let Some(holder) = holder_of(ty) {
            if self.broken_extensions.contains(&self.holder_name(holder)) {
                return LangPath::PoisonOnly;
            }
            if let Some((k, stands)) = find(&self.conformances, interface, ty, args) {
                if self.record_holds(interface, k, stands) {
                    let met = self.conformances.get(interface, k).methods.len();
                    return if met == self.interfaces[interface.0].requirements.len() {
                        LangPath::Written(k)
                    } else {
                        LangPath::PoisonOnly
                    };
                }
                if interface != self.lang.display {
                    return LangPath::None;
                }
            }
        }
        if self.conforms(ty, interface, args) {
            LangPath::Generated
        } else {
            LangPath::None
        }
    }

    /// How an operator of the language interface `interface` (`Eq` or `Ord`) runs for the operand
    /// type `ty`: a type parameter that has the interface runs the generated node, which `mono`
    /// lowers again for its type argument; a class or an enum runs the method of its written
    /// conformance, or the generated node when it has none.
    pub(super) fn operation(&mut self, ty: Type, interface: InterfaceId) -> Operation {
        if matches!(ty.kind(), TypeKind::Param(..) | TypeKind::Assoc(..)) {
            let holds = self.conforms(ty, interface, TypeArgs::NONE);
            return if holds {
                Operation::Run(Dispatch::Generated)
            } else {
                Operation::Missing
            };
        }
        match self.lang_path(ty, interface) {
            LangPath::Written(k) => {
                let (_, stands) = find(&self.conformances, interface, ty, TypeArgs::NONE)
                    .expect("a written path should have a conformance that covers the type");
                let func = self.conformances.get(interface, k).methods[0];
                Operation::Run(Dispatch::Method {
                    func,
                    types: stands,
                })
            }
            LangPath::Generated => Operation::Run(Dispatch::Generated),
            LangPath::PoisonOnly => Operation::Poison,
            LangPath::None => Operation::Missing,
        }
    }

    /// A part of `ty` that makes `ty` fail `interface` with `args` on its own, not always the
    /// first in the order of the generated rule: a class with no matching record, a type whose
    /// matching written record fails its bounds, or a type parameter whose bounds lack the
    /// interface. It is only for a type that `conforms` has found to fail; `ty` itself if the
    /// walk finds no part.
    ///
    /// The walk reads each type once. It finds the strongly connected components of the types it
    /// meets, as Tarjan's algorithm does (`LeafWalk`). The types of one component reach each
    /// other, so they share one answer, which the walk keeps when it closes the component. The
    /// root closes last, so each answer is final. The parameter that an answer names can depend on
    /// the order of the queries. Each such parameter is a true answer, and the help for E0139
    /// names one.
    pub(super) fn failing_leaf(
        &mut self,
        ty: Type,
        interface: InterfaceId,
        args: TypeArgs,
    ) -> Type {
        let key = (ty, interface, args);
        if let Some(&kept) = self.leaves_of(ty).get(&key) {
            return kept.unwrap_or(ty);
        }
        let found = self.leaf(ty, interface, args, &mut LeafWalk::new());
        self.leaves_of(ty).entry(key).or_insert(found);
        found.unwrap_or(ty)
    }

    /// Closes the strongly connected component that is on `walk.stack` from `from`: each of its
    /// types takes `answer` as its final answer. Each of them reaches the type that closes the
    /// component, and so reaches all that the walk of that type reached. A type that already has
    /// a final answer of its own, a part that fails, keeps it.
    fn close_part(&mut self, walk: &mut LeafWalk, from: usize, answer: Option<Type>) {
        for key in walk.stack.drain(from..) {
            walk.index.remove(&key);
            self.leaves_of(key.0).entry(key).or_insert(answer);
        }
    }

    /// The report that `ty`, the type of the operand at `at`, has no `op`, which `interface` (`Eq`
    /// or `Ord`) gives. It is E0139 when the part that fails (`failing_leaf`) is a type
    /// parameter, with the help to bound it. Else it is `NoOperator`, which names the part, and
    /// helps only when the part has no conformance at all, a class or an enum that does not write
    /// one. A part with a conformance whose bound fails says which bound.
    pub(super) fn missing_operator(
        &mut self,
        at: Span,
        ty: Type,
        op: &str,
        interface: InterfaceId,
    ) -> DiagnosticKind {
        let leaf = self.failing_leaf(ty, interface, TypeArgs::NONE);
        if is_parameter(leaf) {
            return DiagnosticKind::ParamLacks {
                ty: self.type_name(ty),
                op: op.to_string(),
                at,
                add: self.add_bound(leaf, interface),
            };
        }
        let record = find(&self.conformances, interface, leaf, TypeArgs::NONE);
        let needs = record
            .filter(|&(k, stands)| !self.record_holds(interface, k, stands))
            .map(|(k, stands)| self.failed_bound(interface, k, stands));
        // A class or an enum with no conformance can get one.
        let make = (record.is_none()
            && matches!(leaf.kind(), TypeKind::Class(..) | TypeKind::Enum(..)))
        .then(|| self.type_name(leaf));
        let object = matches!(ty.kind(), TypeKind::Optional(inner) if inner == leaf) || ty == leaf;
        let form = match make {
            Some(make)
                if object
                    && matches!(leaf.kind(), TypeKind::Class(..))
                    && interface == self.lang.eq =>
            {
                NoOperatorForm::Identity { make }
            }
            make => NoOperatorForm::Operator {
                make,
                interface: self.interfaces[interface.0].name.name.to_string(),
            },
        };
        DiagnosticKind::NoOperator {
            ty: self.type_name(ty),
            op: op.to_string(),
            at,
            leaf: (leaf != ty).then(|| self.type_name(leaf)),
            form,
            needs,
        }
    }

    /// The part that makes the generated rule of `interface` fail for the enum type `ty`, whatever
    /// record `ty` has itself: as `failing_leaf` finds it, `ty` if the walk finds none. `None`
    /// when the rule holds. It is for a conformance that the compiler would write, to say why it
    /// does not.
    pub(super) fn rule_failure(&mut self, ty: Type, interface: InterfaceId) -> Option<Type> {
        match generated(self.lang, ty, interface) {
            Generated::Payloads(id, own) if !self.payloads_conform(id, own, interface) => {
                let mut walk = LeafWalk::new();
                walk.enter((ty, interface, TypeArgs::NONE));
                let leaf = self.payloads_leaf(id, own, interface, &mut walk);
                // This walk reads the payloads of `ty` and ignores its record, but another walk
                // reads the record of `ty` and stops there. A type that reaches `ty` is still on
                // the stack, and its answer is not the answer of another walk, so it is dropped.
                // A part that closed before `ty` does not reach `ty`, so its answers stay.
                Some(leaf.unwrap_or(ty))
            }
            _ => None,
        }
    }

    /// The payload part of the enum `id`, with `own` for its parameters, that fails `interface` on
    /// its own, the first that is not a type parameter, else the first parameter; `None` when each
    /// holds. It ends where `payloads_conform` did: at a marked enum, and at an enum whose query
    /// was too deep, whose payloads no query read.
    fn payloads_leaf(
        &mut self,
        id: EnumId,
        own: TypeArgs,
        interface: InterfaceId,
        walk: &mut LeafWalk,
    ) -> Option<Type> {
        if self.marked[id.0] || self.too_deep.contains(&Holder::Enum(id)) {
            return None;
        }
        // A type parameter fails only for want of a bound, which the program can add. A part that
        // fails whatever the bounds is the better answer, so the first of those wins, in any order
        // of the slots; the first parameter is the answer only when no other part fails.
        let mut param = None;
        for slot in self.payload_slots(id) {
            let Some(payload) = self.payload_type(id, slot) else {
                continue;
            };
            let Some(part) = self.subst(payload, own.get()) else {
                continue;
            };
            let Some(found) = self.leaf(part, interface, TypeArgs::NONE, walk) else {
                continue;
            };
            if !is_parameter(found) {
                return Some(found);
            }
            param = param.or(Some(found));
        }
        param
    }

    /// The answers of `leaf` that `ty` uses: the body's for a type that names a type parameter,
    /// the checker's for any other.
    fn leaves_of(&mut self, ty: Type) -> &mut LeafAnswers {
        if ty.has_param() {
            &mut self.body.leaves
        } else {
            &mut self.leaves
        }
    }

    /// `failing_leaf` for one type; `None` when it holds, or when the walk is reading it already.
    fn leaf(
        &mut self,
        ty: Type,
        interface: InterfaceId,
        args: TypeArgs,
        walk: &mut LeafWalk,
    ) -> Option<Type> {
        ensure_stack(|| {
            match ty.kind() {
                TypeKind::Param(..) | TypeKind::Assoc(..) => {
                    (!self.conforms(ty, interface, args)).then_some(ty)
                }
                TypeKind::Class(..) | TypeKind::Enum(..) => {
                    let broken = holder_of(ty).is_some_and(|holder| {
                        self.broken_extensions.contains(&self.holder_name(holder))
                    });
                    if broken {
                        return None;
                    }
                    // A final answer is final in any walk, so it is read first: a type that this
                    // walk finished with a final answer is not a cut.
                    let key = (ty, interface, args);
                    if let Some(&kept) = self.leaves_of(ty).get(&key) {
                        return kept;
                    }
                    // A type on the stack is a cut: the walk is reading it, or has read it and not
                    // closed its part. It gives `None` and lowers `walk.low` to its place.
                    if let Some(&at) = walk.index.get(&key) {
                        walk.low = walk.low.min(at);
                        return None;
                    }
                    // The answer for a type with no type parameter changes with no body; one for
                    // a type that names a parameter changes with the bounds, so it is kept with
                    // the body (`Body.leaves`), as `Body.conformed` keeps the queries.
                    #[cfg(test)]
                    super::LEAF_STEPS.set(super::LEAF_STEPS.get() + 1);
                    let place = walk.enter(key);
                    let outer = std::mem::replace(&mut walk.low, place);
                    let found =
                        if let Some((k, stands)) = find(&self.conformances, interface, ty, args) {
                            (!self.record_holds(interface, k, stands)).then_some(ty)
                        } else {
                            match generated(self.lang, ty, interface) {
                                Generated::Holds => None,
                                Generated::Fails | Generated::Element(_) => Some(ty),
                                Generated::Payloads(id, own) => {
                                    self.payloads_leaf(id, own, interface, walk)
                                }
                            }
                        };
                    // A part that fails on its own is true for this type at once, whatever the
                    // walk cut. `None` and a parameter wait for the component: a cut hides what
                    // the type in progress reaches. The first type of the component closes it,
                    // when it ends with no cut against a type lower on the stack.
                    let low = walk.low;
                    walk.low = outer.min(low);
                    if found.is_some_and(|part| !is_parameter(part)) {
                        self.leaves_of(ty).insert(key, found);
                    }
                    if low == place {
                        self.close_part(walk, place, found);
                    }
                    found
                }
                _ => match generated(self.lang, ty, interface) {
                    Generated::Holds => None,
                    Generated::Fails | Generated::Payloads(..) => Some(ty),
                    Generated::Element(part) => self.leaf(part, interface, args, walk),
                },
            }
        })
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
        self.check_bound_of(at, ty, bound, with, None)
    }

    /// `check_bound` for a bound of a block, where the call is of the method that `need` names:
    /// the label of a bound of the prelude says that the method needs it.
    pub(super) fn check_bound_of(
        &mut self,
        at: Span,
        ty: Type,
        bound: Bound,
        with: &[Type],
        need: Option<BlockNeed>,
    ) -> bool {
        let args = self.subst_args(bound.args, with);
        if self.conforms(ty, bound.interface, args) {
            self.probe_associated(ty, bound.interface, args);
            return true;
        }
        // The bound goes to the type parameter inside `ty` that makes it fail: `T` for `[T?]`.
        let leaf = self.failing_leaf(ty, bound.interface, args);
        let add = self.add_bound(leaf, bound.interface);
        // No label points into the prelude: its bound is named on the call instead, and a class or
        // an enum with no conformance gets the help to write one. The conformance that fails is
        // the one of the part that fails, as for `==`.
        let prelude = bound.at.in_prelude();
        // A class or an enum may conform, when the types that stand for its parameters do.
        let owner = if prelude { leaf } else { ty };
        let needs = find(&self.conformances, bound.interface, owner, args)
            .map(|(k, stands)| self.failed_bound(bound.interface, k, stands));
        let none = matches!(leaf.kind(), TypeKind::Class(..) | TypeKind::Enum(..))
            && find(&self.conformances, bound.interface, leaf, args).is_none();
        let make = (prelude && none).then(|| self.type_name(leaf));
        let label = (prelude && bound.interface != self.lang.any_object).then(|| {
            let part_fails = leaf != ty
                && (none || matches!(leaf.kind(), TypeKind::Param(..) | TypeKind::Assoc(..)));
            if part_fails {
                PreludeBound::Since {
                    part: self.type_name(leaf),
                    need,
                }
            } else {
                PreludeBound::Call { need }
            }
        });
        self.report(DiagnosticKind::NotConform {
            ty: self.type_name(ty),
            interface: self.bound_text(bound.interface, args),
            at,
            form: NotConformForm::Bound {
                bound: (!prelude).then_some(bound.at),
                label,
                make,
                class_only: bound.interface == self.lang.any_object,
                add,
                needs,
            },
        });
        false
    }

    /// Normalizes each associated type that a bound of `interface` gives `ty`, a class or an
    /// enum that conforms to it, so that a choice that needs itself is found where the type meets
    /// the bound, though no signature writes the associated type. The types are the ones that
    /// `interface` declares and the ones that the interfaces it inherits declare: a body that
    /// writes any of them is instantiated with `ty`. Only the checker can report a loop.
    ///
    /// A body may also write the associated type of an associated type, as `S.B.B`: the type that
    /// a choice gives is probed for the bounds of the associated type that it is chosen for. A
    /// choice of a generic block gives a type that no `extend` block names, so this is the only
    /// place where that type is found. A type is probed once for a bound.
    ///
    /// shortcut: a chosen type that nests deeper than `PROBE_NESTING` is not followed, as a choice
    /// such as `type B = W<W<T>>` gives a larger type at each step and has no end; and a probe
    /// stops after `PROBE_BUDGET` associated types, as choices that each grow the type in their
    /// own way make `2^n` types. The budget costs up to `PROBE_BUDGET` normalizations for each
    /// bound, each with a walk of up to `CHOICE_DEPTH_LIMIT` steps, so a program with many bounds
    /// over growing types takes time in proportion to them, with a large constant. What it does not
    /// follow reaches `mono`, which reports a loop or a chain that is too long (E0030) for a type
    /// that an instance meets. Upgrade when a program needs `check` to find those too.
    fn probe_associated(&mut self, ty: Type, interface: InterfaceId, args: TypeArgs) {
        self.probe_budget = PROBE_BUDGET;
        self.probe_further(ty, interface, args);
    }

    /// `probe_associated`, within the budget that it set.
    fn probe_further(&mut self, ty: Type, interface: InterfaceId, args: TypeArgs) {
        if !matches!(ty.kind(), TypeKind::Class(..) | TypeKind::Enum(..))
            || !self.probed_types.insert((ty, interface, args))
        {
            return;
        }
        let names = self.names_of(interface);
        let mut with = vec![ty];
        with.extend_from_slice(args.get());
        for declaration in &names.list {
            let Some(left) = self.probe_budget.checked_sub(1) else {
                return;
            };
            self.probe_budget = left;
            #[cfg(test)]
            super::PROBE_STEPS.set(super::PROBE_STEPS.get() + 1);
            let args = declaration.args.subst(&with);
            let assoc = Type::intern(TypeKind::Assoc(
                ty,
                declaration.declarer,
                args,
                declaration.k,
            ));
            if !self.probed_assoc.insert(assoc) {
                continue;
            }
            // A type with a parameter is no instance: `mono` never meets it.
            let Some(chosen) = (self.normalize(assoc)).filter(|chosen| {
                !chosen.has_param()
                    && nests_within(*chosen, PROBE_NESTING, &mut FxHashMap::default())
            }) else {
                continue;
            };
            let mut under = vec![ty];
            under.extend_from_slice(args.get());
            // The bounds are copied: the probe below borrows the checker.
            let declared = &self.interfaces[declaration.declarer.0].assoc[declaration.k as usize];
            for bound in declared.bounds.clone() {
                let bound_args = self.subst_args(bound.args, &under);
                self.probe_further(chosen, bound.interface, bound_args);
            }
        }
    }

    /// The bound that would make `ty` conform to `interface`, written with the bounds it has:
    /// `None` unless `ty` is a type parameter in scope, or an associated type that the user's
    /// program declares, and `interface` takes no type parameters.
    pub(super) fn add_bound(&self, ty: Type, interface: InterfaceId) -> Option<AddBound> {
        if self.interfaces[interface.0].type_params.len() != 1 {
            return None;
        }
        let (site, have): (_, &[Bound]) = match ty.kind() {
            TypeKind::Param(name, place) if self.body.type_param(name) == Some(place as usize) => (
                BoundSite::Param(name.to_string()),
                self.body
                    .bounds
                    .get(place as usize)
                    .map_or(&[], Vec::as_slice),
            ),
            // No help edits the prelude.
            TypeKind::Assoc(_, owner, _, k) => {
                let info = &self.interfaces[owner.0].assoc[k as usize];
                if info.name.span.in_prelude() {
                    return None;
                }
                let site = BoundSite::Assoc {
                    interface: self.interfaces[owner.0].name.name.to_string(),
                    name: info.name.name.to_string(),
                };
                (site, &info.bounds)
            }
            _ => return None,
        };
        let mut bounds: Vec<String> = have
            .iter()
            .map(|b| self.bound_text(b.interface, b.args))
            .collect();
        bounds.push(self.bound_text(interface, TypeArgs::NONE));
        Some(AddBound { site, bounds })
    }

    /// Checks that each argument of the applied type `ty`, written at `ats`, meets the bounds
    /// that its class or enum declares. When every conformance is not yet known, the check waits.
    /// Whether it holds, or waits.
    pub(super) fn check_applied(&mut self, ty: Type, ats: Vec<Span>) -> bool {
        self.check_or_wait(Check::Applied { ty, ats })
    }

    /// Checks that each argument of `bound`, written at `ats`, meets the bounds that its
    /// interface declares for its parameter, as `check_applied` does for a class or an enum. The
    /// type `this` has the interface.
    fn check_arguments(&mut self, bound: Bound, this: Type, ats: Vec<Span>) {
        self.check_or_wait(Check::Arguments { bound, this, ats });
    }

    /// Queues `check` while the conformances are not yet known, and else runs it. Whether it
    /// holds, or waits.
    fn check_or_wait(&mut self, check: Check) -> bool {
        let scope = self.pending_bounds.is_some().then(|| self.scope());
        if let (Some(scope), Some(pending)) = (scope, &mut self.pending_bounds) {
            pending.push(PendingBound {
                check,
                scope,
                place: None,
            });
            return true;
        }
        self.check_now(&check)
    }

    /// Runs `check`. Whether it holds: a failed check of arguments is reported and keeps no type
    /// to make unknown, so it counts as held.
    fn check_now(&mut self, check: &Check) -> bool {
        match check {
            Check::Applied { ty, ats } => self.check_applied_now(*ty, ats),
            Check::Arguments { bound, this, ats } => {
                // The bounds of the interface lead with `Self`'s own, which this does not check.
                let bounds = self.interfaces[bound.interface.0].bounds.clone();
                let mut with = vec![*this];
                with.extend_from_slice(bound.args.get());
                for (k, bounds) in bounds.iter().enumerate().skip(1) {
                    for &b in bounds {
                        self.check_bound(ats[k - 1], with[k], b, &with);
                    }
                }
                true
            }
        }
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
        #[cfg(test)]
        super::SCOPES_KEPT.set(super::SCOPES_KEPT.get() + 1);
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
        let mut payloads: FxHashMap<(EnumId, usize, usize), Vec<usize>> = FxHashMap::default();
        for (i, check) in pending.iter().enumerate() {
            if let Some(Slot::Payload(e, v, f)) = check.place {
                payloads.entry((e, v, f)).or_default().push(i);
            }
        }
        self.waiting = Some(Waiting {
            scope_of: pending.iter().map(|check| check.scope).collect(),
            checks: pending.into_iter().map(Some).collect(),
            scopes: std::mem::take(&mut self.scopes),
            payloads,
        });
        let all = self
            .waiting
            .as_ref()
            .map_or(0, |waiting| waiting.checks.len());
        self.run_waiting(&(0..all).collect::<Vec<_>>());
        self.waiting = None;
    }

    /// Runs the waiting checks `indices` that no query ran yet, in order, each in the scope it was
    /// queued in, and makes unknown the type kept at the place of each that fails. A scope is set
    /// up once for the checks that follow each other in it.
    fn run_waiting(&mut self, indices: &[usize]) {
        let waiting = (self.waiting.as_ref()).expect("the checks should run while they wait");
        let groups: Vec<&[usize]> = indices
            .chunk_by(|a, b| waiting.scope_of[*a] == waiting.scope_of[*b])
            .collect();
        for group in groups {
            let waiting = (self.waiting.as_ref()).expect("the checks should run while they wait");
            if group.iter().all(|&i| waiting.checks[i].is_none()) {
                continue;
            }
            let scope = waiting.scopes[waiting.scope_of[group[0]]].clone();
            self.with_body(Body::default(), |checker| {
                checker.body.set_type_params(&scope.params);
                checker.body.set_bounds(0, scope.bounds);
                checker.body.self_type = scope.self_type;
                for &i in group {
                    // A query that an earlier check made may have run this one.
                    let taken = (checker.waiting.as_mut()).and_then(|w| w.checks[i].take());
                    let Some(check) = taken else {
                        continue;
                    };
                    if !checker.check_now(&check.check)
                        && let Some(place) = check.place
                    {
                        checker.make_unknown(place);
                    }
                }
            });
        }
    }

    /// The places of the payloads of the enum `id`: its variant and its field.
    fn payload_slots(&self, id: EnumId) -> Vec<(usize, usize)> {
        let variants = self.enums[id.0].variants.iter().enumerate();
        variants
            .flat_map(|(v, variant)| (0..variant.fields.len()).map(move |f| (v, f)))
            .collect()
    }

    /// The type of the payload at `slot` of the enum `id`, `None` if it is an error. The checks
    /// that keep it run first, so a type that fails its bound is unknown.
    ///
    /// shortcut: a payload whose own checks are running (a check that needs an enum that has the
    /// payload) is read as it is written, so such a loop of checks can answer by the order of its
    /// declarations; upgrade when a program needs the answer of a loop of failing checks. Known
    /// case: `class Hold {}`, `enum Wrap<T: Eq> { W(T) }`, `enum E1 { A(Wrap<E2>) }` and
    /// `enum E2 { B(Wrap<E1>) C(Hold) }` give two errors with `E1` first and one with `E2` first.
    fn payload_type(&mut self, id: EnumId, (variant, field): (usize, usize)) -> Option<Type> {
        self.settle_payload((id, variant, field));
        self.enums[id.0].variants[variant].fields[field]
    }

    /// Runs the waiting checks that keep the payload `slot`. They run as roots of their own
    /// queries: they are no part of the query that reads the payload, whose answer they do not
    /// give.
    fn settle_payload(&mut self, slot: (EnumId, usize, usize)) {
        let Some(waiting) = &mut self.waiting else {
            return;
        };
        let Some(indices) = waiting.payloads.remove(&slot) else {
            return;
        };
        let outer = std::mem::take(&mut self.open);
        self.run_waiting(&indices);
        self.open = outer;
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
            let assoc: Vec<usize> = (info.assoc.iter().enumerate())
                .filter(|(_, assoc)| !assoc.bounds.is_empty())
                .map(|(k, _)| k)
                .collect();
            if !bounded && requirements.is_empty() && assoc.is_empty() {
                continue;
            }
            let own: Vec<Type> = (params.iter().enumerate().skip(1))
                .map(|(i, &name)| {
                    let i = u32::try_from(i)
                        .expect("a declaration should have fewer than 2^32 parameters");
                    Type::intern(TypeKind::Param(name, i))
                })
                .collect();
            let own = TypeArgs::intern(&own);
            self.with_body(Body::default(), |checker| {
                checker.body.set_type_params(&params);
                checker.body.set_bounds(0, bounds);
                // `Self` is first, and conforms to the interface itself.
                checker.read_places(1);
                // The associated types: each over `Self`.
                let this = Type::intern(TypeKind::Param(params[0], 0));
                for k in assoc {
                    let k =
                        u32::try_from(k).expect("an interface should have fewer than 2^32 types");
                    let ty = Type::intern(TypeKind::Assoc(this, InterfaceId(i), own, k));
                    checker.interfaces_for(ty);
                }
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

/// The associated types that a bound of an interface declares, in the order of the closure and
/// by name, and whether the walk stopped at the closure limit.
pub(super) struct Names {
    list: Vec<Declaration>,
    /// The places in `list` of the declarations of each name.
    by_name: FxHashMap<Symbol, Vec<usize>>,
    wide: bool,
}

/// Whether `ty` nests its arguments, arrays and optionals no deeper than `limit` levels. A type
/// shared by many paths is measured once (`depths`), so the cost is the number of distinct types
/// and not of paths: `Pair<T, T>` at each level has `2^n` paths and `n` types.
pub(super) fn nests_within(ty: Type, limit: usize, depths: &mut FxHashMap<Type, usize>) -> bool {
    nesting(ty, limit, depths).is_some()
}

/// How deep `ty` nests, or `None` when that is more than `limit`. The recursion goes no deeper
/// than `limit`.
fn nesting(ty: Type, limit: usize, depths: &mut FxHashMap<Type, usize>) -> Option<usize> {
    #[cfg(test)]
    super::NESTING_STEPS.set(super::NESTING_STEPS.get() + 1);
    let one;
    let inner: &[Type] = match ty.kind() {
        TypeKind::Array(t) | TypeKind::Optional(t) => {
            one = [t];
            &one
        }
        TypeKind::Class(_, args) | TypeKind::Enum(_, args) => args.get(),
        // A normalized type with no type parameter holds no associated type.
        _ => &[],
    };
    if inner.is_empty() {
        return Some(0);
    }
    if let Some(&depth) = depths.get(&ty) {
        return (depth <= limit).then_some(depth);
    }
    let below = limit.checked_sub(1)?;
    let mut deepest = 0;
    for &t in inner {
        deepest = deepest.max(nesting(t, below, depths)?);
    }
    depths.insert(ty, deepest + 1);
    Some(deepest + 1)
}

/// An associated type that an interface declares, or that one it inherits declares.
#[derive(Clone, Copy)]
pub(super) struct Declaration {
    name: Symbol,
    /// The interface that declares it, and its arguments over the parameters of the interface
    /// that a bound names.
    declarer: InterfaceId,
    args: TypeArgs,
    /// Its place among the types that the declarer declares.
    k: u32,
}

/// What the bounds that write associated types wait for, as `resolve_member_bounds` reads it.
/// A key is an associated type written through a parameter of the list, as `S.Item`.
///
/// A bound of the parameter gives the key a *declaration*: the associated type of that name that
/// its interface, or one the interface inherits, declares, with the arguments that the bound
/// writes. Two bounds that give the same declaration give one, whatever interface inherits it:
/// the declaration is *available* once any of them is in scope. A bound that writes a key waits
/// for every declaration of the key that is not available, except the ones that no other bound
/// gives: its own cannot be the one that it names. The bounds of a parameter that name an
/// interface with the same arguments, in the same text, are a *class*: they give the same
/// declarations, which are read once.
struct Waits {
    /// The key of each such type, by the place of the parameter and the name.
    key_of: FxHashMap<(usize, Symbol), usize>,
    /// The keys of each parameter, by name.
    keys_of: Vec<Vec<(Symbol, usize)>>,
    keys: Vec<WaitKey>,
    /// The keys each bound writes.
    writes: Vec<Vec<usize>>,
    /// Whether some bound writes an associated type through the parameter.
    is_root: Vec<bool>,
    /// For each bound, how many keys it waits for.
    blocked: Vec<usize>,
    /// The declaration of a key, by the interface that declares it and its arguments, where an
    /// argument is the text a bound writes.
    declaration_at: FxHashMap<(usize, InterfaceId, TypeArgs), usize>,
    declarations: Vec<Given>,
    /// The class of each bound, if its interface is read.
    class_of: Vec<Option<usize>>,
    class_at: FxHashMap<(usize, InterfaceId, Vec<Type>), usize>,
    classes: Vec<Class>,
}

#[derive(Default)]
struct WaitKey {
    /// How many declarations are not available yet.
    pending: usize,
    /// The bounds that wait, and the number of pending declarations they can do without, by
    /// that number.
    waiting: Vec<(usize, usize)>,
    declarations: Vec<usize>,
}

struct Given {
    key: usize,
    available: bool,
    /// The classes that give it.
    givers: Vec<usize>,
}

struct Class {
    /// The bounds of the class that are not in scope, or `scoped` when the bounds are in scope.
    members: Vec<usize>,
    scoped: bool,
    /// Whether a bound of the class has resolved.
    fired: bool,
    /// The declarations it gives.
    gives: Vec<usize>,
}

impl Waits {
    /// The keys that the bounds `written` write, through the parameters of a list of `params`.
    fn new(written: &[Vec<(usize, &ast::Ident)>], params: usize) -> Self {
        let mut waits = Waits {
            key_of: FxHashMap::default(),
            keys_of: vec![Vec::new(); params],
            keys: Vec::new(),
            writes: Vec::with_capacity(written.len()),
            is_root: vec![false; params],
            blocked: vec![0; written.len()],
            declaration_at: FxHashMap::default(),
            declarations: Vec::new(),
            class_of: vec![None; written.len()],
            class_at: FxHashMap::default(),
            classes: Vec::new(),
        };
        for pairs in written {
            let mut keys = Vec::with_capacity(pairs.len());
            for &(root, name) in pairs {
                waits.is_root[root] = true;
                let next = waits.keys.len();
                let key = *waits.key_of.entry((root, name.name)).or_insert(next);
                if key == next {
                    waits.keys.push(WaitKey::default());
                    waits.keys_of[root].push((name.name, key));
                }
                keys.push(key);
            }
            waits.writes.push(keys);
        }
        waits
    }

    /// The class of the bounds of `root` that name `interface` with the arguments `with`, and
    /// whether it is new. `member` is the bound to add to it, or `None` for a bound that is in
    /// scope.
    fn class(
        &mut self,
        root: usize,
        interface: InterfaceId,
        with: Vec<Type>,
        member: Option<usize>,
    ) -> (usize, bool) {
        let next = self.classes.len();
        let class = *self.class_at.entry((root, interface, with)).or_insert(next);
        let new = class == next;
        if new {
            self.classes.push(Class {
                members: Vec::new(),
                scoped: false,
                fired: false,
                gives: Vec::new(),
            });
        }
        match member {
            Some(m) => {
                self.classes[class].members.push(m);
                self.class_of[m] = Some(class);
            }
            None => self.classes[class].scoped = true,
        }
        (class, new)
    }

    /// Records that `class` gives the declaration of `key` that `declarer` makes with `args`.
    fn give(&mut self, class: usize, key: usize, declarer: InterfaceId, args: TypeArgs) {
        let next = self.declarations.len();
        let at = *self
            .declaration_at
            .entry((key, declarer, args))
            .or_insert(next);
        if at == next {
            self.declarations.push(Given {
                key,
                available: false,
                givers: Vec::new(),
            });
            self.keys[key].declarations.push(at);
        }
        self.declarations[at].givers.push(class);
        self.classes[class].gives.push(at);
    }

    /// Once every declaration is known: counts what each key waits for, and makes each bound
    /// wait. Gives the bounds that wait for no one.
    fn start(&mut self) -> Vec<usize> {
        for class in self.classes.iter().filter(|class| class.scoped) {
            for &at in &class.gives {
                self.declarations[at].available = true;
            }
        }
        // For a bound and a key, how many pending declarations only the bound gives.
        let mut own: FxHashMap<(usize, usize), usize> = FxHashMap::default();
        for given in self.declarations.iter().filter(|given| !given.available) {
            self.keys[given.key].pending += 1;
            if let [only] = given.givers[..]
                && let [m] = self.classes[only].members[..]
            {
                *own.entry((m, given.key)).or_insert(0) += 1;
            }
        }
        for (m, keys) in self.writes.iter().enumerate() {
            for &key in keys {
                let can_do_without = own.get(&(m, key)).copied().unwrap_or(0);
                if self.keys[key].pending > can_do_without {
                    self.blocked[m] += 1;
                    self.keys[key].waiting.push((m, can_do_without));
                }
            }
        }
        for key in &mut self.keys {
            key.waiting.sort_by_key(|&(_, without)| without);
        }
        (0..self.writes.len())
            .filter(|&m| self.blocked[m] == 0)
            .collect()
    }

    /// One declaration of the key is available: the bounds that can do without the ones left are
    /// ready now.
    fn lower(&mut self, key: usize, ready: &mut Vec<usize>) {
        let key = &mut self.keys[key];
        key.pending -= 1;
        while let Some(&(m, without)) = key.waiting.last() {
            if without < key.pending {
                break;
            }
            key.waiting.pop();
            self.blocked[m] -= 1;
            if self.blocked[m] == 0 {
                ready.push(m);
            }
        }
    }

    /// Records that the bounds `wave` are resolved, and gives the bounds that wait for no one
    /// now. The first bound of a class makes available the declarations that the class gives.
    fn resolved(&mut self, wave: &[usize]) -> Vec<usize> {
        let mut ready = Vec::new();
        for &m in wave {
            let Some(c) = self.class_of[m] else {
                continue;
            };
            if std::mem::replace(&mut self.classes[c].fired, true) {
                continue;
            }
            for k in 0..self.classes[c].gives.len() {
                #[cfg(test)]
                super::BOUND_STEPS.set(super::BOUND_STEPS.get() + 1);
                let at = self.classes[c].gives[k];
                if !std::mem::replace(&mut self.declarations[at].available, true) {
                    self.lower(self.declarations[at].key, &mut ready);
                }
            }
        }
        ready
    }

    /// The associated types, as the parameter and the name, that the bounds left write and that
    /// keep a bound that is on a loop of bounds waiting. A bound that only waits for a loop is
    /// not on it.
    fn on_loops<'a>(
        &self,
        written: &[Vec<(usize, &'a ast::Ident)>],
    ) -> Vec<(usize, &'a ast::Ident)> {
        // The bounds, then the keys, then the declarations, then the classes: a bound waits for
        // its keys, a key for the declarations that are not available, a declaration for the
        // classes that give it, and a class for its bounds.
        let (bounds, keys) = (self.writes.len(), self.keys.len());
        let classes = bounds + keys + self.declarations.len();
        let mut edges = vec![Vec::new(); classes + self.classes.len()];
        let mut waits = FxHashSet::default();
        for (k, key) in self.keys.iter().enumerate() {
            for &(m, _) in &key.waiting {
                edges[m].push(bounds + k);
                waits.insert((m, k));
            }
            let pending = key
                .declarations
                .iter()
                .filter(|&&d| !self.declarations[d].available);
            edges[bounds + k].extend(pending.map(|&d| bounds + keys + d));
        }
        for (d, given) in self.declarations.iter().enumerate() {
            edges[bounds + keys + d].extend(given.givers.iter().map(|c| classes + c));
        }
        for (c, class) in self.classes.iter().enumerate() {
            edges[classes + c].clone_from(&class.members);
        }
        let component = components(&edges);
        let mut size = vec![0_usize; component.iter().max().map_or(0, |c| c + 1)];
        for &c in &component {
            size[c] += 1;
        }
        let mut out = Vec::new();
        for m in (0..bounds).filter(|&m| size[component[m]] > 1) {
            for &(root, name) in &written[m] {
                if waits.contains(&(m, self.key_of[&(root, name.name)])) {
                    out.push((root, name));
                }
            }
        }
        out
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
