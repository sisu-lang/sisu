//! Ranges: the E0023 report of a range where it is not allowed, the range index of an array, and
//! the E0021 report of an assignment to a range index.

use super::array::must_unwrap;
use super::call::Dot;
use super::string::single_term;
use super::{Checked, Checker, Expectation, Origin, Poisoned, known_type};
use crate::ast::{self, ExprKind};
use crate::diagnostic::{
    Diagnostic, DiagnosticKind, GetCall, IndexUsage, MissingBound, RangeForm, RangePlace,
    RangePush, RangeTarget, RangeWrite, UnwrapHelp,
};
use crate::parser::is_place;
use crate::source::Span;
use crate::tir::{self, Type, TypeKind};

/// The outermost range index of the assignment target `target`, and the range it holds. `parent`
/// is the index of that range index, as in `xs[1..][0]`, if the target has one.
struct RangeIndex<'a> {
    index: &'a ast::Expr,
    base: &'a ast::Expr,
    range: &'a ast::Range,
    parent: Option<&'a ast::Expr>,
}

/// The range index in `target`, a chain of indexes, if one is there.
fn range_index(target: &ast::Expr) -> Option<RangeIndex<'_>> {
    let mut parent = None;
    let mut index = target;
    while let ExprKind::Index { base, index: at } = &index.kind {
        if let ExprKind::Range(range) = &at.kind {
            return Some(RangeIndex {
                index,
                base,
                range,
                parent,
            });
        }
        parent = Some(index);
        index = base;
    }
    None
}

