//! `any I`, the checker's side. It covers how `any I` is written and which interfaces can be
//! `any`. It also covers which interfaces an `any I` conforms to, and how a value converts to it.

use std::rc::Rc;

use rustc_hash::FxHashSet;

use super::alias::UseCheck;
use super::interface::{BOUND_CLOSURE_LIMIT, Bound, Check};
use super::reports::Reported;
use super::unused_names::Used;
use super::{Checked, CheckedBlock, Checker, DeferredAsType, Origin, Poisoned, block_type};
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

/// One step that makes a value of one type a value of another (see `Checker::conversion`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Conversion {
    /// `T` to `T?`.
    Wrap,
    /// A value to this `any` type.
    ToAny(Type),
}

/// The interfaces that an `any` type conforms to, and whether it is unknown (`any_unknown`). The
/// answer outlives a body, so it is stored with the closure.
#[derive(Clone)]
pub(super) struct AnyClosure {
    /// The interface of the `any` type and each one it inherits, each with its arguments once.
    interfaces: Rc<[(InterfaceId, TypeArgs)]>,
    /// Whether an interface of the closure has an inheritance that failed.
    unknown: bool,
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
    /// The root inherits the interface `owner` with these arguments, which name `Self`.
    InheritsSelf(TypeArgs),
    /// The requirement at this place takes `mut self`. Part 3 lets `any` call it.
    MutSelf(usize),
}

/// The `any` type that a value converts to under `want`, which is `any I` or `(any I)?`, and the
/// layers of optional that the conversion wraps. `None` for any other `want`.
pub(super) fn any_target(want: Type) -> Option<(Type, usize)> {
    match want.kind() {
        TypeKind::Any(..) => Some((want, 0)),
        TypeKind::Optional(payload) if matches!(payload.kind(), TypeKind::Any(..)) => {
            Some((payload, 1))
        }
        _ => None,
    }
}

