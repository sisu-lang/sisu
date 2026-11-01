//! Array expressions: literals and their element join, `[v; n]`, indexing and methods.

use super::any::{AnyJoin, is_target, widen};
use super::expr::{fits, literal_type, no_member, path, typed};
use super::generic::{Rank, mark};
use super::{Checked, Checker, Declared, Poisoned, UNKNOWN, as_optional, payload, type_of};
use crate::ast;
use crate::diagnostic::{Definition, Diagnostic, DiagnosticKind, Literal};
use crate::source::Span;
use crate::tir::{self, Type, TypeArgs, TypeKind};

/// How the elements of a literal join when a `Some` of an object is in the way (`join_objects`).
enum ObjectJoin {
    /// The elements join, optional, with each such `Some` taken off.
    Joined(Type),
    /// The objects do not join: the element at `at`, of type `got`, leaves the `any` type `want`.
    Leaves { at: usize, want: Type, got: Type },
    /// No such join: the elements join as they are.
    Not,
}

/// Whether `ty` is an `any` type, or an array or an optional of one, at any depth.
fn holds_any(mut ty: Type) -> bool {
    while let TypeKind::Array(part) | TypeKind::Optional(part) = ty.kind() {
        ty = part;
    }
    matches!(ty.kind(), TypeKind::Any(..))
}

/// The methods of an array that the compiler writes, with their parameter counts. `push` is not
/// answered here: it writes its receiver's place, which `Checker::push` checks. The prelude's
/// `extend` blocks give an array its other methods.
pub(super) const METHODS: [(&str, usize); 2] = [("len", 0), ("push", 1)];

