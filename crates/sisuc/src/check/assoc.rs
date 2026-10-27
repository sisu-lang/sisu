//! Associated types: the types an interface declares and a conformance chooses, `S.Item` written
//! through a type parameter, and the substitution that replaces an associated type of a known
//! type by the type its conformance chose.

use std::rc::Rc;

use rustc_hash::FxHashMap;

use super::interface::Bound;
use super::{Checker, Definition, DiagnosticKind, Holder, Symbol, built_in_type};
use crate::ast;
use crate::conformance::normalize_noting;
use crate::diagnostic::{CannotExtendForm, UnknownForm, WideKind};
use crate::source::Span;
use crate::tir::{FuncId, InterfaceId, Type, TypeArgs, TypeKind};

use super::interface::BOUND_CLOSURE_LIMIT;

/// The associated type that a conformance chooses: `type Item = i64`.
pub(super) struct Choice {
    pub(super) name: ast::Ident,
    /// The type chosen; `None` is `Error`.
    pub(super) ty: Option<Type>,
    /// Where the type is written.
    pub(super) at: Span,
}

/// The choices of a class, an enum or a block, in the order they are written, and where each is
/// by name, so that a lookup does not read the others.
#[derive(Default)]
pub(super) struct Choices {
    list: Vec<Choice>,
    by_name: FxHashMap<Symbol, usize>,
}

impl Choices {
    /// `list`, whose names are all different (see `Checker::resolve_choices`).
    pub(super) fn new(list: Vec<Choice>) -> Self {
        let by_name = (list.iter().enumerate())
            .map(|(i, choice)| (choice.name.name, i))
            .collect();
        Choices { list, by_name }
    }

    /// The choice of the associated type `name`.
    pub(super) fn get(&self, name: Symbol) -> Option<&Choice> {
        self.by_name.get(&name).map(|&i| &self.list[i])
    }

    /// The `k`th choice written.
    pub(super) fn at(&self, k: usize) -> &Choice {
        &self.list[k]
    }

    pub(super) fn iter(&self) -> std::slice::Iter<'_, Choice> {
        self.list.iter()
    }
}

/// What a bare name stands for where associated types are in scope.
pub(super) enum Named {
    /// The associated type, or the type its conformance chose.
    Type(Type),
    /// A type that failed to resolve, or a choice that names itself: reported already.
    Failed,
    /// No associated type of that name.
    Unknown,
}

