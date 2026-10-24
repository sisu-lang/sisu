//! Generic declarations: type parameters, the type arguments written for them or inferred at
//! a call, and what a value of a type parameter may not do.

use std::{fmt, iter};

use rustc_hash::{FxHashMap, FxHashSet};

use super::enums::VariantInfo;
use super::expr::{int_literal, rank};
use super::prelude::PRELUDE;
use super::stmt::BlockSoFar;
use super::{
    Binding, Checked, Checker, Declared, Definition, DiagnosticKind, Poisoned, UNKNOWN,
    built_in_type, type_of,
};
use crate::ast::{self, StmtKind};
use crate::diagnostic::{ArgumentsForm, MainForm};
use crate::lexer::{TokenKind, lex};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{Type, TypeArgs, TypeKind};

/// Whether `ty`, an array's element, is an optional, or arrays of them: the shape of element
/// that `against` cannot wrap an array literal's elements to.
fn optional_element(mut ty: Type) -> bool {
    while let TypeKind::Array(inner) = ty.kind() {
        ty = inner;
    }
    matches!(ty.kind(), TypeKind::Optional(_))
}

/// Whether `ty` names the type parameter at `i`.
fn names_param(ty: Type, i: usize) -> bool {
    match ty.kind() {
        TypeKind::Param(_, j) => j as usize == i,
        TypeKind::Array(inner) | TypeKind::Optional(inner) => names_param(inner, i),
        TypeKind::Class(_, args) | TypeKind::Enum(_, args) => {
            args.get().iter().any(|&arg| names_param(arg, i))
        }
        _ => false,
    }
}

/// A callee's type parameters as a call binds them: `found[i]` is parameter `i`'s type once
/// the receiver, an argument or the expected type fixes it.
pub(super) struct Bindings {
    found: Vec<Option<Type>>,
    /// The parameters the last `unify` or `constrain` bound: the only ones whose types need
    /// checking.
    fresh: Vec<usize>,
}

impl Bindings {
    /// `count` parameters, the first of them bound to `owner`, a receiver's arguments.
    fn new(count: usize, owner: &[Type]) -> Self {
        let unbound = iter::repeat_n(None, count - owner.len());
        Bindings {
            found: owner.iter().copied().map(Some).chain(unbound).collect(),
            fresh: Vec::new(),
        }
    }

    /// Binds the parameters `pattern` names, the callee's, so that it is `value`, every
    /// parameter of which is the caller's and a fixed type. A `T?` binds `T` to a value that is
    /// not optional, which the call then wraps. `false`, binding nothing, when they cannot.
    pub(super) fn unify(&mut self, pattern: Type, value: Type) -> bool {
        self.fresh.clear();
        let unified = self.bind(pattern, value);
        if !unified {
            for &i in &self.fresh {
                #[cfg(test)]
                VISITS.set(VISITS.get() + 1);
                self.found[i] = None;
            }
            self.fresh.clear();
        }
        unified
    }

    /// As `unify`, but what of `pattern` meets `value` stays bound when the rest does not.
    pub(super) fn constrain(&mut self, pattern: Type, value: Type) {
        self.fresh.clear();
        let _ = self.bind(pattern, value);
    }

    /// Binds each unbound parameter `pattern` names where it meets `value`, recording it in
    /// `fresh`; whether all of `pattern` meets it.
    fn bind(&mut self, pattern: Type, value: Type) -> bool {
        if !pattern.has_param() {
            return pattern == value;
        }
        match (pattern.kind(), value.kind()) {
            (TypeKind::Param(_, i), _) => {
                let i = i as usize;
                if let Some(bound) = self.found[i] {
                    return bound == value;
                }
                self.found[i] = Some(value);
                self.fresh.push(i);
                true
            }
            (TypeKind::Array(p), TypeKind::Array(v))
            | (TypeKind::Optional(p), TypeKind::Optional(v)) => self.bind(p, v),
            (TypeKind::Optional(p), _) => self.bind(p, value),
            (TypeKind::Class(a, p), TypeKind::Class(b, v)) if a == b => self.bind_all(p, v),
            (TypeKind::Enum(a, p), TypeKind::Enum(b, v)) if a == b => self.bind_all(p, v),
            _ => false,
        }
    }

    /// `bind` for each pair, every one of them tried.
    fn bind_all(&mut self, patterns: TypeArgs, values: TypeArgs) -> bool {
        iter::zip(patterns.get(), values.get()).fold(true, |all, (&p, &v)| self.bind(p, v) && all)
    }

    /// `ty`, a callee's, with its parameters' bindings in their place; `None` while it names
    /// one that is unbound.
    pub(super) fn known(&self, ty: Type) -> Option<Type> {
        if !ty.has_param() {
            return Some(ty);
        }
        Some(match ty.kind() {
            TypeKind::Param(_, i) => self.found[i as usize]?,
            TypeKind::Array(element) => Type::array(self.known(element)?),
            TypeKind::Optional(payload) => Type::optional(self.known(payload)?),
            TypeKind::Class(id, args) => Type::intern(TypeKind::Class(id, self.known_all(args)?)),
            TypeKind::Enum(id, args) => Type::intern(TypeKind::Enum(id, self.known_all(args)?)),
            _ => unreachable!("only a kind with a child can name a parameter"),
        })
    }

