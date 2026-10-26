//! Conformances of classes and enums to interfaces: the ones a declaration lists and the ones an
//! `extend` block writes, the requirements each meets, and the conformances each needs.

use rustc_hash::{FxHashMap, FxHashSet};

use super::extend::holder_of;
use super::interface::Bound;
use super::{Checker, Declared, Holder, own_args};
use crate::ast;
use crate::conformance::{self, find};
use crate::diagnostic::{ConformanceForm, DiagnosticKind};
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

/// What a class or an enum lists: its holder, its type, and each interface with where it is
/// written.
type Listed = (Holder, Type, Vec<(Bound, Span)>);

impl Checker<'_> {
    /// Pass 2b, after every signature: reports the methods of one name that a type holds twice,
    /// records each conformance and reports the second of any two that overlap, then checks that
    /// each meets its requirements and, for an `extend` block, the conformances it needs. A
    /// class or an enum with type parameters, and an interface that takes some, wait for Tasks 5
    /// and 6: they report the E0030 stopgap at the interface.
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
        // the closure of arguments may double at each level of inheritance (spec, "Inheritance").
        // The roots of one list share one visited set, so the work follows the closure and not
        // the number of roots.
        for (holder, receiver, roots) in &lists {
            let mut closure = Vec::new();
            // The listed interface through which each entry of `closure` is reached.
            let mut via = Vec::new();
            let mut seen = FxHashSet::default();
            for &(bound, at) in roots {
                self.extend_closure(*receiver, &[bound], &mut closure, &mut seen, |_, _| {});
                via.resize(closure.len(), at);
            }
            // The listed interface at which the stopgap is reported, once.
            let mut stopped = FxHashSet::default();
            for (&(interface, args), &at) in closure.iter().zip(&via) {
                if find(&self.conformances, interface, *receiver, args).is_some() {
                    continue;
                }
                // An inherited interface that takes arguments waits for Task 6, as a listed one.
                if !args.get().is_empty() {
                    if stopped.insert(at) {
                        self.unsupported_at(at);
                    }
                    continue;
                }
                let bound = Bound { interface, args };
                let source = Source::Body(*holder);
                pending.extend(self.add_conformance(bound, *receiver, at, source, &mut ats));
            }
        }
        for p in &pending {
            self.meet_requirements(p);
        }
        for p in pending
            .iter()
            .filter(|p| matches!(p.source, Source::Block(_)))
        {
            self.inherited_conformances(p);
        }
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
                let Some(bound) = self.bound(ty) else {
                    continue;
                };
                // A type with parameters, and an interface that takes some, wait for Tasks 5 and
                // 6.
                if !params.is_empty() || !bound.args.get().is_empty() {
                    self.unsupported(ty);
                    continue;
                }
                roots.push((bound, ty.bare));
            }
            for choice in assoc {
                // Task 7 takes an associated type.
                self.unsupported_at(choice.name.span);
            }
            self.body.set_type_params(&[]);
            self.body.self_type = None;
            lists.push((holder, receiver, roots));
        }
        lists
    }

    /// Records the conformance of `target` to `bound`, which is written at `at`, unless it
    /// overlaps one recorded: that is reported at the one that starts last.
    fn add_conformance(
        &mut self,
        bound: Bound,
        target: Type,
        at: Span,
        source: Source,
        ats: &mut FxHashMap<(InterfaceId, usize), Span>,
    ) -> Option<Pending> {
        let clash = conformance::clash(&self.conformances, bound.interface, target, 0, bound.args);
        if let Some((k, common)) = clash {
            let earlier = ats[&(bound.interface, k)];
            let (first, second) = if earlier.start() <= at.start() {
                (earlier, at)
            } else {
                (at, earlier)
            };
            self.report(DiagnosticKind::ConformsTwice {
                ty: self.type_name(common[0]),
                interface: self.interfaces[bound.interface.0].name.name.to_string(),
                at: second,
                first,
            });
            return None;
        }
        let index = self.conformances.add(
            bound.interface,
            tir::Conformance {
                type_params: 0,
                target,
                args: bound.args,
                methods: Vec::new(),
            },
        );
        ats.insert((bound.interface, index), at);
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
                None => missing.push(self.source_text(requirement.span).to_string()),
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
                            want: self.source_text(requirement.span).to_string(),
                        },
                    });
                }
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
        self.extend_closure(
            p.target,
            &bounds,
            &mut closure,
            &mut seen,
            |from, written| {
                reached.push((from, written));
            },
        );
        for (k, &(interface, args)) in closure.iter().enumerate().skip(1) {
            if find(&self.conformances, interface, p.target, args).is_some() {
                continue;
            }
            let (parent, requires) = reached[k - 1];
            let via = closure[parent].0;
            let (ty, inherited) = (
                self.type_name(p.target),
                self.interfaces[interface.0].name.name.to_string(),
            );
            // shortcut: an inherited interface that takes arguments gets no help, as a block
            // cannot write a generic interface yet; Task 6 writes it with the arguments.
            let extend = args
                .get()
                .is_empty()
                .then(|| format!("extend {ty}: {inherited} {{ ... }}"));
            self.report(DiagnosticKind::NotConform {
                ty,
                inherited,
                via: self.interfaces[via.0].name.name.to_string(),
                at: p.at,
                requires,
                extend,
            });
        }
    }

    /// The source text at `span`.
    fn source_text(&self, span: Span) -> &str {
        self.source.file.text(span)
    }
}
