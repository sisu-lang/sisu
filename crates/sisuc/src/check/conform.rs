//! Conformances of classes and enums to interfaces: the ones a declaration lists and the ones an
//! `extend` block writes, the requirements each meets, and the conformances each needs.

use rustc_hash::{FxHashMap, FxHashSet};

use super::extend::holder_of;
use super::interface::{Bound, RequirementInfo};
use super::{Body, Checker, Choice, Choices, Declared, Holder, own_args};
use crate::ast;
use crate::conformance::{self, find};
use crate::diagnostic::{ConformanceForm, DiagnosticKind, NotConformForm, Ungenerated};
use crate::front_end::LANG_SOURCE;
use crate::source::{PRELUDE_BASE, Source as Text, Span};
use crate::symbol::Symbol;
use crate::tir::{self, Conformances, InterfaceId, Type, TypeKind};

/// Where the methods that meet a conformance's requirements are.
#[derive(Clone, Copy)]
enum Source {
    /// The body of the class or the enum.
    Body(Holder),
    /// The `extend` block at this place among the blocks.
    Block(usize),
}

/// A conformance that is recorded and not yet checked.
struct Pending {
    interface: Bound,
    target: Type,
    /// Where the interface is written in the conformance: in the list of the class or the enum,
    /// or in the block. For an interface that the list gets from another, where that is.
    at: Span,
    source: Source,
    /// Among the conformances to `interface`.
    index: usize,
    /// Whether the method `to_string` of the type made the conformance, with no interface written.
    by_name: bool,
}

/// A listed interface that the compiler writes for the type, with no method of the type to meet
/// it: `enum Expr: Eq` or `class Node: Display`. Checked as Rust's `derive` is, once every
/// conformance is known.
struct Derived {
    interface: Bound,
    target: Type,
    at: Span,
    source: Source,
}

/// What the recording of conformances keeps while it runs.
#[derive(Default)]
struct Added {
    /// Where each conformance is written, by interface and index.
    ats: FxHashMap<(InterfaceId, usize), Span>,
    /// The listed interfaces that the compiler writes.
    derived: Vec<Derived>,
    /// The conformances in `derived` with no methods, so that a repeated listing is found by the
    /// same overlap test as a written one, and checked once.
    seen: Conformances,
    /// Where each conformance of `seen` is listed, by interface and index.
    seen_ats: FxHashMap<(InterfaceId, usize), Span>,
}

/// An interface that the list of a class or an enum gets from another, when a block gave the type
/// its conformance.
struct Found {
    holder: Holder,
    receiver: Type,
    inherited: (InterfaceId, tir::TypeArgs),
    /// The listed interface that the entry is reached through, in the list.
    at: Span,
    /// The interface that inherits it, and where it writes the inheritance.
    via: (InterfaceId, Span),
}

/// What a class or an enum lists: its holder, its type, and each interface with where it is
/// written.
type Listed = (Holder, Type, Vec<(Bound, Span)>);

