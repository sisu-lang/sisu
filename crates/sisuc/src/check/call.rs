//! Calls: functions, methods, and the constructors of classes and enum variants, with their
//! arguments and labels.

use std::fmt;

use super::expr::{no_member, postfix};
use super::number;
use super::prelude::PRELUDE;
use super::{Checked, Checker, Declared, Poisoned, UNKNOWN, block_expr, built_in_type, type_of};
use crate::ast::{self, ExprKind};
use crate::diagnostic::{Definition, Diagnostic, DiagnosticKind, Suggestion, alias, suggestion};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, Type, TypeKind};

impl Checker<'_> {
    pub(super) fn call(&mut self, span: Span, callee: &ast::Ident, args: &[ast::Arg]) -> Checked {
        let declared = (
            self.class_ids.get(&callee.name).copied(),
            self.function_ids.get(&callee.name).copied(),
        );
        // A broken name, a constant's included, poisons every use, a built-in's too. The item's
        // own error is reported; its arguments may hold more.
        if matches!(
            declared,
            (Some(Declared::Broken), _) | (_, Some(Declared::Broken))
        ) || matches!(self.constants.get(&callee.name), Some(Declared::Broken))
        {
            self.stray(args);
            return Err(Poisoned::error());
        }
        if let Some(checked) = self.prelude_call(span, callee, args) {
            return checked;
        }
        if let Some(to @ (Type::I64 | Type::U8 | Type::F64 | Type::CHAR | Type::STRING)) =
            built_in_type(callee.name.as_str())
        {
            return self.convert(span, callee.span, to, args);
        }
        match declared {
            (Some(Declared::Known(class)), _) => self.construct_class(span, class, args),
            (_, Some(Declared::Known(func))) => self.invoke(span, func, None, args),
            _ => match self.bare_variant(callee.name, callee.span) {
                Some(d) => {
                    self.diagnostics.push(d);
                    self.stray(args);
                    Err(Poisoned::error())
                }
                None => self.unknown_call(callee, args),
            },
        }
    }

    /// A call of `callee`, which names nothing callable. `len(xs)`, `sum(xs)` and
    /// `contains(xs, v)`, where `xs` is an array with that method, point to the method, and so
    /// do `sqrt(x)` and `max(a, b)`, where the first argument is a number with it.
    fn unknown_call(&mut self, callee: &ast::Ident, args: &[ast::Arg]) -> Checked {
        let name = callee.name.as_str();
        let idiom = (matches!(name, "len" | "sum" | "contains")
            || number::method_name(name).is_some())
            && args.iter().all(|arg| arg.label.is_none());
        let rest = match args.split_first() {
            Some((first, rest)) if idiom => {
                let receiver = self.expr(&first.value, Some(UNKNOWN));
                // A poisoned receiver is not known to be an array or a number.
                let ty = match &receiver {
                    Ok(tir::Expr { ty, .. }) => match ty.kind() {
                        TypeKind::Array(element)
                            if matches!(name, "len" | "contains")
                                || name == "sum" && matches!(element, Type::I64 | Type::F64) =>
                        {
                            Some(*ty)
                        }
                        _ if number::method(*ty, name).is_some() => Some(*ty),
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(ty) = ty {
                    let next = rest.first().map(|arg| arg.value.span);
                    // The rewrite deletes the text before the receiver and between it and
                    // `next`, so a comment there makes it unsafe.
                    let deleted = [
                        callee.span.start()..first.value.span.start(),
                        first.value.span.end()..next.map_or(first.value.span.end(), Span::start),
                    ];
                    let safe = self.idiom_arguments(name, ty, rest) && !self.comment_in(&deleted);
                    self.report(DiagnosticKind::MethodAsFunction {
                        name: callee.name.to_string(),
                        at: callee.span,
                        receiver: first.value.span,
                        next,
                        wrap: !postfix(&first.value) || self.label_starts_receiver(callee, first),
                        safe,
                    });
                    return Err(Poisoned::error());
                }
                rest
            }
            _ => args,
        };
        let at_start = self.starts_break_value(callee.span);
        let functions = self
            .function_ids
            .keys()
            .map(|function| function.as_str())
            .chain(PRELUDE)
            .chain(["i64", "u8", "f64", "char", "string"])
            .filter(|function| !(at_start && self.is_open_label(function)))
            .map(|function| (function, Definition::Function));
        let suggestion = match alias(name).filter(|p| !(at_start && self.is_open_label(p))) {
            Some(prelude) => Some(Suggestion {
                kind: Definition::Function,
                name: prelude.to_string(),
            }),
            None => suggestion(name, functions.chain(self.class_names())),
        };
        self.report(DiagnosticKind::FunctionNotFound {
            name: callee.name.to_string(),
            at: callee.span,
            suggestion,
        });
        // No `labels`: the name may have meant a class, whose constructor takes them.
        self.stray(rest);
        Err(Poisoned::error())
    }

    /// Whether the receiver `first` of the idiom call `callee(first, ...)` starts with an open
    /// loop label, and the call starts the value of an unlabeled `break`: the fix then writes
    /// the label there, unless it wraps the receiver in parentheses.
    fn label_starts_receiver(&self, callee: &ast::Ident, first: &ast::Arg) -> bool {
        let text = &self.source.text[first.value.span.start()..];
        let end = text
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(text.len());
        self.starts_break_value(callee.span) && self.is_open_label(&text[..end])
    }

    /// Checks the arguments after the receiver of an idiom call of the method `name`, each once.
    /// `receiver` is the receiver's type. For a number, whether the method takes them (see
    /// `number_arguments`). For an array: `contains` takes one of the element's type, or an
    /// integer literal that fits it, that checked cleanly, and `len` and `sum` none.
    fn idiom_arguments(&mut self, name: &str, receiver: Type, rest: &[ast::Arg]) -> bool {
        if let Some((_, params)) = number::method(receiver, name) {
            return self.number_arguments(receiver, params, rest);
        }
        let TypeKind::Array(element) = receiver.kind() else {
            unreachable!("an idiom receiver should be a number with the method or an array");
        };
        let [value] = rest else {
            self.stray(rest);
            return rest.is_empty() && name != "contains";
        };
        let checked = self.expr(&value.value, Some(UNKNOWN));
        name == "contains"
            && checked.is_ok_and(|checked| {
                checked.ty == element
                    || matches!(
                        (&value.value.kind, element),
                        (ExprKind::Int { value, .. }, Type::U8) if u8::try_from(*value).is_ok()
                    )
            })
    }

    /// `receiver.method(args)` as written, `(args)` at `parens`, which `safe` makes
    /// `receiver?.method(args)`. A type or `math` before the `.` may make it an idiom.
    pub(super) fn method_expr(
        &mut self,
        span: Span,
        safe: bool,
        receiver: &ast::Expr,
        method: &ast::Ident,
        args: &[ast::Arg],
        parens: Span,
    ) -> Checked {
        if !safe && let Some(checked) = self.type_path(span, receiver, method, Some((args, parens)))
        {
            return checked;
        }
        self.member(span, safe, receiver, method, |c, checked| {
            c.method_call(span, checked, receiver, method, args)
        })
    }

    /// `receiver.method(args)`, `receiver` checked from `receiver_expr`.
    fn method_call(
        &mut self,
        span: Span,
        receiver: Checked,
        receiver_expr: &ast::Expr,
        method: &ast::Ident,
        args: &[ast::Arg],
    ) -> Checked {
        // The arguments are not part of an assignment target's chain; `expr` restores the flag.
        let target = std::mem::take(&mut self.body.in_target);
        match type_of(&receiver) {
            Some(ty) if matches!(ty.kind(), TypeKind::Array(_)) => {
                return self.array_method(span, receiver, ty, method, args);
            }
            Some(Type::STRING) => return self.string_method(span, receiver, method, args),
            Some(ty @ (Type::I64 | Type::F64)) => {
                return self.number_method(span, receiver, ty, method, args);
            }
            _ => {}
        }
        let Some(func) = self.find_method(type_of(&receiver), receiver_expr, method, target) else {
            self.labels(args);
            self.stray(args);
            return Err(Poisoned::error());
        };
        self.invoke(span, func, Some(receiver), args)
    }

    /// A call of `func`. `receiver` is a method's `self`; `args` match the other parameters.
    fn invoke(
        &mut self,
        span: Span,
        func: tir::FuncId,
        receiver: Option<Checked>,
        args: &[ast::Arg],
    ) -> Checked {
        let labeled = self.labels(args);
        let sig = &self.signatures[func.0];
        let (name, params, ret) = (sig.name.clone(), sig.params.clone(), sig.ret);
        if params.len() != args.len() {
            self.report(DiagnosticKind::ArgumentCount {
                name,
                want: params.len(),
                found: args.len(),
                at: span,
            });
            self.stray(args);
            return Err(Poisoned::error());
        }
        let (checked, mismatched): (Vec<_>, Vec<_>) = args
            .iter()
            .zip(&params)
            .map(|(arg, &param)| self.operand(&arg.value, param))
            .unzip();
        let failed = labeled || mismatched.contains(&true);
        match (receiver.into_iter().chain(checked).collect(), ret) {
            (Ok(args), Some(ret)) if !failed => {
                Ok(self.spilled(span, ret, args, |args| tir::ExprKind::Call { func, args }))
            }
            (_, ret) => Err(Poisoned::of(ret.filter(|_| !failed))),
        }
    }

    /// The node `kind` makes of `operands`, in a block after the bindings `spill` makes of them.
    pub(super) fn spilled(
        &mut self,
        span: Span,
        ty: Type,
        operands: Vec<tir::Expr>,
        kind: impl FnOnce(Vec<tir::Expr>) -> tir::ExprKind,
    ) -> tir::Expr {
        let (stmts, operands) = self.spill(operands);
        let node = tir::Expr {
            kind: kind(operands),
            ty,
            span,
        };
        if stmts.is_empty() {
            return node;
        }
        block_expr(stmts, Some(node), ty, span)
    }

    /// Reports each labeled argument, as only a constructor takes labels. Whether there was one.
    pub(super) fn labels(&mut self, args: &[ast::Arg]) -> bool {
        let mut labeled = false;
        for label in args.iter().filter_map(|a| a.label.as_ref()) {
            self.report(DiagnosticKind::LabeledArgument {
                at: label.span,
                variant: None,
            });
            labeled = true;
        }
        labeled
    }

    /// `C(f: e, ...)`, a construction of the class `class`.
    fn construct_class(&mut self, span: Span, class: tir::ClassId, args: &[ast::Arg]) -> Checked {
        let info = &self.classes[class.0];
        let name = info.name.name.as_str();
        let fields: Vec<(Symbol, Option<Type>)> =
            info.fields.iter().map(|f| (f.name.name, f.ty)).collect();
        let ty = Type::intern(TypeKind::Class(class));
        match self.construct(span, &name, &fields, args) {
            Ok(args) => Ok(self.spilled(span, ty, args, |args| tir::ExprKind::New { class, args })),
            Err(failed) => Err(Poisoned::of((!failed).then_some(ty))),
        }
    }

    /// The arguments of `name(f: e, ...)`, a class's constructor or a variant with labels,
    /// which name every one of `fields` in declaration order. Only the first mistake is
    /// reported; every argument is still checked for its own. `Err` holds whether a mistake was
    /// reported here, rather than in an argument.
    pub(super) fn construct(
        &mut self,
        span: Span,
        name: &dyn fmt::Display,
        fields: &[(Symbol, Option<Type>)],
        args: &[ast::Arg],
    ) -> Result<Vec<tir::Expr>, bool> {
        let names: Vec<_> = fields.iter().map(|(f, _)| *f).collect();
        let mut failed = false;
        let mut checked = Vec::with_capacity(args.len());
        for (i, arg) in args.iter().enumerate() {
            if !failed && let Some(d) = misplaced(name, &names, i, arg) {
                self.diagnostics.push(d);
                failed = true;
            }
            if failed {
                checked.push(self.expr(&arg.value, Some(UNKNOWN)));
            } else {
                let (arg, mismatched) = self.operand(&arg.value, fields[i].1);
                failed = mismatched;
                checked.push(arg);
            }
        }
        if !failed && let Some(missing) = names.get(args.len()) {
            self.report(DiagnosticKind::MissingField {
                field: missing.to_string(),
                fields: names.iter().map(ToString::to_string).collect(),
                at: span,
            });
            failed = true;
        }
        match checked.into_iter().collect() {
            Ok(args) if !failed => Ok(args),
            _ => Err(failed),
        }
    }
}

/// The mistake in argument `i` of `name(...)`, a class's constructor or a variant with labels,
/// whose fields are `fields`, if any.
fn misplaced(
    name: &dyn fmt::Display,
    fields: &[Symbol],
    i: usize,
    arg: &ast::Arg,
) -> Option<Diagnostic> {
    let Some(field) = fields.get(i) else {
        let start = arg
            .label
            .as_ref()
            .map_or(arg.value.span, |l| l.span)
            .start();
        return Some(
            DiagnosticKind::TooManyArguments {
                class: name.to_string(),
                fields: fields.len(),
                at: Span::new(start, arg.value.span.end()),
            }
            .into(),
        );
    };
    let Some(label) = &arg.label else {
        return Some(
            DiagnosticKind::ArgumentNeedsLabel {
                field: field.to_string(),
                at: arg.value.span,
            }
            .into(),
        );
    };
    if label.name == *field {
        None
    } else if fields.contains(&label.name) {
        Some(
            DiagnosticKind::FieldOutOfOrder {
                field: field.to_string(),
                found: label.name.to_string(),
                fields: fields.iter().map(ToString::to_string).collect(),
                at: label.span,
            }
            .into(),
        )
    } else {
        Some(no_member(
            Definition::Field,
            name.to_string(),
            label,
            fields.iter().map(|field| field.as_str()),
        ))
    }
}
