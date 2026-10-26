//! Generic declarations: type parameters, the type arguments written for them or inferred at
//! a call, and what a value of a type parameter may not do.

use std::{fmt, iter};

use rustc_hash::{FxHashMap, FxHashSet};

use super::enums::VariantInfo;
use super::expr::{bare_none, int_literal, option_written, rank};
use super::pattern::{LetPattern, LetValue, arm_key, matchable};
use super::prelude::PRELUDE;
use super::stmt::BlockSoFar;
use super::{
    Binding, Checked, Checker, Declared, Definition, DiagnosticKind, Poisoned, UNKNOWN,
    built_in_type, optional_of, payload, type_of,
};
use crate::ast::{self, StmtKind};
use crate::diagnostic::{ArgumentsForm, MainForm, Unwrap};
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

/// Marks in `named` each type parameter that `ty` names, by its place; `named` has a place for
/// each type parameter in scope. Each type of `ty` is visited once.
fn mark_params(ty: Type, named: &mut [bool]) {
    #[cfg(test)]
    MARKED.set(MARKED.get() + 1);
    // A type without a parameter names none: its subtree is not walked again at each call.
    if !ty.has_param() {
        return;
    }
    match ty.kind() {
        TypeKind::Param(_, j) => named[j as usize] = true,
        TypeKind::Array(inner) | TypeKind::Optional(inner) => mark_params(inner, named),
        TypeKind::Class(_, args) | TypeKind::Enum(_, args) => {
            for &arg in args.get() {
                mark_params(arg, named);
            }
        }
        _ => unreachable!("only a kind with a child can name a parameter"),
    }
}

/// Which of the first `len` type parameters `types` name. A type that did not resolve names
/// nothing.
fn named_params(types: impl Iterator<Item = Option<Type>>, len: usize) -> Vec<bool> {
    let mut named = vec![false; len];
    for ty in types.flatten() {
        mark_params(ty, &mut named);
    }
    named
}

/// Whether the type parameter `name` is broken, as `type_params` reports it: it takes the name
/// of one in `seen`, which then holds it, or of a built-in type. A call poisons such a parameter
/// rather than leave it uninferred, and no call waits for it.
fn broken_param(seen: &mut FxHashSet<Symbol>, name: Symbol) -> bool {
    !seen.insert(name) || built_in_type(name.as_str()).is_some()
}

/// Which of `names`, a declaration's type parameters, are broken (see `broken_param`).
fn broken_params(names: &[Symbol]) -> Vec<bool> {
    let mut seen = FxHashSet::default();
    names
        .iter()
        .map(|&name| broken_param(&mut seen, name))
        .collect()
}

/// Whether one of the type parameters in `names` from `owner` on has no type in `params` that
/// names it, so that no argument can bind it. For a construction, whose built type names all
/// of them: the call needs its context, whatever its arguments are. A broken parameter is
/// not open.
fn leaves_open(params: impl Iterator<Item = Option<Type>>, names: &[Symbol], owner: usize) -> bool {
    let bound = named_params(params, names.len());
    let broken = broken_params(names);
    (owner..names.len()).any(|i| !bound[i] && !broken[i])
}

/// `leaves_open` for a call of a function or method, which returns `ret`: the context binds only
/// what the type of the call names, so the call is open if `ret` names a parameter that no
/// parameter type does. It is not open, whatever else it leaves, if one parameter is named by
/// no type at all: nothing can bind that one, and the call reports it.
fn leaves_open_in(
    params: impl Iterator<Item = Option<Type>>,
    ret: Type,
    names: &[Symbol],
    owner: usize,
) -> bool {
    let bound = named_params(params, names.len());
    let built = named_params(iter::once(Some(ret)), names.len());
    let broken = broken_params(names);
    let mut open = false;
    for i in (owner..names.len()).filter(|&i| !bound[i] && !broken[i]) {
        if !built[i] {
            return false;
        }
        open = true;
    }
    open
}

/// A call of a generic callee as ranking sees it (see `Checker::waits`).
#[derive(Default)]
struct CallShape {
    /// Whether the callee has type parameters of its own to infer.
    generic: bool,
    /// Whether the arguments fit the callee's declaration, whose return type resolved.
    shaped: bool,
    /// Whether no argument can bind one of its type parameters, and the type of the call names
    /// it, so only the call's context can bind it.
    open: bool,
}

/// What `Bindings::resolve` puts for a parameter that is unbound.
#[derive(Clone, Copy)]
enum Unbound {
    /// Nothing: the walk gives `None`.
    Fail,
    /// The parameter itself, by the callee's name.
    Keep,
}

