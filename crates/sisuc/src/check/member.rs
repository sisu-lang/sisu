//! Members: fields and methods found through `.` and `?.`.

use rustc_hash::FxHashMap;

use super::array::innermost;
use super::call::{Dot, takes_range};
use super::expr::{if_some, local, no_member, path};
use super::extend::{holder_of, method_holder};
use super::generic::{Site, named_params};
use super::reports::Reported;
use super::unused_names::Used;
use super::{
    Checked, Checker, Declared, Expectation, Holder, Poisoned, as_optional, known_type, type_of,
    wrap,
};
use crate::ast;
use crate::diagnostic::{Annotate, Definition, Diagnostic, DiagnosticKind, Peel, UnwrapHelp};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, InterfaceId, Type, TypeArgs, TypeKind};

/// How a field access uses the field.
#[derive(Clone, Copy)]
pub(super) enum Access {
    /// A value.
    Read,
    /// The target of an assignment or a compound assignment.
    Assign,
    /// The receiver of a `.push`, which may be a class's own `push`.
    Push,
    /// The place of a `mut` argument, which a `?.` cannot be.
    Mut,
}

/// What a field lookup found: the field's class, its index, and its type, `None` for `Error`.
pub(super) type FoundField = (tir::ClassId, usize, Option<Type>);

