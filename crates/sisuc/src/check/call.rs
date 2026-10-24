//! Calls: functions, methods, and the constructors of classes and enum variants, with their
//! arguments and labels.

use std::fmt;

use super::expr::{fits, no_member, postfix};
use super::generic::{GenericSig, Site, mark, mismatched};
use super::prelude::PRELUDE;
use super::unused_names::Used;
use super::{
    Checked, Checker, Declared, Poisoned, UNKNOWN, block_expr, built_in_type, own_args, type_of,
};
use super::{array, number, string};
use crate::ast::{self, ExprKind};
use crate::diagnostic::{
    Code, Definition, Diagnostic, DiagnosticKind, Suggestion, alias, suggestion,
};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, Type, TypeArgs, TypeKind};

/// The method that a call written as a function names.
enum Idiom {
    /// A method of an array or a number.
    Built,
    /// A method of a class or an enum.
    /// With the type arguments of the receiver's class or enum.
    Method(tir::FuncId, TypeArgs),
}

impl Checker<'_> {
    /// `callee(args)`, which `site` writes with its type arguments.
    pub(super) fn call(&mut self, span: Span, site: Site<'_>, args: &[ast::Arg]) -> Checked {
        let callee = site.name;
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
            (Some(Declared::Known(class)), _) => self.construct_class(span, class, site, args),
            (_, Some(Declared::Known(func))) => self.invoke(span, func, None, site, args),
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
    /// do `sqrt(x)` and `max(a, b)`, where the first argument is a number with it, and
    /// `name(p)`, where `p` is an object of a class or an enum that has the method `name`.
    fn unknown_call(&mut self, callee: &ast::Ident, args: &[ast::Arg]) -> Checked {
        let name = callee.name.as_str();
        let rest = match args.split_first() {
            Some((first, rest)) if args.iter().all(|arg| arg.label.is_none()) => {
                let receiver = self.expr(&first.value, Some(UNKNOWN));
                // A poisoned receiver is not known to have the method.
                let idiom = receiver.as_ref().ok().and_then(|receiver| {
                    Some((self.idiom_of(callee.name, receiver.ty)?, receiver.ty))
                });
                if let Some((idiom, ty)) = idiom {
                    let next = rest.first().map(|arg| arg.value.span);
                    // The rewrite deletes the text before the receiver and between it and
                    // `next`, so a comment there makes it unsafe.
                    let deleted = [
                        callee.span.start()..first.value.span.start(),
                        first.value.span.end()..next.map_or(first.value.span.end(), Span::start),
                    ];
                    let takes = match idiom {
                        Idiom::Built => self.idiom_arguments(name, ty, rest),
                        Idiom::Method(func, owner) => self.method_arguments(func, owner, rest),
                    };
                    self.report(DiagnosticKind::MethodAsFunction {
                        name: callee.name.to_string(),
                        at: callee.span,
                        receiver: first.value.span,
                        next,
                        wrap: !postfix(&first.value) || self.label_starts_receiver(callee, first),
                        safe: takes && !self.comment_in(&deleted),
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

    /// How the method `name` belongs to a receiver of type `ty`: a built-in array or number
    /// method, or a method of a class or an enum. `None` when the type has no such method.
    fn idiom_of(&self, name: Symbol, ty: Type) -> Option<Idiom> {
        let (methods, owner) = match ty.kind() {
            TypeKind::Array(element)
                if matches!(name.as_str(), "len" | "contains")
                    || name.as_str() == "sum" && matches!(element, Type::I64 | Type::F64) =>
            {
                return Some(Idiom::Built);
            }
            _ if number::method(ty, name.as_str()).is_some() => return Some(Idiom::Built),
            TypeKind::Class(id, owner) => (&self.classes[id.0].methods, owner),
            TypeKind::Enum(id, owner) => (&self.enums[id.0].methods, owner),
            _ => return None,
        };
        match methods.get(&name) {
            Some(&Declared::Known(func)) => Some(Idiom::Method(func, owner)),
            _ => None,
        }
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

    /// Checks `rest`, the arguments after the receiver of a call of the method `func`, written
    /// as a function, each once. `owner` is the type arguments of the receiver's class or enum.
    /// The method call takes them when there are as many as the method has parameters. Each
    /// must fit its parameter type, with `owner` in it, as a call `receiver.method(args)`
    /// checks. A method with type parameters of its own takes none of them as written, as they
    /// are not known.
    fn method_arguments(&mut self, func: tir::FuncId, owner: TypeArgs, rest: &[ast::Arg]) -> bool {
        let owner = owner.get();
        let sig = &self.signatures[func.0];
        if sig.type_params.len() > owner.len() {
            self.stray(rest);
            return false;
        }
        // The checks below borrow `self` again.
        let params: Vec<_> = sig
            .params
            .iter()
            .map(|ty| ty.map(|ty| ty.subst(owner)))
            .collect();
        let mut takes = rest.len() == params.len();
        for (arg, &want) in rest
            .iter()
            .zip(params.iter().chain(std::iter::repeat(&None)))
        {
            let checked = self.expr(&arg.value, want.or(Some(UNKNOWN)));
            takes &= want.is_some_and(|want| checked.is_ok_and(|checked| fits(want, checked.ty)));
        }
        takes
    }

    /// Whether `member` of `base`, a path, is a value, whose `<` and `>` were comparisons when
    /// type arguments follow it: a variant without a payload of an enum `base` names without
    /// type arguments, or a constant of `f64`, or another language's name for one.
    fn type_member_value(&self, base: &ast::Expr, member: &ast::Ident) -> bool {
        let ExprKind::Name(name) = base.kind else {
            return false;
        };
        if let Some(&Declared::Known(id)) = self.enum_ids.get(&name) {
            let info = &self.enums[id.0];
            return (info.index.get(&member.name))
                .is_some_and(|&variant| info.variants[variant].fields.is_empty());
        }
        built_in_type(name.as_str()) == Some(Type::F64)
            && number::constant_named(member.name.as_str()).is_some()
    }

    /// `receiver.method<types>(args)` as written, the type arguments in `angles`, `(args)` at
    /// `parens`, which `safe` makes `receiver?.method(args)`. A type or `math` before the `.`
    /// may make it an idiom. Type arguments on a member the receiver has not are not reported:
    /// the unknown member is. `site` writes the method's name with its type arguments.
    pub(super) fn method_expr(
        &mut self,
        span: Span,
        safe: bool,
        receiver: &ast::Expr,
        site: Site<'_>,
        args: &[ast::Arg],
        parens: Span,
    ) -> Checked {
        let (method, types, angles) = (site.name, site.written, site.angles);
        let start = self.diagnostics.len();
        let path = !types.is_empty() && self.path_method(receiver, method);
        if path && self.generic_variant(receiver, method).is_some() {
            // Its arguments alone could not bind what the list was meant to.
            self.variant_args(receiver, method, angles);
            self.stray(args);
            return Err(Poisoned::error());
        }
        if path && self.type_member_value(receiver, method) {
            self.stray(args);
            return Err(self.misread(receiver.span.to(method.span), types.first()));
        }
        // Whether the list is reported only once the member is found: a path's, or a built-in's.
        let mut deferred = path;
        let checked = if !safe
            && let Some(checked) =
                self.type_path(span, receiver, method, Some((args, parens)), site.expected)
        {
            checked
        } else {
            self.member(span, safe, receiver, method, |c, checked| {
                if !path && let Some((first, _)) = types.split_first() {
                    if let Some(value) = c.method_args(&checked, receiver, method, first, angles) {
                        c.stray(args);
                        return Err(value);
                    }
                    deferred = type_of(&checked).is_some_and(|ty| {
                        matches!(
                            ty.kind(),
                            TypeKind::Array(_) | TypeKind::String | TypeKind::I64 | TypeKind::F64
                        )
                    });
                    // A built-in's method takes no type arguments, so two that could be values
                    // were comparisons, of the method's result: the help calls one that takes
                    // no arguments. An unknown one is reported as unknown.
                    let arity =
                        type_of(&checked).and_then(|ty| built_in_arity(ty, method.name.as_str()));
                    if let Some(arity) = arity
                        && types.len() > 1
                        && c.could_be_value(first)
                    {
                        deferred = false;
                        c.stray(args);
                        let at = receiver.span.to(method.span);
                        let text = c.source.text;
                        let compare = (arity == 0).then(|| {
                            let rest = &text[at.end()..first.span.end()];
                            format!("{}(){rest}", &text[at.start()..at.end()])
                        });
                        return Err(c.misread_as(at, compare));
                    }
                }
                c.method_call(span, checked, receiver, site, args)
            })
        };
        // The member was found unknown, or the receiver of a path that is no idiom was: a name
        // or a type, which has no members.
        let unknown = self.diagnostics[start..].iter().any(|d| match d.code() {
            Code::NoMember | Code::UnknownVariant => d.span() == method.span,
            Code::BindingNotFound | Code::TypeAsValue => d.span() == receiver.span,
            _ => false,
        });
        if deferred && !unknown {
            self.not_generic(method.name, angles);
        }
        checked
    }

    /// `receiver.method(args)`, `receiver` checked from `receiver_expr`.
    fn method_call(
        &mut self,
        span: Span,
        receiver: Checked,
        receiver_expr: &ast::Expr,
        site: Site<'_>,
        args: &[ast::Arg],
    ) -> Checked {
        let method = site.name;
        // The arguments are not part of an assignment target's chain; `expr` restores the flag.
        let target = std::mem::take(&mut self.body.in_target);
        match type_of(&receiver) {
            Some(ty) if matches!(ty.kind(), TypeKind::Array(_)) => {
                return self.array_method(span, (receiver, receiver_expr.span), ty, method, args);
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
        self.invoke(span, func, Some(receiver), site, args)
    }

    /// A call of `func`, which `site` writes. `receiver` is a method's `self`; `args` match the
    /// other parameters.
    fn invoke(
        &mut self,
        span: Span,
        func: tir::FuncId,
        receiver: Option<Checked>,
        site: Site<'_>,
        args: &[ast::Arg],
    ) -> Checked {
        self.record(Used::Func(func), span);
        let labeled = self.labels(args);
        let sig = &self.signatures[func.0];
        let (name, params, ret) = (sig.name.clone(), sig.params.clone(), sig.ret);
        let declared = sig.type_params.clone();
        if params.len() != args.len() {
            self.stray(args);
            if !self.misread_in(args) {
                self.report(DiagnosticKind::ArgumentCount {
                    name,
                    want: params.len(),
                    found: args.len(),
                    at: span,
                });
            }
            return Err(Poisoned::error());
        }
        let (checked, types, ret) = if declared.is_empty() {
            let checked = args
                .iter()
                .zip(&params)
                .map(|(arg, &param)| mark(self.operand(&arg.value, param)))
                .collect();
            (checked, TypeArgs::NONE, ret)
        } else {
            // A method's receiver binds its owner's parameters.
            let owner = match receiver.as_ref().and_then(type_of).map(Type::kind) {
                Some(TypeKind::Class(_, owner) | TypeKind::Enum(_, owner)) => owner.get(),
                _ => &[],
            };
            // A list on a method that takes none of its own is reported already.
            let written = if declared.len() > owner.len() {
                site.written
            } else {
                &[]
            };
            let sig = GenericSig {
                name: site.name,
                variant: None,
                declared: &declared,
                owner,
                params: &params,
                ret,
            };
            match self.generic_call(span, &sig, (written, site.angles), args, site.expected) {
                Some(call) => call,
                None => return Err(Poisoned::error()),
            }
        };
        let failed = labeled || checked.iter().any(mismatched);
        match (receiver.into_iter().chain(checked).collect(), ret) {
            (Ok(args), Some(ret)) if !failed => Ok(self.spilled(span, ret, args, |args| {
                tir::ExprKind::Call { func, types, args }
            })),
            (_, ret) => Err(Poisoned::of(ret.filter(|_| !failed))),
        }
    }

    /// Whether one of `args`, checked, holds a comparison misread as a call, which took the
    /// list's commas around it: as the argument, or as an operand of its operators. One inside
    /// a nested list or a block took that list's commas instead.
    pub(super) fn misread_in(&self, args: &[ast::Arg]) -> bool {
        args.iter().any(|arg| self.holds_misread(&arg.value))
    }

    /// Whether `expr`, or an operand of its operators, is a comparison misread as a call.
    fn holds_misread(&self, expr: &ast::Expr) -> bool {
        use ast::ExprKind as K;
        self.body.misread.contains(&expr.span.start())
            || match &expr.kind {
                K::Unary { operand, .. } => self.holds_misread(operand),
                K::Binary { lhs, rhs, .. } | K::Coalesce { lhs, rhs } => {
                    self.holds_misread(lhs) || self.holds_misread(rhs)
                }
                K::Compare { operands, .. } => operands.iter().any(|e| self.holds_misread(e)),
                _ => false,
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

    /// `C(f: e, ...)`, a construction of the class `class`, which `site` writes.
    fn construct_class(
        &mut self,
        span: Span,
        class: tir::ClassId,
        site: Site<'_>,
        args: &[ast::Arg],
    ) -> Checked {
        self.record(Used::Class(class), span);
        let info = &self.classes[class.0];
        let name = info.name.name.as_str();
        let fields: Vec<(Symbol, Option<Type>)> =
            info.fields.iter().map(|f| (f.name.name, f.ty)).collect();
        if !info.type_params.is_empty() {
            let declared = info.type_params.clone();
            let names: Vec<_> = fields.iter().map(|(f, _)| *f).collect();
            if !self.shaped(span, &name, &names, args) {
                return Err(Poisoned::error());
            }
            let params: Vec<_> = fields.iter().map(|(_, ty)| *ty).collect();
            let sig = GenericSig {
                name: site.name,
                variant: None,
                declared: &declared,
                owner: &[],
                params: &params,
                ret: Some(Type::intern(TypeKind::Class(class, own_args(&declared)))),
            };
            let written = (site.written, site.angles);
            let Some((args, _, Some(ty))) =
                self.generic_call(span, &sig, written, args, site.expected)
            else {
                return Err(Poisoned::error());
            };
            let failed = args.iter().any(mismatched);
            return match args.into_iter().collect() {
                Ok(args) => {
                    Ok(self.spilled(span, ty, args, |args| tir::ExprKind::New { class, args }))
                }
                Err(_) => Err(Poisoned::of((!failed).then_some(ty))),
            };
        }
        let ty = Type::intern(TypeKind::Class(class, TypeArgs::NONE));
        match self.construct(span, &name, &fields, args) {
            Ok(args) => Ok(self.spilled(span, ty, args, |args| tir::ExprKind::New { class, args })),
            Err(failed) => Err(Poisoned::of((!failed).then_some(ty))),
        }
    }

    /// Whether the labels of `name(f: e, ...)`, a generic class's constructor or a variant with
    /// labels, name every one of `fields` in declaration order; the first mistake is reported,
    /// and then every argument is checked for its own.
    pub(super) fn shaped(
        &mut self,
        span: Span,
        name: &dyn fmt::Display,
        fields: &[Symbol],
        args: &[ast::Arg],
    ) -> bool {
        let mistake = args
            .iter()
            .enumerate()
            .find_map(|(i, arg)| misplaced(name, fields, i, arg));
        if let Some(d) = mistake {
            self.diagnostics.push(d);
            self.stray(args);
        } else if let Some(missing) = fields.get(args.len()) {
            // Checking the arguments records a comparison misread as a call.
            self.stray(args);
            if !self.misread_in(args) {
                self.report(DiagnosticKind::MissingField {
                    field: missing.to_string(),
                    fields: fields.iter().map(ToString::to_string).collect(),
                    at: span,
                });
            }
        } else {
            return true;
        }
        false
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
            if !self.misread_in(args) {
                self.report(DiagnosticKind::MissingField {
                    field: missing.to_string(),
                    fields: names.iter().map(ToString::to_string).collect(),
                    at: span,
                });
            }
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

/// The parameter count of the built-in method `name` of `ty`, if `ty` has one.
fn built_in_arity(ty: Type, name: &str) -> Option<usize> {
    match ty.kind() {
        TypeKind::Array(_) => (array::METHODS.iter())
            .find(|(method, _)| *method == name)
            .map(|&(_, params)| params),
        TypeKind::String => string::METHODS.contains(&name).then_some(0),
        TypeKind::I64 | TypeKind::F64 => number::method(ty, name).map(|(_, params)| params),
        _ => None,
    }
}