    fn known_all(&self, args: TypeArgs) -> Option<TypeArgs> {
        let list: Option<Vec<Type>> = args.get().iter().map(|&ty| self.known(ty)).collect();
        list.map(|list| TypeArgs::intern(&list))
    }

    /// Every parameter's binding; `Err` holds the first unbound one's index.
    pub(super) fn finish(self) -> Result<TypeArgs, usize> {
        let list = self
            .found
            .iter()
            .enumerate()
            .map(|(i, ty)| ty.ok_or(i))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TypeArgs::intern(&list))
    }
}

#[cfg(test)]
thread_local! {
    /// How many bindings inference has checked or undone on this thread.
    pub(super) static VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many type parameter names the search for reported ones has looked at on this
    /// thread.
    pub(super) static SCANNED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many entries of `body.ahead` lookups have inspected on this thread.
    pub(super) static INSPECTED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A generic callee as a call sees it: a function or method, a class's constructor, or an
/// enum's variant.
pub(super) struct GenericSig<'a> {
    /// The name the call writes: the function's, the method's, the class's, or the variant's
    /// enum's.
    pub(super) name: &'a ast::Ident,
    /// The variant after `name`, which the call builds.
    pub(super) variant: Option<&'a ast::Ident>,
    /// Every type parameter, a method's receiver's first.
    pub(super) declared: &'a [Symbol],
    /// The receiver's type arguments, which bind the first of `declared`.
    pub(super) owner: &'a [Type],
    pub(super) params: &'a [Option<Type>],
    pub(super) ret: Option<Type>,
}

/// How a call writes its callee: its name, the type arguments in `angles` (none written when
/// `written` is empty), and the type the call's context expects.
#[derive(Clone, Copy)]
pub(super) struct Site<'a> {
    pub(super) name: &'a ast::Ident,
    pub(super) written: &'a [ast::TypeExpr],
    pub(super) angles: Span,
    pub(super) expected: Option<Type>,
}

/// A generic call while its arguments bind its callee's parameters.
struct Inference {
    bindings: Bindings,
    /// Each argument once checked, in source order.
    slots: Vec<Option<Checked>>,
    /// The arguments of type `unit` or `never`, which bind only what no other argument does.
    last: Vec<(usize, Type)>,
    /// Whether a parameter was bound to a type that no parameter takes.
    failed: bool,
}

/// `checked`, an argument, made `Error` that its rule rejected when `mismatched`.
pub(super) fn mark((checked, mismatched): (Checked, bool)) -> Checked {
    if mismatched {
        Err(Poisoned {
            ty: None,
            mismatch: true,
        })
    } else {
        checked
    }
}

/// Whether `args` fit parameters `params`, each a resolved type and its label (`None` for a
/// positional one) in order: a call whose own check reports nothing of its shape.
fn fits(
    params: impl ExactSizeIterator<Item = (Option<Type>, Option<Symbol>)>,
    args: &[ast::Arg],
) -> bool {
    params.len() == args.len()
        && iter::zip(params, args)
            .all(|((ty, label), arg)| ty.is_some() && arg.label.as_ref().map(|l| l.name) == label)
}

/// Work that ranking an argument did ahead of its turn (see `needs_context`), kept in
/// `body.ahead` by the span it was done for until checking reaches it: a method's checked
/// receiver, or a block's scope and its statements before its value. It lives no longer than
/// the generic call that ranked it, nor than the diagnostics and locals it made.
pub(super) enum Ahead {
    Receiver(Checked),
    Block(FxHashMap<Symbol, Binding>, BlockSoFar),
}

/// Whether `checked`, an argument, was rejected by its parameter's type (see `mark`).
pub(super) fn mismatched(checked: &Checked) -> bool {
    matches!(checked, Err(Poisoned { mismatch: true, .. }))
}