impl Checker<'_> {
    /// `ty` with the associated types of known types replaced (see `conformance::normalize`).
    /// `None`, after the report of why, when a base has no conformance. A choice that needs its
    /// own associated type is reported once all bodies are checked (see `report_loops`).
    pub(super) fn normalize(&self, ty: Type) -> Option<Type> {
        if !ty.has_assoc() {
            return Some(ty);
        }
        self.normalized(ty)
    }

    /// `normalize` of a type that holds an associated type.
    fn normalized(&self, ty: Type) -> Option<Type> {
        let mut loops = Vec::new();
        let normal = normalize_noting(&self.conformances, ty, &mut loops);
        if !loops.is_empty() {
            let mut known = self.loops.borrow_mut();
            for (assoc, interface, index, k) in loops {
                known.entry((interface, index, k)).or_insert(assoc);
            }
        }
        normal
    }

    /// `ty` with `args` for its type parameters, as `Type::subst`, and then each associated type
    /// of a known type replaced. Every substitution that can put a class or an enum under an
    /// associated type goes through here. `None` poisons the caller, as an unresolved type does.
    pub(super) fn subst(&self, ty: Type, args: &[Type]) -> Option<Type> {
        let (substituted, assoc) = ty.subst_noting_assoc(args);
        if assoc {
            self.normalized(substituted)
        } else {
            Some(substituted)
        }
    }

    /// Reports each choice that `normalize` found to need its own associated type, once, at the
    /// type it writes (E0030). The associated types that depend on it stay unknown.
    pub(super) fn report_loops(&mut self) {
        // The diagnostics are sorted by their place, so the order of the map is no matter.
        let loops = std::mem::take(&mut *self.loops.borrow_mut());
        for (place, assoc) in loops {
            let at = self.choice_ats[&place];
            self.report(DiagnosticKind::UnknownType {
                name: self.type_name(assoc),
                at,
                form: UnknownForm::ChoiceLoops,
            });
        }
    }

    /// `subst` for a list of arguments. A type that does not normalize stays as it is: the
    /// report of why comes from the place that built it.
    pub(super) fn subst_args(&self, list: TypeArgs, args: &[Type]) -> TypeArgs {
        let list = list.subst(args);
        if !list.has_assoc() {
            return list;
        }
        let known: Option<Vec<Type>> = list.get().iter().map(|&ty| self.normalize(ty)).collect();
        known.map_or(list, |known| TypeArgs::intern(&known))
    }

    /// Resolves the choices `choices` of a class, an enum or a block `owner`, where its type
    /// parameters are in scope. A choice cannot name an associated type of its own conformance.
    /// A choice that repeats a name is reported, and the first stays.
    pub(super) fn resolve_choices(
        &mut self,
        choices: &[ast::AssocChoice],
        owner: &str,
    ) -> Vec<Choice> {
        if choices.is_empty() {
            return Vec::new();
        }
        self.body.choosing = choices.iter().map(|choice| choice.name.name).collect();
        let mut first = FxHashMap::default();
        let mut resolved = Vec::new();
        for choice in choices {
            let ty = self.resolve(&choice.ty);
            if self.first_definition(&mut first, Definition::Type, Some(owner), &choice.name) {
                resolved.push(Choice {
                    name: choice.name.clone(),
                    ty,
                    at: choice.ty.span,
                });
            }
        }
        self.body.choosing.clear();
        resolved
    }

    /// The associated types that a bare name stands for in the method `func` of `owner`: the
    /// choices of its `extend` block, or of its class or enum.
    pub(super) fn assoc_scope(&self, func: FuncId, owner: Option<Type>) -> Rc<Choices> {
        #[cfg(test)]
        super::CHOICE_STEPS.set(super::CHOICE_STEPS.get() + 1);
        match self.signatures[func.0].extension {
            Some(at) => Rc::clone(&self.extensions[at].assoc),
            None => match owner.and_then(super::extend::holder_of) {
                Some(Holder::Class(c)) => Rc::clone(&self.classes[c.0].assoc),
                Some(Holder::Enum(e)) => Rc::clone(&self.enums[e.0].assoc),
                None => Rc::default(),
            },
        }
    }

    /// What the bare `name`, written at `at`, stands for. A name whose choice is being resolved
    /// is reported (E0030).
    pub(super) fn assoc_named(&mut self, name: Symbol, at: Span) -> Named {
        if self.body.choosing.contains(&name) {
            let shadows = self.class_ids.contains_key(&name)
                || self.enum_ids.contains_key(&name)
                || built_in_type(name.as_str()).is_some();
            self.report(DiagnosticKind::UnknownType {
                name: name.to_string(),
                at,
                form: UnknownForm::ChoiceNamesItself { shadows },
            });
            return Named::Failed;
        }
        #[cfg(test)]
        super::CHOICE_STEPS.set(super::CHOICE_STEPS.get() + 1);
        match self.body.assoc.get(name) {
            Some(Choice { ty: Some(ty), .. }) => Named::Type(*ty),
            Some(_) => Named::Failed,
            None => Named::Unknown,
        }
    }

    /// Resolves the choices of a class or an enum `owner`, as `resolve_choices` does, and puts
    /// them in scope as the bare names its signatures and bodies may use.
    pub(super) fn declare_choices(
        &mut self,
        choices: &[ast::AssocChoice],
        owner: &str,
    ) -> Rc<Choices> {
        let resolved = Rc::new(Choices::new(self.resolve_choices(choices, owner)));
        self.body.assoc = Rc::clone(&resolved);
        resolved
    }

    /// Reports that the written type `ty`, a target or an interface argument of an `extend`
    /// block, names an associated type.
    pub(super) fn cannot_extend_assoc(&mut self, ty: &ast::TypeExpr) {
        self.report(DiagnosticKind::CannotExtend {
            written: ty.to_string(),
            at: ty.bare,
            form: CannotExtendForm::Associated,
        });
    }

    /// The type `base.name`, written as `ty`: the associated type `name` of the type parameter or
    /// associated type `base` names, through the one interface of its bounds that declares it.
    /// `None`, reported, for a base that is no type parameter, a base with no such associated
    /// type, and a base with two.
    pub(super) fn member_type(
        &mut self,
        ty: &ast::TypeExpr,
        base: &ast::TypeExpr,
        name: &ast::Ident,
    ) -> Option<Type> {
        let base_ty = self.resolve(base)?;
        if !matches!(base_ty.kind(), TypeKind::Param(..) | TypeKind::Assoc(..)) {
            let own = matches!(base.kind, ast::TypeKind::SelfType)
                && self.body.assoc.get(name.name).is_some();
            self.report(DiagnosticKind::UnknownType {
                name: name.name.to_string(),
                at: ty.bare,
                form: UnknownForm::ConcreteBase { own },
            });
            return None;
        }
        let entries = self.interfaces_for(base_ty);
        // A base whose bounds went past the limit is reported already, and unknown.
        if self.is_wide(base_ty) {
            return None;
        }
        let mut found = Vec::new();
        for (interface, args) in entries {
            let assoc = &self.interfaces[interface.0].assoc;
            for (k, declared) in assoc.iter().enumerate() {
                if declared.name.name == name.name {
                    found.push((interface, args, k));
                }
            }
        }
        match found[..] {
            [(interface, args, k)] => {
                let k = u32::try_from(k).expect("an interface should have fewer than 2^32 types");
                Some(Type::intern(TypeKind::Assoc(base_ty, interface, args, k)))
            }
            [] => {
                let add = match self.declaring_assoc.get(&name.name).map(Vec::as_slice) {
                    Some(&[only]) => self.add_bound(base_ty, only),
                    _ => None,
                };
                self.report(DiagnosticKind::UnknownType {
                    name: name.name.to_string(),
                    at: name.span,
                    form: UnknownForm::NoAssociated {
                        base: self.type_name(base_ty),
                        add,
                    },
                });
                None
            }
            _ => {
                self.report(DiagnosticKind::Ambiguous {
                    method: name.name.to_string(),
                    receiver: self.type_name(base_ty),
                    interfaces: (found.iter())
                        .map(|&(interface, args, _)| self.bound_text(interface, args))
                        .collect(),
                    at: name.span,
                    annotate: None,
                });
                None
            }
        }
    }

    /// Whether the bounds of the type parameter or associated type `ty` went past the limit,
    /// which `interfaces_for` has found out: it is reported, and unknown in this body.
    pub(super) fn is_wide(&self, ty: Type) -> bool {
        match ty.kind() {
            TypeKind::Param(_, place) => self.body.wide.contains(&(place as usize)),
            _ => self.body.wide_assoc.contains(&ty),
        }
    }

    /// The interfaces that the type parameter or associated type `ty` conforms to through its
    /// bounds and what they inherit, each with its arguments once.
    pub(super) fn interfaces_for(&mut self, ty: Type) -> Vec<(InterfaceId, TypeArgs)> {
        match ty.kind() {
            TypeKind::Param(_, place) => self.param_interfaces(place as usize).to_vec(),
            TypeKind::Assoc(..) => self.assoc_interfaces(ty).to_vec(),
            _ => unreachable!("only a type parameter or an associated type has bounds"),
        }
    }

    /// Whether the associated type `ty` conforms to `interface` with `args`: through the bounds
    /// of its declaration, or because its bounds went past the limit and it is unknown.
    pub(super) fn assoc_conforms(
        &mut self,
        ty: Type,
        interface: InterfaceId,
        args: TypeArgs,
    ) -> bool {
        self.assoc_interfaces(ty).contains(&(interface, args)) || self.body.wide_assoc.contains(&ty)
    }

    /// The interfaces that the associated type `ty` conforms to through the bounds of its
    /// declaration, found when first asked for in a body.
    ///
    /// The third recorded exception to "A pass is linear in the program" (user, 2026-10-25; ruling
    /// F9 (b)), as for `param_interfaces`: the closure is computed once for each associated type
    /// that a body names, so a body costs the closure's size.
    fn assoc_interfaces(&mut self, ty: Type) -> &[(InterfaceId, TypeArgs)] {
        if !self.body.assoc_conforming.contains_key(&ty) {
            let TypeKind::Assoc(base, interface, args, k) = ty.kind() else {
                unreachable!("only an associated type should be asked for its closure");
            };
            // The declaration's bounds are over the interface's parameters, `Self` first.
            let mut with = vec![base];
            with.extend_from_slice(args.get());
            let bounds: Vec<Bound> = (self.interfaces[interface.0].assoc[k as usize].bounds.iter())
                .map(|b| Bound {
                    interface: b.interface,
                    args: b.args.subst(&with),
                    at: b.at,
                })
                .collect();
            let mut out = Vec::new();
            let mut seen = std::mem::take(&mut self.closure_seen);
            let wide = self.interfaces_of(ty, &bounds, &mut out, &mut seen);
            self.closure_seen = seen;
            if let Err(j) = wide {
                self.too_wide_assoc(ty, bounds[j].at);
            }
            self.body.assoc_conforming.insert(ty, out);
        }
        &self.body.assoc_conforming[&ty]
    }

    /// Reports the bound written at `at` of the associated type `ty`, whose closure went past the
    /// limit: once for the bound, whatever bodies read it. The type is then unknown in this body.
    pub(super) fn too_wide_assoc(&mut self, ty: Type, at: Span) {
        self.body.wide_assoc.insert(ty);
        if self.wide_reported.insert(at) {
            let text = self.source.file.text(at);
            self.report(DiagnosticKind::BoundTooWide {
                kind: WideKind::Bound,
                written: format!("{}: {text}", self.type_name(ty)),
                limit: BOUND_CLOSURE_LIMIT,
                at,
            });
        }
    }
}
