//! Array expressions: literals and their element join, `[v; n]`, indexing and methods.

use super::expr::{fits, literal_type, no_member, path, typed};
use super::generic::{Rank, mark};
use super::{Checked, Checker, Declared, Poisoned, UNKNOWN, as_optional, payload, type_of, wrap};
use crate::ast;
use crate::diagnostic::{Definition, Diagnostic, DiagnosticKind, Literal};
use crate::source::Span;
use crate::tir::{self, Type, TypeKind};

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
        for (i, e) in elements.iter().enumerate() {
            if ranks[i] == Rank::Concrete {
                self.body.quiet_at = if quiet { Some(e.span) } else { outer };
                slots[i] = Some(self.value(e, None));
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
                TypeKind::Class(..) | TypeKind::Param(..) | TypeKind::Assoc(..)
            ) && !self.none_written(value)
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
            return self.unmet_block_bound(span, func, ty, args);
        }
        let Some(&(_, arity)) = built_in else {
            let prelude = self.prelude_method_names(ty);
            let targets = self.prelude_targets(method.name);
            let d = if !prelude.contains(&name) && !targets.is_empty() {
                DiagnosticKind::SumNeedsNumbers {
                    method: name.to_string(),
                    ty: self.type_name(ty),
                    targets: targets.into_iter().map(|t| self.type_name(t)).collect(),
                    at: method.span,
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
                name: name.to_string(),
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
        args: &[ast::Arg],
    ) -> Checked {
        let sig = &self.signatures[func.0];
        let (name, params) = (sig.name.clone(), sig.params.clone());
        let extension = sig
            .extension
            .expect("a method whose block bounds fail should have a block");
        self.labels(args);
        if params.len() != args.len() {
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
            self.report_block_bound(extension, ty, span);
        }
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