impl Checker<'_> {
    /// The arguments of a call of the generic `sig`, which the caller has matched to its
    /// parameters by count and label, checked in four passes that bind its type parameters
    /// (`written` gives every one the receiver does not): (1) those that are neither
    /// literal-like nor need context, each against its parameter's type once that names no
    /// unbound parameter and on its own before; (2) `expected` against the return type, an
    /// optional by its payload, which a `T?` takes as it takes the optional; (3) the
    /// literal-like arguments, as in (1); (4) those that need context, against their
    /// parameter's type. An argument of type `unit` or `never` binds only what none of the
    /// others does. Returns the arguments in source order, the type arguments, and the call's
    /// type; `None`, reported, when a parameter is left unbound or bound to a type no
    /// parameter takes, and then an argument that needs context and has none is not checked,
    /// unless a call in it does not fit its declaration, which it then reports. What ranking
    /// the arguments checked ahead (see `Ahead`) and left unused is dropped.
    pub(super) fn generic_call(
        &mut self,
        span: Span,
        sig: &GenericSig<'_>,
        written: (&[ast::TypeExpr], Span),
        args: &[ast::Arg],
        expected: Option<Type>,
    ) -> Option<(Vec<Checked>, TypeArgs, Option<Type>)> {
        let mark = self.body.ahead.len();
        let call = self.infer_call(span, sig, written, args, expected);
        // A key is kept again only once its last entry is used, so an earlier entry of a key
        // truncated here, if any, is used, and dropping the key loses nothing.
        for (key, _) in self.body.ahead.drain(mark..) {
            self.body.ahead_at.remove(&key);
        }
        call
    }

    /// `generic_call`, before what ranking kept is dropped.
    fn infer_call(
        &mut self,
        span: Span,
        sig: &GenericSig<'_>,
        (written, angles): (&[ast::TypeExpr], Span),
        args: &[ast::Arg],
        expected: Option<Type>,
    ) -> Option<(Vec<Checked>, TypeArgs, Option<Type>)> {
        let owned = sig.owner.len();
        let mut call = Inference {
            bindings: Bindings::new(sig.declared.len(), sig.owner),
            slots: args.iter().map(|_| None).collect(),
            last: Vec::new(),
            failed: false,
        };
        if !written.is_empty() {
            let Some(list) = self.type_args(sig.name, &sig.declared[owned..], written, angles)
            else {
                self.stray(args);
                return None;
            };
            // `type_args` rejects each type that `report_fresh` would.
            for (i, &ty) in list.get().iter().enumerate() {
                call.bindings.found[owned + i] = Some(ty);
            }
        }
        // Whether every call in an argument fits its declaration.
        let mut fit = vec![true; args.len()];
        let ranks: Vec<u8> = iter::zip(args, &mut fit)
            .map(|(arg, fit)| match &arg.value {
                e if self.waits(e, fit) => 2,
                e if self.literal_like(e) => 1,
                _ => 0,
            })
            .collect();
        for (i, arg) in args.iter().enumerate().filter(|&(i, _)| ranks[i] == 0) {
            self.infer_arg(&mut call, i, &arg.value, sig.params[i]);
        }
        if let (Some(ret), Some(want)) = (sig.ret, expected.filter(|&ty| ty != UNKNOWN)) {
            let want = match want.kind() {
                TypeKind::Optional(payload) => payload,
                _ => want,
            };
            // What of the return type cannot meet it is reported where the call's value is used.
            call.bindings.constrain(ret, want);
            self.report_fresh(&mut call, span);
        }
        for (i, arg) in args.iter().enumerate().filter(|&(i, _)| ranks[i] == 1) {
            self.infer_arg(&mut call, i, &arg.value, sig.params[i]);
        }
        for (i, arg) in args.iter().enumerate().filter(|&(i, _)| ranks[i] == 2) {
            // An argument whose type would name an unbound parameter waits, unchecked, unless
            // it holds a call that reports its own mistake, on its own.
            if !fit[i] || sig.params[i].is_none_or(|param| call.bindings.known(param).is_some()) {
                self.infer_arg(&mut call, i, &arg.value, sig.params[i]);
            }
        }
        for (i, ty) in std::mem::take(&mut call.last) {
            let (e, param) = (&args[i].value, sig.params[i]);
            if let Some(param) = param
                && call.bindings.known(param).is_none()
                && self.bind_param(&mut call, param, ty, e.span)
            {
                continue;
            }
            self.no_value(e.span, ty);
            call.slots[i] = Some(Err(Poisoned::error()));
        }
        // A parameter whose name `type_params` reported, a second `T` or a built-in name, may
        // be one no type names: it poisons the call rather than go uninferred. One pass over
        // the names finds them, and only a call that leaves one unbound makes it.
        let mut broken = false;
        if call.bindings.found.contains(&None) {
            let mut seen = FxHashSet::default();
            for (i, &name) in sig.declared.iter().enumerate() {
                #[cfg(test)]
                SCANNED.set(SCANNED.get() + 1);
                let repeated = !seen.insert(name);
                if call.bindings.found[i].is_none()
                    && (repeated || built_in_type(name.as_str()).is_some())
                {
                    call.bindings.found[i] = Some(UNKNOWN);
                    broken = true;
                }
            }
        }
        let list = match call.bindings.finish() {
            Ok(list) if !call.failed && !broken => list,
            Ok(_) => return None,
            Err(i) => {
                // An argument that failed, or a parameter's or the return type that did, may be
                // what would have bound it, and an `UNKNOWN` context is one that failed.
                let errored = call.slots.iter().flatten().any(|c| type_of(c).is_none())
                    || sig.params.contains(&None)
                    || sig.ret.is_none();
                if !errored && !call.failed && expected != Some(UNKNOWN) {
                    self.cannot_infer(sig, i);
                }
                return None;
            }
        };
        let checked = call
            .slots
            .into_iter()
            .map(|slot| {
                slot.expect("every argument should be checked once each parameter is bound")
            })
            .collect();
        Some((checked, list, sig.ret.map(|ret| ret.subst(list.get()))))
    }

    /// Checks `e`, the argument at `i` for `param`: against `param` once the call binds what it
    /// names, and otherwise on its own, which binds what its type fixes.
    fn infer_arg(&mut self, call: &mut Inference, i: usize, e: &ast::Expr, param: Option<Type>) {
        let Some(pattern) = param else {
            call.slots[i] = Some(mark(self.operand(e, None)));
            return;
        };
        let unbound = call.bindings.known(pattern).is_none();
        let checked = self.infer_value(call, e, pattern);
        if unbound && let Some(ty @ (Type::UNIT | Type::NEVER)) = type_of(&checked) {
            call.last.push((i, ty));
        }
        call.slots[i] = Some(checked);
    }

    /// Checks `e` against `pattern`, a parameter's type or an element's: against what the call
    /// binds of it, or, while it names an unbound parameter, on its own, which binds what its
    /// type fixes. A non-empty array literal for an array of optionals (see `optional_element`)
    /// is the exception: its elements bind the parameter one by one (see `inferred_array`).
    /// A value of type `unit` or `never` binds nothing, for `infer_arg` to defer.
    fn infer_value(&mut self, call: &mut Inference, e: &ast::Expr, pattern: Type) -> Checked {
        if let Some(want) = call.bindings.known(pattern) {
            return mark(self.operand(e, Some(want)));
        }
        if let ast::ExprKind::ArrayLit(elements) = &e.kind
            && !elements.is_empty()
            && let TypeKind::Array(element) = pattern.kind()
            && optional_element(element)
        {
            return self.inferred_array(call, e.span, elements, element);
        }
        let checked = self.expr(e, None);
        match type_of(&checked) {
            Some(Type::UNIT | Type::NEVER) | None => checked,
            Some(ty) if !self.bind_param(call, pattern, ty, e.span) => {
                let d = self.mismatch(e.span, Some(e), pattern, ty);
                self.diagnostics.push(d);
                mark((checked, true))
            }
            Some(_) => {
                let want = call.bindings.known(pattern);
                debug_assert!(
                    want.is_some(),
                    "a unified pattern should name only bound parameters"
                );
                mark(self.against(e, checked, want, None))
            }
        }
    }

    /// The array literal `elements` for `[element]`, where `element` names an unbound
    /// parameter. The elements are checked in the three waves of `rank`, each by `infer_value`,
    /// so a later `None` or integer literal meets the parameter already bound by an earlier
    /// element, and so wraps. A `None` or `[]` with nothing bound is reported as `joined`
    /// reports it: the first alone, unless an element failed already or bound nothing.
    fn inferred_array(
        &mut self,
        call: &mut Inference,
        span: Span,
        elements: &[ast::Expr],
        element: Type,
    ) -> Checked {
        let mut slots: Vec<Option<Checked>> = elements.iter().map(|_| None).collect();
        let mut quiet = false;
        for wave in 0..3 {
            for (i, e) in elements.iter().enumerate().filter(|(_, e)| rank(e) == wave) {
                let checked = if wave < 2 || call.bindings.known(element).is_some() {
                    self.infer_value(call, e, element)
                } else {
                    quiet = quiet
                        || (slots.iter().flatten())
                            .any(|c| matches!(type_of(c), None | Some(Type::UNIT | Type::NEVER)));
                    // Only its own diagnostics matter.
                    let _ = self.value(e, quiet.then_some(UNKNOWN));
                    quiet = true;
                    Err(Poisoned::error())
                };
                slots[i] = Some(checked);
            }
        }
        let Some(bound) = call.bindings.known(element) else {
            // Only elements of type `unit` or `never` can have left it unbound, and so bind it.
            let unit = iter::zip(&slots, elements).find_map(|(slot, e)| {
                match slot.as_ref().and_then(type_of) {
                    Some(ty @ (Type::UNIT | Type::NEVER)) => Some((ty, e.span)),
                    _ => None,
                }
            });
            if let Some((ty, at)) = unit {
                self.bind_param(call, element, ty, at);
            }
            return Err(Poisoned::error());
        };
        let mismatched = slots
            .iter()
            .flatten()
            .any(|c| matches!(c, Err(Poisoned { mismatch: true, .. })));
        let checked = slots
            .into_iter()
            .map(|slot| slot.expect("the three waves should check every element, one rank each"))
            .collect();
        self.array(span, bound, checked, mismatched)
    }

    /// `Bindings::unify`, reporting at `at` each parameter it binds to a type that no type
    /// parameter takes.
    fn bind_param(&mut self, call: &mut Inference, pattern: Type, value: Type, at: Span) -> bool {
        let unified = call.bindings.unify(pattern, value);
        self.report_fresh(call, at);
        unified
    }

    /// Reports at `at` each parameter the last binding bound to a type that no type parameter
    /// takes.
    fn report_fresh(&mut self, call: &mut Inference, at: Span) {
        for &i in &call.bindings.fresh {
            #[cfg(test)]
            VISITS.set(VISITS.get() + 1);
            let ty = call.bindings.found[i].expect("a fresh parameter should be bound");
            let optional = matches!(ty.kind(), TypeKind::Optional(_));
            if optional || matches!(ty, Type::UNIT | Type::NEVER) {
                self.report(DiagnosticKind::TypeArgument {
                    ty: self.type_name(ty),
                    at,
                    optional,
                });
                call.failed = true;
            }
        }
    }

    /// Reports the call of `sig` that binds nothing to its parameter at `param`.
    fn cannot_infer(&mut self, sig: &GenericSig<'_>, param: usize) {
        let name = sig.name;
        let dots = vec!["..."; sig.declared.len() - sig.owner.len()].join(", ");
        let (callee, example, at) = match sig.variant {
            Some(variant) => {
                let payload = if sig.params.is_empty() { "" } else { "(...)" };
                (
                    format!("{}.{}", name.name, variant.name),
                    format!("{}<{dots}>.{}{payload}", name.name, variant.name),
                    name.span.to(variant.span),
                )
            }
            None => (
                name.name.to_string(),
                format!("{}<{dots}>(...)", name.name),
                name.span,
            ),
        };
        self.report(DiagnosticKind::CannotInferParam {
            param: sig.declared[param].to_string(),
            callee,
            example,
            at,
            in_result: sig.ret.is_some_and(|ret| names_param(ret, param)),
        });
    }

    /// Whether `e`'s type cannot come from itself: a bare `[]` or `None`; a payload-free
    /// variant of a generic enum; an array literal whose elements all need context; an `if` or
    /// a block whose every value branch does; or a call of a generic function or method, or a
    /// construction of a generic class or variant, its type arguments not written, whose
    /// arguments fit its declaration and all need context. A name resolves as the checker
    /// resolves it: a call's callee by its declaration, whatever local takes it. A method's
    /// receiver is checked here, ahead of its call, and kept for it (see `receiver_ahead`).
    // shortcut: a nested generic call scans its arguments again, quadratic in the depth of the
    // nesting; remember the answer by span if deep nests appear.
    pub(super) fn needs_context(&mut self, e: &ast::Expr) -> bool {
        self.waits(e, &mut true)
    }

    /// `needs_context`, where `fit` is cleared when a call that waits does not fit its
    /// declaration: count, labels, and every parameter's and the return type resolved.
    fn waits(&mut self, e: &ast::Expr, fit: &mut bool) -> bool {
        use ast::ExprKind as K;
        let (generic, shaped, args) = match &e.kind {
            K::NoneLit => return true,
            K::ArrayLit(elements) => return elements.iter().all(|e| self.waits(e, fit)),
            K::Field {
                base,
                name,
                safe: false,
            } => {
                return self
                    .generic_variant(base, name)
                    .is_some_and(|v| v.fields.is_empty());
            }
            K::MethodCall {
                receiver,
                method,
                types,
                args,
                safe,
                ..
            } if types.is_empty() => {
                let variant = self.generic_variant(receiver, method).filter(|_| !safe);
                let (generic, shaped) = if let Some(v) = variant {
                    // Mixed labels are reported at the declaration.
                    let shaped = !v.fields.is_empty()
                        && (v.labels.iter().all(Option::is_some)
                            || v.labels.iter().all(Option::is_none))
                        && fits(v.fields.iter().copied().zip(v.labels.iter().copied()), args);
                    (true, shaped)
                } else {
                    self.generic_method(receiver, method, args, *safe)
                };
                (generic, shaped, args)
            }
            K::Call {
                callee,
                types,
                args,
                ..
            } if types.is_empty() => {
                let (generic, shaped) = self.generic_callee(callee.name, args);
                (generic, shaped, args)
            }
            K::If {
                then_block,
                else_block: Some(else_block),
                ..
            } => return self.branches_wait(&[then_block, else_block], fit),
            K::Block(block) => return self.branches_wait(&[block], fit),
            _ => return false,
        };
        let waits = generic && args.iter().all(|arg| self.waits(&arg.value, fit));
        *fit &= !waits || shaped;
        waits
    }

    /// Whether one of `blocks` ends in a value, and each that does needs context; one that
    /// ends in an exit has none.
    fn branches_wait(&mut self, blocks: &[&ast::Block], fit: &mut bool) -> bool {
        let mut any = false;
        for block in blocks {
            match block.stmts.last().map(|stmt| &stmt.kind) {
                Some(StmtKind::Return(_) | StmtKind::Break { .. } | StmtKind::Continue { .. }) => {}
                Some(StmtKind::Expr(tail)) if self.tail_waits(block, tail, fit) => any = true,
                _ => return false,
            }
        }
        any
    }

    /// Whether `tail`, the value of `block`, needs context, in the block's scope: the
    /// statements before it are checked here, once, and kept with that scope for `block`.
    fn tail_waits(&mut self, block: &ast::Block, tail: &ast::Expr, fit: &mut bool) -> bool {
        // Ranked before: its scope comes back for the value, and goes back after.
        let (index, so_far) =
            if let Some((index, Ahead::Block(scope, so_far))) = self.kept(block.span, true) {
                self.body.scopes.push(scope);
                (index, so_far)
            } else {
                self.body.scopes.push(FxHashMap::default());
                let mut so_far = BlockSoFar::default();
                // A statement is not part of an assignment target's chain.
                let target = std::mem::take(&mut self.body.in_target);
                self.block_statements(block, None, &mut so_far, block.stmts.len() - 1);
                self.body.in_target = target;
                // Its place, held before the value's ranking keeps work of its own.
                (self.keep((block.span, true), None), so_far)
            };
        let waits = self.waits(tail, fit);
        let scope = self
            .body
            .scopes
            .pop()
            .expect("the block's scope should be open while its value is ranked");
        self.body.ahead[index].1 = Some(Ahead::Block(scope, so_far));
        waits
    }

    /// Takes the work kept for `span` (see `Ahead`), a block's if `block`, else a receiver's,
    /// with its place in `body.ahead`.
    pub(super) fn kept(&mut self, span: Span, block: bool) -> Option<(usize, Ahead)> {
        let index = *self.body.ahead_at.get(&(span, block))?;
        #[cfg(test)]
        INSPECTED.set(INSPECTED.get() + 1);
        self.body.ahead[index].1.take().map(|kept| (index, kept))
    }

    /// Keeps `work` in `body.ahead` for `key`, a span and whether it is a block's, and returns
    /// its place there.
    fn keep(&mut self, key: (Span, bool), work: Option<Ahead>) -> usize {
        let index = self.body.ahead.len();
        self.body.ahead.push((key, work));
        self.body.ahead_at.insert(key, index);
        index
    }

    /// The variant `base.member` of a generic enum that `base` names without type arguments.
    pub(super) fn generic_variant(
        &self,
        base: &ast::Expr,
        member: &ast::Ident,
    ) -> Option<&VariantInfo> {
        let ast::ExprKind::Name(name) = base.kind else {
            return None;
        };
        let Some(&Declared::Known(id)) = self.enum_ids.get(&name) else {
            return None;
        };
        let info = &self.enums[id.0];
        let &variant = info.index.get(&member.name)?;
        (!info.type_params.is_empty()).then(|| &info.variants[variant])
    }

    /// Whether `receiver.method(args)`, or `receiver?.method(args)` with `safe`, calls a method
    /// with type parameters of its own, and whether `args` fit its declaration, whose return
    /// type resolved.
    fn generic_method(
        &mut self,
        receiver: &ast::Expr,
        method: &ast::Ident,
        args: &[ast::Arg],
        safe: bool,
    ) -> (bool, bool) {
        let mut ty = self.receiver_ahead(receiver);
        if safe {
            // `?.` calls the method on the payload; on a value that is not optional, which is
            // reported, on the value.
            ty = ty.map(|ty| match ty.kind() {
                TypeKind::Optional(payload) => payload,
                _ => ty,
            });
        }
        let (methods, owner) = match ty.map(Type::kind) {
            Some(TypeKind::Class(id, _)) => {
                let info = &self.classes[id.0];
                (&info.methods, info.type_params.len())
            }
            Some(TypeKind::Enum(id, _)) => {
                let info = &self.enums[id.0];
                (&info.methods, info.type_params.len())
            }
            _ => return (false, false),
        };
        let Some(&Declared::Known(func)) = methods.get(&method.name) else {
            return (false, false);
        };
        let sig = &self.signatures[func.0];
        (
            sig.type_params.len() > owner,
            sig.ret.is_some() && fits(sig.params.iter().map(|&ty| (ty, None)), args),
        )
    }

    /// The type of `receiver`, a method's, checked ahead of the call and kept for `member`.
    /// `None` for a name no local takes: a path, which `type_path` checks, or an unknown name.
    fn receiver_ahead(&mut self, receiver: &ast::Expr) -> Option<Type> {
        match receiver.kind {
            ast::ExprKind::Name(name) if self.lookup(name).is_none() => return None,
            ast::ExprKind::Applied { .. } => return None,
            _ => {}
        }
        let kept = self
            .body
            .ahead_at
            .get(&(receiver.span, false))
            .and_then(|&index| {
                #[cfg(test)]
                INSPECTED.set(INSPECTED.get() + 1);
                self.body.ahead[index].1.as_ref()
            });
        if let Some(Ahead::Receiver(checked)) = kept {
            return type_of(checked);
        }
        // An argument is not part of an assignment target's chain.
        let target = std::mem::take(&mut self.body.in_target);
        let checked = self.value(receiver, None);
        self.body.in_target = target;
        let ty = type_of(&checked);
        self.keep((receiver.span, false), Some(Ahead::Receiver(checked)));
        ty
    }

    /// Whether a call of `name` with `args` calls a generic function or constructs a generic
    /// class, and whether `args` fit its declaration, a function's return type resolved.
    fn generic_callee(&self, name: Symbol, args: &[ast::Arg]) -> (bool, bool) {
        match (self.class_ids.get(&name), self.function_ids.get(&name)) {
            (Some(&Declared::Known(class)), _) => {
                let info = &self.classes[class.0];
                (
                    !info.type_params.is_empty(),
                    fits(info.fields.iter().map(|f| (f.ty, Some(f.name.name))), args),
                )
            }
            (None, Some(&Declared::Known(func))) => {
                let sig = &self.signatures[func.0];
                (
                    !sig.type_params.is_empty(),
                    sig.ret.is_some() && fits(sig.params.iter().map(|&ty| (ty, None)), args),
                )
            }
            _ => (false, false),
        }
    }

    /// Whether `e`'s type comes from its literals' defaults: an integer literal or arithmetic
    /// over them, ranked as an array literal's elements are, or a non-empty array literal whose
    /// elements are each literal-like or need context, one at least literal-like.
    pub(super) fn literal_like(&mut self, e: &ast::Expr) -> bool {
        match &e.kind {
            ast::ExprKind::ArrayLit(elements) => {
                elements.iter().any(|e| self.literal_like(e))
                    && elements
                        .iter()
                        .all(|e| self.literal_like(e) || self.needs_context(e))
            }
            _ => int_literal(e),
        }
    }

    /// The type parameters `owner`, a method's class's or enum's, then `own`, a declaration's.
    /// Reports each of `own` that takes a built-in type's name, or the name of one before it;
    /// `owner`'s are reported where they are declared. Every name keeps its place, so that a
    /// `Param`'s index is its place.
    pub(super) fn type_params(&mut self, owner: &[ast::Ident], own: &[ast::Ident]) -> Vec<Symbol> {
        let mut first = FxHashMap::default();
        for param in owner {
            first
                .entry(param.name)
                .or_insert((param.span, Definition::Type));
        }
        for param in own {
            if !self.built_in(param) {
                self.first_definition(&mut first, Definition::Type, None, param);
            }
        }
        owner.iter().chain(own).map(|param| param.name).collect()
    }

    /// The type arguments `written` in `angles` after `name`, whose declaration's type
    /// parameters are `declared`. `None`, reported, when they do not fit `declared`, or one is
    /// `Error` or cannot be a type argument. A list after a name that takes none is reported,
    /// and the name stands without it.
    pub(super) fn type_args(
        &mut self,
        name: &ast::Ident,
        declared: &[Symbol],
        written: &[ast::TypeExpr],
        angles: Span,
    ) -> Option<TypeArgs> {
        let form = match (declared.len(), written.len()) {
            (0, 0) => return Some(TypeArgs::NONE),
            (0, _) => {
                self.not_generic(name.name, angles);
                return Some(TypeArgs::NONE);
            }
            (_, 0) => ArgumentsForm::Missing {
                params: declared.iter().map(Symbol::to_string).collect(),
            },
            (want, found) if want != found => ArgumentsForm::Count { want, found },
            _ => {
                // Each argument is checked, so this collects into a `Vec` before it gives up.
                let args: Vec<_> = written.iter().map(|arg| self.type_argument(arg)).collect();
                return args
                    .into_iter()
                    .collect::<Option<Vec<_>>>()
                    .map(|args| TypeArgs::intern(&args));
            }
        };
        let at = match form {
            ArgumentsForm::Missing { .. } => name.span,
            _ => angles,
        };
        self.report(DiagnosticKind::TypeArguments {
            name: name.name.to_string(),
            at,
            form,
        });
        None
    }

    /// Reports the type arguments in `angles` after `name`, which takes none: the fix removes
    /// them.
    pub(super) fn not_generic(&mut self, name: Symbol, angles: Span) {
        let safe = !self.comment_in(&[angles.range()]);
        self.report(DiagnosticKind::TypeArguments {
            name: name.to_string(),
            at: angles,
            form: ArgumentsForm::NotGeneric { safe },
        });
    }

    /// The type of the type argument `arg`: `None`, reported, for `unit`, `never` or an
    /// optional, which no type parameter takes.
    fn type_argument(&mut self, arg: &ast::TypeExpr) -> Option<Type> {
        if let ast::TypeKind::Name(name) = arg.kind
            && matches!(name.as_str(), "unit" | "never")
            && self.body.type_param(name).is_none()
            && !self.class_ids.contains_key(&name)
            && !self.enum_ids.contains_key(&name)
        {
            self.report(DiagnosticKind::TypeArgument {
                ty: arg.to_string(),
                at: arg.span,
                optional: false,
            });
            return None;
        }
        let ty = self.resolve(arg)?;
        if arg.optional {
            self.report(DiagnosticKind::TypeArgument {
                ty: self.type_name(ty),
                at: arg.span,
                optional: true,
            });
            return None;
        }
        Some(ty)
    }

    /// Reports `main`, which has type parameters. The fix removes them, unless a comment is
    /// among them or a name in the rest of `main`, outside its strings and comments, is one.
    pub(super) fn main_type_params(&mut self, main: &ast::Function) {
        let angles = main.angles;
        let rest = &self.source.text[angles.end()..main.span.end()];
        // What does not lex alone is held to name one.
        let named = lex(rest).map_or(true, |lexed| {
            lexed.tokens.iter().any(|token| {
                matches!(token.kind, TokenKind::Ident(name)
                    if main.type_params.iter().any(|p| p.name == name))
            })
        });
        let remove = (!named && !self.comment_in(&[angles.range()])).then_some(angles);
        self.report(DiagnosticKind::MainSignature {
            at: angles,
            form: MainForm::TypeParams { remove },
        });
    }

    /// Reports `op` on the operand at `at`, of type `ty`, which names a type parameter: no
    /// operator takes one.
    pub(super) fn param_lacks(&mut self, at: Span, ty: Type, op: impl fmt::Display) {
        self.report(DiagnosticKind::ParamLacks {
            ty: self.type_name(ty),
            op: op.to_string(),
            at,
            contains: false,
        });
    }

    /// Reports `name<...>`, a value with type arguments in `angles`: the `<` and `>` were
    /// comparisons. `name` is the value as written, a name or a field, and `first` the first
    /// type argument, which the help puts in parentheses with `name`, when a `(` follows the list
    /// and `first` could be a value: a name that is not a built-in type, with no `?`.
    pub(super) fn misread(&mut self, name: Span, first: Option<&ast::TypeExpr>) -> Poisoned {
        let first = first.filter(|first| self.could_be_value(first));
        let compare = first.map(|first| self.source.text[name.start()..first.span.end()].into());
        self.misread_as(name, compare)
    }

    /// Whether the type argument `arg` could be a value: a local, or a name that names no type.
    pub(super) fn could_be_value(&self, arg: &ast::TypeExpr) -> bool {
        let ast::TypeKind::Name(n) = arg.kind else {
            return false;
        };
        !arg.optional && self.body.scopes.iter().any(|scope| scope.contains_key(&n))
            || self.names_no_type(arg)
    }

    /// Whether the type argument `arg` is a name, with no `?`, that is no built-in type, class,
    /// enum or type parameter.
    pub(super) fn names_no_type(&self, arg: &ast::TypeExpr) -> bool {
        let ast::TypeKind::Name(n) = arg.kind else {
            return false;
        };
        !arg.optional
            && built_in_type(n.as_str()).is_none()
            && !self.class_ids.contains_key(&n)
            && !self.enum_ids.contains_key(&n)
            && self.body.type_param(n).is_none()
    }

    /// `misread` with the comparison `compare` its help writes, if any.
    pub(super) fn misread_as(&mut self, name: Span, compare: Option<String>) -> Poisoned {
        self.body.misread.insert(name.start());
        self.report(DiagnosticKind::TypeArguments {
            name: self.source.text[name.start()..name.end()].to_string(),
            at: name,
            form: ArgumentsForm::Value { compare },
        });
        Poisoned::error()
    }

    /// Marks `span`, an `a<b, c>(d)` whose type arguments `types` took a comma, as misread when
    /// it does not check and the first and last of `types` name no type: the comma may be its
    /// argument list's, whose count is then not reported (see `misread_in`).
    pub(super) fn misread_unless_checked(
        &mut self,
        span: Span,
        types: &[ast::TypeExpr],
        checked: &Checked,
    ) {
        // Only a list whose first and last argument name no type could join `a <` and `> (d)`.
        if let [first, .., last] = types
            && checked.is_err()
            && self.names_no_type(first)
            && self.names_no_type(last)
        {
            self.body.misread.insert(span.start());
        }
    }

    /// The type arguments `types`, in `angles`, written after `name` in an expression, which
    /// are reported unless a generic declaration takes them. `Some` when `name` is a value,
    /// whose `<` and `>` were comparisons: the expression is poisoned. `call` when a `(`
    /// follows the list, rather than a `.`.
    pub(super) fn named_args(
        &mut self,
        name: &ast::Ident,
        types: &[ast::TypeExpr],
        angles: Span,
        call: bool,
    ) -> Option<Poisoned> {
        let first = types.first()?;
        let n = name.name;
        let params = match (
            self.class_ids.get(&n),
            self.function_ids.get(&n),
            self.enum_ids.get(&n),
        ) {
            (Some(&Declared::Known(id)), ..) => Some(self.classes[id.0].type_params.len()),
            (_, Some(&Declared::Known(id)), _) => Some(self.signatures[id.0].type_params.len()),
            (.., Some(&Declared::Known(id))) => Some(self.enums[id.0].type_params.len()),
            _ => None,
        };
        match params {
            Some(0) => self.not_generic(n, angles),
            None if PRELUDE.contains(&n.as_str()) || built_in_type(n.as_str()).is_some() => {
                self.not_generic(n, angles);
            }
            None if self.lookup(n).is_some()
                || matches!(self.constants.get(&n), Some(Declared::Known(_))) =>
            {
                return Some(self.misread(name.span, call.then_some(first)));
            }
            // A generic's call or use checks them; a broken or unknown name's use reports it.
            _ => {}
        }
        None
    }

    /// Whether `receiver` before `.method` is a path, not a value: an enum, or `math` or a
    /// built-in type that no local takes.
    pub(super) fn path_method(&mut self, receiver: &ast::Expr, method: &ast::Ident) -> bool {
        let name = match &receiver.kind {
            ast::ExprKind::Name(name) => *name,
            ast::ExprKind::Applied { name, .. } => name.name,
            _ => return false,
        };
        if let Some(&Declared::Known(id)) = self.enum_ids.get(&name) {
            // A generic method's list waits for the member's report, which says it is a method.
            let info = &self.enums[id.0];
            return match info.methods.get(&method.name) {
                Some(&Declared::Known(func)) => {
                    self.signatures[func.0].type_params.len() == info.type_params.len()
                }
                _ => true,
            };
        }
        (built_in_type(name.as_str()).is_some() || matches!(name.as_str(), "math" | "Math"))
            && self.lookup(name).is_none()
    }

    /// Reports the type arguments in `angles` of `receiver.method<first, ...>`, `receiver`
    /// checked from `receiver_expr`: a method that takes none, or a field, a value whose `<` and
    /// `>` were comparisons, which is `Some` and poisons the expression. A member the receiver
    /// does not have is reported as that alone, and a built-in receiver's list is left to the
    /// caller.
    pub(super) fn method_args(
        &mut self,
        receiver: &Checked,
        receiver_expr: &ast::Expr,
        method: &ast::Ident,
        first: &ast::TypeExpr,
        angles: Span,
    ) -> Option<Poisoned> {
        let (methods, owner_params, field) = match type_of(receiver)?.kind() {
            TypeKind::Class(id, _) => {
                let info = &self.classes[id.0];
                let field = info.fields.iter().any(|f| f.name.name == method.name);
                (&info.methods, info.type_params.len(), field)
            }
            TypeKind::Enum(id, _) => {
                let info = &self.enums[id.0];
                (&info.methods, info.type_params.len(), false)
            }
            // A built-in's list is reported once its method is found.
            _ => return None,
        };
        match methods.get(&method.name).copied() {
            // A method with parameters of its own takes the list at its call.
            Some(Declared::Known(func))
                if self.signatures[func.0].type_params.len() > owner_params => {}
            Some(Declared::Known(_)) => self.not_generic(method.name, angles),
            None if field => {
                return Some(self.misread(receiver_expr.span.to(method.span), Some(first)));
            }
            _ => {}
        }
        None
    }

    /// Reports the type arguments in `angles` after `variant`, a variant of the generic enum
    /// `base` names, which takes its enum's: the help writes them on the enum.
    pub(super) fn variant_args(&mut self, base: &ast::Expr, variant: &ast::Ident, angles: Span) {
        let text = self.source.text;
        let instead = format!(
            "{}{}.{}",
            &text[base.span.range()],
            &text[angles.range()],
            variant.name
        );
        self.report(DiagnosticKind::TypeArguments {
            name: text[base.span.start()..variant.span.end()].to_string(),
            at: angles,
            form: ArgumentsForm::Variant { instead },
        });
    }
}