impl Checker<'_> {
    /// `[a, b, ...]`. With an expected `[E]` or `[E]?`, each element is checked against `E`.
    /// Without one, the elements join (see `joined`). A `[]` takes its type from `expected`.
    pub(super) fn array_lit(
        &mut self,
        span: Span,
        elements: &[ast::Expr],
        expected: Option<Type>,
    ) -> Checked {
        // The type of an earlier branch is a hint, not a written type (see `Body::hint_at`). A
        // hint of an array of an `any` type is not an expected type: each element joins the
        // elements of the siblings (see `hinted_elements`). Any other hint is an expected type.
        let siblings = self.hinted(span, expected).1;
        if let Some(element) = expected_element(siblings).filter(|&element| holds_any(element)) {
            return if elements.is_empty() {
                // A `[]` has no element to join, so it takes the type of its siblings.
                self.array(span, element, Vec::new(), false)
            } else {
                self.ranked(|c| c.hinted_elements(span, elements, element))
            };
        }
        if let Some(element) = expected_element(expected) {
            let (checked, mismatched): (Vec<_>, Vec<_>) = elements
                .iter()
                .map(|e| self.operand(e, Some(element)))
                .unzip();
            return self.array(span, element, checked, mismatched.contains(&true));
        }
        let quiet = expected == Some(UNKNOWN) || self.quiet(span, expected);
        if elements.is_empty() {
            let (d, mismatch) = match expected {
                _ if quiet => return Err(Poisoned::error()),
                Some(ty) => (
                    DiagnosticKind::Mismatch {
                        want: vec![self.type_name(ty)],
                        got: "[]".to_string(),
                        at: span,
                        source: self.expected_from(span),
                        fix: None,
                        conversion: false,
                        layers: false,
                    },
                    true,
                ),
                None => (
                    DiagnosticKind::CannotInfer {
                        literal: Literal::EmptyArray,
                        binding: self.let_binding(span),
                        at: span,
                        regroup: None,
                    },
                    false,
                ),
            };
            self.report(d);
            return Err(Poisoned { ty: None, mismatch });
        }
        self.ranked(|c| c.joined(span, elements, quiet))
    }

    /// The `elements` of a literal whose siblings are arrays of `element`, a type that holds an
    /// `any` type (see `Body::hint_at`). Each is a hinted value beside an `element` value, and
    /// they join as the branches of an `if` do (`join`), with `element` as the siblings' type.
    /// An array does not widen: the join must go into `element`, or the first element that does
    /// not is the mistake, named by its own type.
    fn hinted_elements(&mut self, span: Span, elements: &[ast::Expr], element: Type) -> Checked {
        let fits_element =
            |c: &mut Self, ty: Type| fits(element, ty) || c.converts_to_any(ty, element);
        // The type of each element as written: a `Some` that converted its value is the
        // optional of the value's type.
        let mut own: Vec<Option<Type>> = vec![None; elements.len()];
        let sources: Vec<_> = elements.iter().map(|e| self.value_branch(e)).collect();
        let siblings = (None, Some(element));
        let (blocks, joined) = self.join(span, &sources, siblings, |c, i, expected| {
            let checked = c.value(&elements[i], expected);
            own[i] = checked.as_ref().ok().map(|e| {
                if let tir::ExprKind::Wrap(inner) = &e.kind
                    && let tir::ExprKind::ToAny(value) = &inner.kind
                {
                    Type::optional(value.ty)
                } else {
                    e.ty
                }
            });
            checked.map(|value| tir::Block {
                stmts: Vec::new(),
                ty: value.ty,
                value: Some(Box::new(value)),
            })
        });
        let Ok(joined) = joined else {
            return Err(Poisoned::error());
        };
        if joined != Type::NEVER && !fits_element(self, joined) {
            let at = (0..elements.len())
                .find(|&i| own[i].is_some_and(|ty| !fits_element(self, ty)))
                .unwrap_or(0);
            let e = &elements[at];
            if matches!(e.kind, ast::ExprKind::NoneLit) {
                // As in a written array: "found `None`".
                drop(self.none(e.span, Some(element)));
            } else {
                let d = self.leaves_any(e.span, Some(e), element, own[at].unwrap_or(joined));
                self.diagnostics.push(d);
            }
            return Err(Poisoned::error());
        }
        let checked = blocks
            .into_iter()
            .map(|block| {
                let value =
                    block.map(|b| *b.value.expect("an element's block should be its value"));
                match value {
                    Ok(e) if e.ty != element => {
                        self.instantiates |= is_target(element);
                        Ok(widen(e, element))
                    }
                    value => value,
                }
            })
            .collect();
        self.array(span, element, checked, false)
    }

    /// The elements of a literal with no expected array type, joined in three waves by
    /// `sibling_ranked`: the concrete elements; the elements that take a literal's type, typed
    /// beside the first concrete element; then the elements that wait, against the join. The join
    /// runs in source order as `branches` joins. An element that makes an optional by itself
    /// (`Ranked::optional`) makes the element type optional, and is typed against the join made
    /// optional; an element that waits and is not such is typed against the join as it is. With
    /// no typed element, the first element that waits cannot be inferred, which `quiet` (the
    /// context failed) leaves unreported.
    /// With `quiet`, each concrete element is a quiet value too (see `Body::quiet_at`). An
    /// element whose type is `Error` poisons the array.
    fn joined(&mut self, span: Span, elements: &[ast::Expr], quiet: bool) -> Checked {
        let mut slots: Vec<Option<Checked>> = elements.iter().map(|_| None).collect();
        let (ranks, optional): (Vec<Rank>, Vec<bool>) =
            elements.iter().map(|e| self.sibling_ranked(e)).unzip();
        let outer = self.body.quiet_at;
        // The first typed element that is an `any` type, widened by the later ones, is a hint
        // for the elements after it, as the first branch of an `if` is for the others.
        let mut anchor: Option<Type> = None;
        for (i, e) in elements.iter().enumerate() {
            if ranks[i] == Rank::Concrete {
                self.body.quiet_at = if quiet { Some(e.span) } else { outer };
                let checked = match anchor {
                    Some(any) => self.value_beside(e, any),
                    None => self.value(e, None),
                };
                anchor = self.widen_anchor(anchor, type_of(&checked));
                slots[i] = Some(checked);
            }
        }
        self.body.quiet_at = outer;
        let literal = literal_type(slots.iter().flatten().next().and_then(type_of));
        for (i, e) in elements.iter().enumerate() {
            if ranks[i] == Rank::Literal {
                // An `optional` element may be `None`, which the literal's type does not hold.
                let want = if optional[i] {
                    as_optional(literal)
                } else {
                    literal
                };
                slots[i] = Some(self.value(e, Some(want)));
            }
        }
        let mut failed = slots.iter().flatten().any(|c| type_of(c).is_none());
        let mut joined: Option<Type> = None;
        if !failed {
            let typed: Vec<(usize, Type)> = (slots.iter().enumerate())
                .filter_map(|(i, slot)| Some((i, slot.as_ref().and_then(type_of)?)))
                .collect();
            match self.join_objects(&mut slots, &typed) {
                ObjectJoin::Joined(ty) => joined = Some(ty),
                ObjectJoin::Leaves { at, want, got } => {
                    let value = slots[at].as_ref().and_then(|slot| slot.as_ref().ok());
                    let (at_span, source, want, got) =
                        match value.and_then(|v| self.object_in_some(v, want)) {
                            Some((span, want, object)) => (span, None, want, object),
                            None => (elements[at].span, Some(&elements[at]), want, got),
                        };
                    let d = self.leaves_any(at_span, source, want, got);
                    self.diagnostics.push(d);
                    failed = true;
                }
                ObjectJoin::Not => {
                    (joined, failed) = self.fold_elements(elements, &typed);
                }
            }
        }
        let Some(joined) = joined.filter(|_| !failed) else {
            let waiting = elements
                .iter()
                .zip(&ranks)
                .filter(|&(_, &r)| r == Rank::Waits);
            for (i, (e, _)) in waiting.enumerate() {
                let expected = (i > 0 || failed || quiet).then_some(UNKNOWN);
                // Only its own diagnostics matter.
                drop(self.value(e, expected));
            }
            return Err(Poisoned::error());
        };
        let want = as_optional(joined);
        for (i, e) in elements.iter().enumerate() {
            if ranks[i] == Rank::Waits {
                // What waits and makes no optional by itself (`Ranked::optional`) may still give a
                // type that the join does not take, such as `[]`.
                slots[i] = Some(if optional[i] {
                    self.value(e, Some(want))
                } else {
                    mark(self.operand(e, Some(joined)))
                });
            }
        }
        let element = if optional.contains(&true) {
            want
        } else {
            joined
        };
        // A `[]` that rejected `want` makes the literal `Error`.
        let mismatched = slots
            .iter()
            .any(|slot| matches!(slot, Some(Err(Poisoned { mismatch: true, .. }))));
        let checked = slots
            .into_iter()
            .map(|slot| {
                match slot.expect("the three waves should check every element, one rank each") {
                    Ok(e) if e.ty != element => Ok(widen(e, element)),
                    checked => checked,
                }
            })
            .collect();
        self.array(span, element, checked, mismatched)
    }

    /// How the `typed` elements join when a `Some` of an object is in the way: such an element
    /// joins an `any` element as the object, and the join is optional, as the branches of an `if`
    /// do. The `Some` of each such slot is taken off, for `widen` to convert and wrap. When the
    /// objects do not join either, the member that leaves the `any` type is the mistake, as in an
    /// `if`.
    fn join_objects(
        &mut self,
        slots: &mut [Option<Checked>],
        typed: &[(usize, Type)],
    ) -> ObjectJoin {
        if !typed
            .iter()
            .any(|&(_, ty)| matches!(ty.kind(), TypeKind::Any(..)))
            || matches!(
                self.any_join_all(typed),
                AnyJoin::Joined { .. } | AnyJoin::NoAny
            )
        {
            return ObjectJoin::Not;
        }
        let object = |slot: &Option<Checked>| {
            let e = slot.as_ref()?.as_ref().ok()?;
            match &e.kind {
                tir::ExprKind::Wrap(inner)
                    if matches!(inner.ty.kind(), TypeKind::Class(..) | TypeKind::Any(..)) =>
                {
                    Some(inner.ty)
                }
                _ => None,
            }
        };
        let objects: Vec<(usize, Type)> = typed
            .iter()
            .map(|&(i, ty)| (i, object(&slots[i]).unwrap_or(ty)))
            .collect();
        if objects == typed {
            return ObjectJoin::Not;
        }
        let ty = match self.any_join_all(&objects) {
            AnyJoin::Joined { ty } => ty,
            AnyJoin::Leaves {
                ty,
                member: (at, got),
            } => return ObjectJoin::Leaves { at, want: ty, got },
            AnyJoin::NoAny | AnyJoin::Fails { .. } => return ObjectJoin::Not,
        };
        for &(i, _) in typed {
            if object(&slots[i]).is_some()
                && let Some(Ok(e)) = &mut slots[i]
                && let tir::ExprKind::Wrap(inner) =
                    std::mem::replace(&mut e.kind, tir::ExprKind::None)
            {
                *e = *inner;
            }
        }
        self.instantiates = true;
        ObjectJoin::Joined(as_optional(ty))
    }

    /// The value `e`, typed beside an element of the `any` type `any`: with `any` made optional as
    /// a hint (see `Body::hint_at`), as the branch of an `if` after an `any` branch is. So the
    /// payload of a `Some` converts to `any`.
    fn value_beside(&mut self, e: &ast::Expr, any: Type) -> Checked {
        let outer = self.pass_hint(Some(e.span), Some(any));
        let checked = self.value(e, Some(as_optional(any)));
        self.body.hint_at = outer;
        checked
    }

    /// `anchor`, the `any` type that the elements before give, widened by an element of type
    /// `got`: the first element that is an `any` type, or the join of the two.
    fn widen_anchor(&mut self, anchor: Option<Type>, got: Option<Type>) -> Option<Type> {
        let bare = |ty: Type| matches!(ty.kind(), TypeKind::Any(..)).then_some(ty);
        match (anchor, got) {
            (None, Some(ty)) => bare(ty),
            (Some(any), Some(ty)) => Some(self.any_join(any, ty).and_then(bare).unwrap_or(any)),
            (anchor, None) => anchor,
        }
    }

    /// The type that the typed `elements` join to, each an index and a type, and whether the join
    /// failed (reported). With an `any` element, the join is that type, whatever the order. Else
    /// the types fold in source order.
    fn fold_elements(
        &mut self,
        elements: &[ast::Expr],
        typed: &[(usize, Type)],
    ) -> (Option<Type>, bool) {
        let report = |c: &mut Self, at: usize, want: Type, got: Type| {
            let d = c.mismatch(elements[at].span, Some(&elements[at]), want, got);
            c.diagnostics.push(d);
        };
        match self.any_join_all(typed) {
            AnyJoin::Joined { ty } => {
                self.instantiates = true;
                return (Some(ty), false);
            }
            AnyJoin::Fails {
                earlier: (_, want),
                later: (at, got),
            } => {
                report(self, at, want, got);
                return (None, true);
            }
            AnyJoin::Leaves {
                ty,
                member: (at, got),
            } => {
                let d = self.leaves_any(elements[at].span, Some(&elements[at]), ty, got);
                self.diagnostics.push(d);
                return (None, true);
            }
            AnyJoin::NoAny => {}
        }
        let mut joined: Option<Type> = None;
        for &(i, ty) in typed {
            match joined {
                Some(t) if fits(t, ty) => {}
                Some(t) if !fits(ty, t) => {
                    report(self, i, t, ty);
                    return (joined, true);
                }
                _ => joined = Some(ty),
            }
        }
        // The join can grow past an earlier element: each converts through one layer only.
        if let Some(t) = joined
            && let Some(&(i, ty)) = typed.iter().find(|&&(_, ty)| !fits(t, ty))
        {
            report(self, i, t, ty);
            return (joined, true);
        }
        (joined, false)
    }

    /// The `ArrayLit` of `checked`, an `[element]`, unless an element failed. `mismatched`
    /// when the literal's rule reported an element, which makes it `Error`.
    pub(super) fn array(
        &mut self,
        span: Span,
        element: Type,
        checked: Vec<Checked>,
        mismatched: bool,
    ) -> Checked {
        let ty = Type::array(element);
        match checked.into_iter().collect::<Result<Vec<_>, _>>() {
            Ok(elements) => Ok(self.spilled(span, ty, elements, tir::ExprKind::ArrayLit)),
            _ => Err(Poisoned {
                ty: (!mismatched).then_some(ty),
                mismatch: mismatched,
            }),
        }
    }

    /// `[value; len]`: `value` is checked with the expected element type as its hint, then
    /// against it, and `len` against `i64`. A `value` of a class, under any layers of optional,
    /// is reported, as every slot would hold the same object; the literal `None` is not. That
    /// rule sees `value`'s own type, before a mismatch with the element type erases it.
    pub(super) fn array_fill(
        &mut self,
        span: Span,
        value: &ast::Expr,
        len: &ast::Expr,
        expected: Option<Type>,
    ) -> Checked {
        let element = expected_element(expected);
        // Without an element type, a `None` or `[]` cannot be inferred, unless `expected` is
        // `UNKNOWN` or the literal is the quiet value, which makes `value` quiet too.
        let hint = element.or_else(|| expected.filter(|&ty| ty == UNKNOWN));
        let outer = self.pass_quiet(span, Some(value.span), expected);
        // `operand`, split so the class rule runs between the check and the wrap.
        let checked = self.value(value, hint);
        self.body.quiet_at = outer;
        let class = type_of(&checked).map(innermost).filter(|ty| {
            matches!(
                ty.kind(),
                TypeKind::Class(..) | TypeKind::Param(..) | TypeKind::Assoc(..) | TypeKind::Any(..)
            ) && !self.none_written(value)
        });
        if let Some(class) = class {
            let any = matches!(class.kind(), TypeKind::Any(..));
            // An `any` of a class-only interface holds an object, as a class is one.
            let may_be_class = matches!(class.kind(), TypeKind::Param(..) | TypeKind::Assoc(..))
                || (any && !self.conforms(class, self.lang.any_object, TypeArgs::NONE));
            self.report(DiagnosticKind::SameObject {
                class: self.type_name(class),
                at: value.span,
                param: may_be_class,
                any,
            });
        }
        let (checked, mismatched) = match element {
            Some(element) => self.against(value, checked, Some(element), None),
            None => (checked, false),
        };
        let (len, len_mismatched) = self.operand(len, Some(Type::I64));
        let failed = mismatched || class.is_some() || len_mismatched;
        let ty = type_of(&checked).map(Type::array);
        match (checked, len, ty) {
            (Ok(value), Ok(len), Some(ty)) if !failed => {
                Ok(self.spilled(span, ty, vec![value, len], |operands| {
                    let (value, len) = pair(operands);
                    tir::ExprKind::ArrayFill { value, len }
                }))
            }
            (.., ty) => Err(Poisoned::of(ty.filter(|_| !failed))),
        }
    }

    /// `base[index]`: `base` is an array, not an optional one, and `index` an `i64`.
    pub(super) fn index(&mut self, span: Span, base: &ast::Expr, index: &ast::Expr) -> Checked {
        let outer = std::mem::replace(&mut self.body.indexed, true);
        let checked = self.value(base, None);
        let element = self.element(base, type_of(&checked));
        // The index is not part of the chain that it ends.
        self.body.indexed = false;
        let (index, mismatched) = self.operand(index, Some(Type::I64));
        self.body.indexed = outer;
        match (checked, index, element) {
            (Ok(base), Ok(index), Some(element)) => {
                Ok(self.spilled(span, element, vec![base, index], |operands| {
                    let (base, index) = pair(operands);
                    tir::ExprKind::Index { base, index }
                }))
            }
            (.., element) => Err(Poisoned::of(element.filter(|_| !mismatched))),
        }
    }

    /// The element type of `base`, which is indexed and has type `ty` (`None` is `Error`). An
    /// optional array, or a value that is no array, is reported and gives `Error`.
    pub(super) fn element(&mut self, base: &ast::Expr, ty: Option<Type>) -> Option<Type> {
        let ty = ty?;
        let d = match ty.kind() {
            TypeKind::Array(element) => return Some(element),
            TypeKind::Optional(payload) if matches!(payload.kind(), TypeKind::Array(_)) => {
                optional_array(base)
            }
            _ => DiagnosticKind::CannotIndex {
                ty: self.type_name(ty),
                at: base.span,
            }
            .into(),
        };
        self.diagnostics.push(d);
        None
    }

    /// `receiver.method(args)` on `receiver`, an array of type `ty`, when no block of the prelude
    /// gives `ty` the method: `len()`, which the compiler writes; a method of a block of the
    /// prelude whose bounds fail for `ty` (`unmet_block_bound`); or no method, which is E0069 when
    /// a block gives the method to another array type.
    pub(super) fn array_method(
        &mut self,
        span: Span,
        receiver: Checked,
        ty: Type,
        method: &ast::Ident,
        args: &[ast::Arg],
    ) -> Checked {
        let name = method.name.as_str();
        let built_in = METHODS.iter().find(|(m, _)| *m == name);
        if built_in.is_none()
            && let Some(Declared::Known(func)) = self.own_method(ty, method.name)
        {
            return self.unmet_block_bound(span, func, ty, method, args);
        }
        let Some(&(_, arity)) = built_in else {
            if self.rejected_methods.contains(&method.name) {
                return self.call_of_rejected_method(method, args);
            }
            let prelude = self.prelude_method_names(ty);
            let targets = self.prelude_targets(method.name);
            let d = if !prelude.contains(&name) && !targets.is_empty() {
                DiagnosticKind::SumNeedsNumbers {
                    method: name.to_string(),
                    ty: self.type_name(ty),
                    targets: targets.into_iter().map(|t| self.type_name(t)).collect(),
                    at: method.span,
                    function: false,
                }
                .into()
            } else {
                let names = METHODS.iter().map(|&(m, _)| m).chain(prelude);
                no_member(Definition::Method, self.type_name(ty), method, names)
            };
            return self.rejected(d, args);
        };
        if args.len() != arity {
            let d = DiagnosticKind::ArgumentCount {
                name: self.method_name(ty, name),
                want: arity,
                found: args.len(),
                at: span,
            };
            return self.rejected(d.into(), args);
        }
        let kind = receiver.ok().map(|r| tir::ExprKind::Len(Box::new(r)));
        typed(span, Type::I64, false, kind)
    }

    /// A call at `span` of the method `func` of a block of the prelude that covers the array type
    /// `ty` and whose bounds fail for it: E0154 at the call, with no label in the prelude. A call
    /// with the wrong number of arguments is the call-count error. An argument that is not of the
    /// type the method wants is the mistake before the bound, as it is for `==`: a conformance
    /// would not make the program right. So is an argument that failed on its own, unless a type
    /// parameter is in what the method wants. The bound is reported after the arguments.
    fn unmet_block_bound(
        &mut self,
        span: Span,
        func: tir::FuncId,
        ty: Type,
        method: &ast::Ident,
        args: &[ast::Arg],
    ) -> Checked {
        let sig = &self.signatures[func.0];
        let params = sig.params.clone();
        let extension = sig
            .extension
            .expect("a method whose block bounds fail should have a block");
        self.labels(args);
        if params.len() != args.len() {
            let name = self.method_name(ty, method.name.as_str());
            return self.wrong_count(span, name, params.len(), args);
        }
        let stands = self.owner_args(func, ty).get().to_vec();
        let mut own_mistake = false;
        for (arg, param) in args.iter().zip(params) {
            let want = param.and_then(|param| self.subst(param, &stands));
            let (argument, mismatched) = self.operand(&arg.value, want);
            // An argument with no type has failed, but a type parameter keeps its report beside
            // the error of such an argument.
            let failed = type_of(&argument).is_none() && !want.is_some_and(Type::has_param);
            own_mistake |= mismatched || failed;
        }
        if !own_mistake {
            self.report_block_bound(extension, ty, span, method.name.as_str());
        }
        Err(Poisoned::error())
    }

    /// A call of the method `method` that a rejected `extend` block may have given (see
    /// `Checker.rejected_methods`): poisoned, with no report of its own.
    pub(super) fn call_of_rejected_method(
        &mut self,
        method: &ast::Ident,
        args: &[ast::Arg],
    ) -> Checked {
        self.body.rejected_calls.insert(method.span);
        self.labels(args);
        self.stray(args);
        Err(Poisoned::error())
    }

    /// Reports `d` for a method call that cannot be checked, and its arguments' own errors.
    pub(super) fn rejected(&mut self, d: Diagnostic, args: &[ast::Arg]) -> Checked {
        self.diagnostics.push(d);
        self.labels(args);
        self.stray(args);
        Err(Poisoned::error())
    }
}

/// "`m` may be `None`", at the optional array `base` that an index or a `for` needs unwrapped.
pub(super) fn optional_array(base: &ast::Expr) -> Diagnostic {
    DiagnosticKind::MayBeNone {
        value: path(base),
        at: base.span,
        access: None,
        write: false,
        peel: None,
    }
    .into()
}

/// `E`, when `expected` is `[E]` or `[E]?`.
fn expected_element(expected: Option<Type>) -> Option<Type> {
    match expected.map(payload).map(Type::kind) {
        Some(TypeKind::Array(element)) => Some(element),
        _ => None,
    }
}

/// The two operands `spilled` hands back for a node of two.
pub(super) fn pair(operands: Vec<tir::Expr>) -> (Box<tir::Expr>, Box<tir::Expr>) {
    let [a, b]: [tir::Expr; 2] = operands
        .try_into()
        .expect("`spilled` should hand its closure one operand per input, and there are two");
    (Box::new(a), Box::new(b))
}

/// `ty` with every layer of optional taken off.
fn innermost(mut ty: Type) -> Type {
    while let TypeKind::Optional(inner) = ty.kind() {
        ty = inner;
    }
    ty
}