impl Checker<'_> {
    /// Pass 2b, after every signature: reports the methods of one name that a type holds twice,
    /// records each conformance and reports the second of any two that overlap, then checks that
    /// each meets its requirements and, for an `extend` block, the conformances it needs.
    pub(super) fn conformances(&mut self, items: &[&ast::Item]) {
        self.extension_clashes();
        let mut added = Added::default();
        let mut pending = Vec::new();
        let lists = self.listed_conformances(items);
        for (holder, receiver, roots) in &lists {
            for &(bound, at) in roots {
                let source = Source::Body(*holder);
                let one = self.add_listed(bound, *receiver, at, source, (&mut added, None));
                pending.extend(one);
            }
        }
        let blocks: Vec<_> = (self.extensions.iter().enumerate())
            .filter_map(|(i, ext)| {
                let (bound, at) = ext.interface?;
                Some((i, bound, at, ext.target))
            })
            .collect();
        for (block, bound, at, target) in blocks {
            let source = Source::Block(block);
            let derived = added.derived.len();
            let one = self.add_listed(bound, target, at, source, (&mut added, None));
            // A listing that the compiler writes is no second conformance.
            self.extensions[block].duplicate = one.is_none() && added.derived.len() == derived;
            pending.extend(one);
        }
        // A listing that the compiler writes is a second conformance when a body or a block
        // writes the same one. Those that a listed interface inherits are no listing.
        let mut clashed = FxHashSet::default();
        for (i, d) in added.derived.iter().enumerate() {
            let params = self.source_params(d.source).0.len();
            let Bound {
                interface, args, ..
            } = d.interface;
            let clash = conformance::clash(&self.conformances, interface, d.target, params, args);
            if let Some((k, common)) = clash {
                let earlier = added.ats[&(interface, k)];
                self.conforms_twice(d.interface, &common, (earlier, d.at));
                clashed.insert(i);
            }
        }
        let (inherited_found, gives_display) =
            self.listed_closures(&lists, &mut added, &mut pending);
        pending.extend(self.name_displays(&gives_display, &mut added));
        // The choices of the associated types are recorded: an enum that grows through one is
        // marked, before the first query.
        let growing = self.growing_enums();
        for (marked, grows) in self.marked.iter_mut().zip(growing) {
            *marked |= grows;
        }
        // Every conformance is known now: the checks that waited for them run, and a type that
        // fails is unknown before a requirement is compared with a method.
        self.run_pending_bounds();
        // A listing that clashes is reported once: its rule is not checked as well.
        for (i, derived) in added.derived.iter().enumerate() {
            if !clashed.contains(&i) {
                self.check_derived(derived);
            }
        }
        for p in &pending {
            self.meet_requirements(p);
        }
        for p in &pending {
            self.check_choices(p);
        }
        self.stray_choices(&lists, &pending);
        for p in pending
            .iter()
            .filter(|p| matches!(p.source, Source::Block(_)))
        {
            self.inherited_conformances(p);
        }
        for found in &inherited_found {
            self.listed_inheritance(found);
        }
        for (interface, conformance) in self.conformances.iter() {
            for &method in &conformance.methods {
                self.meets.entry(method).or_default().push(interface);
            }
        }
        // A broken block may be named late: what was answered before it is asked again.
        self.conformed.clear();
        self.leaves.clear();
    }

    /// Records the interfaces that the listed ones inherit, and gives the conformances that a
    /// block gave instead, which are asked once every conformance is known, and the holders whose
    /// list gives them `Display`.
    fn listed_closures(
        &mut self,
        lists: &[Listed],
        added: &mut Added,
        pending: &mut Vec<Pending>,
    ) -> (Vec<Found>, FxHashSet<Holder>) {
        // The interfaces a listed one inherits are the type's too, unless a block gives them: the
        // body holds their methods as well.
        //
        // The third recorded exception to "A pass is linear in the program" (user, 2026-10-25;
        // ruling F9): a list's closure has an entry for each conformance that the list gives, and
        // the closure of arguments may double at each level of inheritance, so the walk of each
        // listed interface stops at `BOUND_CLOSURE_LIMIT` entries and E0157 is reported at it.
        // The work follows the sum of the closures, which are at most the limit each.
        let mut inherited_found = Vec::new();
        // The holders whose list gives them `Display`, directly or by inheritance.
        let mut gives_display = FxHashSet::default();
        for (holder, receiver, roots) in lists {
            let mut closure = Vec::new();
            // The listed interface through which each entry of `closure` is reached.
            let mut via = Vec::new();
            // For each entry of `closure`: the entry that inherits it and where that writes the
            // inheritance, or `None` for a listed interface.
            let mut parents: Vec<Option<(usize, Span)>> = Vec::new();
            let mut seen = FxHashSet::default();
            let mut wide = false;
            for &(bound, at) in roots {
                let mut reached = Vec::new();
                let walked = self.extend_closure(
                    *receiver,
                    &[bound],
                    &mut closure,
                    &mut seen,
                    |parent, written| reached.push(Some((parent, written))),
                );
                // A new root comes first, and the callback does not report it.
                if closure.len() > parents.len() {
                    reached.insert(0, None);
                }
                parents.extend(reached);
                via.resize(closure.len(), at);
                if walked.is_err() {
                    // The conformances of the list are unknown now: what the walk reached before
                    // the limit is no part of them.
                    self.too_wide_at(*receiver, at);
                    wide = true;
                    break;
                }
            }
            if closure.iter().any(|&(i, _)| i == self.lang.display) {
                gives_display.insert(*holder);
            }
            if wide {
                continue;
            }
            for (k, (&(interface, args), &at)) in closure.iter().zip(&via).enumerate() {
                if find(&self.conformances, interface, *receiver, args).is_some() {
                    // A block gave it: it holds under the conditions of the block, which the
                    // list's own parameters must meet. Asked once every conformance is known.
                    if let Some((parent, requires)) = parents[k] {
                        inherited_found.push(Found {
                            holder: *holder,
                            receiver: *receiver,
                            inherited: (interface, args),
                            at,
                            via: (closure[parent].0, requires),
                        });
                    }
                    continue;
                }
                // A listed interface that the compiler writes was added with the list.
                if parents[k].is_none() {
                    continue;
                }
                let bound = Bound {
                    interface,
                    args,
                    at,
                };
                let source = Source::Body(*holder);
                let from = parents[k].map(|(parent, written)| (closure[parent].0, written));
                let one = self.add_listed(bound, *receiver, at, source, (added, from));
                pending.extend(one);
            }
        }
        (inherited_found, gives_display)
    }

    /// For each class and enum that lists interfaces, what it lists. Reports a list entry that
    /// names no interface.
    fn listed_conformances(&mut self, items: &[&ast::Item]) -> Vec<Listed> {
        let mut lists = Vec::new();
        let (mut next_class, mut next_enum) = (0, 0);
        for item in items {
            let (holder, params, conforms, assoc) = match item {
                ast::Item::Class(class) => {
                    let id = tir::ClassId(next_class);
                    next_class += 1;
                    let params = self.classes[id.0].type_params.clone();
                    (Holder::Class(id), params, &class.conforms, &class.assoc)
                }
                ast::Item::Enum(e) => {
                    let id = tir::EnumId(next_enum);
                    next_enum += 1;
                    let params = self.enums[id.0].type_params.clone();
                    (Holder::Enum(id), params, &e.conforms, &e.assoc)
                }
                _ => continue,
            };
            if conforms.is_empty() && assoc.is_empty() {
                continue;
            }
            let args = own_args(&params);
            let receiver = Type::intern(match holder {
                Holder::Class(id) => TypeKind::Class(id, args),
                Holder::Enum(id) => TypeKind::Enum(id, args),
            });
            self.body.self_type = Some(receiver);
            self.body.set_type_params(&params);
            let mut roots = Vec::new();
            for ty in conforms {
                let Some(bound) = self.bound(ty, Some(receiver)) else {
                    continue;
                };
                roots.push((bound, ty.bare));
            }
            self.body.set_type_params(&[]);
            self.body.self_type = None;
            lists.push((holder, receiver, roots));
        }
        lists
    }

    /// The type parameters of the class, enum or block that `source` is, which are those of the
    /// conformance, and their bounds.
    fn source_params(&self, source: Source) -> (Vec<Symbol>, Vec<Vec<Bound>>) {
        match source {
            Source::Body(Holder::Class(c)) => {
                let info = &self.classes[c.0];
                (info.type_params.clone(), info.bounds.clone())
            }
            Source::Body(Holder::Enum(e)) => {
                let info = &self.enums[e.0];
                (info.type_params.clone(), info.bounds.clone())
            }
            Source::Block(b) => {
                let ext = &self.extensions[b];
                (ext.type_params.clone(), ext.bounds.clone())
            }
        }
    }

    /// Records the conformance of `target` to `bound`, which is written at `at`, unless it
    /// overlaps one recorded: that is reported at the one that starts last. The type parameters
    /// of the conformance are those of `source`.
    fn add_conformance(
        &mut self,
        bound: Bound,
        target: Type,
        at: Span,
        source: Source,
        added: &mut Added,
    ) -> Option<Pending> {
        let (params, bounds) = self.source_params(source);
        let clash = conformance::clash(
            &self.conformances,
            bound.interface,
            target,
            params.len(),
            bound.args,
        );
        if let Some((k, common)) = clash {
            let earlier = added.ats[&(bound.interface, k)];
            self.conforms_twice(bound, &common, (earlier, at));
            return None;
        }
        let (assoc, written) = self.chosen(bound.interface, source);
        let index = self.conformances.add(
            bound.interface,
            tir::Conformance {
                type_params: params.len(),
                target,
                args: bound.args,
                methods: Vec::new(),
                assoc,
                bounds: if bounds.iter().any(|bounds| !bounds.is_empty()) {
                    (bounds.iter())
                        .map(|list| {
                            list.iter()
                                .map(|b| tir::ParamBound {
                                    interface: b.interface,
                                    args: b.args,
                                })
                                .collect()
                        })
                        .collect()
                } else {
                    Vec::new()
                },
            },
        );
        for (k, at) in written {
            self.choice_ats.insert((bound.interface, index, k), at);
        }
        added.ats.insert((bound.interface, index), at);
        if bounds.iter().any(|bounds| !bounds.is_empty()) {
            self.conformance_bounds
                .insert((bound.interface, index), (params, bounds));
        }
        Some(Pending {
            interface: bound,
            target,
            at,
            source,
            index,
            by_name: false,
        })
    }

    /// Reports that the type `common[0]` conforms to the interface of `bound` with the arguments
    /// `common[1..]` twice: at the one of the two places that starts last, with the other as the
    /// first.
    fn conforms_twice(&mut self, bound: Bound, common: &[Type], places: (Span, Span)) {
        let (first, second) = if places.0.start() <= places.1.start() {
            places
        } else {
            (places.1, places.0)
        };
        self.report(DiagnosticKind::ConformsTwice {
            ty: self.type_name(common[0]),
            interface: self.bound_text(bound.interface, tir::TypeArgs::intern(&common[1..])),
            at: second,
            first,
            generic: !bound.args.get().is_empty(),
        });
    }

    /// `add_conformance` for an interface that a list or an `extend` block writes, or that a list
    /// gets from `via`, an interface that inherits it, and where it writes that. Only a class
    /// conforms to `AnyObject`; that is reported, and the conformance is recorded all the same,
    /// so that nothing else reports it. An interface that the compiler writes for the type, which
    /// no method of the type meets (`enum Expr: Eq`), adds no record: the generated rule answers.
    fn add_listed(
        &mut self,
        bound: Bound,
        target: Type,
        at: Span,
        source: Source,
        (added, via): (&mut Added, Option<(InterfaceId, Span)>),
    ) -> Option<Pending> {
        if bound.interface == self.lang.any_object && !matches!(target.kind(), TypeKind::Class(..))
        {
            self.report_class_only(target, at, via);
        }
        if self.is_derived(bound.interface, target, source) {
            let params = self.source_params(source).0.len();
            let clash =
                conformance::clash(&added.seen, bound.interface, target, params, bound.args);
            if let Some((k, common)) = clash {
                // An interface that another listed one inherits is no second listing.
                if via.is_none() {
                    let earlier = added.seen_ats[&(bound.interface, k)];
                    self.conforms_twice(bound, &common, (earlier, at));
                }
                return None;
            }
            let index = added.seen.add(
                bound.interface,
                tir::Conformance {
                    type_params: params,
                    target,
                    args: bound.args,
                    methods: Vec::new(),
                    assoc: Vec::new(),
                    bounds: Vec::new(),
                },
            );
            added.seen_ats.insert((bound.interface, index), at);
            added.derived.push(Derived {
                interface: bound,
                target,
                at,
                source,
            });
            return None;
        }
        self.add_conformance(bound, target, at, source, added)
    }

    /// Reports that `target`, which is no class, conforms to `AnyObject` at `at`, through `via`
    /// when an interface it lists inherits `AnyObject`.
    fn report_class_only(&mut self, target: Type, at: Span, via: Option<(InterfaceId, Span)>) {
        let via = via.map(|(interface, written)| {
            (self.interfaces[interface.0].name.name.to_string(), written)
        });
        self.report(DiagnosticKind::ClassOnly {
            ty: self.type_name(target),
            at,
            via,
        });
    }

    /// Whether the listing of `interface` for `target` in `source` is one that the compiler writes,
    /// with no method to meet it: an enum's `Eq` or any type's `Display`, when `source` has no
    /// method of the requirement's name, and an `extend` block has no method at all. A broken
    /// method counts as one: its own error says what is wrong.
    fn is_derived(&self, interface: InterfaceId, target: Type, source: Source) -> bool {
        let eq_of_enum = interface == self.lang.eq && matches!(target.kind(), TypeKind::Enum(..));
        if !eq_of_enum && interface != self.lang.display {
            return false;
        }
        match source {
            Source::Body(holder) => {
                let methods = match holder {
                    Holder::Class(c) => &self.classes[c.0].methods,
                    Holder::Enum(e) => &self.enums[e.0].methods,
                };
                (self.interfaces[interface.0].requirements.iter())
                    .all(|requirement| !methods.contains_key(&requirement.name.name))
            }
            Source::Block(block) => {
                let ext = &self.extensions[block];
                ext.methods.is_empty()
            }
        }
    }

    /// Records the `Display` of each type whose method `to_string` makes it: the first method of
    /// that name in the body of a class or an enum, or else in a plain `extend` block. A type
    /// whose list gives it `Display` (`gives`) and whose body has the method has the conformance
    /// already: the method meets it. A list that no method meets adds no record, so the method of
    /// a plain block makes the `Display` then. The method must have `Display`'s signature, as
    /// `meet_requirements` checks. A second method of the name has its own error, so it adds no
    /// second conformance.
    fn name_displays(&mut self, gives: &FxHashSet<Holder>, added: &mut Added) -> Vec<Pending> {
        let name = Symbol::intern("to_string");
        let mut named: Vec<(Holder, Type, Source, tir::FuncId)> = Vec::new();
        for (c, class) in self.classes.iter().enumerate() {
            if let Some(&Declared::Known(func)) = class.methods.get(&name) {
                let holder = Holder::Class(tir::ClassId(c));
                let args = own_args(&class.type_params);
                let ty = Type::intern(TypeKind::Class(tir::ClassId(c), args));
                named.push((holder, ty, Source::Body(holder), func));
            }
        }
        for (e, info) in self.enums.iter().enumerate() {
            if let Some(&Declared::Known(func)) = info.methods.get(&name) {
                let holder = Holder::Enum(tir::EnumId(e));
                let args = own_args(&info.type_params);
                let ty = Type::intern(TypeKind::Enum(tir::EnumId(e), args));
                named.push((holder, ty, Source::Body(holder), func));
            }
        }
        for (block, ext) in self.extensions.iter().enumerate() {
            if let (None, Some(&func)) = (ext.interface, ext.methods.get(&name)) {
                named.push((ext.holder, ext.target, Source::Block(block), func));
            }
        }
        let mut pending = Vec::new();
        // The holders with a conformance from a method already.
        let mut done = FxHashSet::default();
        for (holder, target, source, func) in named {
            let met_by_list = matches!(source, Source::Body(_)) && gives.contains(&holder);
            if !done.insert(holder) || met_by_list {
                continue;
            }
            let at = self.signatures[func.0].span;
            let bound = Bound {
                interface: self.lang.display,
                args: tir::TypeArgs::NONE,
                at,
            };
            if let Some(mut one) = self.add_conformance(bound, target, at, source, added) {
                one.by_name = true;
                pending.push(one);
            }
        }
        pending
    }

    /// Checks a listing that the compiler writes, as Rust's `derive` is checked: the generated
    /// rule must hold for the type with each of its parameters taken to conform, so `enum
    /// Wrap<T>: Eq { A(T) }` is clean, and `enum H: Eq { N(Node) }` is not. `Display` holds for
    /// every type, so only an enum's `Eq` can fail, and then the failing leaf is named.
    fn check_derived(&mut self, derived: &Derived) {
        let Derived {
            interface,
            target,
            at,
            source,
        } = *derived;
        if interface.interface != self.lang.eq {
            return;
        }
        let Some(leaf) = self.derive_failure(interface, target, source) else {
            return;
        };
        let info = &self.interfaces[interface.interface.0];
        let requirement = &info.requirements[0];
        let signature = self.required_text(source, interface, requirement);
        self.report(DiagnosticKind::ConformanceBody {
            ty: self.type_name(target),
            interface: info.name.name.to_string(),
            at,
            form: ConformanceForm::Missing {
                signatures: vec![signature],
                generated: Some(Ungenerated::Since(self.type_name(leaf))),
            },
        });
    }

    /// The part that makes the rule that the compiler writes for `interface` fail for `target`, as
    /// Rust's `derive` is checked: each parameter of `source` is taken to conform, besides its own
    /// bounds. `None` when the rule holds.
    fn derive_failure(&mut self, interface: Bound, target: Type, source: Source) -> Option<Type> {
        let (params, declared) = self.source_params(source);
        let bounds: Vec<Vec<Bound>> = (0..params.len())
            .map(|i| {
                let mut list = declared.get(i).cloned().unwrap_or_default();
                list.push(interface);
                list
            })
            .collect();
        let (leaf, _) = self.with_body(Body::default(), |checker| {
            checker.body.set_type_params(&params);
            checker.body.set_bounds(0, bounds);
            checker.rule_failure(target, interface.interface)
        });
        leaf
    }

    /// Reports what the conformance `p` lacks of its interface's requirements, and records the
    /// methods that meet them. A method of an `extend` block that is no requirement is reported.
    fn meet_requirements(&mut self, p: &Pending) {
        let info = &self.interfaces[p.interface.interface.0];
        let (ty, interface) = (self.type_name(p.target), info.name.name.to_string());
        let mut base = vec![p.target];
        base.extend_from_slice(p.interface.args.get());
        let mut methods = Vec::new();
        let mut missing = Vec::new();
        let mut reports = Vec::new();
        for requirement in &info.requirements {
            let name = requirement.name.name;
            let found = match p.source {
                Source::Body(Holder::Class(c)) => self.classes[c.0].methods.get(&name).copied(),
                Source::Body(Holder::Enum(e)) => self.enums[e.0].methods.get(&name).copied(),
                Source::Block(b) => {
                    let ext = &self.extensions[b];
                    match ext.methods.get(&name) {
                        Some(&f) => Some(Declared::Known(f)),
                        None => self
                            .broken_extension_methods
                            .contains(&(ext.holder, name))
                            .then_some(Declared::Broken),
                    }
                }
            };
            match found {
                None => missing.push(self.required_text(p.source, p.interface, requirement)),
                // Its own error is reported.
                Some(Declared::Broken) => {}
                Some(Declared::Known(f)) if self.meets(requirement, f, &base) => methods.push(f),
                Some(Declared::Known(f)) => {
                    let sig = &self.signatures[f.0];
                    reports.push(DiagnosticKind::ConformanceBody {
                        ty: ty.clone(),
                        interface: interface.clone(),
                        at: sig.span,
                        form: ConformanceForm::Wrong {
                            method: name.to_string(),
                            found: self.source_text(sig.header).trim_end().to_string(),
                            want: self.required_text(p.source, p.interface, requirement),
                        },
                    });
                }
            }
        }
        // The choices come after the methods in the help, as the interface's types do in its text.
        for declared in &info.assoc {
            if !self.chooses(p.source, declared.name.name) {
                missing.push(format!("type {}", declared.name.name));
            }
        }
        if !missing.is_empty() {
            let generated = self.ungenerated(p);
            reports.push(DiagnosticKind::ConformanceBody {
                ty: ty.clone(),
                interface: interface.clone(),
                at: p.at,
                form: ConformanceForm::Missing {
                    signatures: missing,
                    generated,
                },
            });
        }
        if let (Source::Block(b), false) = (p.source, p.by_name) {
            let info = &self.interfaces[p.interface.interface.0];
            let required: FxHashSet<Symbol> = (info.requirements.iter())
                .map(|requirement| requirement.name.name)
                .collect();
            for (&name, &f) in &self.extensions[b].methods {
                if !required.contains(&name) {
                    reports.push(DiagnosticKind::ConformanceBody {
                        ty: ty.clone(),
                        interface: interface.clone(),
                        at: self.signatures[f.0].span,
                        form: ConformanceForm::NotRequirement {
                            method: name.to_string(),
                            target: ty.clone(),
                        },
                    });
                }
            }
        }
        self.conformances
            .get_mut(p.interface.interface, p.index)
            .methods = methods;
        for report in reports {
            self.report(report);
        }
    }

    /// Why the compiler writes no conformance in place of `p`, when `p` is one of `Eq` and `Ord`:
    /// nothing generates `Eq` for a class, or `Ord` for anything that is not built in, and an enum
    /// gets `Eq` unless a part of it has none, which is the part to name. `None` when it would
    /// write one, as it does for an `Eq` block that holds a method of another name.
    fn ungenerated(&mut self, p: &Pending) -> Option<Ungenerated> {
        let language = p.interface.interface;
        if language == self.lang.ord {
            return Some(Ungenerated::Never);
        }
        if language != self.lang.eq {
            return None;
        }
        if !matches!(p.target.kind(), TypeKind::Enum(..)) {
            return Some(Ungenerated::Never);
        }
        let leaf = self.derive_failure(p.interface, p.target, p.source)?;
        Some(Ungenerated::Since(self.type_name(leaf)))
    }

    /// Whether `source` writes a choice for the associated type `name`, whether or not its type
    /// resolved.
    fn chooses(&self, source: Source, name: Symbol) -> bool {
        self.choice(source, name).is_some()
    }

    /// The choice of the associated type `name` that `source` makes.
    fn choice(&self, source: Source, name: Symbol) -> Option<&Choice> {
        #[cfg(test)]
        super::CHOICE_STEPS.set(super::CHOICE_STEPS.get() + 1);
        self.choices(source).get(name)
    }

    /// The associated types that `source` chooses.
    fn choices(&self, source: Source) -> &Choices {
        match source {
            Source::Body(Holder::Class(c)) => &self.classes[c.0].assoc,
            Source::Body(Holder::Enum(e)) => &self.enums[e.0].assoc,
            Source::Block(b) => &self.extensions[b].assoc,
        }
    }

    /// Checks that each type that the conformance `p` chooses meets the bounds its associated
    /// type declares, with the type parameters and bounds of the class, the enum or the block in
    /// scope. A bound that fails is reported at the type, with the bound as its secondary label.
    fn check_choices(&mut self, p: &Pending) {
        let interface = p.interface.interface;
        let declared: Vec<Vec<Bound>> = (self.interfaces[interface.0].assoc.iter())
            .map(|assoc| assoc.bounds.clone())
            .collect();
        if declared.iter().all(Vec::is_empty) {
            return;
        }
        let (params, bounds) = self.source_params(p.source);
        let mut with = vec![p.target];
        with.extend_from_slice(p.interface.args.get());
        // Each declared type, with the type that is chosen for it and where that is written.
        let chosen: Vec<Option<(Type, Span)>> = (self.interfaces[interface.0].assoc.iter())
            .map(|assoc| {
                let choice = self.choice(p.source, assoc.name.name)?;
                Some((choice.ty?, choice.at))
            })
            .collect();
        self.with_body(Body::default(), |checker| {
            checker.body.set_type_params(&params);
            checker.body.set_bounds(0, bounds);
            for (chosen, bounds) in chosen.into_iter().zip(declared) {
                let Some((ty, at)) = chosen else { continue };
                for bound in bounds {
                    checker.check_bound(at, ty, bound, &with);
                }
            }
        });
    }

    /// Reports each choice of a class, an enum or an `extend` block that no interface it
    /// conforms to declares: the third form of E0152. `lists` are the declarations that list
    /// interfaces, and `pending` the conformances, which hold the interfaces that a list gets
    /// from another. Each conformance is read once to find the associated types of its holder.
    fn stray_choices(&mut self, lists: &[Listed], pending: &[Pending]) {
        let mut reports = Vec::new();
        // The names of the associated types that the interfaces of each listing holder declare.
        let mut declared: FxHashMap<Holder, FxHashSet<Symbol>> = FxHashMap::default();
        for p in pending {
            let assoc = &self.interfaces[p.interface.interface.0].assoc;
            #[cfg(test)]
            super::CHOICE_STEPS.set(super::CHOICE_STEPS.get() + 1 + assoc.len());
            if let Source::Body(holder) = p.source {
                declared
                    .entry(holder)
                    .or_default()
                    .extend(assoc.iter().map(|assoc| assoc.name.name));
            }
        }
        for (holder, receiver, roots) in lists {
            if roots.is_empty() {
                continue;
            }
            let of: Vec<String> = (roots.iter())
                .map(|(bound, _)| self.interfaces[bound.interface.0].name.name.to_string())
                .collect();
            for choice in self.choices(Source::Body(*holder)).iter() {
                #[cfg(test)]
                super::CHOICE_STEPS.set(super::CHOICE_STEPS.get() + 1);
                if !declared
                    .get(holder)
                    .is_some_and(|names| names.contains(&choice.name.name))
                {
                    reports.push(self.not_associated(*receiver, choice, &of));
                }
            }
        }
        for (b, ext) in self.extensions.iter().enumerate() {
            let Some((bound, _)) = ext.interface else {
                continue;
            };
            let info = &self.interfaces[bound.interface.0];
            #[cfg(test)]
            super::CHOICE_STEPS.set(super::CHOICE_STEPS.get() + info.assoc.len());
            let declared: FxHashSet<Symbol> =
                info.assoc.iter().map(|assoc| assoc.name.name).collect();
            let of = vec![info.name.name.to_string()];
            for choice in self.choices(Source::Block(b)).iter() {
                #[cfg(test)]
                super::CHOICE_STEPS.set(super::CHOICE_STEPS.get() + 1);
                if !declared.contains(&choice.name.name) {
                    reports.push(self.not_associated(ext.target, choice, &of));
                }
            }
        }
        for report in reports {
            self.report(report);
        }
    }

    /// The report that `choice`, of the type `ty`, names an associated type of none of `of`.
    fn not_associated(&self, ty: Type, choice: &Choice, of: &[String]) -> DiagnosticKind {
        DiagnosticKind::ConformanceBody {
            ty: self.type_name(ty),
            interface: of[0].clone(),
            at: choice.name.span,
            form: ConformanceForm::NotAssociated {
                name: choice.name.name.to_string(),
                of: of.to_vec(),
            },
        }
    }

    /// Reports each interface that the one in the `extend` block `p` inherits and the type does
    /// not conform to. A block holds the methods of its interface only, so an inherited
    /// interface comes from the type's list or from a block of its own.
    ///
    /// The third recorded exception to "A pass is linear in the program" (user, 2026-10-25;
    /// ruling F9): the closure of the block's interface has an entry for each conformance that
    /// the type may need, and each missing one is its own report. The closure records which entry
    /// inherits each other, so a report reads that and does not search.
    fn inherited_conformances(&mut self, p: &Pending) {
        let holder =
            holder_of(p.target).expect("a conformance's target should be a class or an enum");
        // A broken block may have written the conformance.
        if self.broken_extensions.contains(&self.holder_name(holder)) {
            return;
        }
        let mut closure = Vec::new();
        // For each entry of `closure` after the first: the entry that inherits it, nearest first,
        // and where that one writes the inheritance.
        let mut reached = Vec::new();
        let mut seen = FxHashSet::default();
        let bounds = [p.interface];
        let walked = self.extend_closure(
            p.target,
            &bounds,
            &mut closure,
            &mut seen,
            |from, written| {
                reached.push((from, written));
            },
        );
        if walked.is_err() {
            // The walk of the root added nothing, so no conformance is asked for.
            self.too_wide_at(p.target, p.at);
        }
        let Source::Block(block) = p.source else {
            unreachable!("only an `extend` block should ask for its inherited conformances");
        };
        // The conformance holds under the conditions of the block: its bounds are in scope.
        let (params, bounds) = self.source_params(p.source);
        let scope = Body::default();
        self.with_body(scope, |checker| {
            checker.body.set_type_params(&params);
            checker.body.set_bounds(0, bounds);
            for (k, &(interface, args)) in closure.iter().enumerate().skip(1) {
                let (parent, requires) = reached[k - 1];
                let via = closure[parent].0;
                if interface == checker.lang.any_object
                    && !matches!(p.target.kind(), TypeKind::Class(..))
                {
                    checker.report_class_only(p.target, p.at, Some((via, requires)));
                    continue;
                }
                if checker.conforms(p.target, interface, args) {
                    continue;
                }
                checker.report_inherited(
                    (p.target, p.at),
                    Some(block),
                    (interface, args),
                    (via, requires),
                );
            }
        });
    }

    /// Reports an interface that the list of a class or an enum gets from another when the block
    /// that gives it fails under the list's own parameters and bounds.
    fn listed_inheritance(&mut self, found: &Found) {
        let (params, bounds) = self.source_params(Source::Body(found.holder));
        let (interface, args) = found.inherited;
        self.with_body(Body::default(), |checker| {
            checker.body.set_type_params(&params);
            checker.body.set_bounds(0, bounds);
            if !checker.conforms(found.receiver, interface, args) {
                checker.report_inherited(
                    (found.receiver, found.at),
                    None,
                    found.inherited,
                    found.via,
                );
            }
        });
    }

    /// Reports that `target` does not conform to `inherited`, which the interface `via` inherits
    /// at `requires`, at `at` where the list or the `extend` block `block` writes the
    /// conformance. The type has no conformance to it, or has one whose bounds fail under the
    /// conditions of the list or the block.
    fn report_inherited(
        &mut self,
        (target, at): (Type, Span),
        block: Option<usize>,
        (interface, args): (InterfaceId, tir::TypeArgs),
        (via, requires): (InterfaceId, Span),
    ) {
        let (ty, inherited) = (self.type_name(target), self.bound_text(interface, args));
        let (mut extend, mut needs) = (None, None);
        match find(&self.conformances, interface, target, args) {
            // Following the advice would give `ConformsTwice`, so the report points at the bound
            // that the other conformance needs instead.
            Some((k, bound)) => needs = Some(self.failed_bound(interface, k, bound)),
            None => extend = block.map(|b| self.extend_help(b, &ty, &inherited)),
        }
        self.report(DiagnosticKind::NotConform {
            ty,
            interface: inherited,
            at,
            form: NotConformForm::Inherited {
                via: self.interfaces[via.0].name.name.to_string(),
                // No label points into the prelude.
                requires: (!requires.in_prelude()).then_some(requires),
                extend,
                needs,
            },
        });
    }

    /// The first bound of the conformance `index` of `interface` that fails for `bound`, what
    /// stands for its parameters: the bound as that conformance writes it, and where.
    pub(super) fn failed_bound(
        &mut self,
        interface: InterfaceId,
        index: usize,
        bound: tir::TypeArgs,
    ) -> (String, Span) {
        let (params, bounds) = (self.conformance_bounds.get(&(interface, index)))
            .expect("a conformance that a type does not meet should have bounds")
            .clone();
        let failing = (bounds.iter().enumerate())
            .flat_map(|(i, bounds)| bounds.iter().map(move |b| (i, b)))
            .find(|&(i, b)| {
                let args = self.subst_args(b.args, bound.get());
                !self.conforms(bound.get()[i], b.interface, args)
            })
            .expect("a conformance that a type does not meet should have a bound that fails");
        let (i, b) = failing;
        (
            format!("{}: {}", params[i], self.bound_text(b.interface, b.args)),
            b.at,
        )
    }

    /// The block that would give the target of the block `block` the conformance to `inherited`,
    /// written with the block's own parameters and bounds.
    fn extend_help(&self, block: usize, ty: &str, inherited: &str) -> String {
        let ext = &self.extensions[block];
        let params: Vec<String> = (ext.type_params.iter().zip(&ext.bounds))
            .map(|(name, bounds)| {
                let bounds: Vec<String> = (bounds.iter())
                    .map(|b| self.bound_text(b.interface, b.args))
                    .collect();
                if bounds.is_empty() {
                    name.to_string()
                } else {
                    format!("{name}: {}", bounds.join(" & "))
                }
            })
            .collect();
        let list = if params.is_empty() {
            String::new()
        } else {
            format!("<{}>", params.join(", "))
        };
        format!("extend{list} {ty}: {inherited} {{ ... }}")
    }

    /// The text of `requirement` as a block writes it for the conformance to `bound`: the
    /// interface's arguments stand where its parameters do, in the types of the signature and in
    /// the bounds of the requirement's own type parameters, and `Self` stays. A type parameter of
    /// the requirement that has the name of a parameter of `source` or of a type that an argument
    /// writes is renamed, as the argument would take its place.
    fn required_text(&self, source: Source, bound: Bound, requirement: &RequirementInfo) -> String {
        let params = &self.interfaces[bound.interface.0].type_params;
        let mut with = vec![Type::intern(TypeKind::Param(params[0], 0))];
        with.extend_from_slice(bound.args.get());
        let words = |text: &str| -> Vec<String> {
            let name = |c: char| c.is_alphanumeric() || c == '_';
            text.split(|c| !name(c))
                .filter(|word| !word.is_empty())
                .map(str::to_string)
                .collect()
        };
        let mut captured: FxHashSet<String> = (self.source_params(source).0.iter())
            .map(ToString::to_string)
            .collect();
        for &arg in bound.args.get() {
            captured.extend(words(&self.type_name(arg)));
        }
        // The names the help writes: a new name is none of them.
        let mut taken = captured.clone();
        taken.extend(words(self.source_text(requirement.span)));
        let mut renamed: Vec<(Span, String)> = Vec::new();
        for (i, (&name, &at)) in
            (params.len()..).zip(requirement.own.iter().zip(&requirement.own_at))
        {
            let i = u32::try_from(i).expect("a declaration should have fewer than 2^32 parameters");
            let mut new = name;
            if captured.contains(name.as_str()) {
                let fresh = (2..u32::MAX)
                    .map(|k| format!("{name}{k}"))
                    .find(|fresh| !taken.contains(fresh))
                    .expect("a name should be free");
                taken.insert(fresh.clone());
                new = Symbol::intern(&fresh);
                renamed.push((at, fresh));
            }
            with.push(Type::intern(TypeKind::Param(new, i)));
        }
        let written = (requirement.params_at.iter().copied())
            .zip(requirement.params.iter().copied())
            .chain(requirement.ret_at.into_iter().zip([requirement.ret]));
        // The places that change, in the order of the text: the bounds, then the signature.
        let mut changed = renamed;
        for b in requirement.bounds.iter().flatten() {
            let args = b.args.subst(&with);
            if args != b.args {
                changed.push((b.at, self.bound_text(b.interface, args)));
            }
        }
        for (at, ty) in written {
            let Some(ty) = ty else {
                continue;
            };
            let substituted = ty.subst(&with);
            if substituted != ty {
                changed.push((at, self.type_name(substituted)));
            }
        }
        changed.sort_by_key(|(at, _)| at.start());
        let mut out = String::new();
        let mut from = requirement.span.start();
        for (at, text) in changed {
            out.push_str(self.source_slice(from..at.start()));
            out.push_str(&text);
            from = at.end();
        }
        out.push_str(self.source_slice(from..requirement.span.end()));
        out
    }

    /// The type that `source` chooses for each associated type of `interface`, in order; `None`
    /// for one that it chooses no type for, or whose type failed. Also where each type that holds
    /// an associated type is written.
    fn chosen(
        &self,
        interface: InterfaceId,
        source: Source,
    ) -> (Vec<Option<Type>>, Vec<(usize, Span)>) {
        let mut written = Vec::new();
        let chosen = (self.interfaces[interface.0].assoc.iter().enumerate())
            .map(|(k, declared)| {
                let choice = self.choice(source, declared.name.name)?;
                let ty = choice.ty?;
                if ty.has_assoc() {
                    written.push((k, choice.at));
                }
                Some(ty)
            })
            .collect();
        (chosen, written)
    }

    /// The source text at `span`: the program's, or, for a span in the prelude, the language
    /// interfaces', the only declarations of the prelude that a diagnostic quotes (a requirement
    /// of `Eq` that a class lacks). They come first in the prelude, so their spans index
    /// `LANG_SOURCE` from `PRELUDE_BASE`.
    fn source_text(&self, span: Span) -> &str {
        self.source_slice(span.range())
    }

    /// The source text between the offsets of `range`, as `source_text` reads it.
    fn source_slice(&self, range: std::ops::Range<usize>) -> &str {
        if range.start >= PRELUDE_BASE as usize {
            return Text::new(LANG_SOURCE, PRELUDE_BASE as usize).slice(range);
        }
        self.source.file.slice(range)
    }
}
