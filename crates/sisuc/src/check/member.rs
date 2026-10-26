//! Members: fields and methods found through `.` and `?.`.

use super::expr::{if_some, local, no_member, path};
use super::extend::holder_of;
use super::unused_names::Used;
use super::{Checked, Checker, Declared, Poisoned, UNKNOWN, as_optional, type_of, wrap};
use crate::ast;
use crate::diagnostic::{Annotate, Definition, Diagnostic, DiagnosticKind, Peel};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, InterfaceId, Type, TypeKind};

impl Checker<'_> {
    /// `receiver.member`, which `member` checks on the checked receiver. With `safe`, it is
    /// `receiver?.member`, and `member` checks it on a fresh local bound to the receiver's
    /// value. Its type `U` makes the result a `U?`, or `unit` for a method with no value. A
    /// receiver that is not optional is reported, and the member still checked on it. `name` is
    /// the member's name.
    pub(super) fn member(
        &mut self,
        span: Span,
        safe: bool,
        receiver: &ast::Expr,
        name: &ast::Ident,
        member: impl FnOnce(&mut Self, Checked) -> Checked,
    ) -> Checked {
        let scrutinee = self.value_kept(receiver);
        if !safe {
            return member(self, scrutinee);
        }
        let Some(TypeKind::Optional(payload)) = type_of(&scrutinee).map(Type::kind) else {
            if type_of(&scrutinee).is_some() {
                let kind = DiagnosticKind::SafeAccessNotOptional {
                    value: path(receiver),
                    at: receiver.span,
                    access: Span::new(receiver.span.end(), name.span.start()),
                };
                self.diagnostics.push(kind.into());
            }
            // Only its own diagnostics matter.
            let _ = member(self, Err(Poisoned::of(type_of(&scrutinee))));
            return Err(Poisoned::error());
        };
        // `?.` takes one layer off: on a `C??` it leaves a `C?`, which has no members.
        if matches!(payload.kind(), TypeKind::Optional(_)) {
            let kind = DiagnosticKind::MayBeNone {
                value: path(receiver),
                at: receiver.span,
                access: None,
                peel: Some(Peel {
                    receiver: self.source.file.text(receiver.span).to_string(),
                    left: self.type_name(payload),
                    member: name.name.to_string(),
                }),
            };
            self.diagnostics.push(kind.into());
            // Only its own diagnostics matter.
            let _ = member(self, Err(Poisoned::error()));
            return Err(Poisoned::error());
        }
        let bind = self.fresh(payload);
        let member = member(self, Ok(local(bind, payload, receiver.span)));
        let ty = match type_of(&member) {
            Some(Type::UNIT) => Type::UNIT,
            Some(ty) => as_optional(ty),
            None => return Err(Poisoned::error()),
        };
        let (Ok(scrutinee), Ok(member)) = (scrutinee, member) else {
            return Err(Poisoned::typed(ty));
        };
        let none = (ty != Type::UNIT).then_some(tir::Expr {
            kind: tir::ExprKind::None,
            ty,
            span,
        });
        let value = if member.ty == ty {
            member
        } else {
            wrap(member, ty)
        };
        Ok(if_some(bind, scrutinee, value, none, span))
    }

    /// `base.name`, `base` checked from `receiver`.
    pub(super) fn field(
        &mut self,
        span: Span,
        base: Checked,
        receiver: &ast::Expr,
        name: &ast::Ident,
    ) -> Checked {
        let Some((class, index, ty)) = self.find_field(type_of(&base), receiver, name, false)
        else {
            return Err(Poisoned::error());
        };
        self.record(Used::Field(class, index), span);
        match (base, ty) {
            (Ok(base), Some(ty)) => Ok(tir::Expr {
                kind: tir::ExprKind::Field {
                    base: Box::new(base),
                    index,
                },
                ty,
                span,
            }),
            (_, ty) => Err(Poisoned::of(ty)),
        }
    }

    /// The type of field `index` of a value of the class type `base`: `base`'s arguments stand
    /// for its class's parameters. `None` when the field's type is `Error`.
    pub(super) fn field_type(&self, base: Type, index: usize) -> Option<Type> {
        let TypeKind::Class(class, args) = base.kind() else {
            panic!("a field should only be found on a value of a class type, not `{base}`")
        };
        let info = &self.classes[class.0];
        assert_eq!(
            args.get().len(),
            info.type_params.len(),
            "a class type should give each of its class's type parameters an argument"
        );
        info.fields[index].ty.map(|ty| ty.subst(args.get()))
    }

    /// The class, index and type of field `name` of `receiver`, a value of type `base`. `None` when
    /// there is none, reported unless `base` is `Error`. `target` when the field is assigned,
    /// where neither `()` nor `?.` can follow the access.
    pub(super) fn find_field(
        &mut self,
        base: Option<Type>,
        receiver: &ast::Expr,
        name: &ast::Ident,
        target: bool,
    ) -> Option<(tir::ClassId, usize, Option<Type>)> {
        let ty = base?;
        let method = |owner, safe: bool| DiagnosticKind::MethodNotField {
            name: name.name.to_string(),
            owner,
            at: name.span,
            call: (!target).then_some(safe),
        };
        let d = match ty.kind() {
            TypeKind::Class(class, _) => {
                let info = &self.classes[class.0];
                if let Some(index) = info.fields.iter().position(|f| f.name.name == name.name) {
                    return Some((class, index, self.field_type(ty, index)));
                }
                match self.own_method(ty, name.name) {
                    Some(Declared::Known(func)) => {
                        let no_arguments = self.signatures[func.0].params.is_empty();
                        method(self.type_name(ty), no_arguments).into()
                    }
                    // Its own error is reported, and what it held is unknown.
                    Some(Declared::Broken) => return None,
                    None => {
                        let fields = info.fields.iter().map(|f| f.name.name.as_str());
                        no_member(Definition::Field, self.type_name(ty), name, fields)
                    }
                }
            }
            TypeKind::Enum(..) => match self.own_method(ty, name.name) {
                Some(Declared::Known(func)) => {
                    let no_arguments = self.signatures[func.0].params.is_empty();
                    method(self.type_name(ty), no_arguments).into()
                }
                // Its own error is reported.
                Some(Declared::Broken) => return None,
                None => DiagnosticKind::EnumField {
                    enum_name: self.type_name(ty),
                    at: name.span,
                }
                .into(),
            },
            TypeKind::Array(_) if name.name.as_str() == "len" => {
                method(self.type_name(ty), true).into()
            }
            TypeKind::Optional(_) => may_be_none(receiver, name, !target && !self.body.in_target),
            _ => no_member(Definition::Field, self.type_name(ty), name, []),
        };
        self.diagnostics.push(d);
        None
    }

    /// The method of the class or enum type `ty` that its body or a plain `extend` block gives
    /// the name `name`, if any; a broken one for a method that a syntax error took. The first in
    /// source order wins, and two are reported (E0034) elsewhere.
    pub(super) fn own_method(&self, ty: Type, name: Symbol) -> Option<Declared<tir::FuncId>> {
        let holder = holder_of(ty)?;
        let body = match holder {
            super::Holder::Class(c) => self.classes[c.0].methods.get(&name),
            super::Holder::Enum(e) => self.enums[e.0].methods.get(&name),
        };
        if let Some(&body) = body {
            return Some(body);
        }
        let blocks = self.extension_methods.get(&(holder, name));
        let plain = blocks.into_iter().flatten().find_map(|&(at, func)| {
            let ext = &self.extensions[at];
            (ext.interface.is_none() && ext.target == ty).then_some(Declared::Known(func))
        });
        plain.or_else(|| {
            (self.broken_extension_methods.contains(&(holder, name))).then_some(Declared::Broken)
        })
    }

    /// The methods named `name` of the conformance blocks that cover `ty`, each with its block's
    /// interface, in source order.
    ///
    /// The third recorded exception to "A pass is linear in the program" (user, 2026-10-25; ruling
    /// F9 (e)): a call reads every block that holds a method of its name, `n` for `n` blocks, and
    /// only those.
    pub(super) fn conforming_methods(
        &self,
        ty: Type,
        name: Symbol,
    ) -> Vec<(tir::FuncId, InterfaceId)> {
        let Some(holder) = holder_of(ty) else {
            return Vec::new();
        };
        let blocks = self.extension_methods.get(&(holder, name));
        (blocks.into_iter().flatten())
            .filter_map(|&(at, func)| {
                let ext = &self.extensions[at];
                let (bound, _) = ext.interface?;
                (ext.target == ty && !ext.duplicate).then_some((func, bound.interface))
            })
            .collect()
    }

    /// The method `name` of `receiver`, a value of type `ty`, which the call `call` (its span, and
    /// the type its context expects) names. The type's own method wins; else the method of the
    /// one conformance block that has one, or of the one whose result is the expected type.
    /// `None` when there is none, reported unless `ty` is `Error`. `target` when the call is in an
    /// assignment target's chain, where `?.` cannot follow the receiver.
    pub(super) fn find_method(
        &mut self,
        ty: Option<Type>,
        receiver: &ast::Expr,
        name: &ast::Ident,
        target: bool,
        call: (Span, Option<Type>),
    ) -> Option<tir::FuncId> {
        let ty = ty?;
        match self.own_method(ty, name.name) {
            Some(Declared::Known(func)) => return Some(func),
            // Its own error is reported; the caller checks the arguments.
            Some(Declared::Broken) => return None,
            None => {}
        }
        match self.conforming_methods(ty, name.name)[..] {
            [] => {}
            [(func, _)] => return Some(func),
            ref several => return self.pick_conforming(ty, name, several, call),
        }
        // A broken block may have given the type the method.
        let broken = holder_of(ty)
            .is_some_and(|holder| self.broken_extensions.contains(&self.holder_name(holder)));
        if broken {
            return None;
        }
        let d = match ty.kind() {
            TypeKind::Class(class, _) => {
                let info = &self.classes[class.0];
                if info.fields.iter().any(|f| f.name.name == name.name) {
                    DiagnosticKind::FieldNotMethod {
                        name: name.name.to_string(),
                        owner: self.type_name(ty),
                        at: name.span,
                    }
                    .into()
                } else {
                    let methods = info.methods.keys().map(|method| method.as_str());
                    no_member(Definition::Method, self.type_name(ty), name, methods)
                }
            }
            TypeKind::Enum(id, _) => {
                let info = &self.enums[id.0];
                if info.variants.iter().any(|v| v.name.name == name.name) {
                    DiagnosticKind::MemberKind {
                        name: name.name.to_string(),
                        enum_name: self.type_name(ty),
                        method: false,
                        at: name.span,
                    }
                    .into()
                } else {
                    let methods = info.methods.keys().map(|method| method.as_str());
                    no_member(Definition::Method, self.type_name(ty), name, methods)
                }
            }
            TypeKind::Optional(_) => may_be_none(receiver, name, !target),
            _ => no_member(Definition::Method, self.type_name(ty), name, []),
        };
        self.diagnostics.push(d);
        None
    }

    /// The method among `candidates`, which are methods of conformance blocks that cover `ty`,
    /// that the call `call` names: the only one whose result is the type its context expects, or,
    /// when none is, the only one whose result is the payload of an expected optional. Otherwise
    /// the call is ambiguous, reported with the `let` that picks one answer.
    fn pick_conforming(
        &mut self,
        ty: Type,
        name: &ast::Ident,
        candidates: &[(tir::FuncId, InterfaceId)],
        (span, expected): (Span, Option<Type>),
    ) -> Option<tir::FuncId> {
        let rets: Vec<Option<Type>> = (candidates.iter())
            .map(|&(func, _)| self.signatures[func.0].ret)
            .collect();
        let returning = |want: Type| -> Vec<usize> {
            (rets.iter().enumerate())
                .filter(|&(_, &ret)| ret == Some(want))
                .map(|(i, _)| i)
                .collect()
        };
        if let Some(expected) = expected {
            let mut hits = returning(expected);
            if hits.is_empty()
                && let TypeKind::Optional(payload) = expected.kind()
            {
                hits = returning(payload);
            }
            if let [only] = hits[..] {
                return Some(candidates[only].0);
            }
        }
        // The first result that exactly one candidate has tells the `let` what to write.
        let unique = rets
            .iter()
            .flatten()
            .find(|&&ret| returning(ret).len() == 1);
        let annotate = unique.map(|&ret| Annotate {
            call: self.source.file.text(span).to_string(),
            binding: self.let_binding(span).unwrap_or_else(|| "x".to_string()),
            ty: self.type_name(ret),
        });
        self.report(DiagnosticKind::Ambiguous {
            method: name.name.to_string(),
            receiver: self.type_name(ty),
            interfaces: (candidates.iter())
                .map(|&(_, interface)| self.interfaces[interface.0].name.name.to_string())
                .collect(),
            at: name.span,
            annotate,
        });
        None
    }

    /// Checks the arguments of a call that cannot take them, for their own mistakes.
    pub(super) fn stray(&mut self, args: &[ast::Arg]) {
        for arg in args {
            // Only its diagnostics matter.
            let _ = self.expr(&arg.value, Some(UNKNOWN));
        }
    }
}

/// "`node` may be `None`", at the optional `receiver` of a `.` before `name`. With `fix`, a
/// `?.` may take the `.`'s place.
fn may_be_none(receiver: &ast::Expr, name: &ast::Ident, fix: bool) -> Diagnostic {
    DiagnosticKind::MayBeNone {
        value: path(receiver),
        at: receiver.span,
        access: fix.then(|| Span::new(receiver.span.end(), name.span.start())),
        peel: None,
    }
    .into()
}