impl Checker<'_> {
    /// E0023 for `range`, which stands at `place` and is not an index.
    pub(super) fn range_outside(range: &ast::Range, place: RangePlace) -> Diagnostic {
        let form = match (&range.start, &range.end) {
            (None, None) => RangeForm::MissingBound {
                missing: if range.inclusive {
                    MissingBound::BothInclusive
                } else {
                    MissingBound::Both
                },
                place,
            },
            (None, Some(_)) => RangeForm::MissingBound {
                missing: MissingBound::Start,
                place,
            },
            (Some(_), None) => RangeForm::MissingBound {
                missing: MissingBound::End,
                place,
            },
            (Some(_), Some(_)) => RangeForm::Anywhere,
        };
        DiagnosticKind::RangeOutsideFor {
            at: range.span,
            form,
        }
        .into()
    }

    /// The `get` of the array type `array`, called at `span` with a range among `args`: E0055 for
    /// a label, E0053 for a count that is not one, and E0032 for each range, which `get` takes as
    /// an `i64`. The call is poisoned, so no error follows. The other arguments, and the bounds
    /// of a range, report their own mistakes. `base` is the receiver, when the help of E0032 can
    /// write it: `base[a..b]` needs an array that no `?.` or optional touched.
    pub(super) fn array_get_range(
        &mut self,
        (span, array): (Span, Type),
        base: Option<&ast::Expr>,
        args: &[ast::Arg],
    ) -> Checked {
        let mut reported = self.labels(args);
        let base = base.map(|base| self.source.line_text(base.span));
        for arg in args {
            if let ExprKind::Range(range) = &arg.value.kind {
                let error = self.error(DiagnosticKind::ArrayGetTakesIndex {
                    at: range.span,
                    base: base.clone(),
                });
                reported.get_or_insert(error);
                self.bounds(range);
            } else {
                // Only its diagnostics matter.
                drop(self.expr(&arg.value, Expectation::Poisoned));
            }
        }
        // A comparison misread as a call explains the count, which is known after the arguments.
        if args.len() != 1 && self.misread_in(args).is_none() {
            let error = self.error(DiagnosticKind::ArgumentCount {
                name: self.method_name(array, "get"),
                want: 1,
                found: args.len(),
                at: span,
            });
            reported.get_or_insert(error);
        }
        Err(Poisoned::untyped(
            reported.expect("a range among the arguments should be reported"),
        ))
    }

    /// The range index `base[range]` at `span`. `base[..]` and `a..=` are errors for good: E0023,
    /// with the root of `base` stamped as a `push` onto the index would stamp it, so no warning
    /// follows. On an array or a string it is a `Slice` of the same type, each bound an `i64`. A
    /// base that is neither is E0068, and an optional one E0062, with the bounds not checked, for
    /// the one report.
    pub(super) fn range_index(
        &mut self,
        span: Span,
        base: &ast::Expr,
        range: &ast::Range,
    ) -> Checked {
        if error_for_good(range) {
            self.stamp_push(base);
            drop(self.value(base, Expectation::None));
            let d = self.index_outside(base, range, None);
            return Err(Poisoned::untyped(self.diagnostics.error(d)));
        }
        let outer = std::mem::replace(&mut self.body.indexed, true);
        let checked = self.value(base, Expectation::None);
        // The bounds are not part of the chain that the index ends.
        self.body.indexed = false;
        let ty = match known_type(&checked) {
            Ok(ty) => ty,
            Err(reported) => {
                // The base has reported itself; the bounds report their own mistakes.
                self.bounds(range);
                self.body.indexed = outer;
                return Err(Poisoned::untyped(reported));
            }
        };
        if !matches!(ty.kind(), TypeKind::Array(_)) && ty != Type::STRING {
            self.body.indexed = outer;
            let d = self.cannot_slice(base, ty);
            return Err(Poisoned::untyped(self.diagnostics.error(d)));
        }
        let sliced = self.slice_of(span, (checked, ty), range, false);
        self.body.indexed = outer;
        sliced
    }

    /// The `Slice` of `range` over `checked`, a string or an array of type `ty`: with `safe`, a
    /// string's `get`, whose type is `ty?`. Each bound is an `i64`.
    fn slice_of(
        &mut self,
        span: Span,
        (checked, ty): (Checked, Type),
        range: &ast::Range,
        safe: bool,
    ) -> Checked {
        let (start, end, mismatched) = self.bounds(range);
        let (has_start, has_end) = (start.is_some(), end.is_some());
        let result = if safe { Type::optional(ty) } else { ty };
        let parts = [Some(checked), start, end].into_iter().flatten();
        let operands = match parts.collect::<Result<Vec<_>, _>>() {
            Ok(operands) => operands,
            Err(failed) => {
                let ty = (!mismatched).then_some(result);
                return Err(Poisoned::new(ty, failed.reported()));
            }
        };
        Ok(self.spilled(span, result, operands, |operands| {
            let mut operands = operands.into_iter().map(Box::new);
            let mut next = || {
                operands
                    .next()
                    .expect("`spilled` should return each operand")
            };
            let base = next();
            let start = has_start.then(&mut next);
            let end = has_end.then(&mut next);
            tir::ExprKind::Slice {
                base,
                start,
                end,
                inclusive: range.inclusive,
                safe,
                span: range.span,
            }
        }))
    }

    /// `receiver.get(args)` on a string, `receiver_expr` its source, or `receiver?.get(args)` when
    /// `dot` is `Safe`: the string's substring in a range written in the call, an
    /// `ast::ExprKind::Range`, as a `Slice` that is `safe` and has type `string?`. An argument
    /// that is no range is E0032; `..`, `..=` and `a..=` are E0023, as they are in an index; a
    /// wrong count is E0053; a label is E0055.
    pub(super) fn string_get(
        &mut self,
        (span, dot): (Span, Dot),
        (receiver, receiver_expr): (Checked, &ast::Expr),
        args: &[ast::Arg],
    ) -> Checked {
        let labeled = self.labels(args);
        // A `?.` that is `safe` and has a receiver that failed is itself an error (E0049, E0062).
        let call = match (dot, &receiver) {
            (Dot::Plain, _) => GetCall::Plain,
            (Dot::Payload, _) => GetCall::Payload,
            (Dot::Safe, Ok(_)) => GetCall::Safe,
            (Dot::Safe, Err(_)) => GetCall::SafeFailed,
        };
        let [arg] = args else {
            let d = DiagnosticKind::ArgumentCount {
                name: self.method_name(Type::STRING, "get"),
                want: 1,
                found: args.len(),
                at: span,
            };
            let reported = self.diagnostics.error(d);
            self.get_arguments(args);
            return Err(Poisoned::untyped(reported));
        };
        let ExprKind::Range(range) = &arg.value.kind else {
            // The argument reports its own mistakes, else it is a value that is no range.
            let (_, failed) = self.checked_alone(&arg.value);
            let reported = failed.unwrap_or_else(|| {
                self.error(DiagnosticKind::StringGetNeedsRange {
                    at: arg.value.span,
                    base: self.source.quote(receiver_expr.span),
                    call,
                })
            });
            return Err(Poisoned::untyped(reported));
        };
        if error_for_good(range) {
            self.stamp_push(receiver_expr);
            let d = self.index_outside(receiver_expr, range, Some(call));
            return Err(Poisoned::untyped(self.diagnostics.error(d)));
        }
        let sliced = self.slice_of(span, (receiver, Type::STRING), range, true);
        // A label is E0055; the call is poisoned, with its type, so that no error follows.
        match (sliced, labeled) {
            (Ok(_), Some(labeled)) => Err(Poisoned::typed(Type::optional(Type::STRING), labeled)),
            (sliced, _) => sliced,
        }
    }

    /// Checks the arguments of a string's `get` that is not checked as a call, for their own
    /// mistakes: a range is allowed there, so only its bounds report.
    pub(super) fn get_arguments(&mut self, args: &[ast::Arg]) {
        for arg in args {
            if let ExprKind::Range(range) = &arg.value.kind {
                self.bounds(range);
            } else {
                drop(self.expr(&arg.value, Expectation::Poisoned));
            }
        }
    }

    /// E0062 for the optional array or string `base`, else E0068, of the range index of `base`,
    /// which has type `ty` and is neither an array nor a string.
    fn cannot_slice(&self, base: &ast::Expr, ty: Type) -> Diagnostic {
        match ty.kind() {
            TypeKind::Optional(inner)
                if inner == Type::STRING || matches!(inner.kind(), TypeKind::Array(_)) =>
            {
                must_unwrap(base, UnwrapHelp::It)
            }
            _ => DiagnosticKind::CannotIndex {
                ty: self.type_name(ty),
                at: base.span,
                range: true,
            }
            .into(),
        }
    }

    /// The checked bounds of `range`, each against `i64`, and whether one was a mistake.
    fn bounds(&mut self, range: &ast::Range) -> (Option<Checked>, Option<Checked>, bool) {
        let mut mismatched = false;
        let mut bound = |checker: &mut Self, bound: &Option<Box<ast::Expr>>| {
            bound.as_ref().map(|bound| {
                let (checked, wrong) =
                    checker.operand(bound, Expectation::Required(Type::I64, Origin::None));
                mismatched |= wrong;
                checked
            })
        };
        let start = bound(self, &range.start);
        let end = bound(self, &range.end);
        (start, end, mismatched)
    }

    /// E0023 for `range`, which is an error for good (`error_for_good`) in the index of `base`, or
    /// with `get` in the argument of its `get`: `Some(call)`, where `call` says how it is written.
    fn index_outside(
        &self,
        base: &ast::Expr,
        range: &ast::Range,
        get: Option<GetCall>,
    ) -> Diagnostic {
        let form = match (&range.start, &range.end) {
            (None, None) if let Some(call) = get => RangeForm::NoBoundsGet {
                base: self.source.quote(base.span),
                inclusive: range.inclusive,
                call,
            },
            (None, None) => RangeForm::NoBounds {
                base: self.source.quote(base.span),
                inclusive: range.inclusive,
            },
            (Some(start), None) if range.inclusive => RangeForm::NoEnd {
                quote: (self.source.quote(range.span)).zip(self.source.quote(start.span)),
            },
            _ => RangeForm::Anywhere,
        };
        DiagnosticKind::RangeOutsideFor {
            at: range.span,
            form,
        }
        .into()
    }

    /// `target = value`, or `target op= value` when `op` is given, where `target` holds a range
    /// index: E0021, or E0023 when the range has no bounds. The range index gives a new value, so
    /// the write would be lost. The root is stamped as a written one, so no warning follows. The
    /// bounds are not checked, for the one report; `value` is, for its own mistakes. `None` when
    /// `target` holds no range index.
    pub(super) fn assign_to_range(
        &mut self,
        target: &ast::Expr,
        op: Option<ast::BinaryOp>,
        value: &ast::Expr,
    ) -> Option<Result<tir::Stmt, Poisoned>> {
        let found = range_index(target)?;
        self.stamp_push(target);
        // The base is read, as `xs[0..2]` reads it, and reports its own mistakes.
        let base_type = known_type(&self.value(found.base, Expectation::None));
        let range = found.range;
        // `xs[..][i]` is an element of `xs`, which E0021 helps; `xs[..]` is `xs`.
        let element_of_all = found.parent.is_some() && range.start.is_none() && !range.inclusive;
        let d = if error_for_good(range) && !element_of_all {
            Ok(self.index_outside(found.base, range, None))
        } else {
            let place = is_place(found.base) && self.rooted(found.base);
            base_type.map(|ty| self.range_write(&found, op, (target, value), (ty, place)))
        };
        let reported = d.map_or_else(|reported| reported, |d| self.diagnostics.error(d));
        drop(self.expr(value, Expectation::Poisoned));
        Some(Err(Poisoned::typed(Type::UNIT, reported)))
    }

    /// E0021 for the range index `found` in `target = value`, by the type `ty` of its base, or
    /// E0023 where no help fits yet. When the base is no `place`, writing to it back would be
    /// E0021 again, so the help is to store it first.
    fn range_write(
        &self,
        found: &RangeIndex<'_>,
        op: Option<ast::BinaryOp>,
        (target, value): (&ast::Expr, &ast::Expr),
        (ty, place): (Type, bool),
    ) -> Diagnostic {
        let write = match (ty.kind(), found.parent) {
            (TypeKind::Array(_), None) => RangeWrite::Array,
            (TypeKind::Array(_), Some(parent)) => {
                let op = op.map_or_else(|| "=".to_string(), |op| format!("{op}="));
                let store = format!(" {op} {}", self.source.line_text(value.span));
                self.range_element(found, (parent, target), store)
            }
            _ if ty == Type::STRING && found.parent.is_none() => match op {
                None => RangeWrite::Replace {
                    value: self.term(value),
                },
                Some(ast::BinaryOp::Add) => RangeWrite::Append {
                    value: self.source.line_text(value.span),
                    grouped: self.term(value),
                },
                Some(_) => RangeWrite::Other,
            },
            // An element of a substring is a single index of a string. The write help names the
            // base. It fits only when the base is a place whose root can change. That element
            // must be the whole target, and the range must start at the base start, as in
            // `s[..b][i] = v`. A range counts bytes and `chars()` counts characters, so no other
            // start has a correct index in the help.
            _ if ty == Type::STRING => {
                let whole = place
                    && found
                        .parent
                        .is_some_and(|parent| parent.span == target.span);
                let (base, usage) =
                    if whole && found.range.start.is_none() && self.root_can_change(found.base) {
                        (found.base, IndexUsage::Write)
                    } else {
                        (found.index, IndexUsage::Read)
                    };
                return DiagnosticKind::StringIndex {
                    at: found.index.span,
                    base: self.source.quote(base.span),
                    usage,
                }
                .into();
            }
            _ => return self.cannot_slice(found.base, ty),
        };
        DiagnosticKind::AssignToExpression {
            at: found.index.span,
            range: Some(self.range_target(found, write, place)),
        }
        .into()
    }

    /// The part of the help that names the element `parent` of the range index `found`, in the
    /// target `target`: the same element of the base, which `store` (` = v`, `.push(v)`) writes.
    fn range_element(
        &self,
        found: &RangeIndex<'_>,
        (parent, target): (&ast::Expr, &ast::Expr),
        store: String,
    ) -> RangeWrite {
        let ExprKind::Index { index, .. } = &parent.kind else {
            unreachable!("the parent of a range index should be an index")
        };
        RangeWrite::Element {
            index: found.range.start.as_deref().map_or_else(
                || self.source.line_text(index.span),
                |start| format!("{} + {}", self.term(start), self.term(index)),
            ),
            tail: self
                .source
                .line_text(Span::new(parent.span.end(), target.span.end())),
            store,
        }
    }

    /// The parts of `found` for the help: `write` says what the help does with them, and `place`
    /// whether its base is one, else no write to the base is allowed.
    fn range_target(&self, found: &RangeIndex<'_>, write: RangeWrite, place: bool) -> RangeTarget {
        RangeTarget {
            base: self.source.line_text(found.base.span),
            start: (found.range.start.as_deref()).map(|start| self.source.line_text(start.span)),
            end: (found.range.end.as_deref()).map(|end| self.end_text(found.range, end)),
            write: if place { write } else { RangeWrite::Temporary },
        }
    }

    /// The help of a `push` onto `receiver`, which is no place, when `receiver` holds a range
    /// index; `args` are the arguments of the `push`. The element of a range index is the same
    /// element of its base: `xs[1..][0].push(4)` is `xs[1 + 0].push(4)`.
    pub(super) fn push_range(
        &mut self,
        receiver: &ast::Expr,
        args: &[ast::Arg],
    ) -> Option<RangePush> {
        let found = range_index(receiver)?;
        let Some(parent) = found.parent else {
            return Some(RangePush::Slice(self.source.line_text(receiver.span)));
        };
        let place = is_place(found.base) && self.rooted(found.base);
        let arg = match args {
            [arg] => self.source.line_text(arg.value.span),
            _ => "...".to_string(),
        };
        let write = self.range_element(&found, (parent, receiver), format!(".push({arg})"));
        Some(RangePush::Element(self.range_target(&found, write, place)))
    }

    /// `e` as the help writes a part that it adds to: in parentheses when it is more than one
    /// term.
    fn term(&self, e: &ast::Expr) -> String {
        let text = self.source.line_text(e.span);
        if single_term(e) {
            text
        } else {
            format!("({text})")
        }
    }

    /// The text of the end that `..end` writes for the same range: `b + 1` for `..=b`, computed
    /// when `b` is an integer literal.
    fn end_text(&self, range: &ast::Range, end: &ast::Expr) -> String {
        let text = self.source.line_text(end.span);
        match end.kind {
            _ if !range.inclusive => text,
            ExprKind::Int { value, .. } if value < i64::MAX => (value + 1).to_string(),
            _ if single_term(end) => format!("{text} + 1"),
            _ => format!("({text}) + 1"),
        }
    }
}

/// Whether `range` is wrong in an index whatever the base is: `..`, `..=` and `a..=`.
fn error_for_good(range: &ast::Range) -> bool {
    range.end.is_none() && (range.start.is_none() || range.inclusive)
}