/// What a method lookup found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Found {
    /// A method that the program writes.
    Func(tir::FuncId),
    /// No written `to_string` answers the call, so the `Display` that the compiler writes does.
    /// `listed` is true when the check of the type arguments has reported the call's list of them.
    GeneratedDisplay { listed: bool },
}

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
        self.member_of(span, safe, scrutinee, receiver, name, member)
    }

    /// `member` of the checked `scrutinee`, the receiver `receiver`.
    pub(super) fn member_of(
        &mut self,
        span: Span,
        safe: bool,
        scrutinee: Checked,
        receiver: &ast::Expr,
        name: &ast::Ident,
        member: impl FnOnce(&mut Self, Checked) -> Checked,
    ) -> Checked {
        if !safe {
            return member(self, scrutinee);
        }
        let payload = match self.safe_payload(&scrutinee, receiver, name) {
            Ok(payload) => payload,
            Err(failed) => {
                let reported = failed.reported();
                // Only its own diagnostics matter.
                drop(member(self, Err(failed)));
                return Err(Poisoned::untyped(reported));
            }
        };
        let bind = self.fresh(payload);
        let member = member(self, Ok(local(bind, payload, receiver.span)));
        safe_value(span, bind, scrutinee, member)
    }

    /// The type that `receiver?.name` takes out of the optional `scrutinee`. A receiver that is
    /// no optional, or an optional of an optional, is reported. The `Err` is what the member is
    /// checked on then.
    pub(super) fn safe_payload(
        &mut self,
        scrutinee: &Checked,
        receiver: &ast::Expr,
        name: &ast::Ident,
    ) -> Result<Type, Poisoned> {
        let Some(TypeKind::Optional(payload)) = type_of(scrutinee).map(Type::kind) else {
            return Err(match known_type(scrutinee) {
                Ok(ty) => {
                    let kind = DiagnosticKind::SafeAccessNotOptional {
                        value: path(receiver),
                        at: receiver.span,
                        access: Span::new(receiver.span.end(), name.span.start()),
                    };
                    Poisoned::typed(ty, self.diagnostics.error(kind))
                }
                Err(reported) => Poisoned::untyped(reported),
            });
        };
        // `?.` takes one layer off: on a `C??` it leaves a `C?`, which has no members.
        if matches!(payload.kind(), TypeKind::Optional(_)) {
            let kind = DiagnosticKind::MayBeNone {
                value: path(receiver),
                at: receiver.span,
                access: None,
                write: false,
                peel: Some(Peel {
                    receiver: self.source.text(receiver.span).to_string(),
                    left: self.type_name(payload),
                    member: name.name.to_string(),
                    layers: optional_layers(payload),
                }),
                unwrap: UnwrapHelp::It,
            };
            let reported = self.diagnostics.error(kind);
            // The member is checked on the type that the `??` peeled, so that its arguments
            // report only their own mistakes.
            return Err(Poisoned::typed(innermost(payload), reported));
        }
        Ok(payload)
    }

    /// `base.name`, `base` checked from `receiver`.
    pub(super) fn field(
        &mut self,
        span: Span,
        base: Checked,
        receiver: &ast::Expr,
        name: &ast::Ident,
    ) -> Checked {
        let ty = known_type(&base).map_err(Poisoned::untyped)?;
        if let TypeKind::Optional(payload) = ty.kind() {
            let (_, reported) = (self.field_of_payload(payload, receiver, name, Access::Read))
                .map_err(Poisoned::untyped)?;
            // E0062 is reported. The member is untyped: the fix of the E0062 decides it.
            return Err(Poisoned::untyped(reported));
        }
        let (class, index, ty) =
            (self.find_field(ty, receiver, name, Access::Read)).map_err(Poisoned::untyped)?;
        self.record(Used::Field(class, index), span);
        match (base, ty) {
            (Ok(base), Some(ty)) => Ok(tir::Expr::new(
                tir::ExprKind::Field {
                    base: Box::new(base),
                    index,
                },
                ty,
                span,
            )),
            (Err(failed), ty) => Err(Poisoned::new(ty, failed.reported())),
            // The field's declared type did not resolve.
            (Ok(_), None) => Err(Poisoned::untyped(self.proof(name.span))), // proof: declaration
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
        info.fields[index]
            .ty
            .and_then(|ty| self.subst(ty, args.get()))
    }

    /// The class, index and type of field `name` of `receiver`, a value of type `base`. `Err` with
    /// the proof when there is none, reported here or before. `access` says how the field is used;
    /// no `()` can follow the access of a field that is not read. A receiver that may be `None`
    /// gets the `?.` fix unless an index follows in the chain (`Body::indexed`). On an optional,
    /// they are the field's on the payload, after E0062.
    pub(super) fn find_field(
        &mut self,
        ty: Type,
        receiver: &ast::Expr,
        name: &ast::Ident,
        access: Access,
    ) -> Result<FoundField, Reported> {
        let method = |owner, safe: bool| DiagnosticKind::MethodNotField {
            name: name.name.to_string(),
            owner,
            at: name.span,
            call: matches!(access, Access::Read).then_some(safe),
        };
        let d = match ty.kind() {
            TypeKind::Class(class, _) => {
                let info = &self.classes[class.0];
                if let Some(index) = info.field(name.name) {
                    return Ok((class, index, self.field_type(ty, index)));
                }
                match self.own_method(ty, name.name) {
                    Some(Declared::Known(func)) => {
                        let no_arguments = self.signatures[func.0].params.is_empty();
                        method(self.type_name(ty), no_arguments).into()
                    }
                    // Its own error is reported, and what it held is unknown.
                    Some(Declared::Poisoned(reported)) => return Err(reported),
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
                Some(Declared::Poisoned(reported)) => return Err(reported),
                None => DiagnosticKind::EnumField {
                    enum_name: self.type_name(ty),
                    at: name.span,
                }
                .into(),
            },
            TypeKind::Array(_) if name.name.as_str() == "len" => {
                method(self.type_name(ty), true).into()
            }
            // A field of the payload gives no value here: `field` and the caller of a write ask
            // for its class, index and type.
            TypeKind::Optional(payload) => {
                return (self.field_of_payload(payload, receiver, name, access))
                    .map(|(found, _)| found);
            }
            _ => no_member(Definition::Field, self.type_name(ty), name, []),
        };
        Err(self.diagnostics.error(d))
    }

    /// The class, index and type of field `name` of the innermost payload of the optional
    /// `receiver`, whose type is `payload?`, as `find_field` finds them. A `.` checks the member
    /// as `?.` does, on the payload with no value. E0062 follows only when the field is found.
    /// A field that is missing, or of the wrong kind, is the error: the `?.` fix would lead to
    /// it. A `payload` that is optional too gets no `?.` fix: `?.` takes one layer only. Its help
    /// peels the other layers first (`UnwrapHelp::Peel`), unless an index follows (`indexed`) or
    /// the field is a `mut` argument: the peeled `?.` fails there. With the field, the proof of
    /// the E0062.
    pub(super) fn field_of_payload(
        &mut self,
        payload: Type,
        receiver: &ast::Expr,
        name: &ast::Ident,
        access: Access,
    ) -> Result<(FoundField, Reported), Reported> {
        let inner = innermost(payload);
        let found = self.find_field(inner, receiver, name, access)?;
        let write = match access {
            Access::Read => false,
            Access::Assign | Access::Mut => true,
            Access::Push => self.field_is_array(inner, name.name),
        };
        // A path through `?.` is a place that cannot be a `mut` argument (E0170).
        let fix = payload_fix(payload, self.body.indexed) && !matches!(access, Access::Mut);
        // The peeled `?.` fails before an index and as a `mut` argument, as the `?.` fix does.
        let unwrap = if self.body.indexed || matches!(access, Access::Mut) {
            UnwrapHelp::It
        } else {
            self.peel_or(payload, (receiver, name), UnwrapHelp::It)
        };
        let reported = (self.diagnostics).error(may_be_none(receiver, name, fix, write, unwrap));
        Ok((found, reported))
    }

    /// Whether `ty` is a class type whose field `name` has an array type or an optional array
    /// type, so that a `push` onto it writes.
    fn field_is_array(&self, ty: Type, name: Symbol) -> bool {
        let TypeKind::Class(class, _) = ty.kind() else {
            return false;
        };
        let held = self.classes[class.0]
            .field(name)
            .and_then(|index| self.field_type(ty, index));
        let held = held.map(|held| match held.kind() {
            TypeKind::Optional(payload) => payload,
            _ => held,
        });
        held.is_some_and(|held| matches!(held.kind(), TypeKind::Array(_)))
    }

    /// The method of the class or enum type `ty` that its body or a plain `extend` block gives
    /// the name `name`, if any; a broken one for a method that a syntax error took. The first in
    /// source order wins, and two are reported (E0034) elsewhere.
    pub(super) fn own_method(&self, ty: Type, name: Symbol) -> Option<Declared<tir::FuncId>> {
        let holder = method_holder(ty)?;
        let body = match holder {
            Holder::Class(c) => self.classes[c.0].methods.get(&name),
            Holder::Enum(e) => self.enums[e.0].methods.get(&name),
            Holder::Array | Holder::I64 => None,
        };
        if let Some(&body) = body {
            return Some(body);
        }
        let blocks = self.extension_methods.get(&(holder, name));
        let plain = blocks.into_iter().flatten().find_map(|&(at, func)| {
            let ext = &self.extensions[at];
            (ext.interface.is_none() && self.covers(at, ty).is_some())
                .then_some(Declared::Known(func))
        });
        plain.or_else(|| {
            let reported = self.broken_extension_methods.get(&(holder, name))?;
            Some(Declared::Poisoned(*reported))
        })
    }

    /// Fills `Checker.candidates` with the methods named `name` of the conformance blocks that
    /// cover `ty` and whose bounds hold, each with its block's interface, in source order. The
    /// first block that covers `ty` and fails its bounds is `failed_block`.
    ///
    /// The third recorded exception to "A pass is linear in the program" (user, 2026-10-25; ruling
    /// F9 (e)): a call reads every block that holds a method of its name, `n` for `n` blocks, and
    /// only those.
    pub(super) fn conforming_methods(&mut self, ty: Type, name: Symbol) {
        self.candidates.clear();
        self.failed_block = None;
        let Some(holder) = method_holder(ty) else {
            return;
        };
        let key = (holder, name);
        let count = self.extension_methods.get(&key).map_or(0, Vec::len);
        for k in 0..count {
            let (at, func) = self.extension_methods[&key][k];
            let ext = &self.extensions[at];
            // A plain block has no interface, and so no conformance.
            let Some((bound, _)) = ext.interface.filter(|_| !ext.duplicate) else {
                continue;
            };
            let Some(stands) = self.covers(at, ty) else {
                continue;
            };
            if self.block_holds(at, stands) {
                self.candidates.push((
                    func,
                    bound.interface,
                    self.subst_args(bound.args, stands.get()),
                ));
            } else {
                self.failed_block.get_or_insert((at, func));
            }
        }
    }

    /// The method named by `site` of `receiver`, whose checked value is `value`, in the call that
    /// starts at `span`. The type's own method wins; else the method of the one conformance block
    /// that has one, or the one that the call's context picks (`pick_conforming`). `Err` with
    /// the proof when there is none, reported unless the type is `Error`. The type is no
    /// optional: `method_of_payload` takes the method from the payload.
    pub(super) fn find_method(
        &mut self,
        value: &Checked,
        site: Site<'_>,
        (span, nullary): (Span, bool),
    ) -> Result<Found, Reported> {
        let ty = known_type(value)?;
        let name = site.name;
        let mut masked = None;
        match self.own_method(ty, name.name) {
            Some(Declared::Known(func)) if self.plain_block_holds(func, ty) => {
                return Ok(Found::Func(func));
            }
            // A conformance that applies answers the call instead.
            Some(Declared::Known(func)) => masked = Some(func),
            // Its own error is reported; the caller checks the arguments.
            Some(Declared::Poisoned(reported)) => return Err(reported),
            None => {}
        }
        self.conforming_methods(ty, name.name);
        match self.candidates[..] {
            [] => {
                // Every type has a `Display`: when no method `to_string` answers the call, because
                // there is none or its bounds fail for the receiver, the generated one does. No
                // other method has a fallback. A written method whose bounds fail keeps its report
                // when the call has an argument: the call is not the generated one's. The same
                // holds for a list of type arguments, when the written method has type parameters
                // of its own: the generated `to_string` takes none.
                let written = masked.or_else(|| self.failed_block.map(|(_, func)| func));
                let generic = !site.written.is_empty()
                    && written.is_some_and(|func| self.signatures[func.0].own > 0);
                if name.name.as_str() == "to_string"
                    && ((nullary && !generic) || written.is_none())
                    && self.broken_block(ty).is_none()
                    && !self.names_to_string_field(ty)
                {
                    // A masked method with no type parameter took the list in `method_args`.
                    let listed = masked.is_some_and(|func| self.signatures[func.0].own == 0);
                    return Ok(Found::GeneratedDisplay { listed });
                }
                // No conformance answers, and the bounds of the plain block fail for the receiver.
                if let Some(func) = masked {
                    let at = (self.signatures[func.0].extension)
                        .expect("a method whose block bounds fail should have a block");
                    return Err(self.report_block_bound(at, ty, span, name.name.as_str()));
                }
                // A block covered the receiver, and its bounds fail: the type argument is wrong.
                if let Some((at, _)) = self.failed_block {
                    return Err(self.report_block_bound(at, ty, span, name.name.as_str()));
                }
            }
            [(func, ..)] => return Ok(Found::Func(func)),
            _ => {
                // Taken while the call picks, so that the buffer is free to the calls it reads.
                let candidates = std::mem::take(&mut self.candidates);
                let picked = self.pick_conforming(ty, site, span, &candidates);
                self.candidates = candidates;
                return picked.map(Found::Func);
            }
        }
        // A broken block may have given the type the method.
        if let Some(reported) = self.broken_block(ty) {
            return Err(reported);
        }
        // The duplicate conformance of the block is reported.
        if self.in_duplicate_block(ty, name.name) {
            return Err(self.proof(name.span)); // proof: declaration
        }
        // A rejected block may have given any type the method.
        if let Some(&reported) = self.rejected_methods.get(&name.name) {
            return Err(reported);
        }
        let d = match ty.kind() {
            TypeKind::Class(class, _) => {
                let info = &self.classes[class.0];
                if info.field(name.name).is_some() {
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
                if info.index.contains_key(&name.name) {
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
            _ => no_member(Definition::Method, self.type_name(ty), name, []),
        };
        Err(self.report_lookup_failure(d, name))
    }

    /// Reports `d`, the failure to find the method `name`, and records it for E0062 on an
    /// optional receiver (`Body::failed_lookups`). Each report of a lookup at `name` comes here.
    pub(super) fn report_lookup_failure(&mut self, d: Diagnostic, name: &ast::Ident) -> Reported {
        self.body.failed_lookups.insert(name.span);
        self.diagnostics.error(d)
    }

    /// `report_lookup_failure` for a method call that cannot be checked: the arguments report
    /// their own errors, and the call is poisoned.
    pub(super) fn rejected_lookup(
        &mut self,
        d: Diagnostic,
        name: &ast::Ident,
        args: &[ast::Arg],
    ) -> Checked {
        let reported = self.report_lookup_failure(d, name);
        self.labels(args);
        self.stray(args);
        Err(Poisoned::untyped(reported))
    }

    /// `receiver.method(args)` on `value`, a value of type `payload?`, `receiver` its source. A `.`
    /// checks the call as `?.` does: on the innermost payload, with no value. So the method's
    /// name and its arguments report their own mistakes. E0062 comes first, unless the lookup
    /// of the name failed (`Body::failed_lookups`). A method that is missing, or a field,
    /// stands alone: the `?.` fix would lead to it. `indexed` is `Body::indexed` of the call:
    /// an index follows it in its chain. The result is poisoned with no type: the fix of the
    /// E0062 decides the type (ADR 0049). A `payload` that is optional too gets the peel help,
    /// unless a `push` writes or an index follows: the peeled `?.` fails there.
    pub(super) fn method_of_payload(
        &mut self,
        span: Span,
        (value, receiver): (Checked, &ast::Expr),
        payload: Type,
        site: Site<'_>,
        args: &[ast::Arg],
        indexed: bool,
    ) -> Checked {
        let inner = innermost(payload);
        // The E0062 below, or the failed lookup of the name, is the report. No list holds it yet.
        let held = self.proof(site.name.span); // proof: held misfit
        let checked = Err(Poisoned::typed(inner, held));
        // The call reports its own mistakes, and its type waits for the fix.
        drop(self.method_call(span, Dot::Payload, checked, receiver, site, args));
        let name = site.name;
        if self.body.failed_lookups.contains(&name.span) {
            return Err(Poisoned::untyped(held));
        }
        // A `push` onto an optional array writes: the `?.` fix makes it do nothing.
        let write = name.name.as_str() == "push" && matches!(inner.kind(), TypeKind::Array(_));
        let unwrap = if write {
            self.pushed_array_unwrap(receiver, &value)
        } else if indexed {
            UnwrapHelp::It
        } else {
            self.peel_or(payload, (receiver, name), UnwrapHelp::It)
        };
        let fix = payload_fix(payload, indexed);
        let d = may_be_none(receiver, name, fix, write, unwrap);
        Err(Poisoned::untyped(self.diagnostics.error(d)))
    }

    /// The help of an E0062 for a `.` after `receiver`, an optional of the `payload?`, before
    /// `name`: the peel help when `payload` is optional too, else `otherwise`.
    fn peel_or(
        &self,
        payload: Type,
        (receiver, name): (&ast::Expr, &ast::Ident),
        otherwise: UnwrapHelp,
    ) -> UnwrapHelp {
        match optional_layers(payload) {
            0 => otherwise,
            layers => UnwrapHelp::Peel {
                receiver: self.source.text(receiver.span).to_string(),
                member: name.name.to_string(),
                layers,
            },
        }
    }

    /// The unwrap help for the optional array `receiver`, whose checked value is `value`, that a
    /// `push` writes. An unwrap gives a copy of the array, which the `push` does not change. Only
    /// the object of a `?.` helps, when its field stores the array: unwrapping the object makes
    /// the field a place. Else no unwrap does.
    /// `Checker::optional_array_target` states the same rule for an element place: change both.
    fn pushed_array_unwrap(&self, receiver: &ast::Expr, value: &Checked) -> UnwrapHelp {
        let (
            ast::ExprKind::Field {
                base: object,
                name,
                safe: true,
                ..
            },
            Ok(tir::Expr {
                kind: tir::ExprKind::IfSome { scrutinee, .. },
                ..
            }),
        ) = (&receiver.kind, value)
        else {
            return UnwrapHelp::Nothing;
        };
        // The scrutinee is an optional of a class: only a class has a field.
        if let TypeKind::Optional(owner) = scrutinee.ty.kind()
            && let TypeKind::Class(class, _) = owner.kind()
            && self.classes[class.0]
                .field(name.name)
                .and_then(|index| self.field_type(owner, index))
                .is_some_and(|held| matches!(held.kind(), TypeKind::Array(_)))
            && self.one_line(object.span)
        {
            UnwrapHelp::Object(self.source.text(object.span).to_string())
        } else {
            UnwrapHelp::Nothing
        }
    }

    /// Whether `ty` is a class with a field `to_string`, which a call of that name does not
    /// reach. The collection of the fields found the answer, so a call reads no field.
    fn names_to_string_field(&self, ty: Type) -> bool {
        matches!(ty.kind(), TypeKind::Class(class, _) if self.classes[class.0].names_to_string)
    }

    /// Whether a block that repeats a conformance of `ty` (E0153) declares the method `name`. The
    /// lookup skips the block, and its error says what is wrong.
    ///
    /// shortcut: the methods of such a block are not checked against the interface until the user
    /// removes one of the two conformances. Then the normal check runs.
    fn in_duplicate_block(&self, ty: Type, name: Symbol) -> bool {
        let blocks =
            method_holder(ty).and_then(|holder| self.extension_methods.get(&(holder, name)));
        blocks.is_some_and(|blocks| {
            blocks
                .iter()
                .any(|&(at, _)| self.extensions[at].duplicate && self.covers(at, ty).is_some())
        })
    }

    /// The proof of the report of a broken `extend` block of the class or enum `ty`, if it has
    /// one: it may have given `ty` the method that a call names.
    fn broken_block(&self, ty: Type) -> Option<Reported> {
        let holder = holder_of(ty)?;
        self.broken_extensions
            .get(&self.holder_name(holder))
            .copied()
    }

    /// `own_method`, unless the method is a plain block's and the bounds of its block fail for
    /// `ty`: then a conformance may answer a call (see `find_method`).
    pub(super) fn applicable_method(
        &mut self,
        ty: Type,
        name: Symbol,
    ) -> Option<Declared<tir::FuncId>> {
        match self.own_method(ty, name) {
            Some(Declared::Known(func)) if !self.plain_block_holds(func, ty) => None,
            found => found,
        }
    }

    /// The method `name` that an `extend` block of the prelude gives `ty`, an array or an `i64`,
    /// if any. The method of a class or an enum is `find_method`'s. A method whose block bounds
    /// fail for `ty` is none.
    pub(super) fn prelude_method(&mut self, ty: Type, name: Symbol) -> Option<tir::FuncId> {
        if !matches!(ty.kind(), TypeKind::Array(_) | TypeKind::I64) {
            return None;
        }
        // A block of the prelude has no syntax error, so no method of it is broken.
        if let Some(Declared::Known(func)) = self.applicable_method(ty, name) {
            Some(func)
        } else {
            None
        }
    }

    /// The names of the methods that the `extend` blocks of the prelude give `ty`, an array or an
    /// `i64`: what a report of an unknown method suggests.
    pub(super) fn prelude_method_names(&self, ty: Type) -> Vec<&'static str> {
        let holder = method_holder(ty);
        let mut names: Vec<&'static str> = (self.builtin_methods.iter())
            .inspect(|_| {
                #[cfg(test)]
                super::PRELUDE_NAME_STEPS.set(super::PRELUDE_NAME_STEPS.get() + 1);
            })
            .filter(|&&(h, _, at)| Some(h) == holder && self.covers(at, ty).is_some())
            .map(|&(_, name, _)| name.as_str())
            .collect();
        // Two blocks may give the same name.
        names.sort_unstable();
        names.dedup();
        names
    }

    /// The array types that the prelude's `extend` blocks give a method `name`, in source order:
    /// what a receiver needs to have it.
    pub(super) fn prelude_targets(&self, name: Symbol) -> Vec<Type> {
        let mut targets: Vec<Type> = (self.builtin_methods.iter())
            .filter(|&&(holder, method, _)| holder == Holder::Array && method == name)
            .map(|&(_, _, at)| self.extensions[at].target)
            .collect();
        targets.dedup();
        targets
    }

    /// Whether the bounds of the block of the method `func`, which a plain block gives or the
    /// body, hold for a receiver of type `ty`.
    fn plain_block_holds(&mut self, func: tir::FuncId, ty: Type) -> bool {
        let Some(at) = self.signatures[func.0].extension else {
            return true;
        };
        let stands = (self.covers(at, ty))
            .expect("the block of a method that a call found should cover the receiver");
        self.block_holds(at, stands)
    }

    /// The type `ret` that a candidate method returns on a receiver of type `ty`, in a call that
    /// writes `written` for the method's own type parameters: the type arguments of its owner
    /// stand for the owner's parameters. `None` when `ret` still holds a parameter that the call
    /// leaves to inference: the context cannot tell that candidate apart.
    fn candidate_result(
        &self,
        func: tir::FuncId,
        ty: Type,
        ret: Type,
        written: &[Type],
    ) -> Option<Type> {
        if !ret.has_param() {
            return Some(ret);
        }
        let mut with = self.owner_args(func, ty).get().to_vec();
        let owner = with.len();
        let sig = &self.signatures[func.0];
        let names = named_params(std::iter::once(Some(ret)), owner + sig.own);
        if !names[owner..].contains(&true) {
            return self.subst(ret, &with);
        }
        if written.len() != sig.own {
            return None;
        }
        with.extend_from_slice(written);
        self.subst(ret, &with)
    }

    /// The method among `candidates`, which are methods of conformance blocks that cover `ty`,
    /// that the call at `span`, which `site` writes, names (see `pick_among`).
    fn pick_conforming(
        &mut self,
        ty: Type,
        site: Site<'_>,
        span: Span,
        candidates: &[(tir::FuncId, InterfaceId, TypeArgs)],
    ) -> Result<tir::FuncId, Reported> {
        if Self::context_failed(site) {
            return Err(self.proof(span)); // proof: quiet reader
        }
        // A result that is `Error` has its own error.
        let results: Option<Vec<Type>> = (candidates.iter())
            .map(|&(func, ..)| self.signatures[func.0].ret)
            .collect();
        let Some(results) = results else {
            return Err(self.proof(span)); // proof: declaration
        };
        let written = self.written_list(site)?;
        let rets: Vec<Option<Type>> = (candidates.iter().zip(results))
            .map(|(&(func, ..), ret)| self.candidate_result(func, ty, ret, &written))
            .collect();
        let interfaces = (candidates.iter())
            .map(|&(_, interface, args)| self.bound_text(interface, args))
            .collect();
        let picked = self.pick_among(ty, site, span, &rets, interfaces)?;
        Ok(candidates[picked].0)
    }

    /// Whether the context of the call that `site` writes failed, and reported it: this call is
    /// poisoned with it, and picking among its candidates would only add to it.
    pub(super) fn context_failed(site: Site<'_>) -> bool {
        site.expected == Expectation::Poisoned
    }

    /// The type arguments that `site` writes, resolved without a report; none when it writes
    /// none. `Err` when a list does not resolve: it is reported, and poisons the call.
    pub(super) fn written_list(&mut self, site: Site<'_>) -> Result<Vec<Type>, Reported> {
        if site.written.is_empty() {
            return Ok(Vec::new());
        }
        match self.written_types(site.written) {
            Some(types) => Ok(types),
            // `resolve` gives no proof for a written type that does not resolve.
            None => Err(self.proof(site.angles)), // proof: declaration
        }
    }

    /// The place among the candidates of a call of `site` at `span`, on a receiver of type `ty`,
    /// of the only one whose result `rets` is the type its context expects, or, when none is, the
    /// only one whose result is the payload of an expected optional. A result that is `None`
    /// cannot be told from the others. Otherwise the call is ambiguous, reported with the `let`
    /// that picks one answer; `interfaces` name the candidates for the report.
    pub(super) fn pick_among(
        &mut self,
        ty: Type,
        site: Site<'_>,
        span: Span,
        rets: &[Option<Type>],
        interfaces: Vec<String>,
    ) -> Result<usize, Reported> {
        let expected = site.expected;
        // Whether a result is unknown, so that its candidate may return anything the others do.
        let open = rets.contains(&None);
        if let Some(expected) = expected.ty().filter(|_| !open) {
            let matching = |want: Type| {
                let mut hits = candidate_results(rets).filter(|&(_, ret)| ret == Some(want));
                let first = hits.next().map(|(i, _)| i);
                (first, hits.next().is_some())
            };
            let (mut first, mut more) = matching(expected);
            if first.is_none()
                && let TypeKind::Optional(payload) = expected.kind()
            {
                (first, more) = matching(payload);
            }
            if let (Some(only), false) = (first, more) {
                return Ok(only);
            }
        }
        // The first result that exactly one candidate has tells the `let` what to write.
        let mut counts: FxHashMap<Type, usize> = FxHashMap::default();
        for ret in candidate_results(rets).filter_map(|(_, ret)| ret) {
            *counts.entry(ret).or_default() += 1;
        }
        let unique =
            candidate_results(rets).find_map(|(_, ret)| ret.filter(|ret| counts[ret] == 1));
        let annotate = unique.filter(|_| !open).map(|ret| Annotate {
            call: self.source.text(span).to_string(),
            binding: self.let_binding(span).unwrap_or_else(|| "x".to_string()),
            ty: self.type_name(ret),
        });
        Err(self.error(DiagnosticKind::Ambiguous {
            method: site.name.name.to_string(),
            receiver: self.type_name(ty),
            interfaces,
            at: site.name.span,
            annotate,
        }))
    }

    /// The types `written`, resolved as a call's type arguments are, without a report: the call
    /// reports them when it checks its arguments. `None` when one does not resolve; then its
    /// reports stay, as no later step makes them.
    fn written_types(&mut self, written: &[ast::TypeExpr]) -> Option<Vec<Type>> {
        let before = self.diagnostics.mark();
        let types: Option<Vec<Type>> = written.iter().map(|ty| self.resolve(ty)).collect();
        if types.is_some() {
            // The types hold no poisoned value, so no value outlives the reports that are cut.
            self.diagnostics.drop_since(before);
        }
        types
    }

    /// `stray` for the arguments of `ty.name(..)`, where a string's `get` allows a range.
    pub(super) fn stray_call(&mut self, ty: Option<Type>, name: Symbol, args: &[ast::Arg]) {
        if takes_range(ty, name) {
            self.get_arguments(args);
        } else {
            self.stray(args);
        }
    }

    /// Checks the arguments of a call that cannot take them, for their own mistakes.
    pub(super) fn stray(&mut self, args: &[ast::Arg]) {
        for arg in args {
            // Only its diagnostics matter.
            drop(self.expr(&arg.value, Expectation::Poisoned));
        }
    }
}

/// The results of the candidates of a call, each with its place. The passes over them read through
/// here, so that a test counts the steps.
fn candidate_results(rets: &[Option<Type>]) -> impl Iterator<Item = (usize, Option<Type>)> {
    rets.iter().copied().enumerate().inspect(|_| {
        #[cfg(test)]
        super::PICK_STEPS.set(super::PICK_STEPS.get() + 1);
    })
}

/// The layers of optional that `ty` has.
fn optional_layers(mut ty: Type) -> usize {
    let mut layers = 0;
    while let TypeKind::Optional(inner) = ty.kind() {
        layers += 1;
        ty = inner;
    }
    layers
}

/// Whether E0062 for a `.` on `payload?` offers the `?.` fix. No index follows it in its chain.
/// The `payload` is no optional: `?.` takes one layer only.
fn payload_fix(payload: Type, indexed: bool) -> bool {
    !indexed && !matches!(payload.kind(), TypeKind::Optional(_))
}

/// "`node` may be `None`", at the optional `receiver` of a `.` before `name`. With `fix`, a
/// `?.` may take the `.`'s place. `write` when `name` is the field that an assignment or a `push`
/// changes, or the `push` itself. `unwrap` says which unwrap help fits.
fn may_be_none(
    receiver: &ast::Expr,
    name: &ast::Ident,
    fix: bool,
    write: bool,
    unwrap: UnwrapHelp,
) -> Diagnostic {
    // Without a fix, the help says what to do.
    let unwrap = match unwrap {
        UnwrapHelp::Nothing if !fix => UnwrapHelp::It,
        unwrap => unwrap,
    };
    DiagnosticKind::MayBeNone {
        value: path(receiver),
        at: receiver.span,
        access: fix.then(|| Span::new(receiver.span.end(), name.span.start())),
        write,
        peel: None,
        unwrap,
    }
    .into()
}

/// `receiver?.name`, whose `member` was checked on `bind`, the payload of `scrutinee`: its type
/// `U` makes the result a `U?`, or `unit` for a method with no value.
pub(super) fn safe_value(
    span: Span,
    bind: tir::LocalId,
    scrutinee: Checked,
    member: Checked,
) -> Checked {
    let ty = match known_type(&member) {
        Ok(Type::UNIT) => Type::UNIT,
        Ok(ty) => as_optional(ty),
        // A misfit's found type is optional already (see `Checker::report_unbound`).
        Err(reported) => {
            let found = member.err().and_then(|poisoned| poisoned.found);
            return Err(Poisoned {
                found,
                ..Poisoned::untyped(reported)
            });
        }
    };
    let (scrutinee, member) = match (scrutinee, member) {
        (Ok(scrutinee), Ok(member)) => (scrutinee, member),
        (Err(failed), _) | (_, Err(failed)) => return Err(Poisoned::typed(ty, failed.reported())),
    };
    let none = (ty != Type::UNIT).then_some(tir::Expr::new(tir::ExprKind::None, ty, span));
    let value = if member.ty == ty {
        member
    } else {
        wrap(member, ty)
    };
    Ok(if_some(bind, scrutinee, value, none, span))
}