impl Checker<'_> {
    /// The type `any I` that `interface`, written as `written`, makes. The interface and its
    /// arguments resolve as a bound does. The check that the interface can be `any` waits for
    /// every interface, as a bound check does. The result is `None`, reported, when `written`
    /// names no interface or the interface cannot be `any`.
    pub(super) fn resolve_any(&mut self, written: &ast::TypeExpr) -> Option<Type> {
        let bound = self.bound(written, None)?;
        let any = Type::intern(TypeKind::Any(bound.interface, bound.args));
        // A program that names an `any` type needs the layout of its table, which only `mono`
        // gives, whether or not a value converts to it.
        self.instantiates = true;
        if let ast::TypeKind::Applied(applied) = &written.kind {
            // In an alias body, an argument that names a parameter waits for each use of the alias.
            if self.body.alias.is_some() && any.has_param() {
                // An `any` that names a parameter waits, so the check holds.
                let _ = self.make_check(UseCheck::Applied(any), Span::new(0, 0));
            } else {
                let ats = applied.args.iter().map(|arg| arg.span).collect();
                self.check_arguments(bound, any, ats);
            }
        }
        self.record(Used::Interface(bound.interface), bound.at);
        self.check_or_wait(Check::Any { bound }).ok().map(|()| any)
    }

    /// Reports that the interface `name`, written as the type `ty` at `at`, is no type. The help
    /// offers `any` when the interface can be `any`, which is known once every interface has its
    /// requirements: until then the report waits. In the body of a type
    /// alias, the help offers `any` only, because an alias takes no bound.
    pub(super) fn interface_as_type(
        &mut self,
        interface: InterfaceId,
        name: String,
        ty: &ast::TypeExpr,
        at: Span,
    ) {
        let written = self.source.type_text(ty.bare).into_owned();
        let alias = self.body.alias.is_some();
        self.report_as_type(DeferredAsType {
            interface,
            name,
            written,
            at,
            layers: usize::from(ty.optional),
            alias,
        });
    }

    /// `interface_as_type` for the name as written and the `?` after it, which a held report
    /// keeps.
    fn report_as_type(&mut self, held: DeferredAsType) {
        if self.interfaces_resolved {
            let any = self.why_not_any(held.interface).is_none();
            self.error(DiagnosticKind::InterfaceAsType {
                name: held.name,
                written: Some(held.written),
                at: held.at,
                any,
                layers: held.layers,
                alias: held.alias,
            });
        } else {
            self.deferred_as_type.push(held);
        }
    }

    /// Makes the reports that `interface_as_type` held back.
    pub(super) fn report_deferred_as_type(&mut self) {
        for held in std::mem::take(&mut self.deferred_as_type) {
            self.report_as_type(held);
        }
    }

    /// Reports that the interface of `bound` cannot be `any`, for the reason `reason`.
    pub(super) fn cannot_be_any(&mut self, bound: Bound, reason: AnyReason) -> Reported {
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
            ReasonKind::InheritsSelf(args) => NotAny::InheritsSelf {
                inherited: self.bound_text(reason.owner, args),
            },
            ReasonKind::MutSelf(at) => NotAny::MutSelf {
                method: method(self, at),
            },
        };
        let owner = (reason.owner != bound.interface
            && !matches!(reason.kind, ReasonKind::InheritsSelf(_)))
        .then(|| self.interfaces[reason.owner.0].name.name.to_string());
        self.error(DiagnosticKind::CannotBeAny {
            interface: self.bound_text(bound.interface, bound.args),
            owner,
            reason: why,
            at: bound.at,
        })
    }

    /// Why `interface` cannot be `any`, or `None` when it can. The reason is the first one among
    /// its own requirements, then in each interface it inherits, depth first in the order of the
    /// lists. A `mut self` requirement is the reason only when no interface has another. The
    /// answers are kept by interface. The reason of each interface in the walk is kept by
    /// interface and arguments, so a shared ancestor is read once.
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
        let lasting = closure.iter().find_map(|&(entry, args)| {
            // An argument that names `Self` names the type behind the `any` in a table, which
            // has no conformance for it.
            if args.get().iter().any(|&arg| mentions_self(arg)) {
                return Some(AnyReason {
                    owner: entry,
                    kind: ReasonKind::InheritsSelf(args),
                });
            }
            self.entry_reason(entry, args, this)
        });
        // A `mut self` requirement is a reason only until part 3, so a lasting reason wins.
        let answer = lasting.or_else(|| {
            closure.iter().find_map(|&(entry, _)| {
                let requirements = &self.interfaces[entry.0].requirements;
                let at = requirements.iter().position(|r| r.self_mut.is_some())?;
                Some(AnyReason {
                    owner: entry,
                    kind: ReasonKind::MutSelf(at),
                })
            })
        });
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
        // No key holds a variable today; the guard keeps it so when an `any` argument can.
        if !args.holds_infer() {
            self.any_entries.insert((interface, args), answer);
        }
        answer
    }

    /// `any_ty`, an `any I`, and every interface that `I` inherits, each with its arguments once.
    /// These are the interfaces that `any_ty` conforms to. Its calls use their requirements.
    pub(super) fn any_interfaces(&mut self, any_ty: Type) -> Rc<[(InterfaceId, TypeArgs)]> {
        self.any_closure_of(any_ty).interfaces
    }

    /// Whether `any_ty`, an `any I`, is unknown: an interface of its closure, `I` included, has an
    /// inheritance that failed. It conforms to every interface then, as a type parameter does
    /// (`is_unknown`).
    pub(super) fn any_unknown(&mut self, any_ty: Type) -> bool {
        self.any_closure_of(any_ty).unknown
    }

    /// The closure of `any_ty`, found when first asked for.
    fn any_closure_of(&mut self, any_ty: Type) -> AnyClosure {
        if let Some(closure) = self.any_closures.get(&any_ty) {
            return closure.clone();
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
        let unknown = self.closure_inherits_failed(&closure);
        let closure = AnyClosure {
            interfaces: closure,
            unknown,
        };
        if !any_ty.holds_infer() {
            self.any_closures.insert(any_ty, closure.clone());
        }
        closure
    }

    /// Whether `any_ty`, an `any I`, conforms to `interface` with `args`: `I` is `interface`, or
    /// inherits it, or `any_ty` is unknown. `Display` is every type's, and `conforms` answers it
    /// first.
    pub(super) fn any_conforms(
        &mut self,
        any_ty: Type,
        interface: InterfaceId,
        args: TypeArgs,
    ) -> bool {
        let closure = self.any_closure_of(any_ty);
        closure.unknown || closure.interfaces.contains(&(interface, args))
    }

    /// `e`, of type `got`, where `want` is expected, when `want` is `any I` or `(any I)?`:
    /// `Ok` holds the checked value and whether the rule failed, as `close` returns them. `Err`
    /// gives `checked` back when no conversion applies, for the caller to report a mismatch.
    ///
    /// A value converts when its type conforms to `I`: an object keeps its pointer, an `any` of
    /// an interface that inherits `I` keeps its data, and any other value goes into a counted box
    /// (`mono` and `codegen` choose, by the type that a parameter stands for). An optional
    /// does not convert: unwrap it first. An optional `any` value is written `(any I)?`.
    pub(super) fn any_conversion(
        &mut self,
        at: Span,
        checked: Checked,
        want: Type,
        got: Type,
        origin: Origin,
    ) -> Result<(Checked, bool), Checked> {
        let Some((target, layers)) = any_target(want) else {
            return Err(checked);
        };
        let TypeKind::Any(interface, args) = target.kind() else {
            unreachable!("the target should be an `any` type");
        };
        if !can_convert(got) {
            return Err(checked);
        }
        if !self.conforms(got, interface, args) {
            let reported = self.error(self.not_conform(at, got, target, origin));
            return Ok((Err(Poisoned::untyped(reported)), true));
        }
        let steps: &[Conversion] = if layers == 1 {
            &[Conversion::ToAny(target), Conversion::Wrap]
        } else {
            &[Conversion::ToAny(target)]
        };
        let converted = match checked {
            Ok(e) => self.apply(e, steps),
            Err(poisoned) => {
                self.converted_to_any(at, got, target);
                Err(Poisoned::typed(want, poisoned.reported()))
            }
        };
        Ok((converted, false))
    }

    /// Converts the value of `block`, which ends at `at`, where `want` is expected. It does what
    /// `any_conversion` does for an expression, for the value of a function's body or a branch.
    pub(super) fn any_block(
        &mut self,
        block: &mut CheckedBlock,
        want: Type,
        at: Span,
        origin: Origin,
    ) {
        let Some(got) = block_type(block) else {
            return;
        };
        let any = matches!(want.kind(), TypeKind::Any(..) | TypeKind::Optional(_));
        if got == want || !any || got == Type::NEVER || got == Type::UNIT {
            return;
        }
        match block {
            Ok(done) => {
                let value = done
                    .value
                    .take()
                    .expect("a block checked as a value type should end in its value");
                match self.any_conversion(at, Ok(*value), want, got, origin) {
                    Ok((Ok(converted), _)) => {
                        done.value = Some(Box::new(converted));
                        done.ty = want;
                    }
                    Err(Ok(unchanged)) => done.value = Some(Box::new(unchanged)),
                    Ok((Err(poisoned), _)) | Err(Err(poisoned)) => *block = Err(poisoned),
                }
            }
            Err(poisoned) => {
                let reported = poisoned.reported();
                let Err(poisoned) = std::mem::replace(block, Err(Poisoned::untyped(reported)))
                else {
                    unreachable!("the block should have failed");
                };
                *block = match self.any_conversion(at, Err(poisoned), want, got, origin) {
                    Ok((converted, _)) | Err(converted) => Err(converted
                        .err()
                        .unwrap_or_else(|| Poisoned::untyped(reported))),
                };
            }
        }
    }

    /// `e`, converted to `want`, an `any` type or its optional, that `converts_to_any` has
    /// accepted for the type of `e`.
    pub(super) fn make_any(&mut self, e: tir::Expr, want: Type) -> Checked {
        match want.kind() {
            TypeKind::Optional(payload) => {
                self.apply(e, &[Conversion::ToAny(payload), Conversion::Wrap])
            }
            _ => self.apply(e, &[Conversion::ToAny(want)]),
        }
    }

    /// The steps that make a value of type `from` a value of type `to` at its top, or `None`. It
    /// unifies before it wraps: a value whose type unifies with `to` takes no step. Else an
    /// optional `to` wraps a value whose type unifies with its payload. Under an optional `to`, a
    /// unification that breaks a bound that it made ready gives way to a wrap that breaks none.
    /// When the wrap breaks a bound too, the unification stays, and the bound reports (E0154). So
    /// `ns()` with `fn ns<T: Seen>() -> T?` wraps under `Eye??`: `T` is `Eye`. An `any` type takes
    /// a value whose type conforms to its interface. A value of an `Any` variable does not convert
    /// to an `any` type: it unifies. A value of an `Int` variable converts as `i64`, the type that
    /// a literal takes under an `any` type. It binds what it unifies, so a caller that only tests
    /// asks under a snapshot.
    pub(super) fn conversion(&mut self, from: Type, to: Type) -> Option<Vec<Conversion>> {
        if let TypeKind::Optional(payload) = self.table.resolve(to).kind() {
            if self.unify_within_bounds(from, to) {
                return Some(Vec::new());
            }
            if self.unify_within_bounds(from, payload) {
                return Some(vec![Conversion::Wrap]);
            }
        }
        if self.unify(from, to) {
            return Some(Vec::new());
        }
        let (from, to) = (self.table.resolve(from), self.table.resolve(to));
        let (target, wrapped) = match to.kind() {
            TypeKind::Optional(payload) => {
                if self.unify(from, payload) {
                    return Some(vec![Conversion::Wrap]);
                }
                (payload, true)
            }
            _ => (to, false),
        };
        let TypeKind::Any(interface, args) = target.kind() else {
            return None;
        };
        let open = from;
        let from = match from.kind() {
            TypeKind::Infer(_) => self.table.default_open(from),
            _ => from,
        };
        if from.holds_infer() || !can_convert(from) || !self.conforms(from, interface, args) {
            return None;
        }
        if open != from && !self.unify(open, from) {
            return None;
        }
        Some(if wrapped {
            vec![Conversion::ToAny(target), Conversion::Wrap]
        } else {
            vec![Conversion::ToAny(target)]
        })
    }

    /// `e` after each of `steps`, in order. A `Wrap` of a value with no value to lift is E0043,
    /// as `lift` reports it. A `ToAny` does the records of a conversion to an `any` type.
    pub(super) fn apply(&mut self, mut e: tir::Expr, steps: &[Conversion]) -> Checked {
        for &step in steps {
            e = match step {
                Conversion::Wrap => {
                    let want = Type::optional(e.ty);
                    self.lift(e, want)?
                }
                Conversion::ToAny(target) => {
                    let got = self.table.resolve(e.ty);
                    self.converted_to_any(e.span, got, target);
                    to_any(e, target)
                }
            };
        }
        Ok(e)
    }

    /// The records of a value of type `got` at `at` that converts to `target`, an `any` type: the
    /// interface is used, and the value can be shown.
    fn converted_to_any(&mut self, at: Span, got: Type, target: Type) {
        // A conversion makes a table, which only `mono` reaches.
        self.instantiates = true;
        if let TypeKind::Any(interface, _) = target.kind() {
            self.record(Used::Interface(interface), at);
        }
        self.record_shown_any(got, target);
    }

    /// Whether a value of type `got` converts to `want`, which `any_conversion` answers by doing
    /// it: `want` is `any I` or `(any I)?`, and `got` conforms.
    pub(super) fn converts_to_any(&mut self, got: Type, want: Type) -> bool {
        let target = match want.kind() {
            TypeKind::Optional(payload) => payload,
            _ => want,
        };
        let TypeKind::Any(interface, args) = target.kind() else {
            return false;
        };
        can_convert(got) && self.conforms(got, interface, args)
    }

    /// The E0154 of the value at `at`, of type `got`, against `want`. `want` is an `any` type, or
    /// an optional one, and `got` does not conform to its interface. `None` when the mistake is a
    /// mismatch.
    pub(super) fn any_misfit(
        &mut self,
        at: Span,
        want: Type,
        got: Type,
        origin: Origin,
    ) -> Option<DiagnosticKind> {
        let target = match want.kind() {
            TypeKind::Optional(payload) => payload,
            _ => want,
        };
        match target.kind() {
            // Two `any` types that do not join are a mismatch, not a conformance.
            TypeKind::Any(interface, args)
                if can_convert(got)
                    && !matches!(got.kind(), TypeKind::Any(..))
                    && !self.conforms(got, interface, args) =>
            {
                Some(self.not_conform(at, got, target, origin))
            }
            _ => None,
        }
    }

    /// The value at `at`, of type `got`, does not conform to the interface of `target`, an `any`
    /// type.
    fn not_conform(&self, at: Span, got: Type, target: Type, origin: Origin) -> DiagnosticKind {
        let TypeKind::Any(interface, args) = target.kind() else {
            unreachable!("the target should be an `any` type");
        };
        DiagnosticKind::NotConform {
            ty: self.type_name(got),
            interface: self.bound_text(interface, args),
            at,
            form: NotConformForm::Any {
                origin: self.origin_fact(origin),
            },
        }
    }
}

/// Whether a value of type `got` can convert to an `any` type, if it conforms. An optional does
/// not: unwrap it first. `unit` and `never` have no value to hold.
fn can_convert(got: Type) -> bool {
    !matches!(got.kind(), TypeKind::Optional(_)) && got != Type::NEVER && got != Type::UNIT
}

/// `e` as the `any` type `target`.
pub(super) fn to_any(e: tir::Expr, target: Type) -> tir::Expr {
    let span = e.span;
    tir::Expr::new(tir::ExprKind::ToAny(Box::new(e)), target, span)
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
            TypeKind::Class(_, args)
            | TypeKind::Struct(_, args)
            | TypeKind::Enum(_, args)
            | TypeKind::Any(_, args) => {
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
