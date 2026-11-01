//! `any I`, the checker's side. It covers how `any I` is written and which interfaces can be
//! `any`. It also covers which interfaces an `any I` conforms to, and how a value converts to it.

use std::rc::Rc;

use rustc_hash::FxHashSet;

use super::interface::{BOUND_CLOSURE_LIMIT, Bound, Check};
use super::unused_names::Used;
use super::{Checked, CheckedBlock, Checker, Poisoned, block_type, wrap};
use crate::ast;
use crate::conformance::{any_closure, interface_closure};
use crate::diagnostic::{DiagnosticKind, NotAny, NotConformForm};
use crate::source::Span;
use crate::tir::{self, InterfaceId, Type, TypeArgs, TypeKind};

#[cfg(test)]
thread_local! {
    /// How many interfaces `why_not_any` has put in a closure, on this thread. It adds how many
    /// `entry_reason` has read for a reason. An answer that either kept is not counted.
    pub(super) static ANY_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// What `any_join_all` found.
pub(super) enum AnyJoin {
    /// No member is an `any` type: the caller joins as before.
    NoAny,
    /// The members join to `ty`, the widest `any` type, first met at member `first`.
    Joined { first: usize, ty: Type },
    /// Two members do not join: the later one, an index and its type, does not convert to the
    /// earlier one, or the other way round when the `any` type is the later one.
    Fails {
        earlier: (usize, Type),
        later: (usize, Type),
    },
}

impl AnyJoin {
    /// The failure of the member `other` against `ours`, the join so far, as a fold in the
    /// order of the members reports it: the later member against the earlier one.
    fn fails(ours: (usize, Type), other: (usize, Type)) -> AnyJoin {
        let (earlier, later) = if other.0 < ours.0 {
            (other, ours)
        } else {
            (ours, other)
        };
        AnyJoin::Fails { earlier, later }
    }
}

/// Why an interface cannot be `any`. The interface `owner` is the interface itself or one that it
/// inherits. `kind` names its requirement or associated type, by place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AnyReason {
    owner: InterfaceId,
    kind: ReasonKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReasonKind {
    /// The requirement at this place has another `Self` in its parameters.
    TakesSelf(usize),
    /// The requirement at this place returns this type, which names `Self`.
    Returns(usize, Type),
    /// The requirement at this place has type parameters of its own.
    Generic(usize),
    /// The associated type at this place.
    Assoc(usize),
}

impl Checker<'_> {
    /// The type `any I` that `interface`, written as `written`, makes. The interface and its
    /// arguments resolve as a bound does. The check that the interface can be `any` waits for
    /// every interface, as a bound check does. The result is `None`, reported, when `written`
    /// names no interface or the interface cannot be `any`.
    pub(super) fn resolve_any(
        &mut self,
        ty: &ast::TypeExpr,
        written: &ast::TypeExpr,
    ) -> Option<Type> {
        let bound = self.bound(written, None)?;
        let any = Type::intern(TypeKind::Any(bound.interface, bound.args));
        if let ast::TypeKind::Applied(applied) = &written.kind {
            let ats = applied.args.iter().map(|arg| arg.span).collect();
            self.check_arguments(bound, any, ats);
        }
        if !ty.span.in_prelude() {
            self.any_written.entry(any).or_insert(ty.bare);
        }
        self.record(Used::Interface(bound.interface), bound.at);
        self.check_or_wait(Check::Any { bound }).then_some(any)
    }

    /// Reports that the interface `name`, written as the type `ty` at `at`, is no type. The help
    /// offers `any` when the interface can be `any`, which is known once every interface has its
    /// requirements: until then the report waits.
    pub(super) fn interface_as_type(
        &mut self,
        interface: InterfaceId,
        name: String,
        ty: &ast::TypeExpr,
        at: Span,
    ) {
        let written = self.source.slice(ty.bare.range()).to_string();
        self.report_as_type(interface, name, written, (at, usize::from(ty.optional)));
    }

    /// `interface_as_type` for the name as written and the `?` after it, which a held report
    /// keeps.
    fn report_as_type(
        &mut self,
        interface: InterfaceId,
        name: String,
        written: String,
        (at, layers): (Span, usize),
    ) {
        if self.interfaces_resolved {
            let any = self.why_not_any(interface).is_none();
            self.report(DiagnosticKind::InterfaceAsType {
                name,
                written: Some(written),
                at,
                any,
                layers,
            });
        } else {
            self.deferred_as_type
                .push((interface, name, written, (at, layers)));
        }
    }

    /// Makes the reports that `interface_as_type` held back.
    pub(super) fn report_deferred_as_type(&mut self) {
        for (interface, name, written, at) in std::mem::take(&mut self.deferred_as_type) {
            self.report_as_type(interface, name, written, at);
        }
    }

    /// Reports that the interface of `bound` cannot be `any`, for the reason `reason`.
    pub(super) fn cannot_be_any(&mut self, bound: Bound, reason: AnyReason) {
        let method = |this: &Self, at: usize| {
            this.interfaces[reason.owner.0].requirements[at]
                .name
                .name
                .to_string()
        };
        let why = match reason.kind {
            ReasonKind::TakesSelf(at) => NotAny::TakesSelf {
                method: method(self, at),
            },
            ReasonKind::Returns(at, ret) => NotAny::Returns {
                method: method(self, at),
                ret: self.type_name(ret),
            },
            ReasonKind::Generic(at) => NotAny::Generic {
                method: method(self, at),
            },
            ReasonKind::Assoc(at) => {
                let info = &self.interfaces[reason.owner.0];
                NotAny::Assoc {
                    owner: info.name.name.to_string(),
                    assoc: info.assoc[at].name.name.to_string(),
                }
            }
        };
        let owner = (reason.owner != bound.interface)
            .then(|| self.interfaces[reason.owner.0].name.name.to_string());
        self.report(DiagnosticKind::CannotBeAny {
            interface: self.bound_text(bound.interface, bound.args),
            owner,
            reason: why,
            at: bound.at,
        });
    }

    /// Why `interface` cannot be `any`, or `None` when it can. The reason is the first one among
    /// its own requirements, then in each interface it inherits, depth first in the order of the
    /// lists. The answers are kept by interface. The reason of each interface in the walk is kept
    /// by interface and arguments, so a shared ancestor is read once.
    ///
    /// The requirements are read over the interface's own parameters and `Self`. The answer is
    /// the same for every argument of a generic interface, because an argument names no `Self`.
    pub(super) fn why_not_any(&mut self, interface: InterfaceId) -> Option<AnyReason> {
        if let Some(&answer) = self.any_roots.get(&interface) {
            return answer;
        }
        let params = &self.interfaces[interface.0].type_params;
        let this = Type::intern(TypeKind::Param(params[0], 0));
        let own: Vec<Type> = (1..)
            .zip(&params[1..])
            .map(|(i, &name)| Type::intern(TypeKind::Param(name, i)))
            .collect();
        let interfaces = &self.interfaces;
        let closure = interface_closure(
            |inherited: InterfaceId| {
                interfaces[inherited.0]
                    .inherits
                    .iter()
                    .map(|b| (b.interface, b.args))
            },
            (interface, TypeArgs::intern(&own)),
            this,
            BOUND_CLOSURE_LIMIT,
            || {
                #[cfg(test)]
                ANY_VISITS.set(ANY_VISITS.get() + 1);
            },
        );
        let answer = closure
            .into_iter()
            .find_map(|(entry, args)| self.entry_reason(entry, args, this));
        self.any_roots.insert(interface, answer);
        answer
    }

    /// The first reason that `interface` cannot be `any`, as an entry of a walk with these `args`.
    /// `this` stands for the `Self` of the root. A reason is an associated type, or a requirement
    /// that is generic, takes `Self` in a parameter, or returns it.
    fn entry_reason(
        &mut self,
        interface: InterfaceId,
        args: TypeArgs,
        this: Type,
    ) -> Option<AnyReason> {
        if let Some(&answer) = self.any_entries.get(&(interface, args)) {
            return answer;
        }
        #[cfg(test)]
        ANY_VISITS.set(ANY_VISITS.get() + 1);
        let info = &self.interfaces[interface.0];
        let mut with = vec![this];
        with.extend_from_slice(args.get());
        let reason = |kind| {
            Some(AnyReason {
                owner: interface,
                kind,
            })
        };
        let answer = if info.assoc.is_empty() {
            (info.requirements.iter().enumerate()).find_map(|(at, requirement)| {
                if !requirement.own.is_empty() {
                    return reason(ReasonKind::Generic(at));
                }
                let takes = (requirement.params.iter().flatten())
                    .any(|&param| mentions_self(param.subst(&with)));
                if takes {
                    return reason(ReasonKind::TakesSelf(at));
                }
                let ret = requirement.ret?.subst(&with);
                mentions_self(ret)
                    .then_some(ReasonKind::Returns(at, ret))
                    .and_then(reason)
            })
        } else {
            reason(ReasonKind::Assoc(0))
        };
        self.any_entries.insert((interface, args), answer);
        answer
    }

    /// `any_ty`, an `any I`, and every interface that `I` inherits, each with its arguments once.
    /// These are the interfaces that `any_ty` conforms to. Its calls use their requirements.
    pub(super) fn any_interfaces(&mut self, any_ty: Type) -> Rc<[(InterfaceId, TypeArgs)]> {
        if let Some(closure) = self.any_closures.get(&any_ty) {
            return Rc::clone(closure);
        }
        let interfaces = &self.interfaces;
        let closure: Rc<[(InterfaceId, TypeArgs)]> = any_closure(
            |inherited: InterfaceId| {
                interfaces[inherited.0]
                    .inherits
                    .iter()
                    .map(|b| (b.interface, b.args))
            },
            any_ty,
            || {},
        )
        .into();
        self.any_closures.insert(any_ty, Rc::clone(&closure));
        closure
    }

    /// Whether `any_ty`, an `any I`, conforms to `interface` with `args`: `I` is `interface`, or
    /// inherits it. `Display` is every type's, and `conforms` answers it first.
    pub(super) fn any_conforms(
        &mut self,
        any_ty: Type,
        interface: InterfaceId,
        args: TypeArgs,
    ) -> bool {
        self.any_interfaces(any_ty).contains(&(interface, args))
    }

    /// `e`, of type `got`, where `want` is expected, when `want` is `any I` or `(any I)?`:
    /// `Ok` holds the checked value and whether the rule failed, as `against` returns them. `Err`
    /// gives `checked` back when no conversion applies, for the caller to report a mismatch.
    ///
    /// A value converts when its type conforms to `I`. It must also be an object, an `any` of an
    /// interface that inherits `I`, or a type with `AnyObject`. Any other value is a `Mismatch`
    /// until it can be boxed. An optional does not convert, since a `None` has no object.
    pub(super) fn any_conversion(
        &mut self,
        at: Span,
        checked: Checked,
        want: Type,
        got: Type,
    ) -> Result<(Checked, bool), Checked> {
        let (target, layers) = match want.kind() {
            TypeKind::Any(..) => (want, 0),
            TypeKind::Optional(payload) if matches!(payload.kind(), TypeKind::Any(..)) => {
                (payload, 1)
            }
            _ => return Err(checked),
        };
        let TypeKind::Any(interface, args) = target.kind() else {
            unreachable!("the target should be an `any` type");
        };
        if matches!(got.kind(), TypeKind::Optional(_)) || got == Type::NEVER || got == Type::UNIT {
            return Err(checked);
        }
        if !self.conforms(got, interface, args) {
            let asks = self.any_written.get(&target).copied();
            self.report(DiagnosticKind::NotConform {
                ty: self.type_name(got),
                interface: self.bound_text(interface, args),
                at,
                form: NotConformForm::Any { asks },
            });
            return Ok((Err(Poisoned::error()), true));
        }
        if !self.converts_now(got) {
            return Err(checked);
        }
        // A conversion makes a table, which only `mono` reaches.
        self.instantiates = true;
        self.record(Used::Interface(interface), at);
        let converted = match checked {
            Ok(e) => {
                let e = to_any(e, target);
                Ok(if layers == 1 { wrap(e, want) } else { e })
            }
            Err(_) => Err(Poisoned::typed(want)),
        };
        Ok((converted, false))
    }

    /// Converts the value of `block`, which ends at `at`, where `want` is expected. It does what
    /// `any_conversion` does for an expression, for the value of a function's body or a branch.
    pub(super) fn any_block(&mut self, block: &mut CheckedBlock, want: Type, at: Span) {
        let Some(got) = block_type(block) else {
            return;
        };
        let any = matches!(want.kind(), TypeKind::Any(..) | TypeKind::Optional(_));
        if got == want || !any || got == Type::NEVER || got == Type::UNIT {
            return;
        }
        match std::mem::replace(block, Err(Poisoned::error())) {
            Ok(mut done) => {
                let value = done
                    .value
                    .take()
                    .expect("a block checked as a value type should end in its value");
                match self.any_conversion(at, Ok(*value), want, got) {
                    Ok((Ok(converted), _)) => {
                        done.value = Some(Box::new(converted));
                        done.ty = want;
                        *block = Ok(done);
                    }
                    Err(Ok(unchanged)) => {
                        done.value = Some(Box::new(unchanged));
                        *block = Ok(done);
                    }
                    Ok((Err(poisoned), _)) | Err(Err(poisoned)) => *block = Err(poisoned),
                }
            }
            Err(poisoned) => {
                *block = match self.any_conversion(at, Err(poisoned), want, got) {
                    Ok((converted, _)) | Err(converted) => {
                        Err(converted.err().unwrap_or_else(Poisoned::error))
                    }
                };
            }
        }
    }

    /// `e`, converted to `want`, an `any` type or its optional, that `converts_to_any` has
    /// accepted for the type of `e`.
    pub(super) fn make_any(&mut self, e: tir::Expr, want: Type) -> tir::Expr {
        self.instantiates = true;
        match want.kind() {
            TypeKind::Optional(payload) => wrap(to_any(e, payload), want),
            _ => to_any(e, want),
        }
    }

    /// Whether a value of type `got` converts to `want`, which `any_conversion` answers by doing
    /// it: `want` is `any I` or `(any I)?`, and `got` conforms and needs no box.
    pub(super) fn converts_to_any(&mut self, got: Type, want: Type) -> bool {
        let target = match want.kind() {
            TypeKind::Optional(payload) => payload,
            _ => want,
        };
        let TypeKind::Any(interface, args) = target.kind() else {
            return false;
        };
        !matches!(got.kind(), TypeKind::Optional(_))
            && got != Type::NEVER
            && got != Type::UNIT
            && self.conforms(got, interface, args)
            && self.converts_now(got)
    }

    /// The type that values of types `a` and `b` join to when one converts to the other: the `any`
    /// type, with its optional layer. A join never makes an `any` type that neither side has.
    pub(super) fn any_join(&mut self, a: Type, b: Type) -> Option<Type> {
        if a == b || self.converts_to_any(b, a) {
            Some(a)
        } else if self.converts_to_any(a, b) {
            Some(b)
        } else {
            None
        }
    }

    /// The join of the typed `members`, each an index and a type, when one of them is an `any`
    /// type. The join is the widest of those, and each other member must convert to it. It does
    /// not depend on the order of the members, as `any_join` of two does not see a third.
    pub(super) fn any_join_all(&mut self, members: &[(usize, Type)]) -> AnyJoin {
        let mut targets = members.iter().filter(|&&(_, ty)| is_target(ty));
        let Some(&(first, mut ty)) = targets.next() else {
            return AnyJoin::NoAny;
        };
        for &(other, t) in targets {
            match self.any_join(ty, t) {
                Some(wider) => ty = wider,
                None => return AnyJoin::fails((first, ty), (other, t)),
            }
        }
        for &(other, t) in members {
            if !is_target(t) && t != ty && !self.converts_to_any(t, ty) {
                return AnyJoin::fails((first, ty), (other, t));
            }
        }
        AnyJoin::Joined { first, ty }
    }

    /// Whether a value of type `got`, which conforms, converts to `any` without a box. The type
    /// is an object type, an `any`, or a type whose values are objects or `any`s (`AnyObject`).
    fn converts_now(&mut self, got: Type) -> bool {
        match got.kind() {
            TypeKind::Class(..) | TypeKind::Any(..) => true,
            TypeKind::Param(..) | TypeKind::Assoc(..) => {
                self.conforms(got, self.lang.any_object, TypeArgs::NONE)
            }
            _ => false,
        }
    }
}