/// A callee's type parameters as a call binds them: `found[i]` is parameter `i`'s type once
/// the receiver, an argument or the expected type fixes it.
pub(super) struct Bindings {
    found: Vec<Option<Type>>,
    /// The parameters the last `unify` or `constrain` bound: the only ones whose types need
    /// checking.
    fresh: Vec<usize>,
    /// How many parameters `bind` has bound and kept: a change shows a new binding.
    kept: usize,
}

impl Bindings {
    /// `count` parameters, the first of them bound to `owner`, a receiver's arguments.
    fn new(count: usize, owner: &[Type]) -> Self {
        let unbound = iter::repeat_n(None, count - owner.len());
        Bindings {
            found: owner.iter().copied().map(Some).chain(unbound).collect(),
            fresh: Vec::new(),
            kept: 0,
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
            self.kept -= self.fresh.len();
            self.fresh.clear();
        }
        unified
    }

    /// As `unify`, but what of `pattern` meets `value` stays bound when the rest does not.
    /// Whether all of `pattern` meets it.
    pub(super) fn constrain(&mut self, pattern: Type, value: Type) -> bool {
        self.fresh.clear();
        self.bind(pattern, value)
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
                self.kept += 1;
                true
            }
            (TypeKind::Array(p), TypeKind::Array(v))
            | (TypeKind::Optional(p), TypeKind::Optional(v)) => self.bind(p, v),
            // A value converts through one `?` only.
            (TypeKind::Optional(p), _) if !matches!(p.kind(), TypeKind::Optional(_)) => {
                self.bind(p, value)
            }
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
        self.resolve(ty, Unbound::Fail)
    }

    /// As `known`, with each unbound parameter by the callee's name, so it always gives a type.
    /// For a diagnostic: a repeated name shows that two places must be one type.
    fn printed(&self, ty: Type) -> Type {
        self.resolve(ty, Unbound::Keep)
            .expect("a parameter should stand for itself while it is unbound")
    }

    /// The walk of `known` and `printed`: `unbound` says what an unbound parameter becomes.
    fn resolve(&self, ty: Type, unbound: Unbound) -> Option<Type> {
        #[cfg(test)]
        if matches!(unbound, Unbound::Fail) {
            WALKED.set(WALKED.get() + 1);
        }
        if !ty.has_param() {
            return Some(ty);
        }
        Some(match ty.kind() {
            TypeKind::Param(_, i) => match (self.found[i as usize], unbound) {
                (Some(bound), _) => bound,
                (None, Unbound::Fail) => return None,
                (None, Unbound::Keep) => ty,
            },
            TypeKind::Array(element) => Type::array(self.resolve(element, unbound)?),
            TypeKind::Optional(payload) => Type::optional(self.resolve(payload, unbound)?),
            TypeKind::Class(id, args) => {
                Type::intern(TypeKind::Class(id, self.resolve_all(args, unbound)?))
            }
            TypeKind::Enum(id, args) => {
                Type::intern(TypeKind::Enum(id, self.resolve_all(args, unbound)?))
            }
            _ => unreachable!("only a kind with a child can name a parameter"),
        })
    }

    fn resolve_all(&self, args: TypeArgs, unbound: Unbound) -> Option<TypeArgs> {
        let list: Option<Vec<Type>> = args
            .get()
            .iter()
            .map(|&ty| self.resolve(ty, unbound))
            .collect();
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
    /// How many types `Bindings::known` has looked at on this thread. A `printed` walk, which
    /// only a diagnostic uses, is not counted.
    pub(super) static WALKED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many type parameter names the search for reported ones has looked at on this
    /// thread.
    pub(super) static SCANNED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many types the walk for the type parameters that types name has visited on this
    /// thread.
    pub(super) static MARKED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many lookups of `body.ahead` have found an entry on this thread.
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
/// `body.ahead` by the span it was done for until checking reaches it. It lives no longer than
/// the generic call that ranked it, nor than the diagnostics and locals it made.
pub(super) enum Ahead {
    /// A method's checked receiver, or a `match`'s scrutinee.
    Receiver(Checked),
    /// An `if let`'s value, as `let_value` gives it.
    Let(Checked, LetValue, bool),
    /// A block's scope, and its statements before its value.
    Block(FxHashMap<Symbol, Binding>, BlockSoFar),
    /// A branch with a pattern, an `if let`'s first block or a `match` arm: what the pattern
    /// bound, the arm's checked guard, the scope that holds the pattern's names and the
    /// branch's statements, and its statements before its value (none for an arm whose body
    /// is not a block).
    Bound(
        LetPattern,
        Option<Checked>,
        FxHashMap<Symbol, Binding>,
        BlockSoFar,
    ),
}

/// Whether one of `branches` has a value, and each that has one needs context: each is `None`
/// when its branch ends in an exit, else whether its value needs context. It stops at the first
/// value that does not, so the branches after it are not ranked.
fn branches_wait(branches: impl IntoIterator<Item = Option<bool>>) -> bool {
    let mut any = false;
    for waits in branches {
        match waits {
            None => {}
            Some(true) => any = true,
            Some(false) => return false,
        }
    }
    any
}

/// Whether `checked`, an argument, was rejected by its parameter's type (see `mark`).
pub(super) fn mismatched(checked: &Checked) -> bool {
    matches!(checked, Err(Poisoned { mismatch: true, .. }))
}

impl Checker<'_> {
    /// The arguments of a call of the generic `sig`, checked in four passes that bind its type
    /// parameters. The caller has matched the arguments to the parameters by count and label.
    /// `written` gives every type parameter that the receiver does not give.
    ///
    /// 1. The arguments that are neither literal-like nor need context. Each is checked against
    ///    its parameter's type once that type names no unbound parameter, and on its own before.
    /// 2. `expected` against the return type, whole. When the return type cannot meet an
    ///    optional `expected` whole, it meets its payload. When it meets neither with the help
    ///    of pass 1, but does meet one alone, each argument of pass 1 that contradicts it is
    ///    reported at the argument.
    /// 3. The literal-like arguments, as in pass 1.
    /// 4. The arguments that need context, against their parameter's type.
    ///
    /// An argument of type `unit` or `never` binds only what none of the others binds. Returns
    /// the arguments in source order, the type arguments and the call's type. Returns `None`,
    /// reported, when a parameter is left unbound or bound to a type that no parameter takes.
    /// Then an argument that needs context and has none is checked against `UNKNOWN`, for its
    /// own mistakes only. What ranking the arguments checked ahead (see `Ahead`) and left unused
    /// is dropped.
    pub(super) fn generic_call(
        &mut self,
        span: Span,
        sig: &GenericSig<'_>,
        written: (&[ast::TypeExpr], Span),
        args: &[ast::Arg],
        expected: Option<Type>,
    ) -> Option<(Vec<Checked>, TypeArgs, Option<Type>)> {
        let mark = self.body.ahead.len();
        // The context of the quiet value failed, as an `UNKNOWN` one did.
        let expected = expected.or(self.quiet(span, expected).then_some(UNKNOWN));
        let call = self.infer_call(span, sig, written, args, expected);
        // A key is kept again only once its last entry is used, so an earlier entry of a key
        // truncated here, if any, is used, and dropping the key loses nothing. A placeholder
        // (`keep` with no work) is the exception, but ranking fills each one before this runs.
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
            // The return type meets `want` whole if it can and an argument waits for it:
            // `let x: i64? = pick(5, None)` binds `T = i64?`. Else it meets the payload of an
            // optional `want`, and the call's value is wrapped: `y ?? id(2)` on an `i64?` `y`
            // gives the `i64` that `y ?? 2` gives. What of it cannot meet either is reported
            // where the value is used, unless the context fits alone: then the arguments that
            // contradict it are reported, each at its own place. Type arguments that the call
            // writes are its own.
            let met = (ranks.contains(&2) && call.bindings.unify(ret, want))
                || call.bindings.constrain(ret, payload(want));
            if !met && written.is_empty() {
                self.blame_arguments(&mut call, sig, args, ret, want);
            }
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
                let broken_name = broken_param(&mut seen, name);
                if call.bindings.found[i].is_none() && broken_name {
                    call.bindings.found[i] = Some(UNKNOWN);
                    broken = true;
                }
            }
        }
        let list = match call.bindings.finish() {
            Ok(list) if !call.failed && !broken => Some(list),
            Ok(_) => None,
            Err(i) => {
                // An argument that failed, or a parameter's or the return type that did, may be
                // what would have bound it, and an `UNKNOWN` context is one that failed.
                let errored = call.slots.iter().flatten().any(|c| type_of(c).is_none())
                    || sig.params.contains(&None)
                    || sig.ret.is_none();
                if !errored && !call.failed && expected != Some(UNKNOWN) {
                    self.cannot_infer(sig, i);
                }
                None
            }
        };
        let Some(list) = list else {
            // What waited for context and has none is checked against a failed one, so it
            // reports only its own mistakes, and uses what ranking kept for it.
            for (arg, slot) in iter::zip(args, &call.slots) {
                if slot.is_none() {
                    let _ = self.expr(&arg.value, Some(UNKNOWN));
                }
            }
            return None;
        };
        self.instantiates = true;
        let checked = call
            .slots
            .into_iter()
            .map(|slot| {
                slot.expect("every argument should be checked once each parameter is bound")
            })
            .collect();
        Some((checked, list, sig.ret.map(|ret| ret.subst(list.get()))))
    }

    /// Reports each argument already checked whose type is not what `want`, the call's context,
    /// makes of its parameter, once `ret` meets `want` or its payload with no argument's help.
    /// Such an argument is rejected, as a rule rejects its operand, and the later arguments
    /// meet the context's types in place of the blamed arguments'. Reports nothing, and leaves
    /// the bindings, when the context does not fit `ret` alone as a value of `ret` fits it, when
    /// an argument was rejected already, or when every argument fits.
    fn blame_arguments(
        &mut self,
        call: &mut Inference,
        sig: &GenericSig<'_>,
        args: &[ast::Arg],
        ret: Type,
        want: Type,
    ) {
        let mut context = Bindings::new(sig.declared.len(), sig.owner);
        if !context.unify(ret, want) && !context.unify(ret, payload(want)) {
            return;
        }
        // `unify` lets a `T?` take a value that is not optional, which the result cannot do.
        let result = context.known(ret);
        if result != Some(want) && result.map(optional_of) != Some(want) {
            return;
        }
        // The error of an earlier argument is the call's error, as it is without a context.
        if call.slots.iter().flatten().any(mismatched) {
            return;
        }
        let mut blamed = false;
        for (i, arg) in args.iter().enumerate() {
            // An argument of type `unit` or `never` binds only what no other does (see `last`).
            let got = call.slots[i].as_ref().and_then(type_of);
            let (Some(param), Some(got)) = (sig.params[i], got) else {
                continue;
            };
            let Some(fit) = context.known(param) else {
                continue;
            };
            // As `against`: the type, or its payload wrapped.
            if got == fit
                || matches!(got, Type::UNIT | Type::NEVER)
                || matches!(fit.kind(), TypeKind::Optional(inner) if inner == got)
            {
                continue;
            }
            let d = self.mismatch(arg.value.span, Some(&arg.value), fit, got);
            self.diagnostics.push(d);
            call.slots[i] = Some(Err(Poisoned {
                ty: None,
                mismatch: true,
            }));
            blamed = true;
        }
        if blamed {
            // A parameter that the context leaves open keeps what an argument bound.
            for (found, bound) in iter::zip(&mut context.found, &call.bindings.found) {
                *found = found.or(*bound);
            }
            call.bindings = context;
        }
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
        match call.bindings.known(pattern) {
            Some(want) => mark(self.operand(e, Some(want))),
            None => self.infer_unbound(call, e, pattern),
        }
    }

    /// `infer_value` for a `pattern` that names an unbound parameter, with no walk of
    /// `pattern` to find this out.
    fn infer_unbound(&mut self, call: &mut Inference, e: &ast::Expr, pattern: Type) -> Checked {
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
                let want = call.bindings.printed(pattern);
                let d = self.mismatch(e.span, Some(e), want, ty);
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
    /// parameter. The elements are checked in the three waves of `rank`, as `infer_value` does,
    /// so a later `None` or integer literal meets the parameter already bound by an earlier
    /// element, and so wraps. A `None` or `[]` with nothing bound is reported as `joined`
    /// reports it: the first alone, unless an element failed already. An element of type `unit`
    /// or `never` has no value, which is reported at once, and binds nothing.
    fn inferred_array(
        &mut self,
        call: &mut Inference,
        span: Span,
        elements: &[ast::Expr],
        element: Type,
    ) -> Checked {
        let mut slots: Vec<Option<Checked>> = elements.iter().map(|_| None).collect();
        let mut quiet = false;
        // `element`'s type once an element binds all it names. A later element meets it with
        // no new walk of `element`, so the work does not grow with elements times its depth.
        let mut bound = None;
        for wave in 0..3 {
            for (i, e) in elements.iter().enumerate().filter(|(_, e)| rank(e) == wave) {
                let checked = if let Some(want) = bound {
                    mark(self.operand(e, Some(want)))
                } else if wave < 2 {
                    let kept = call.bindings.kept;
                    let checked = self.infer_unbound(call, e, element);
                    // Only a new binding can bind `element`.
                    if call.bindings.kept != kept {
                        bound = call.bindings.known(element);
                    }
                    checked
                } else {
                    quiet = quiet || slots.iter().flatten().any(|c| type_of(c).is_none());
                    // Only its own diagnostics matter.
                    let _ = self.value(e, quiet.then_some(UNKNOWN));
                    quiet = true;
                    Err(Poisoned::error())
                };
                // An element has a value: `infer_value` leaves `unit` and `never` for
                // `infer_arg`, which an element has not.
                slots[i] = Some(match type_of(&checked) {
                    Some(ty @ (Type::UNIT | Type::NEVER)) => {
                        self.no_value(e.span, ty);
                        Err(Poisoned::error())
                    }
                    _ => checked,
                });
            }
        }
        let Some(bound) = bound else {
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
            if matches!(ty, Type::UNIT | Type::NEVER) {
                self.report(DiagnosticKind::TypeArgument {
                    ty: self.type_name(ty),
                    at,
                });
                call.failed = true;
            }
        }
    }

    /// Reports the call of `sig` that binds nothing to its parameter at `param`.
    fn cannot_infer(&mut self, sig: &GenericSig<'_>, param: usize) {
        let name = sig.name;
        // `Some(...)`, or `Option.Some(...)`. A user variant `Some`, which E0026 reports, is
        // neither.
        let option = sig.variant.is_none_or(|_| name.name.as_str() == "Option");
        if option && sig.variant.unwrap_or(name).name.as_str() == "Some" {
            let (callee, at) = match sig.variant {
                Some(variant) => (
                    format!("{}.{}", name.name, variant.name),
                    name.span.to(variant.span),
                ),
                None => (name.name.to_string(), name.span),
            };
            self.report(DiagnosticKind::CannotInferSome { callee, at });
            return;
        }
        // `Ok(...)` and `Err(...)` write the variant as the callee and the enum not at all.
        if let Some(variant) = sig.variant.filter(|v| v.span == name.span) {
            self.report(DiagnosticKind::CannotInferResult {
                param: sig.declared[param].to_string(),
                variant: variant.name.to_string(),
                at: name.span,
            });
            return;
        }
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
            in_result: named_params(iter::once(sig.ret), sig.declared.len())[param],
        });
    }

    /// Whether `e`'s type cannot come from itself: a bare `[]` or `None`; a payload-free
    /// variant of a generic enum; an array literal whose elements all need context; an `if`, an
    /// `if let` or a `match` whose every value branch does, or a block whose value does; or a
    /// call of a generic function or method, or a construction of a generic class or variant,
    /// its type arguments not written, whose arguments fit its declaration and all need
    /// context, or, if they fit, whose parameter types leave one of its own type parameters
    /// unnamed that the call's type names, which no argument can bind. A name resolves as the
    /// checker resolves it: a call's callee by its declaration, whatever local takes it. A
    /// method's receiver, an `if let`'s value and a `match`'s scrutinee are checked here, ahead of
    /// their turn, and kept for it (see `Ahead`).
    // shortcut: a nested generic call scans its arguments again, quadratic in the depth of the
    // nesting; remember the answer by span if deep nests appear.
    pub(super) fn needs_context(&mut self, e: &ast::Expr) -> bool {
        self.waits(e, &mut true)
    }

    /// `needs_context`, where `fit` is cleared when a call that waits does not fit its
    /// declaration: count, labels, and every parameter's and the return type resolved.
    fn waits(&mut self, e: &ast::Expr, fit: &mut bool) -> bool {
        use ast::ExprKind as K;
        let (
            CallShape {
                generic,
                shaped,
                open,
            },
            args,
        ) = match &e.kind {
            _ if bare_none(e) => return true,
            K::ArrayLit(elements) => return elements.iter().all(|e| self.waits(e, fit)),
            K::Field {
                base,
                name,
                safe: false,
            } => {
                return self
                    .generic_variant(base, name)
                    .is_some_and(|(_, v)| v.fields.is_empty());
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
                let shape = if let Some((names, v)) = variant {
                    // Mixed labels are reported at the declaration.
                    let shaped = !v.fields.is_empty()
                        && (v.labels.iter().all(Option::is_some)
                            || v.labels.iter().all(Option::is_none))
                        && fits(v.fields.iter().copied().zip(v.labels.iter().copied()), args);
                    CallShape {
                        generic: true,
                        shaped,
                        open: shaped && leaves_open(v.fields.iter().copied(), names, 0),
                    }
                } else if !safe
                    && method.name.as_str() == "Some"
                    && option_written(receiver) == Some(false)
                {
                    // `Option.Some` is `Some`, as `generic_callee` takes it.
                    CallShape {
                        generic: true,
                        shaped: matches!(&args[..], [arg] if arg.label.is_none()),
                        open: false,
                    }
                } else {
                    self.generic_method(receiver, method, args, *safe)
                };
                (shape, args)
            }
            K::Call {
                callee,
                types,
                args,
                ..
            } if types.is_empty() => (self.generic_callee(callee.name, args), args),
            K::If {
                then_block,
                else_block: Some(else_block),
                ..
            } => {
                let blocks = [then_block, else_block].into_iter();
                return branches_wait(blocks.map(|b| self.block_waits(b, fit, |_| None)));
            }
            K::IfLet {
                pattern,
                value,
                then_block,
                else_block: Some(else_block),
            } => return self.if_let_waits(pattern, value, then_block, else_block, fit),
            K::Match {
                scrutinee, arms, ..
            } => {
                let form = LetValue::Match {
                    ty: self.value_ahead(scrutinee).filter(|&ty| matchable(ty)),
                };
                return branches_wait(arms.iter().map(|arm| {
                    let bind =
                        |c: &mut Self| Some(c.bind_pattern(None, &form, &arm.pattern, Some(arm)));
                    match &arm.body.kind {
                        K::Block(block) => self.block_waits(block, fit, bind),
                        _ => Some(self.tail_waits(arm_key(arm), None, &arm.body, fit, bind)),
                    }
                }));
            }
            K::Block(block) => return self.block_waits(block, fit, |_| None) == Some(true),
            _ => return false,
        };
        let waits = generic && (open || args.iter().all(|arg| self.waits(&arg.value, fit)));
        *fit &= !waits || shaped;
        waits
    }

    /// Whether the `if let pattern = value { then_block } else { else_block }` waits, as
    /// `branches_wait` decides. Its value is checked once, and kept for it (see `Ahead::Let`).
    fn if_let_waits(
        &mut self,
        pattern: &ast::Pattern,
        value: &ast::Expr,
        then_block: &ast::Block,
        else_block: &ast::Block,
        fit: &mut bool,
    ) -> bool {
        // Ranked before: its value comes back to bind the first block, and goes back after.
        let kept = self.kept(value.span, false);
        let (index, checked, form, mismatched) =
            if let Some((index, Ahead::Let(checked, form, mismatched))) = kept {
                (index, checked, form, mismatched)
            } else {
                // An argument is not part of an assignment target's chain.
                let target = std::mem::take(&mut self.body.in_target);
                let (checked, form, mismatched) = self.let_value(Unwrap::IfLet, pattern, value);
                self.body.in_target = target;
                let index = self.keep((value.span, false), None);
                (index, checked, form, mismatched)
            };
        let then = self.block_waits(then_block, fit, |c| {
            Some(c.bind_pattern(Some(Unwrap::IfLet), &form, pattern, None))
        });
        self.body.ahead[index].1 = Some(Ahead::Let(checked, form, mismatched));
        let other = iter::once_with(|| self.block_waits(else_block, fit, |_| None));
        branches_wait(iter::once(then).chain(other))
    }

    /// Whether `block` ends in a value that needs context (see `tail_waits`, which `bind` binds
    /// for); `None` when it ends in an exit, which leaves no value.
    fn block_waits(
        &mut self,
        block: &ast::Block,
        fit: &mut bool,
        bind: impl FnOnce(&mut Self) -> Option<(LetPattern, Option<Checked>)>,
    ) -> Option<bool> {
        match block.stmts.last().map(|stmt| &stmt.kind) {
            Some(StmtKind::Return(_) | StmtKind::Break { .. } | StmtKind::Continue { .. }) => None,
            Some(StmtKind::Expr(tail)) => {
                Some(self.tail_waits(block.span, Some(block), tail, fit, bind))
            }
            _ => Some(false),
        }
    }

    /// Whether `tail`, the value of the branch at `key`, needs context, in the branch's scope.
    /// Here, once, `bind` binds its names in that scope, if any, and the statements of `block`
    /// before `tail` are checked. The scope is kept for the branch (see `Ahead`). `block` is
    /// `None` for a `match` arm whose body is `tail`.
    fn tail_waits(
        &mut self,
        key: Span,
        block: Option<&ast::Block>,
        tail: &ast::Expr,
        fit: &mut bool,
        bind: impl FnOnce(&mut Self) -> Option<(LetPattern, Option<Checked>)>,
    ) -> bool {
        // Ranked before: its scope comes back for the value, and goes back after.
        let kept = match self.kept(key, true) {
            Some((index, Ahead::Block(scope, so_far))) => Some((index, None, scope, so_far)),
            Some((index, Ahead::Bound(bound, guard, scope, so_far))) => {
                Some((index, Some((bound, guard)), scope, so_far))
            }
            _ => None,
        };
        let (index, bound, so_far) = if let Some((index, bound, scope, so_far)) = kept {
            self.body.scopes.push(scope);
            (index, bound, so_far)
        } else {
            self.body.scopes.push(FxHashMap::default());
            // Neither a pattern's guard nor a statement is part of an assignment target's chain.
            let target = std::mem::take(&mut self.body.in_target);
            let bound = bind(self);
            let mut so_far = BlockSoFar::default();
            if let Some(block) = block {
                self.block_statements(block, None, &mut so_far, block.stmts.len() - 1);
            }
            self.body.in_target = target;
            // Its place, held before the value's ranking keeps work of its own.
            (self.keep((key, true), None), bound, so_far)
        };
        let waits = self.waits(tail, fit);
        let scope = self
            .body
            .scopes
            .pop()
            .expect("the branch's scope should be open while its value is ranked");
        self.body.ahead[index].1 = Some(match bound {
            Some((bound, guard)) => Ahead::Bound(bound, guard, scope, so_far),
            None => Ahead::Block(scope, so_far),
        });
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

    /// The variant `base.member` of a generic enum that `base` names without type arguments,
    /// and the enum's type parameters.
    pub(super) fn generic_variant(
        &self,
        base: &ast::Expr,
        member: &ast::Ident,
    ) -> Option<(&[Symbol], &VariantInfo)> {
        let ast::ExprKind::Name(name) = base.kind else {
            return None;
        };
        let Some(&Declared::Known(id)) = self.enum_ids.get(&name) else {
            return None;
        };
        let info = &self.enums[id.0];
        let &variant = info.index.get(&member.name)?;
        (!info.type_params.is_empty()).then(|| (&info.type_params[..], &info.variants[variant]))
    }

    /// The shape of `receiver.method(args)`, or of `receiver?.method(args)` with `safe`, a call
    /// of a method with type parameters of its own.
    fn generic_method(
        &mut self,
        receiver: &ast::Expr,
        method: &ast::Ident,
        args: &[ast::Arg],
        safe: bool,
    ) -> CallShape {
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
            _ => return CallShape::default(),
        };
        let Some(&Declared::Known(func)) = methods.get(&method.name) else {
            return CallShape::default();
        };
        let sig = &self.signatures[func.0];
        let shaped = sig.ret.is_some() && fits(sig.params.iter().map(|&ty| (ty, None)), args);
        CallShape {
            generic: sig.type_params.len() > owner,
            shaped,
            // The receiver binds the owner's type parameters.
            open: shaped
                && sig.ret.is_some_and(|ret| {
                    leaves_open_in(sig.params.iter().copied(), ret, &sig.type_params, owner)
                }),
        }
    }

    /// The type of `receiver`, a method's, checked ahead of the call and kept for `member`.
    /// `None` for a name no local takes: a path, which `type_path` checks, or an unknown name.
    fn receiver_ahead(&mut self, receiver: &ast::Expr) -> Option<Type> {
        match receiver.kind {
            ast::ExprKind::Name(name) if self.lookup(name).is_none() => return None,
            ast::ExprKind::Applied { .. } => return None,
            _ => {}
        }
        self.value_ahead(receiver)
    }

    /// `e` checked as a value: as ranking checked it ahead, a method's receiver or a `match`'s
    /// scrutinee (see `value_ahead`), else now.
    pub(super) fn value_kept(&mut self, e: &ast::Expr) -> Checked {
        match self.kept(e.span, false) {
            Some((_, Ahead::Receiver(checked))) => checked,
            _ => self.value(e, None),
        }
    }

    /// The type of `e`, checked ahead of its turn and kept for it as `Ahead::Receiver`.
    fn value_ahead(&mut self, e: &ast::Expr) -> Option<Type> {
        let kept = self.body.ahead_at.get(&(e.span, false)).and_then(|&index| {
            #[cfg(test)]
            INSPECTED.set(INSPECTED.get() + 1);
            self.body.ahead[index].1.as_ref()
        });
        if let Some(Ahead::Receiver(checked)) = kept {
            return type_of(checked);
        }
        // An argument is not part of an assignment target's chain.
        let target = std::mem::take(&mut self.body.in_target);
        let checked = self.value(e, None);
        self.body.in_target = target;
        let ty = type_of(&checked);
        self.keep((e.span, false), Some(Ahead::Receiver(checked)));
        ty
    }

    /// The shape of a call of `name` with `args` that calls a generic function, constructs a
    /// generic class, or builds `Some`, `Ok` or `Err`.
    fn generic_callee(&self, name: Symbol, args: &[ast::Arg]) -> CallShape {
        match (self.class_ids.get(&name), self.function_ids.get(&name)) {
            (Some(&Declared::Known(class)), _) => {
                let info = &self.classes[class.0];
                let shaped = fits(info.fields.iter().map(|f| (f.ty, Some(f.name.name))), args);
                CallShape {
                    generic: !info.type_params.is_empty(),
                    shaped,
                    open: shaped
                        && leaves_open(info.fields.iter().map(|f| f.ty), &info.type_params, 0),
                }
            }
            (None, Some(&Declared::Known(func))) => {
                let sig = &self.signatures[func.0];
                let shaped =
                    sig.ret.is_some() && fits(sig.params.iter().map(|&ty| (ty, None)), args);
                CallShape {
                    generic: !sig.type_params.is_empty(),
                    shaped,
                    open: shaped
                        && sig.ret.is_some_and(|ret| {
                            leaves_open_in(sig.params.iter().copied(), ret, &sig.type_params, 0)
                        }),
                }
            }
            // `Some<T>(x: T) -> T?`, and the prelude's `Ok` and `Err`, which no user item can
            // take the name of.
            (None, None) if name.as_str() == "Some" => CallShape {
                generic: true,
                shaped: matches!(args, [arg] if arg.label.is_none()),
                open: false,
            },
            (None, None) => self
                .result_variant(name)
                .map_or_else(CallShape::default, |(id, v)| {
                    let info = &self.enums[id.0];
                    let shaped = matches!(args, [arg] if arg.label.is_none());
                    CallShape {
                        generic: true,
                        shaped,
                        open: shaped
                            && leaves_open(
                                info.variants[v].fields.iter().copied(),
                                &info.type_params,
                                0,
                            ),
                    }
                }),
            _ => CallShape::default(),
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
                self.instantiates = true;
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

    /// The type of the type argument `arg`: `None`, reported, for `unit` or `never`, which no
    /// type parameter takes.
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
            });
            return None;
        }
        self.resolve(arg)
    }

    /// Reports `main`, which has type parameters. The fix removes them, unless a comment is
    /// among them or a name in the rest of `main`, outside its strings and comments, is one.
    pub(super) fn main_type_params(&mut self, main: &ast::Function) {
        let angles = main.angles;
        let rest = self.source.file.slice(angles.end()..main.span.end());
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
        let compare = first.map(|first| {
            self.source
                .file
                .slice(name.start()..first.span.end())
                .into()
        });
        self.misread_as(name, compare)
    }

    /// Whether the type argument `arg` could be a value: a local, or a name that names no type.
    pub(super) fn could_be_value(&self, arg: &ast::TypeExpr) -> bool {
        let ast::TypeKind::Name(n) = arg.kind else {
            return false;
        };
        arg.optional == 0 && self.body.scopes.iter().any(|scope| scope.contains_key(&n))
            || self.names_no_type(arg)
    }

    /// Whether the type argument `arg` is a name, with no `?`, that is no built-in type, class,
    /// enum or type parameter.
    pub(super) fn names_no_type(&self, arg: &ast::TypeExpr) -> bool {
        let ast::TypeKind::Name(n) = arg.kind else {
            return false;
        };
        arg.optional == 0
            && built_in_type(n.as_str()).is_none()
            && !self.class_ids.contains_key(&n)
            && !self.enum_ids.contains_key(&n)
            && self.body.type_param(n).is_none()
    }

    /// `misread` with the comparison `compare` its help writes, if any.
    pub(super) fn misread_as(&mut self, name: Span, compare: Option<String>) -> Poisoned {
        self.body.misread.insert(name.start());
        self.report(DiagnosticKind::TypeArguments {
            name: self.source.file.slice(name.start()..name.end()).to_string(),
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

    /// Whether `receiver` before `.method` is a path, not a value: an enum, `Option`, or `math`
    /// or a built-in type that no local takes.
    pub(super) fn path_method(&mut self, receiver: &ast::Expr, method: &ast::Ident) -> bool {
        let name = match &receiver.kind {
            ast::ExprKind::Name(name) => *name,
            ast::ExprKind::Applied { name, .. } => name.name,
            _ => return false,
        };
        // No item can take the name of the optional type, whose members are its variants.
        if name.as_str() == "Option" {
            return true;
        }
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
    /// `base` names. The enum takes them, so the help writes them on the enum. There is no help
    /// when one of `types` names no type: the list may be a comparison.
    pub(super) fn variant_args(
        &mut self,
        base: &ast::Expr,
        variant: &ast::Ident,
        types: &[ast::TypeExpr],
        angles: Span,
    ) {
        let text = self.source.file;
        let instead = types.iter().all(|ty| !self.names_no_type(ty)).then(|| {
            format!(
                "{}{}.{}",
                text.text(base.span),
                text.text(angles),
                variant.name
            )
        });
        self.report(DiagnosticKind::TypeArguments {
            name: text
                .slice(base.span.start()..variant.span.end())
                .to_string(),
            at: angles,
            form: ArgumentsForm::Variant { instead },
        });
    }
}
