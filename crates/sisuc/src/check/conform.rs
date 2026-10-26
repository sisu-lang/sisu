//! Conformances of classes and enums to interfaces: the ones a declaration lists and the ones an
//! `extend` block writes, the requirements each meets, and the conformances each needs.

use rustc_hash::{FxHashMap, FxHashSet};

use super::extend::holder_of;
use super::interface::{Bound, RequirementInfo};
use super::{Body, Checker, Choice, Choices, Declared, Holder, own_args};
use crate::ast;
use crate::conformance::{self, find};
use crate::diagnostic::{ConformanceForm, DiagnosticKind, NotConformForm};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, InterfaceId, Type, TypeKind};

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
        // Where each conformance is written, by interface and index.
        let mut ats: FxHashMap<(InterfaceId, usize), Span> = FxHashMap::default();
        let mut pending = Vec::new();
        let lists = self.listed_conformances(items);
        for (holder, receiver, roots) in &lists {
            for &(bound, at) in roots {
                let source = Source::Body(*holder);
                pending.extend(self.add_conformance(bound, *receiver, at, source, &mut ats));
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
            let added = self.add_conformance(bound, target, at, source, &mut ats);
            self.extensions[block].duplicate = added.is_none();
            pending.extend(added);
        }
        // The interfaces a listed one inherits are the type's too, unless a block gives them: the
        // body holds their methods as well.
        //
        // The third recorded exception to "A pass is linear in the program" (user, 2026-10-25;
        // ruling F9): a list's closure has an entry for each conformance that the list gives, and
        // the closure of arguments may double at each level of inheritance, so the walk of each
        // listed interface stops at `BOUND_CLOSURE_LIMIT` entries and E0157 is reported at it.
        // The work follows the sum of the closures, which are at most the limit each.
        let mut inherited_found = Vec::new();
        for (holder, receiver, roots) in &lists {
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
                let bound = Bound {
                    interface,
                    args,
                    at,
                };
                let source = Source::Body(*holder);
                pending.extend(self.add_conformance(bound, *receiver, at, source, &mut ats));
            }
        }
        // Every conformance is known now: the checks that waited for them run, and a type that
        // fails is unknown before a requirement is compared with a method.
        self.run_pending_bounds();
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
        ats: &mut FxHashMap<(InterfaceId, usize), Span>,
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
            let earlier = ats[&(bound.interface, k)];
            let (first, second) = if earlier.start() <= at.start() {
                (earlier, at)
            } else {
                (at, earlier)
            };
            self.report(DiagnosticKind::ConformsTwice {
                ty: self.type_name(common[0]),
                interface: self.bound_text(bound.interface, tir::TypeArgs::intern(&common[1..])),
                at: second,
                first,
                generic: !bound.args.get().is_empty(),
            });
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
            },
        );
        for (k, at) in written {
            self.choice_ats.insert((bound.interface, index, k), at);
        }
        ats.insert((bound.interface, index), at);
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
        })
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
                None => missing.push(self.required_text(p.interface, requirement)),
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
                            want: self.required_text(p.interface, requirement),
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
            reports.push(DiagnosticKind::ConformanceBody {
                ty: ty.clone(),
                interface: interface.clone(),
                at: p.at,
                form: ConformanceForm::Missing(missing),
            });
        }
        if let Source::Block(b) = p.source {
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
            if ext.assoc.is_empty() {
                continue;
            }
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
                if checker.conforms(p.target, interface, args) {
                    continue;
                }
                let (parent, requires) = reached[k - 1];
                let via = closure[parent].0;
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
                requires,
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
    /// interface's arguments stand where its parameters do, and `Self` stays. Shortcut: a bound of
    /// the requirement's own type parameters is written as the interface declares it.
    fn required_text(&self, bound: Bound, requirement: &RequirementInfo) -> String {
        let params = &self.interfaces[bound.interface.0].type_params;
        let mut with = vec![Type::intern(TypeKind::Param(params[0], 0))];
        with.extend_from_slice(bound.args.get());
        for (i, &name) in (params.len()..).zip(&requirement.own) {
            let i = u32::try_from(i).expect("a declaration should have fewer than 2^32 parameters");
            with.push(Type::intern(TypeKind::Param(name, i)));
        }
        let written = (requirement.params_at.iter().copied())
            .zip(requirement.params.iter().copied())
            .chain(requirement.ret_at.into_iter().zip([requirement.ret]));
        let mut out = String::new();
        let mut from = requirement.span.start();
        for (at, ty) in written {
            let Some(ty) = ty.filter(|ty| ty.subst(&with) != *ty) else {
                continue;
            };
            out.push_str(self.source.file.slice(from..at.start()));
            out.push_str(&self.type_name(ty.subst(&with)));
            from = at.end();
        }
        out.push_str(self.source.file.slice(from..requirement.span.end()));
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

    /// The source text at `span`.
    fn source_text(&self, span: Span) -> &str {
        self.source.file.text(span)
    }
}
