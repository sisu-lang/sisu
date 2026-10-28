//! Array expressions: literals and their element join, `[v; n]`, indexing and methods.

use super::expr::{fits, literal_type, no_member, none_literal, path, rank, typed};
use super::interface::Operation;
use super::unused_names::Used;
use super::{Checked, Checker, Poisoned, UNKNOWN, as_optional, payload, type_of, wrap};
use crate::ast;
use crate::diagnostic::{Definition, Diagnostic, DiagnosticKind, Literal};
use crate::source::Span;
use crate::tir::{self, Type, TypeKind};

/// The methods of an array, with their parameter counts.
pub(super) const METHODS: [(&str, usize); 3] = [("len", 0), ("sum", 0), ("contains", 1)];

impl Checker<'_> {
    /// `[a, b, ...]`. With an expected `[E]` or `[E]?`, each element is checked against `E`.
    /// Without one, the elements join (see `joined`). A `[]` takes its type from `expected`.
    pub(super) fn array_lit(
        &mut self,
        span: Span,
        elements: &[ast::Expr],
        expected: Option<Type>,
    ) -> Checked {
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
        self.joined(span, elements, quiet)
    }

    /// The elements of a literal with no expected array type, joined in three waves by `rank`:
    /// the concrete elements; the integer literals, typed beside the first concrete element;
    /// then each `None` and `[]`, against the join made optional. The join runs in source
    /// order as `branches` joins, and a `None` makes the element type optional. With no typed
    /// element, the first `None` or `[]` cannot be inferred, which `quiet` (the context failed)
    /// leaves unreported. With `quiet`, each concrete element is a quiet value too (see
    /// `Body::quiet_at`). An element whose type is `Error` poisons the array.
    fn joined(&mut self, span: Span, elements: &[ast::Expr], quiet: bool) -> Checked {
        let mut slots: Vec<Option<Checked>> = elements.iter().map(|_| None).collect();
        let outer = self.body.quiet_at;
        for (slot, e) in slots.iter_mut().zip(elements) {
            if rank(e) == 0 {
                self.body.quiet_at = if quiet { Some(e.span) } else { outer };
                *slot = Some(self.value(e, None));
            }
        }
        self.body.quiet_at = outer;
        let literal = literal_type(slots.iter().flatten().next().and_then(type_of));
        for (slot, e) in slots.iter_mut().zip(elements) {
            if rank(e) == 1 {
                *slot = Some(self.value(e, Some(literal)));
            }
        }
        let mut failed = slots.iter().flatten().any(|c| type_of(c).is_none());
        let mut joined: Option<Type> = None;
        if !failed {
            for (slot, e) in slots.iter().zip(elements) {
                let Some(ty) = slot.as_ref().and_then(type_of) else {
                    continue;
                };
                match joined {
                    Some(t) if fits(t, ty) => {}
                    Some(t) if !fits(ty, t) => {
                        let d = self.mismatch(e.span, Some(e), t, ty);
                        self.diagnostics.push(d);
                        failed = true;
                        break;
                    }
                    _ => joined = Some(ty),
                }
            }
            // The join can grow past an earlier element: each converts through one layer only.
            if let Some(t) = joined.filter(|_| !failed) {
                let short = slots.iter().zip(elements).find_map(|(slot, e)| {
                    let ty = slot.as_ref().and_then(type_of)?;
                    (!fits(t, ty)).then_some((e, ty))
                });
                if let Some((e, ty)) = short {
                    let d = self.mismatch(e.span, Some(e), t, ty);
                    self.diagnostics.push(d);
                    failed = true;
                }
            }
        }
        let Some(joined) = joined.filter(|_| !failed) else {
            for (i, e) in elements.iter().filter(|e| rank(e) == 2).enumerate() {
                let expected = (i > 0 || failed || quiet).then_some(UNKNOWN);
                // Only its own diagnostics matter.
                drop(self.value(e, expected));
            }
            return Err(Poisoned::error());
        };
        let want = as_optional(joined);
        for (slot, e) in slots.iter_mut().zip(elements) {
            if rank(e) == 2 {
                *slot = Some(self.value(e, Some(want)));
            }
        }
        let element = if elements.iter().any(none_literal) {
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
                    Ok(e) if e.ty != element => Ok(wrap(e, element)),
                    checked => checked,
                }
            })
            .collect();
        self.array(span, element, checked, mismatched)
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
            _ => Err(Poisoned::of((!mismatched).then_some(ty))),
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
                TypeKind::Class(..) | TypeKind::Param(..) | TypeKind::Assoc(..)
            ) && !none_literal(value)
        });
        if let Some(class) = class {
            self.report(DiagnosticKind::SameObject {
                class: self.type_name(class),
                at: value.span,
                param: matches!(class.kind(), TypeKind::Param(..) | TypeKind::Assoc(..)),
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
        let checked = self.value(base, None);
        let element = self.element(base, type_of(&checked));
        // The index is not part of an assignment target's chain; `expr` restores the flag.
        self.body.in_target = false;
        let (index, mismatched) = self.operand(index, Some(Type::I64));
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

    /// `receiver.method(args)` on `receiver`, an array of type `ty`, written at `at`. The
    /// `METHODS`: `len()`, `sum()` on an `[i64]` or an `[f64]`, and `contains(x)`, whose argument
    /// is checked as an element, which has `==` unless its type names a type parameter.
    pub(super) fn array_method(
        &mut self,
        span: Span,
        (receiver, at): (Checked, Span),
        ty: Type,
        method: &ast::Ident,
        args: &[ast::Arg],
    ) -> Checked {
        let TypeKind::Array(element) = ty.kind() else {
            unreachable!("`method_call` should pass an array type to `array_method`");
        };
        let name = method.name.as_str();
        let Some(&(_, arity)) = METHODS.iter().find(|(m, _)| *m == name) else {
            let names = METHODS.iter().map(|&(m, _)| m);
            let d = no_member(Definition::Method, self.type_name(ty), method, names);
            return self.rejected(d, args);
        };
        if args.len() != arity {
            let d = DiagnosticKind::ArgumentCount {
                name: name.to_string(),
                want: arity,
                found: args.len(),
                at: span,
            };
            return self.rejected(d.into(), args);
        }
        match name {
            "len" => {
                let kind = receiver.ok().map(|r| tir::ExprKind::Len(Box::new(r)));
                typed(span, Type::I64, false, kind)
            }
            "sum" if !matches!(element, Type::I64 | Type::F64) => {
                let d = DiagnosticKind::SumNeedsNumbers {
                    ty: self.type_name(ty),
                    at: method.span,
                };
                self.rejected(d.into(), args)
            }
            "sum" => {
                let kind = receiver.ok().map(|r| tir::ExprKind::Sum(Box::new(r)));
                typed(span, element, false, kind)
            }
            _ if element.has_param()
                && self.operation(element, self.lang.eq) == Operation::Missing =>
            {
                let add = self.lacking_bound(element, self.lang.eq);
                let d = DiagnosticKind::ParamLacks {
                    ty: self.type_name(element),
                    op: "==".to_string(),
                    at,
                    contains: true,
                    add,
                };
                self.rejected(d.into(), args)
            }
            _ => {
                let labeled = self.labels(args);
                // A class compares field by field, as `==` does; a `None` reads no field.
                if !none_literal(&args[0].value) {
                    self.record(Used::Equal(element), span);
                }
                let (value, mismatched) = self.operand(&args[0].value, Some(element));
                match (receiver, value) {
                    (Ok(base), Ok(value)) if !labeled => {
                        Ok(
                            self.spilled(span, Type::BOOL, vec![base, value], |operands| {
                                let (base, value) = pair(operands);
                                tir::ExprKind::Contains { base, value }
                            }),
                        )
                    }
                    _ => Err(Poisoned::of(
                        (!labeled && !mismatched).then_some(Type::BOOL),
                    )),
                }
            }
        }
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
