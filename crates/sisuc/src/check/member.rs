//! Members: fields and methods found through `.` and `?.`.

use rustc_hash::FxHashMap;

use super::expr::{if_some, local, no_member, path};
use super::extend::{holder_of, method_holder};
use super::generic::{Site, named_params};
use super::unused_names::Used;
use super::{Checked, Checker, Declared, Holder, Poisoned, UNKNOWN, as_optional, type_of, wrap};
use crate::ast;
use crate::diagnostic::{Annotate, Definition, Diagnostic, DiagnosticKind, Peel};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, InterfaceId, Type, TypeArgs, TypeKind};

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
                // Only its own diagnostics matter.
                drop(member(self, Err(failed)));
                return Err(Poisoned::error());
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
            if type_of(scrutinee).is_some() {
                let kind = DiagnosticKind::SafeAccessNotOptional {
                    value: path(receiver),
                    at: receiver.span,
                    access: Span::new(receiver.span.end(), name.span.start()),
                };
                self.diagnostics.push(kind.into());
            }
            return Err(Poisoned::of(type_of(scrutinee)));
        };
        // `?.` takes one layer off: on a `C??` it leaves a `C?`, which has no members.
        if matches!(payload.kind(), TypeKind::Optional(_)) {
            let kind = DiagnosticKind::MayBeNone {
                value: path(receiver),
                at: receiver.span,
                access: None,
                peel: Some(Peel {
                    receiver: self.source.text(receiver.span).to_string(),
                    left: self.type_name(payload),
                    member: name.name.to_string(),
                }),
            };
            self.diagnostics.push(kind.into());
            return Err(Poisoned::error());
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
        info.fields[index]
            .ty
            .and_then(|ty| self.subst(ty, args.get()))
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
                if let Some(index) = info.field(name.name) {
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
            (self.broken_extension_methods.contains(&(holder, name))).then_some(Declared::Broken)
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

    /// The method named by `site` of `receiver`, a value of type `ty`, in the call that starts at
    /// `span`. The type's own method wins; else the method of the one conformance block that has
    /// one, or the one that the call's context picks (`pick_conforming`). `None` when there is
    /// none, reported unless `ty` is `Error`. `target` when the call is in an assignment target's
    /// chain, where `?.` cannot follow the receiver.
    pub(super) fn find_method(
        &mut self,
        ty: Option<Type>,
        receiver: &ast::Expr,
        site: Site<'_>,
        target: bool,
        (span, nullary): (Span, bool),
    ) -> Option<Found> {
        let ty = ty?;
        let name = site.name;
        let mut masked = None;
        match self.own_method(ty, name.name) {
            Some(Declared::Known(func)) if self.plain_block_holds(func, ty) => {
                return Some(Found::Func(func));
            }
            // A conformance that applies answers the call instead.
            Some(Declared::Known(func)) => masked = Some(func),
            // Its own error is reported; the caller checks the arguments.
            Some(Declared::Broken) => return None,
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
                    && !self.has_broken_block(ty)
                    && !self.names_to_string_field(ty)
                {
                    // A masked method with no type parameter took the list in `method_args`.
                    let listed = masked.is_some_and(|func| self.signatures[func.0].own == 0);
                    return Some(Found::GeneratedDisplay { listed });
                }
                // No conformance answers, and the bounds of the plain block fail for the receiver.
                if let Some(func) = masked {
                    let at = (self.signatures[func.0].extension)
                        .expect("a method whose block bounds fail should have a block");
                    self.report_block_bound(at, ty, span);
                    return None;
                }
                // A block covered the receiver, and its bounds fail: the type argument is wrong.
                if let Some((at, _)) = self.failed_block {
                    self.report_block_bound(at, ty, span);
                    return None;
                }
            }
            [(func, ..)] => return Some(Found::Func(func)),
            _ => {
                // Taken while the call picks, so that the buffer is free to the calls it reads.
                let candidates = std::mem::take(&mut self.candidates);
                let picked = self.pick_conforming(ty, site, span, &candidates);
                self.candidates = candidates;
                return picked.map(Found::Func);
            }
        }
        // A broken block may have given the type the method.
        if self.has_broken_block(ty) {
            return None;
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
            TypeKind::Optional(_) => may_be_none(receiver, name, !target),
            _ => no_member(Definition::Method, self.type_name(ty), name, []),
        };
        self.diagnostics.push(d);
        None
    }

    /// Whether `ty` is a class with a field `to_string`, which a call of that name does not
    /// reach. The collection of the fields found the answer, so a call reads no field.
    fn names_to_string_field(&self, ty: Type) -> bool {
        matches!(ty.kind(), TypeKind::Class(class, _) if self.classes[class.0].names_to_string)
    }

    /// Whether an `extend` block of the class or enum `ty` is broken: it may have given `ty`
    /// the method that a call names.
    fn has_broken_block(&self, ty: Type) -> bool {
        holder_of(ty)
            .is_some_and(|holder| self.broken_extensions.contains(&self.holder_name(holder)))
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
    ) -> Option<tir::FuncId> {
        if self.context_failed(site, span) {
            return None;
        }
        // A result that is `Error` has its own error.
        let results: Option<Vec<Type>> = (candidates.iter())
            .map(|&(func, ..)| self.signatures[func.0].ret)
            .collect();
        let results = results?;
        let written = self.written_list(site)?;
        let rets: Vec<Option<Type>> = (candidates.iter().zip(results))
            .map(|(&(func, ..), ret)| self.candidate_result(func, ty, ret, &written))
            .collect();
        let interfaces = (candidates.iter())
            .map(|&(_, interface, args)| self.bound_text(interface, args))
            .collect();
        let picked = self.pick_among(ty, site, span, &rets, interfaces)?;
        Some(candidates[picked].0)
    }

    /// Whether the context of the call at `span` failed, and reported it: this call is poisoned
    /// with it, and picking among its candidates would only add to it.
    pub(super) fn context_failed(&self, site: Site<'_>, span: Span) -> bool {
        site.expected == Some(UNKNOWN) || self.quiet(span, site.expected)
    }

    /// The type arguments that `site` writes, resolved without a report; none when it writes
    /// none. `None` when a list does not resolve: it is reported, and poisons the call.
    pub(super) fn written_list(&mut self, site: Site<'_>) -> Option<Vec<Type>> {
        if site.written.is_empty() {
            Some(Vec::new())
        } else {
            self.written_types(site.written)
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
    ) -> Option<usize> {
        let expected = site.expected;
        // Whether a result is unknown, so that its candidate may return anything the others do.
        let open = rets.contains(&None);
        if let Some(expected) = expected.filter(|_| !open) {
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
                return Some(only);
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
        self.report(DiagnosticKind::Ambiguous {
            method: site.name.name.to_string(),
            receiver: self.type_name(ty),
            interfaces,
            at: site.name.span,
            annotate,
        });
        None
    }

    /// The types `written`, resolved as a call's type arguments are, without a report: the call
    /// reports them when it checks its arguments. `None` when one does not resolve; then its
    /// reports stay, as no later step makes them.
    fn written_types(&mut self, written: &[ast::TypeExpr]) -> Option<Vec<Type>> {
        let before = self.diagnostics.len();
        let types: Option<Vec<Type>> = written.iter().map(|ty| self.resolve(ty)).collect();
        if types.is_some() {
            self.diagnostics.truncate(before);
        }
        types
    }

    /// Checks the arguments of a call that cannot take them, for their own mistakes.
    pub(super) fn stray(&mut self, args: &[ast::Arg]) {
        for arg in args {
            // Only its diagnostics matter.
            drop(self.expr(&arg.value, Some(UNKNOWN)));
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

/// `receiver?.name`, whose `member` was checked on `bind`, the payload of `scrutinee`: its type
/// `U` makes the result a `U?`, or `unit` for a method with no value.
pub(super) fn safe_value(
    span: Span,
    bind: tir::LocalId,
    scrutinee: Checked,
    member: Checked,
) -> Checked {
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