/// Whether `want` is `any I` or `(any I)?`: the types that a value converts to.
pub(super) fn is_target(want: Type) -> bool {
    match want.kind() {
        TypeKind::Any(..) => true,
        TypeKind::Optional(payload) => matches!(payload.kind(), TypeKind::Any(..)),
        _ => false,
    }
}

/// `expected`, split in two: the context that is no `any` type, and the one that is.
pub(super) fn split_context(expected: Option<Type>) -> (Option<Type>, Option<Type>) {
    (
        expected.filter(|&want| !is_target(want)),
        expected.filter(|&want| is_target(want)),
    )
}

/// `e`, of a type that a join widened, as `want`. That is an `any` type, an optional `any` type,
/// or another optional.
pub(super) fn widen(e: tir::Expr, want: Type) -> tir::Expr {
    match want.kind() {
        TypeKind::Any(..) => to_any(e, want),
        TypeKind::Optional(payload) if matches!(payload.kind(), TypeKind::Any(..)) => {
            let e = if e.ty == payload {
                e
            } else {
                to_any(e, payload)
            };
            wrap(e, want)
        }
        _ => wrap(e, want),
    }
}

/// `e` as the `any` type `target`.
pub(super) fn to_any(e: tir::Expr, target: Type) -> tir::Expr {
    tir::Expr {
        span: e.span,
        kind: tir::ExprKind::ToAny(Box::new(e)),
        ty: target,
    }
}

/// Whether `ty` names `Self`, the parameter at place 0 of an interface's signature.
fn mentions_self(ty: Type) -> bool {
    let mut seen = FxHashSet::default();
    let mut stack = vec![ty];
    while let Some(ty) = stack.pop() {
        if !ty.has_param() || !seen.insert(ty) {
            continue;
        }
        match ty.kind() {
            TypeKind::Param(_, place) => {
                if place == 0 {
                    return true;
                }
            }
            TypeKind::Array(part) | TypeKind::Optional(part) => stack.push(part),
            TypeKind::Class(_, args) | TypeKind::Enum(_, args) | TypeKind::Any(_, args) => {
                stack.extend_from_slice(args.get());
            }
            TypeKind::Assoc(base, _, args, _) => {
                stack.push(base);
                stack.extend_from_slice(args.get());
            }
            _ => {}
        }
    }
    false
}
