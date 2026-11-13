//! Array expressions: literals and their element join, `[v; n]`, indexing and methods.

use super::any::{AnyJoin, is_target, widen};
use super::expr::{fits, literal_type, no_member, path, typed};
use super::generic::{Rank, mark, mismatch_poison};
use super::place::{Change, first_safe, is_struct};
use super::reports::Reported;
use super::{
    Checked, Checker, Declared, Expectation, Origin, Poisoned, as_optional, known_type, payload,
    type_of,
};
use crate::ast;
use crate::diagnostic::{
    Definition, Diagnostic, DiagnosticKind, Expected, IndexUsage, Literal, UnwrapHelp,
};
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
pub(super) const METHODS: &[(&str, usize)] = &[("len", 0), ("push", 1)];

impl Checker<'_> {
    /// `[a, b, ...]`. With an expected `[E]` or `[E]?`, each element is checked against `E`.
    /// Without one, the elements join (see `joined`). A `[]` takes its type from `expected`.
    pub(super) fn array_lit(
        &mut self,
        span: Span,
        elements: &[ast::Expr],
        expected: Expectation,
    ) -> Checked {
        // The type of an earlier branch is a hint, not a written type (see `Expectation::Hint`).
        // A hint of an array of an `any` type is not an expected type: each element joins the
        // elements of the siblings (see `hinted_elements`). Any other hint is an expected type.
        if let Some(element) =
            expected_element(expected.siblings()).filter(|&element| holds_any(element))
        {
            return if elements.is_empty() {
                // A `[]` has no element to join, so it takes the type of its siblings.
                self.array(span, element, Vec::new(), false)
            } else {
                self.ranked(|c| c.hinted_elements(span, elements, element))
            };
        }
        if let Some(element) = expected_element(expected.ty()) {
            let (checked, mismatched): (Vec<_>, Vec<_>) = elements
                .iter()
                .map(|e| self.operand(e, Expectation::Required(element, expected.origin())))
                .unzip();
            return self.array(span, element, checked, mismatched.contains(&true));
        }
        let quiet = expected == Expectation::Poisoned;
        if elements.is_empty() {
            let d = match expected {
                Expectation::Poisoned => {
                    return Err(Poisoned::untyped(self.poisoned_context(span)));
                }
                Expectation::Required(ty, _) | Expectation::Hint { want: ty, .. } => {
                    DiagnosticKind::Mismatch {
                        want: Expected::Types(vec![self.type_name(ty)]),
                        got: "[]".to_string(),
                        at: span,
                        origin: self.origin_fact(expected.origin()),
                        fix: None,
                        conversion: false,
                        layers: false,
                    }
                }
                Expectation::None => {
                    let reported = self.report_uninferred(Literal::EmptyArray, span);
                    return Err(Poisoned::untyped(reported));
                }
            };
            return Err(mismatch_poison(self.error(d)));
        }
        self.ranked(|c| c.joined(span, elements, quiet))
    }

    /// The `elements` of a literal whose siblings are arrays of `element`, a type that holds an
    /// `any` type (see `Expectation::Hint`). Each is a hinted value beside an `element` value.
    /// They join as the branches of an `if` do (`join`), with `element` as the siblings' type.
    /// An array does not widen: the join must go into `element`, or the first element that does
    /// not is the mistake, named by its own type.
    fn hinted_elements(&mut self, span: Span, elements: &[ast::Expr], element: Type) -> Checked {
        #[cfg(test)]
        super::reached("hinted_elements");
        let fits_element =
            |c: &mut Self, ty: Type| fits(element, ty) || c.converts_to_any(ty, element);
        // The type of each element as written: a `Some` that converted its value is the
        // optional of the value's type.
        let mut own: Vec<Option<Type>> = vec![None; elements.len()];
        let sources: Vec<_> = elements.iter().map(|e| self.value_branch(e)).collect();
        let siblings = (Expectation::None, Some((element, Origin::None)));
        let (blocks, joined) = self.join(&sources, siblings, |c, i, expected| {
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
        let joined = match joined {
            Ok(joined) => joined,
            Err(failed) => return Err(Poisoned::untyped(failed.reported())),
        };
        if joined != Type::NEVER && !fits_element(self, joined) {
            let at = (0..elements.len())
                .find(|&i| own[i].is_some_and(|ty| !fits_element(self, ty)))
                .unwrap_or(0);
            let e = &elements[at];
            let reported = if matches!(e.kind, ast::ExprKind::NoneLit) {
                // As in a written array: "found `None`".
                match self.none(e.span, Expectation::Required(element, Origin::None)) {
                    Err(failed) => failed.reported(),
                    // An optional element takes the `None`, and the check stays quiet.
                    // Else an element that failed holds the proof.
                    Ok(_) => (blocks.iter())
                        .find_map(|block| block.as_ref().err().map(Poisoned::reported))
                        .unwrap_or_else(|| self.proof(e.span)), // proof: quiet reader
                }
            } else {
                let got = own[at].unwrap_or(joined);
                let d = self.leaves_any(e.span, Some(e), element, got, Origin::None);
                self.diagnostics.error(d)
            };
            return Err(Poisoned::untyped(reported));
        }
        let checked = blocks
            .into_iter()
            .map(|block| {
                let value =
                    block.map(|b| *b.value.expect("an element's block should be its value"));
                match value {
                    Ok(e) if e.ty != element => {
                        self.instantiates |= is_target(element);
                        self.record_shown_any(e.ty, element);
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
    /// With `quiet`, a concrete element that no anchor hints is checked against a failed
    /// context. An element whose type is `Error` poisons the array.
    fn joined(&mut self, span: Span, elements: &[ast::Expr], quiet: bool) -> Checked {
        let mut slots: Vec<Option<Checked>> = elements.iter().map(|_| None).collect();
        let (ranks, optional): (Vec<Rank>, Vec<bool>) =
            elements.iter().map(|e| self.sibling_ranked(e)).unzip();
        // The first typed element that is an `any` type, widened by the later ones, is a hint
        // for the elements after it, as the first branch of an `if` is for the others.
        let mut anchor: Option<Type> = None;
        for (i, e) in elements.iter().enumerate() {
            if ranks[i] == Rank::Concrete {
                let checked = match anchor {
                    Some(any) => self.value_beside(e, any),
                    None if quiet => self.value(e, Expectation::Poisoned),
                    None => self.value(e, Expectation::None),
                };
                anchor = self.widen_anchor(anchor, type_of(&checked));
                slots[i] = Some(checked);
            }
        }
        let first = slots.iter().flatten().next().and_then(type_of);
        let literal = literal_type(first);
        for (i, e) in elements.iter().enumerate() {
            if ranks[i] == Rank::Literal {
                // An `optional` element may be `None`, which the literal's type does not hold.
                let want = if optional[i] {
                    as_optional(literal)
                } else {
                    literal
                };
                slots[i] = Some(self.value(e, Expectation::Required(want, Origin::None)));
            }
        }
        let mut failed = slots.iter().flatten().find_map(|c| known_type(c).err());
        let mut joined: Option<Type> = None;
        if failed.is_none() {
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
                    let d = self.leaves_any(at_span, source, want, got, Origin::None);
                    failed = Some(self.diagnostics.error(d));
                }
                ObjectJoin::Not => {
                    (joined, failed) = self.fold_elements(elements, &typed, (&ranks, first));
                }
            }
        }
        let Some(joined) = joined.filter(|_| failed.is_none()) else {
            let waiting = elements
                .iter()
                .zip(&ranks)
                .filter(|&(_, &r)| r == Rank::Waits);
            let mut reported = failed;
            for (i, (e, _)) in waiting.enumerate() {
                let expected = if i > 0 || failed.is_some() || quiet {
                    Expectation::Poisoned
                } else {
                    Expectation::None
                };
                // Only its own diagnostics matter, and the proof of the first that fails.
                let value = self.value(e, expected);
                reported = reported.or_else(|| value.as_ref().err().map(Poisoned::reported));
            }
            // With no typed element, the first element that waits holds the proof.
            let reported = reported.unwrap_or_else(|| self.proof(span)); // proof: quiet reader
            return Err(Poisoned::untyped(reported));
        };
        let want = as_optional(joined);
        for (i, e) in elements.iter().enumerate() {
            if ranks[i] == Rank::Waits {
                // What waits and makes no optional by itself (`Ranked::optional`) may still give a
                // type that the join does not take, such as `[]`.
                slots[i] = Some(if optional[i] {
                    self.value(e, Expectation::Required(want, Origin::None))
                } else {
                    mark(self.operand(e, Expectation::Required(joined, Origin::None)))
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
                    Ok(e) if e.ty != element => {
                        self.record_shown_any(e.ty, element);
                        Ok(widen(e, element))
                    }
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
    /// a hint (see `Expectation::Hint`), as the branch of an `if` after an `any` branch is. So the
    /// payload of a `Some` converts to `any`.
    fn value_beside(&mut self, e: &ast::Expr, any: Type) -> Checked {
        #[cfg(test)]
        super::reached("value_beside");
        let hint = Expectation::Hint {
            want: as_optional(any),
            siblings: any,
            origin: Origin::None,
        };
        self.value(e, hint)
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
    /// the types fold in source order. A report names the element that gave the array the type
    /// it expects. That is the first of that type, the concrete elements before the literal ones
    /// (`ranks`). A literal that took it from `first`, the first concrete element, is skipped (see
    /// `Rank::took`).
    fn fold_elements(
        &mut self,
        elements: &[ast::Expr],
        typed: &[(usize, Type)],
        (ranks, first): (&[Rank], Option<Type>),
    ) -> (Option<Type>, Option<Reported>) {
        let report = |c: &mut Self, at: usize, want: Type, got: Type| {
            let origin = (typed.iter())
                .filter(|&&(i, ty)| ty == want && !first.is_some_and(|s| ranks[i].took(s)))
                .min_by_key(|&&(i, _)| ranks[i])
                .map_or(Origin::None, |&(i, _)| Origin::Element {
                    at: elements[i].span,
                    ty: want,
                });
            let d = c.mismatch(elements[at].span, Some(&elements[at]), want, got, origin);
            Some(c.diagnostics.error(d))
        };
        match self.any_join_all(typed) {
            AnyJoin::Joined { ty } => {
                self.instantiates = true;
                return (Some(ty), None);
            }
            AnyJoin::Fails {
                earlier: (_, want),
                later: (at, got),
            } => {
                return (None, report(self, at, want, got));
            }
            AnyJoin::Leaves {
                ty,
                member: (at, got),
            } => {
                let d = self.leaves_any(
                    elements[at].span,
                    Some(&elements[at]),
                    ty,
                    got,
                    Origin::None,
                );
                return (None, Some(self.diagnostics.error(d)));
            }
            AnyJoin::NoAny => {}
        }
        let mut joined: Option<Type> = None;
        for &(i, ty) in typed {
            match joined {
                Some(t) if fits(t, ty) => {}
                Some(t) if !fits(ty, t) => {
                    return (joined, report(self, i, t, ty));
                }
                _ => joined = Some(ty),
            }
        }
        // The join can grow past an earlier element: each converts through one layer only.
        if let Some(t) = joined
            && let Some(&(i, ty)) = typed.iter().find(|&&(_, ty)| !fits(t, ty))
        {
            return (joined, report(self, i, t, ty));
        }
        (joined, None)
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
            Err(failed) => Err(Poisoned {
                ty: (!mismatched).then_some(ty),
                mismatch: mismatched,
                ..Poisoned::untyped(failed.reported())
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
        expected: Expectation,
    ) -> Checked {
        let element = expected_element(expected.ty());
        // Without an element type, a `None` or `[]` cannot be inferred, unless `expected` is
        // `Poisoned`.
        let hint = match element {
            Some(element) => Expectation::Required(element, expected.origin()),
            None if expected == Expectation::Poisoned => Expectation::Poisoned,
            None => Expectation::None,
        };
        // `operand`, split so the class rule runs between the check and the wrap.
        let checked = self.value(value, hint);
        let class = type_of(&checked).map(innermost).filter(|ty| {
            matches!(
                ty.kind(),
                TypeKind::Class(..) | TypeKind::Param(..) | TypeKind::Assoc(..) | TypeKind::Any(..)
            ) && !self.none_written(value)
        });
        let mut same = None;
        if let Some(class) = class {
            let any = matches!(class.kind(), TypeKind::Any(..));
            // An `any` of a class-only interface holds an object, as a class is one.
            let may_be_class = matches!(class.kind(), TypeKind::Param(..) | TypeKind::Assoc(..))
                || (any && !self.conforms(class, self.lang.any_object, TypeArgs::NONE));
            same = Some(self.error(DiagnosticKind::SameObject {
                class: self.type_name(class),
                at: value.span,
                param: may_be_class,
                any,
            }));
        }
        let (checked, mismatched) = match element {
            Some(_) => self.against(value, checked, hint),
            None => (checked, false),
        };
        let (len, len_mismatched) =
            self.operand(len, Expectation::Required(Type::I64, Origin::None));
        let failed = mismatched || same.is_some() || len_mismatched;
        let ty = type_of(&checked).map(Type::array);
        match (checked, len, ty) {
            (Ok(value), Ok(len), Some(ty)) if !failed => {
                Ok(self.spilled(span, ty, vec![value, len], |operands| {
                    let (value, len) = pair(operands);
                    tir::ExprKind::ArrayFill { value, len }
                }))
            }
            (Err(part), _, ty) | (_, Err(part), ty) => {
                Err(Poisoned::new(ty.filter(|_| !failed), part.reported()))
            }
            // Both parts check, so the class rule failed, and reported.
            _ => Err(Poisoned::untyped(same.expect(
                "an array fill whose parts check should fail by its class rule",
            ))),
        }
    }

    /// `base[index]`: `base` is an array, not an optional one, and `index` an `i64`.
    pub(super) fn index(&mut self, span: Span, base: &ast::Expr, index: &ast::Expr) -> Checked {
        if let ast::ExprKind::Range(range) = &index.kind {
            return self.range_index(span, base, range);
        }
        let outer = std::mem::replace(&mut self.body.indexed, true);
        let checked = self.value(base, Expectation::None);
        // An optional array is reported, and its element goes on typed, but no code reads it.
        let optional = type_of(&checked).is_some_and(is_optional_array);
        let class_write = (self.body.write_chain.is_some() && optional)
            .then(|| self.class_unwrap(base, &checked))
            .flatten();
        let (element, report) = match (class_write, type_of(&checked)) {
            (Some(unwrap), Some(ty)) => {
                let reported = self.diagnostics.error(optional_array(base, unwrap));
                (optional_element(ty), Some(reported))
            }
            _ => self.element(
                base,
                (type_of(&checked), checked.is_err()),
                IndexUsage::Read,
            ),
        };
        // The index is not part of the chain that it ends.
        self.body.indexed = false;
        let chain = std::mem::take(&mut self.body.write_chain);
        // A base with no element, such as a string, takes no `i64` index either.
        let origin = if element.is_some() {
            Origin::Index
        } else {
            Origin::None
        };
        let (index, mismatched) = self.operand(index, Expectation::Required(Type::I64, origin));
        self.body.indexed = outer;
        self.body.write_chain = chain;
        match (checked, index, element) {
            (Ok(base), Ok(index), Some(element)) if !optional => {
                Ok(self.spilled(span, element, vec![base, index], |operands| {
                    let (base, index) = pair(operands);
                    tir::ExprKind::Index { base, index }
                }))
            }
            // A failed index is untyped: a repair to an index differs from one to a range.
            (base, index, element) => {
                let ty = element.filter(|_| !mismatched && type_of(&index).is_some());
                let reported = match (base, index) {
                    (Err(part), _) | (_, Err(part)) => part.reported(),
                    // A base that checks and gives no element, or an optional one, is reported.
                    (Ok(_), Ok(_)) => {
                        report.expect("an index whose parts check should fail by its base's report")
                    }
                };
                Err(Poisoned::new(ty, reported))
            }
        }
    }

    /// The help of the E0062 of `base`, an optional array under the chain of a write. It is
    /// `None` unless the checked value `checked` is a `?.` that takes a class object out of its
    /// optional. The object's field is a place, so the help unwraps the object. So does
    /// `Checker::optional_array_target` for an element place.
    fn class_unwrap(&self, base: &ast::Expr, checked: &Checked) -> Option<UnwrapHelp> {
        let Ok(tir::Expr {
            kind: tir::ExprKind::IfSome { bind, .. },
            ..
        }) = checked
        else {
            return None;
        };
        if !matches!(self.body.locals[bind.0].ty.kind(), TypeKind::Class(..)) {
            return None;
        }
        Some(match &base.kind {
            ast::ExprKind::Field {
                base: object,
                safe: true,
                ..
            } if self.one_line(object.span) => {
                UnwrapHelp::Object(self.source.text(object.span).to_string())
            }
            _ => UnwrapHelp::Nothing,
        })
    }

    /// The element type of `base`, which is indexed and has type `ty` (`None` is `Error`). An
    /// optional array is reported and gives the element of its payload. A string, or a value that
    /// is no array, is reported and gives `Error`. With the proof of the report, when there is one.
    /// A base that `failed` under the chain of a write to a struct has reported that write.
    pub(super) fn element(
        &mut self,
        base: &ast::Expr,
        (ty, failed): (Option<Type>, bool),
        usage: IndexUsage,
    ) -> (Option<Type>, Option<Reported>) {
        let Some(ty) = ty else {
            return (None, None);
        };
        let d: Diagnostic = match ty.kind() {
            TypeKind::Array(element) => return (Some(element), None),
            TypeKind::Optional(payload) if matches!(payload.kind(), TypeKind::Array(_)) => {
                let element = optional_element(ty);
                // A struct element under the chain of a write, after a `?.`: part 1 cannot write
                // through that `?.`, and an unwrap gives a copy.
                let on_struct = self.body.write_chain.is_some()
                    && element.is_some_and(|e| is_struct(innermost(e)));
                if on_struct && failed {
                    return (element, None);
                }
                let chain = (self.body.write_chain)
                    .filter(|&(_, change)| on_struct && change != Change::MutArg)
                    .zip(first_safe(base));
                let unwrap = if on_struct {
                    UnwrapHelp::Nothing
                } else {
                    UnwrapHelp::It
                };
                let reported = match chain {
                    Some((write, object)) => {
                        let d = match self.chain_write(write, base, object) {
                            DiagnosticKind::ChainWriteOnStruct { at, .. } => {
                                DiagnosticKind::ChainWriteOnStruct { at, replace: None }
                            }
                            d => d,
                        };
                        self.error(d)
                    }
                    None => self.diagnostics.error(optional_array(base, unwrap)),
                };
                return (element, Some(reported));
            }
            _ if ty == Type::STRING => DiagnosticKind::StringIndex {
                at: base.span,
                base: self.source.line_text(base.span),
                usage,
            }
            .into(),
            _ => DiagnosticKind::CannotIndex {
                ty: self.type_name(ty),
                at: base.span,
                range: false,
            }
            .into(),
        };
        (None, Some(self.diagnostics.error(d)))
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
            if let Some(&reported) = self.rejected_methods.get(&method.name) {
                return self.call_of_rejected_method(method, args, reported);
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
            return self.rejected_lookup(d, method, args);
        };
        if args.len() != arity {
            let d = DiagnosticKind::ArgumentCount {
                name: self.method_name(ty, name),
                want: arity,
                found: args.len(),
                at: span,
            };
            return self.rejected(d.into(), args, Some(Type::I64));
        }
        let kind = receiver
            .map(|r| tir::ExprKind::Len(Box::new(r)))
            .map_err(|failed| failed.reported());
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
            return self.wrong_count(span, name, params.len(), args, None);
        }
        let stands = self.owner_args(func, ty).get().to_vec();
        let mut own_mistake = None;
        for (arg, param) in args.iter().zip(params) {
            let want = param.and_then(|param| self.subst(param, &stands));
            let (argument, mismatched) =
                self.operand(&arg.value, Expectation::or_poisoned(want, Origin::None));
            // An argument with no type has failed, but a type parameter keeps its report beside
            // the error of such an argument.
            let failed = type_of(&argument).is_none() && !want.is_some_and(Type::has_param);
            if mismatched || failed {
                own_mistake = own_mistake.or_else(|| argument.err().map(|part| part.reported()));
            }
        }
        let reported = match own_mistake {
            Some(reported) => reported,
            None => self.report_block_bound(extension, ty, span, method.name.as_str()),
        };
        Err(Poisoned::untyped(reported))
    }

    /// A call of the method `method` that a rejected `extend` block may have given (see
    /// `Checker.rejected_methods`): poisoned, with no report of its own. `reported` is the proof
    /// of the block's report.
    pub(super) fn call_of_rejected_method(
        &mut self,
        method: &ast::Ident,
        args: &[ast::Arg],
        reported: Reported,
    ) -> Checked {
        self.body.rejected_calls.insert(method.span);
        self.labels(args);
        self.stray(args);
        Err(Poisoned::untyped(reported))
    }

    /// Reports `d` for a method call that cannot be checked, and its arguments' own errors. `ret`
    /// is the type of the call when its method fixes it.
    pub(super) fn rejected(
        &mut self,
        d: Diagnostic,
        args: &[ast::Arg],
        ret: Option<Type>,
    ) -> Checked {
        let reported = self.diagnostics.error(d);
        self.labels(args);
        self.stray(args);
        Err(Poisoned::new(ret, reported))
    }
}

/// "`m` may be `None`", at the optional array `base` that an index or a `for` needs unwrapped.
/// `unwrap` says what the help unwraps.
pub(super) fn optional_array(base: &ast::Expr, unwrap: UnwrapHelp) -> Diagnostic {
    DiagnosticKind::MayBeNone {
        value: path(base),
        at: base.span,
        access: None,
        write: false,
        peel: None,
        unwrap,
    }
    .into()
}

/// Whether `ty` is `[T]?`.
pub(super) fn is_optional_array(ty: Type) -> bool {
    optional_element(ty).is_some()
}

/// `T`, when `ty` is `[T]?`.
pub(super) fn optional_element(ty: Type) -> Option<Type> {
    let TypeKind::Optional(payload) = ty.kind() else {
        return None;
    };
    match payload.kind() {
        TypeKind::Array(element) => Some(element),
        _ => None,
    }
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
pub(super) fn innermost(mut ty: Type) -> Type {
    while let TypeKind::Optional(inner) = ty.kind() {
        ty = inner;
    }
    ty
}
