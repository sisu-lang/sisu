//! Members: fields and methods found through `.` and `?.`.

use super::expr::{if_some, local, no_member, path};
use super::{Checked, Checker, Declared, Poisoned, UNKNOWN, optional_of, type_of, wrap};
use crate::ast;
use crate::diagnostic::{Definition, Diagnostic, DiagnosticKind};
use crate::source::Span;
use crate::tir::{self, Type, TypeKind};

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
        let scrutinee = self.value(receiver, None);
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
        let bind = self.fresh(payload);
        let member = member(self, Ok(local(bind, payload, receiver.span)));
        let ty = match type_of(&member) {
            Some(Type::UNIT) => Type::UNIT,
            Some(ty) => optional_of(ty),
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
        let Some((class, index)) = self.find_field(type_of(&base), receiver, name, false) else {
            return Err(Poisoned::error());
        };
        let ty = self.classes[class.0].fields[index].ty;
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

    /// The class and index of field `name` of `receiver`, a value of type `base`. `None` when
    /// there is none, reported unless `base` is `Error`. `target` when the field is assigned,
    /// where neither `()` nor `?.` can follow the access.
    pub(super) fn find_field(
        &mut self,
        base: Option<Type>,
        receiver: &ast::Expr,
        name: &ast::Ident,
        target: bool,
    ) -> Option<(tir::ClassId, usize)> {
        let ty = base?;
        let method = |owner, safe: bool| DiagnosticKind::MethodNotField {
            name: name.name.to_string(),
            owner,
            at: name.span,
            call: (!target).then_some(safe),
        };
        let d = match ty.kind() {
            TypeKind::Class(class) => {
                let info = &self.classes[class.0];
                if let Some(index) = info.fields.iter().position(|f| f.name.name == name.name) {
                    return Some((class, index));
                }
                match info.methods.get(&name.name) {
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
            TypeKind::Enum(id) => match self.enums[id.0].methods.get(&name.name) {
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

    /// The method `name` of `receiver`, a value of type `ty`. `None` when there is none,
    /// reported unless `ty` is `Error`. `target` when the call is in an assignment target's
    /// chain, where `?.` cannot follow the receiver.
    pub(super) fn find_method(
        &mut self,
        ty: Option<Type>,
        receiver: &ast::Expr,
        name: &ast::Ident,
        target: bool,
    ) -> Option<tir::FuncId> {
        let ty = ty?;
        let d = match ty.kind() {
            TypeKind::Class(class) => {
                let info = &self.classes[class.0];
                match info.methods.get(&name.name) {
                    Some(&Declared::Known(func)) => return Some(func),
                    // Its own error is reported; the caller checks the arguments.
                    Some(Declared::Broken) => return None,
                    None => {}
                }
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
            TypeKind::Enum(id) => {
                let info = &self.enums[id.0];
                match info.methods.get(&name.name) {
                    Some(&Declared::Known(func)) => return Some(func),
                    // Its own error is reported; the caller checks the arguments.
                    Some(Declared::Broken) => return None,
                    None => {}
                }
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
    }
    .into()
}
