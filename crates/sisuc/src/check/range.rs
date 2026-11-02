//! Ranges: the E0023 report of a range where it is not allowed, and the E0021 report of an
//! assignment to a range index.

use super::string::single_term;
use super::{Checked, Checker, Poisoned, UNKNOWN, type_of};
use crate::ast::{self, ExprKind};
use crate::diagnostic::{
    Diagnostic, DiagnosticKind, MissingBound, RangeForm, RangePlace, RangeTarget, RangeWrite,
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

    /// E0023 for each range in the arguments of a method, which Tasks 4 and 5 replace: the first
    /// form, whichever bound is missing and whether or not the method exists. The other arguments
    /// report their own mistakes.
    pub(super) fn range_arguments(&mut self, receiver: &ast::Expr, args: &[ast::Arg]) -> Checked {
        // A `push` has checked its receiver as a place, which waits for the `push` that is not
        // reached.
        drop(self.take_pushed(receiver));
        for arg in args {
            if let ExprKind::Range(range) = &arg.value.kind {
                self.diagnostics.push(
                    DiagnosticKind::RangeOutsideFor {
                        at: range.span,
                        form: RangeForm::Anywhere,
                    }
                    .into(),
                );
            } else {
                // Only its diagnostics matter.
                drop(self.expr(&arg.value, Some(UNKNOWN)));
            }
        }
        Err(Poisoned::error())
    }

    /// E0023 for the range index `base[range]`. Until the checker takes a range index of an array
    /// (Task 3), of a string (Task 4) and a method's range argument (Task 5), the report is the
    /// one of a range anywhere, except `base[..]` and `a..=`, which are errors for good. The root
    /// of `base` is stamped as a `push` onto the index would stamp it, so no warning follows; the
    /// base of an error for good reports its own mistakes.
    pub(super) fn range_index(&mut self, base: &ast::Expr, range: &ast::Range) -> Checked {
        self.stamp_push(base);
        if error_for_good(range) {
            drop(self.value(base, None));
        }
        self.diagnostics.push(self.index_outside(base, range));
        Err(Poisoned::error())
    }

    fn index_outside(&self, base: &ast::Expr, range: &ast::Range) -> Diagnostic {
        let text = |e: &ast::Expr| self.source.text(e.span).to_string();
        let form = match (&range.start, &range.end) {
            (None, None) => RangeForm::NoBounds {
                base: text(base),
                inclusive: range.inclusive,
            },
            (Some(start), None) if range.inclusive => RangeForm::NoEnd {
                range: self.source.text(range.span).to_string(),
                start: text(start),
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
        let base_type = type_of(&self.value(found.base, None));
        let range = found.range;
        // `xs[..][i]` is an element of `xs`, which E0021 helps; `xs[..]` is `xs`.
        let element_of_all = found.parent.is_some() && range.start.is_none() && !range.inclusive;
        let d = if error_for_good(range) && !element_of_all {
            Some(self.index_outside(found.base, range))
        } else {
            let place = is_place(found.base) && self.rooted(found.base);
            base_type.map(|ty| self.range_write(&found, op, (target, value), (ty, place)))
        };
        self.diagnostics.extend(d);
        drop(self.expr(value, Some(UNKNOWN)));
        Some(Err(Poisoned::typed(Type::UNIT)))
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
        let text = |e: &ast::Expr| self.source.text(e.span).to_string();
        // A part that is more than one term goes in parentheses where the help adds to it.
        let term = |e: &ast::Expr| {
            if single_term(e) {
                text(e)
            } else {
                format!("({})", text(e))
            }
        };
        let start = found.range.start.as_deref();
        let write = match (ty.kind(), found.parent) {
            (TypeKind::Array(_), None) => RangeWrite::Array,
            (TypeKind::Array(_), Some(parent)) => {
                let ExprKind::Index { index, .. } = &parent.kind else {
                    unreachable!("the parent of a range index should be an index")
                };
                RangeWrite::Element {
                    index: start.map_or_else(
                        || text(index),
                        |start| format!("{} + {}", term(start), term(index)),
                    ),
                    tail: self
                        .source
                        .text(Span::new(parent.span.end(), target.span.end()))
                        .to_string(),
                    op: op.map_or_else(|| "=".to_string(), |op| format!("{op}=")),
                    value: text(value),
                }
            }
            _ if ty == Type::STRING && found.parent.is_none() => match op {
                None => RangeWrite::Replace { value: term(value) },
                Some(ast::BinaryOp::Add) => RangeWrite::Append {
                    value: text(value),
                    grouped: term(value),
                },
                Some(_) => RangeWrite::Other,
            },
            // A string's element, and a base that is neither an array nor a string, wait for
            // the range forms of E0068 (Tasks 3 and 4).
            _ => return self.index_outside(found.base, found.range),
        };
        DiagnosticKind::AssignToExpression {
            at: found.index.span,
            range: Some(RangeTarget {
                base: text(found.base),
                start: start.map(text),
                end: (found.range.end.as_deref()).map(|end| self.end_text(found.range, end)),
                write: if place { write } else { RangeWrite::Temporary },
            }),
        }
        .into()
    }

    /// The text of the end that `..end` writes for the same range: `b + 1` for `..=b`, computed
    /// when `b` is an integer literal.
    fn end_text(&self, range: &ast::Range, end: &ast::Expr) -> String {
        let text = self.source.text(end.span);
        match end.kind {
            _ if !range.inclusive => text.to_string(),
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
