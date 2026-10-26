//! `extend` blocks: their targets, their methods, and the methods of one name that a class or an
//! enum holds in its body and in plain blocks.

use rustc_hash::FxHashMap;

use super::interface::Bound;
use super::{Checker, Declared, Definition, DiagnosticKind, Holder, Symbol, UNKNOWN};
use crate::ast;
use crate::diagnostic::{CannotExtendForm, Earlier, Owner as MethodOwner};
use crate::source::Span;
use crate::tir::{self, Type, TypeKind};

/// An `extend` block whose target is a class or an enum.
pub(super) struct Extension {
    pub(super) holder: Holder,
    pub(super) target: Type,
    /// The interface of an `extend T: I` block, and where it is written.
    pub(super) interface: Option<(Bound, Span)>,
    /// The first method of each name.
    pub(super) methods: FxHashMap<Symbol, tir::FuncId>,
    /// Whether the type already conforms to the interface, so that the block's methods answer no
    /// call: its conformance is reported (E0153).
    pub(super) duplicate: bool,
}

impl<'a> Checker<'_> {
    /// Resolves the target and the interface of `extend`, then the signatures of its methods, and
    /// adds each method to `declared` with `self`'s type. The block is recorded when its target
    /// is a class or an enum that the checker can take, and its interface, if it writes one, is an
    /// interface that it can take. Otherwise its methods get signatures, so that the ids of the
    /// functions after them stay in place, and `declared` holds them with an unknown receiver:
    /// their bodies are not checked, since the block's own error says what is wrong.
    pub(super) fn extend_signatures(
        &mut self,
        extend: &'a ast::Extend,
        declared: &mut Vec<(&'a ast::Function, Option<Type>)>,
    ) {
        let params = self.type_params(&[], &extend.type_params);
        self.body.set_type_params(&params);
        self.bounds_of(&extend.type_params);
        let target = self.extend_target(&extend.target);
        let interface =
            (extend.interface.as_ref()).map(|ty| self.bound(ty).map(|bound| (bound, ty.bare)));
        for choice in &extend.assoc {
            // Task 7 takes an associated type.
            self.unsupported_at(choice.name.span);
        }
        // A target with type arguments, and an interface that takes some, wait for Tasks 5 and 6.
        let generic_target = target.is_some_and(|ty| !takes_none(ty));
        let generic_interface = matches!(interface, Some(Some((b, _))) if !b.args.get().is_empty());
        if generic_target {
            self.unsupported(&extend.target);
        }
        if generic_interface && let Some(ty) = &extend.interface {
            self.unsupported(ty);
        }
        let failed = generic_target || generic_interface || matches!(interface, Some(None));
        let usable = target.filter(|_| !failed);
        let receiver = usable.unwrap_or(UNKNOWN);
        let holder = usable.and_then(holder_of);
        self.body.self_type = Some(receiver);
        let name = self.type_name(receiver);
        let at = self.extensions.len();
        let mut methods = FxHashMap::default();
        let mut first = FxHashMap::default();
        for method in &extend.methods {
            let func = tir::FuncId(self.signatures.len());
            if self.first_definition(&mut first, Definition::Method, Some(&name), &method.name)
                && let Some(holder) = holder
            {
                methods.insert(method.name.name, func);
                self.extension_methods
                    .entry((holder, method.name.name))
                    .or_default()
                    .push((at, func));
            }
            let signature = format!("{name}.{}", method.name.name);
            self.signature(method, signature, &extend.type_params);
            if let Some(Some((bound, _))) = interface {
                self.signatures[func.0].conformance = Some((bound.interface, bound.args));
            }
            if let (Some(mut_self), Some(holder)) = (&method.mut_self, holder) {
                let owner = match holder {
                    Holder::Class(_) => MethodOwner::Class,
                    Holder::Enum(_) => MethodOwner::Enum(name.clone()),
                };
                self.report(DiagnosticKind::MutSelf {
                    keyword: mut_self.keyword,
                    receiver: mut_self.receiver,
                    safe: mut_self.annotation.is_none(),
                    owner,
                });
            }
            declared.push((method, Some(receiver)));
        }
        self.body.set_type_params(&[]);
        self.body.self_type = None;
        if let (Some(target), Some(holder)) = (usable, holder) {
            for method in &extend.broken_methods {
                self.broken_extension_methods.insert((holder, method.name));
            }
            self.extensions.push(Extension {
                holder,
                target,
                interface: interface.flatten(),
                methods,
                duplicate: false,
            });
        } else if let Some(holder) = target.and_then(holder_of) {
            // The methods of a failed block may have given the target methods.
            let name = self.holder_name(holder);
            self.broken_extensions.insert(name);
        }
    }

    /// The type that `ty`, the target of an `extend` block, names: a class or an enum. Reports a
    /// target that is not one, and `None` for it.
    fn extend_target(&mut self, ty: &ast::TypeExpr) -> Option<Type> {
        if self.is_interface_name(ty) {
            let name = match &ty.kind {
                ast::TypeKind::Applied(applied) => applied.name,
                ast::TypeKind::Name(name) => *name,
                _ => unreachable!("an interface name should be a name or an applied name"),
            };
            // Following the advice of `InterfaceAsType` would give `CannotExtend`, so it has none.
            self.report(DiagnosticKind::InterfaceAsType {
                name: name.to_string(),
                written: None,
                at: ty.bare,
            });
            return None;
        }
        let resolved = self.resolve(ty)?;
        let form = match resolved.kind() {
            TypeKind::Class(..) | TypeKind::Enum(..) => return Some(resolved),
            TypeKind::Param(..) => CannotExtendForm::TypeParam,
            _ => CannotExtendForm::BuiltIn,
        };
        self.report(DiagnosticKind::CannotExtend {
            written: ty.to_string(),
            at: ty.bare,
            form,
        });
        None
    }

    /// The name of the class or enum `holder`.
    pub(super) fn holder_name(&self, holder: Holder) -> Symbol {
        match holder {
            Holder::Class(c) => self.classes[c.0].name.name,
            Holder::Enum(e) => self.enums[e.0].name.name,
        }
    }

    /// Reports each method that a class or an enum holds in its body and in plain `extend`
    /// blocks, or in two plain blocks, by one name, and each plain block's method named like a
    /// field or a variant. They share a namespace; the one that starts last is the one reported.
    /// The methods of a conformance block are another namespace: the block's. Only the blocks that
    /// hold a method of the name are read, so the work follows the methods and not the pairs of
    /// blocks. The targets of the plain blocks of one class or enum overlap, as they cannot name
    /// type arguments yet.
    pub(super) fn extension_clashes(&mut self) {
        let mut reports = Vec::new();
        for (&(holder, name), list) in &self.extension_methods {
            let owner = self.holder_name(holder).to_string();
            let plain = list
                .iter()
                .filter(|&&(at, _)| self.extensions[at].interface.is_none());
            // (start, kind, name's span) of each definition of the name.
            let mut defs: Vec<(Span, Definition)> =
                self.body_definition(holder, name).into_iter().collect();
            defs.extend(plain.map(|&(_, f)| (self.signatures[f.0].span, Definition::Method)));
            defs.sort_by_key(|(span, _)| span.start());
            let Some(&(first, first_kind)) = defs.first() else {
                continue;
            };
            for &(span, kind) in &defs[1..] {
                reports.push(super::defined_twice(
                    kind,
                    &ast::Ident { name, span },
                    Earlier::At {
                        span: first,
                        kind: first_kind,
                        owner: Some(owner.clone()),
                    },
                ));
            }
        }
        for report in reports {
            self.report(report);
        }
    }

    /// The method, field or variant `name` of `holder`'s own declaration, and where its name is.
    fn body_definition(&self, holder: Holder, name: Symbol) -> Option<(Span, Definition)> {
        match holder {
            Holder::Class(c) => {
                let info = &self.classes[c.0];
                match info.methods.get(&name) {
                    Some(&Declared::Known(f)) => {
                        Some((self.signatures[f.0].span, Definition::Method))
                    }
                    _ => (info.fields.iter())
                        .find(|f| f.name.name == name)
                        .map(|f| (f.name.span, Definition::Field)),
                }
            }
            Holder::Enum(e) => {
                let info = &self.enums[e.0];
                match info.methods.get(&name) {
                    Some(&Declared::Known(f)) => {
                        Some((self.signatures[f.0].span, Definition::Method))
                    }
                    _ => info
                        .index
                        .get(&name)
                        .map(|&v| (info.variants[v].name.span, Definition::Variant)),
                }
            }
        }
    }
}

/// Whether the class or enum type `ty` takes no type arguments.
fn takes_none(ty: Type) -> bool {
    match ty.kind() {
        TypeKind::Class(_, args) | TypeKind::Enum(_, args) => args.get().is_empty(),
        _ => true,
    }
}

/// The class or enum that `ty` is.
pub(super) fn holder_of(ty: Type) -> Option<Holder> {
    match ty.kind() {
        TypeKind::Class(id, _) => Some(Holder::Class(id)),
        TypeKind::Enum(id, _) => Some(Holder::Enum(id)),
        _ => None,
    }
}
