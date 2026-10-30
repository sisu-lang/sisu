//! Calls: functions, methods, and the constructors of classes and enum variants, with their
//! arguments and labels.

use std::fmt;

use super::expr::{fits, no_member, option_written, path, postfix};
use super::generic::{GenericSig, Site, mark, mismatched};
use super::interface::LangPath;
use super::member::Found;
use super::prelude::PRELUDE;
use super::unused_names::Used;
use super::{
    Checked, Checker, Declared, Lowered, Poisoned, UNKNOWN, block_expr, built_in_type, own_args,
    type_of,
};
use super::{array, number, string};
use crate::ast::{self, ExprKind};
use crate::diagnostic::{
    Code, Definition, Diagnostic, DiagnosticKind, MethodReceiver, ReceiverArgument, Suggestion,
    alias, suggestion,
};
use crate::lang::{self, Dispatch};
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
            || matches!(self.interface_ids.get(&callee.name), Some(Declared::Broken))
        {
            self.stray(args);
            return Err(Poisoned::error());
        }
        if let Some(checked) = self.prelude_call(span, callee, args, site.expected) {
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
            _ if self.interface_ids.contains_key(&callee.name) => {
                self.report(DiagnosticKind::InterfaceAsValue {
                    name: callee.name.to_string(),
                    at: callee.span,
                });
                self.stray(args);
                Err(Poisoned::error())
            }
            // Here `Some` is the prelude's. A user item named `Some` is reported, and the two arms
            // above call it.
            _ if callee.name.as_str() == "Some" => self.some(span, site, None, args),
            // `Ok` and `Err` are the prelude's, and no user item can take their names.
            _ if let Some((id, _)) = self.result_variant(callee.name) => {
                self.enum_path(span, id, callee, Some((args, callee.span)), site)
            }
            _ => match self.bare_variant(callee.name, callee.span) {
                Some(d) => {
                    self.diagnostics.push(d);
                    self.stray(args);
                    Err(Poisoned::error())
                }
                None => self.unknown_call(site, args),
            },
        }
    }

    /// A call of `callee`, which names nothing callable. `len(xs)`, `sum(xs)` and
    /// `contains(xs, v)`, where `xs` is an array with that method, point to the method, and so
    /// do `sqrt(x)` and `max(a, b)`, where the first argument is a number with it, and
    /// `name(p)`, where `p` is an object of a class or an enum that has the method `name`, or an
    /// optional of one. In a method, `name(args)` where its class or enum has the method `name`
    /// points to `self.name(args)`. `site` writes `callee` with its type arguments.
    fn unknown_call(&mut self, site: Site<'_>, args: &[ast::Arg]) -> Checked {
        let callee = site.name;
        let name = callee.name.as_str();
        let unlabeled = args.iter().all(|arg| arg.label.is_none());
        let mut first_checked = None;
        let rest = match args.split_first() {
            Some((first, rest)) if unlabeled => {
                let receiver = self.expr(&first.value, Some(UNKNOWN));
                // When it fits the method of `self`, the call is most likely `self.name(args)`,
                // as in a language that calls a bare method name on `this`.
                let want = self.self_parameter(callee.name, args.len());
                let fits_self =
                    want.is_some_and(|want| receiver.as_ref().is_ok_and(|r| fits(want, r.ty)));
                // A poisoned receiver is not known to have the method.
                let idiom = receiver
                    .as_ref()
                    .ok()
                    .filter(|_| !fits_self)
                    .and_then(|receiver| {
                        let (ty, optional) = match receiver.ty.kind() {
                            TypeKind::Optional(payload) => (payload, true),
                            _ => (receiver.ty, false),
                        };
                        Some((self.idiom_of(callee.name, ty)?, ty, optional))
                    });
                if let Some((idiom, ty, optional)) = idiom {
                    let next = rest.first().map(|arg| arg.value.span);
                    // The rewrite deletes the text before the receiver and between it and
                    // `next`, so a comment there makes it unsafe.
                    let (lo, hi) = (first.value.span.start(), first.value.span.end());
                    let deleted = [callee.span.start()..lo, hi..next.map_or(hi, Span::start)];
                    let takes = match idiom {
                        Idiom::Built => self.idiom_arguments(name, ty, rest),
                        Idiom::Method(func, owner) => {
                            self.method_arguments(func, owner, rest, None)
                        }
                    };
                    let argument = ReceiverArgument {
                        at: first.value.span,
                        next,
                        wrap: !postfix(&first.value) || self.label_starts_receiver(callee, first),
                    };
                    let receiver = if optional {
                        MethodReceiver::Optional {
                            argument,
                            value: path(&first.value),
                            takes,
                        }
                    } else {
                        MethodReceiver::Argument {
                            argument,
                            safe: takes && !self.comment_in(&deleted),
                        }
                    };
                    self.report(DiagnosticKind::MethodAsFunction {
                        name: callee.name.to_string(),
                        at: callee.span,
                        receiver,
                    });
                    return Err(Poisoned::error());
                }
                first_checked = Some(receiver);
                rest
            }
            _ => args,
        };
        // Not the first argument's method, but one of the class or the enum that holds the call.
        let own = self
            .body
            .self_type
            .and_then(|ty| self.idiom_of(callee.name, ty));
        if let Some(Idiom::Method(func, owner)) = own {
            let takes = self.method_arguments(func, owner, args, first_checked);
            self.report(DiagnosticKind::MethodAsFunction {
                name: callee.name.to_string(),
                at: callee.span,
                receiver: MethodReceiver::SelfValue {
                    // `self.name<T>(args)` keeps the type arguments, which a method call
                    // does not take (E0137).
                    safe: takes && unlabeled && site.written.is_empty(),
                },
            });
            return Err(Poisoned::error());
        }
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

    /// The type of the first parameter of the method `name` of `self`. `None` unless the method
    /// takes `count` arguments and has no type parameters of its own.
    fn self_parameter(&self, name: Symbol, count: usize) -> Option<Type> {
        let Some(Idiom::Method(func, owner)) =
            self.body.self_type.and_then(|ty| self.idiom_of(name, ty))
        else {
            return None;
        };
        let sig = &self.signatures[func.0];
        let owner = owner.get();
        if sig.type_params.len() > owner.len() || sig.params.len() != count {
            return None;
        }
        sig.params
            .first()
            .copied()
            .flatten()
            .and_then(|ty| self.subst(ty, owner))
    }

    /// How the method `name` belongs to a receiver of type `ty`: a built-in array or `f64`
    /// method, or a method of a class, an enum, an array or an `i64` that the program or the
    /// prelude writes. `None` when the type has no such method.
    fn idiom_of(&self, name: Symbol, ty: Type) -> Option<Idiom> {
        match ty.kind() {
            TypeKind::Array(_) if name.as_str() == "len" => {
                return Some(Idiom::Built);
            }
            _ if number::method(ty, name.as_str()).is_some() => return Some(Idiom::Built),
            TypeKind::Class(..) | TypeKind::Enum(..) | TypeKind::Array(_) | TypeKind::I64 => {}
            _ => return None,
        }
        match self.own_method(ty, name) {
            Some(Declared::Known(func)) => Some(Idiom::Method(func, self.owner_args(func, ty))),
            _ => None,
        }
    }

    /// Whether the receiver `first` of the idiom call `callee(first, ...)` starts with an open
    /// loop label, and the call starts the value of an unlabeled `break`: the fix then writes
    /// the label there, unless it wraps the receiver in parentheses.
    fn label_starts_receiver(&self, callee: &ast::Ident, first: &ast::Arg) -> bool {
        let text = self.source.file.rest(first.value.span.start());
        let end = text
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(text.len());
        self.starts_break_value(callee.span) && self.is_open_label(&text[..end])
    }

    /// Checks the arguments after the receiver of an idiom call of the method `name`, each once.
    /// `receiver` is the receiver's type. For a number, whether the method takes them (see
    /// `number_arguments`). For an array: `len` takes none.
    fn idiom_arguments(&mut self, name: &str, receiver: Type, rest: &[ast::Arg]) -> bool {
        if let Some((_, params)) = number::method(receiver, name) {
            return self.number_arguments(receiver, params, rest);
        }
        self.stray(rest);
        rest.is_empty()
    }

    /// Checks `args`, the arguments of a call of the method `func`, each once. `first` is the
    /// first one when it is checked already. `owner` is the type arguments of the method's class
    /// or enum. The method call takes them when there are as many as the method has parameters.
    /// Each must fit its parameter type, with `owner` in it, as a call `receiver.method(args)`
    /// checks. A method with type parameters of its own takes none of them as written, as they
    /// are not known.
    fn method_arguments(
        &mut self,
        func: tir::FuncId,
        owner: TypeArgs,
        args: &[ast::Arg],
        first: Option<Checked>,
    ) -> bool {
        let owner = owner.get();
        let sig = &self.signatures[func.0];
        if sig.type_params.len() > owner.len() {
            // The first argument is reported already.
            self.stray(&args[usize::from(first.is_some())..]);
            return false;
        }
        let mut takes = args.len() == sig.params.len();
        // The checks below borrow `self` again. Only the parameters that have an argument are
        // made, so that a call with few arguments costs little.
        let params: Vec<_> = sig
            .params
            .iter()
            .take(args.len())
            .map(|ty| ty.and_then(|ty| self.subst(ty, owner)))
            .collect();
        let mut first = first;
        for (arg, &want) in args
            .iter()
            .zip(params.iter().chain(std::iter::repeat(&None)))
        {
            let checked = first
                .take()
                .unwrap_or_else(|| self.expr(&arg.value, want.or(Some(UNKNOWN))));
            takes &= want.is_some_and(|want| checked.is_ok_and(|checked| fits(want, checked.ty)));
        }
        takes
    }

    /// The parameter count of the built-in method `name` of `ty`, if `ty` has one: a method of
    /// the prelude's `extend` of an array type or `i64`, or one the compiler writes.
    pub(super) fn built_in_arity(&self, ty: Type, name: &str) -> Option<usize> {
        if matches!(ty.kind(), TypeKind::Array(_) | TypeKind::I64)
            && let Some(Declared::Known(func)) = self.own_method(ty, Symbol::intern(name))
        {
            return Some(self.signatures[func.0].params.len());
        }
        match ty.kind() {
            TypeKind::Array(_) => (array::METHODS.iter())
                .find(|(method, _)| *method == name)
                .map(|&(_, params)| params),
            TypeKind::String => string::METHODS.contains(&name).then_some(0),
            TypeKind::F64 => number::method(ty, name).map(|(_, params)| params),
            _ => None,
        }
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
        // `Option` is generic, as an enum with the variants `Some` and `None` would be.
        let option_variant = option_written(receiver) == Some(false)
            && matches!(method.name.as_str(), "Some" | "None");
        if path && (option_variant || self.generic_variant(receiver, method).is_some()) {
            // Its arguments alone could not bind what the list was meant to.
            self.variant_args(receiver, method, types, angles);
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
            // The receiver of a `push` is a place, checked once and held for the call.
            let scrutinee =
                if method.name.as_str() == "push" && !safe && self.is_push_place(receiver) {
                    self.push_receiver(receiver)
                } else {
                    self.value_kept(receiver)
                };
            self.member_of(span, safe, scrutinee, receiver, method, |c, checked| {
                if !path && let Some((first, _)) = types.split_first() {
                    if let Some(value) = c.method_args(&checked, receiver, method, first, angles) {
                        c.stray(args);
                        return Err(value);
                    }
                    // A built-in's generated `to_string`, `eq` and `less` report their own list.
                    deferred = !matches!(method.name.as_str(), "to_string" | "eq" | "less")
                        && type_of(&checked).is_some_and(|ty| {
                            matches!(
                                ty.kind(),
                                TypeKind::Array(_)
                                    | TypeKind::String
                                    | TypeKind::I64
                                    | TypeKind::F64
                            )
                        });
                    // A built-in's method takes no type arguments, so two that could be values
                    // were comparisons, of the method's result: the help calls one that takes
                    // no arguments. An unknown one is reported as unknown.
                    let arity =
                        type_of(&checked).and_then(|ty| c.built_in_arity(ty, method.name.as_str()));
                    if let Some(arity) = arity
                        && types.len() > 1
                        && c.could_be_value(first)
                    {
                        deferred = false;
                        c.stray(args);
                        let at = receiver.span.to(method.span);
                        let text = c.source.file;
                        let compare = (arity == 0).then(|| {
                            let rest = text.slice(at.end()..first.span.end());
                            format!("{}(){rest}", text.text(at))
                        });
                        return Err(c.misread_as(at, compare));
                    }
                }
                c.method_call(span, safe, checked, receiver, site, args)
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

    /// `receiver.method(args)`, `receiver` checked from `receiver_expr`. `safe` when it is
    /// `receiver?.method(args)`.
    fn method_call(
        &mut self,
        span: Span,
        safe: bool,
        receiver: Checked,
        receiver_expr: &ast::Expr,
        site: Site<'_>,
        args: &[ast::Arg],
    ) -> Checked {
        let method = site.name;
        // The arguments are not part of an assignment target's chain; `expr` restores the flag.
        let target = std::mem::take(&mut self.body.in_target);
        let receiver = match self.generated_operator(span, receiver, site, args) {
            Ok(checked) => return checked,
            Err(receiver) => receiver,
        };
        // A `push` onto an array writes its place, so it is not a method of the prelude's.
        if method.name.as_str() == "push" {
            let place = self.take_pushed(receiver_expr);
            match type_of(&receiver) {
                // A type that is known and is no array, a class with a `push` method of its own,
                // reads its receiver and writes nothing.
                Some(ty) if !matches!(ty.kind(), TypeKind::Array(_)) => {
                    if let (Ok(read), Some(_)) = (&receiver, place) {
                        self.read_pushed(read, receiver_expr);
                    }
                }
                _ => return self.push(span, safe, (receiver, place), receiver_expr, args),
            }
        }
        match type_of(&receiver) {
            // A built-in type, an array and an optional have a `Display` that the compiler writes,
            // and no method of their own named `to_string`.
            Some(ty) if method.name.as_str() == "to_string" && generated_display_only(ty) => {
                return self.generated_display(span, receiver, site, args, false);
            }
            // A method of the prelude's `extend` of an array type or `i64` answers before the
            // methods the compiler writes.
            Some(ty) if let Some(func) = self.prelude_method(ty, method.name) => {
                return self.invoke(span, func, Some(receiver), site, args);
            }
            Some(ty) if matches!(ty.kind(), TypeKind::Array(_)) => {
                return self.array_method(span, receiver, ty, method, args);
            }
            Some(Type::STRING) => return self.string_method(span, receiver, method, args),
            Some(ty @ (Type::I64 | Type::F64)) => {
                return self.number_method(span, receiver, ty, method, args);
            }
            Some(ty) if matches!(ty.kind(), TypeKind::Param(..) | TypeKind::Assoc(..)) => {
                return self.bound_method(span, receiver, ty, site, args);
            }
            _ => {}
        }
        let ty = type_of(&receiver);
        match self.find_method(ty, receiver_expr, site, target, (span, args.is_empty())) {
            Some(Found::Func(func)) => self.invoke(span, func, Some(receiver), site, args),
            Some(Found::GeneratedDisplay { listed }) => {
                self.generated_display(span, receiver, site, args, listed)
            }
            None => {
                self.labels(args);
                self.stray(args);
                Err(Poisoned::error())
            }
        }
    }

    /// `receiver.to_string()` on a value whose `Display` the compiler writes: the text that
    /// `print` writes, as a one-part interpolation (`lang::to_string`). The call takes no
    /// argument and no type argument, as a written method with no type parameter of its own.
    /// With `listed`, the list of type arguments is already reported.
    fn generated_display(
        &mut self,
        span: Span,
        receiver: Checked,
        site: Site<'_>,
        args: &[ast::Arg],
        listed: bool,
    ) -> Checked {
        if !listed && !site.written.is_empty() {
            self.not_generic(site.name.name, site.angles);
        }
        if !args.is_empty() {
            let d = DiagnosticKind::ArgumentCount {
                name: "to_string".to_string(),
                want: 0,
                found: args.len(),
                at: span,
            };
            return self.rejected(d.into(), args);
        }
        match receiver {
            Ok(receiver) => {
                self.spellings.insert(span, Lowered::Method);
                Ok(self.spilled(span, Type::STRING, vec![receiver], |parts| {
                    let [part]: [tir::Expr; 1] = parts
                        .try_into()
                        .expect("`spilled` should hand its closure one operand per input");
                    lang::to_string(part)
                }))
            }
            Err(_) => Err(Poisoned::of(Some(Type::STRING))),
        }
    }

    /// `receiver.eq(other)` or `receiver.less(other)` on a type whose `Eq` or `Ord` the compiler
    /// writes: the operator itself, as `==` and `<` make it. `Err` gives the receiver back when
    /// the call is something else: a method that the type writes, which wins (ADR 0023), or a
    /// method of another name. The arguments are checked as for any method: one count, no label,
    /// and the call takes no type argument.
    fn generated_operator(
        &mut self,
        span: Span,
        receiver: Checked,
        site: Site<'_>,
        args: &[ast::Arg],
    ) -> Result<Checked, Checked> {
        let method = site.name;
        let interface = match method.name.as_str() {
            "eq" => self.lang.eq,
            "less" => self.lang.ord,
            _ => return Err(receiver),
        };
        let Some(ty) = type_of(&receiver) else {
            return Err(receiver);
        };
        // A method counts as written when a block covers `ty`, even if its bounds fail: then
        // `find_method` reports the failed bound, and the generated operator is no fallback (only
        // `to_string` has one). `find_method` repeats this lookup: it needs the buffers that this
        // one fills, and a call of any other method must not pay for them here.
        let written = self.own_method(ty, method.name).is_some() || {
            self.conforming_methods(ty, method.name);
            !self.candidates.is_empty() || self.failed_block.is_some()
        };
        if written
            || matches!(ty.kind(), TypeKind::Param(..) | TypeKind::Assoc(..))
            || self.lang_path(ty, interface) != LangPath::Generated
        {
            return Err(receiver);
        }
        if !site.written.is_empty() {
            self.not_generic(method.name, site.angles);
        }
        if args.len() != 1 {
            let d = DiagnosticKind::ArgumentCount {
                name: method.name.to_string(),
                want: 1,
                found: args.len(),
                at: span,
            };
            return Ok(self.rejected(d.into(), args));
        }
        let labeled = self.labels(args);
        let (other, mismatched) = self.operand(&args[0].value, Some(ty));
        Ok(match (receiver, other) {
            (Ok(lhs), Ok(rhs)) if !labeled => {
                let eq = interface == self.lang.eq;
                // No program shows this label for `less`: only numbers and strings have a
                // generated `Ord`, and no cycle can pass them.
                self.spellings.insert(span, Lowered::Method);
                Ok(self.spilled(span, Type::BOOL, vec![lhs, rhs], |operands| {
                    let [lhs, rhs]: [tir::Expr; 2] = operands
                        .try_into()
                        .expect("`spilled` should hand its closure one operand per input");
                    if eq {
                        lang::equal(
                            Dispatch::Generated,
                            false,
                            Box::new(lhs),
                            Box::new(rhs),
                            span,
                        )
                    } else {
                        tir::ExprKind::Compare {
                            operands: vec![lhs, rhs],
                            ops: vec![ast::CompareOp::Lt],
                        }
                    }
                }))
            }
            _ => Err(Poisoned::of(
                (!mismatched && !labeled).then_some(Type::BOOL),
            )),
        })
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
        // A call from the user's file into the prelude needs the prelude's function in the
        // program: `mono` roots only the user's functions (see `tir::Program::instantiates`).
        if self.signatures[func.0].span.in_prelude() && !span.in_prelude() {
            self.instantiates = true;
        }
        // A method that meets a requirement uses the interfaces that it meets.
        if let Some(interfaces) = self.meets.get(&func).cloned() {
            for interface in interfaces {
                self.record(Used::Interface(interface), span);
            }
        }
        let labeled = self.labels(args);
        let sig = &self.signatures[func.0];
        let (name, params, ret) = (sig.name.clone(), sig.params.clone(), sig.ret);
        let declared = sig.type_params.clone();
        let bounds = sig.bounds.clone();
        let conforming = sig.conformance.is_some();
        // A method's receiver binds its owner's parameters.
        let owner = match receiver.as_ref().and_then(type_of) {
            Some(ty) => self.owner_args(func, ty).get(),
            None => &[],
        };
        // The method of a conformance block is found by the call, so the list a method that takes
        // none of its own is given is reported here, as `method_args` does for the others.
        if conforming && !site.written.is_empty() && declared.len() <= owner.len() {
            self.not_generic(site.name.name, site.angles);
        }
        if params.len() != args.len() {
            return self.wrong_count(span, name, params.len(), args);
        }
        let (checked, types, ret) = if declared.is_empty() {
            let checked = args
                .iter()
                .zip(&params)
                .map(|(arg, &param)| mark(self.operand(&arg.value, param)))
                .collect();
            (checked, TypeArgs::NONE, ret)
        } else {
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
                bounds: &bounds,
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

    /// A call at `span` of the method `name` that wants `want` arguments and has the wrong count:
    /// the count is the error, unless a comparison misread as a call explains it.
    pub(super) fn wrong_count(
        &mut self,
        span: Span,
        name: String,
        want: usize,
        args: &[ast::Arg],
    ) -> Checked {
        self.stray(args);
        if !self.misread_in(args) {
            self.report(DiagnosticKind::ArgumentCount {
                name,
                want,
                found: args.len(),
                at: span,
            });
        }
        Err(Poisoned::error())
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
            let bounds = info.bounds.clone();
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
                bounds: &bounds,
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

/// Whether the `Display` of a value of type `ty` is the compiler's alone: a number, a `bool`, a
/// `char`, a `string`, an array or an optional. A class or an enum may write its own, and a type
/// parameter is a call through its bounds.
fn generated_display_only(ty: Type) -> bool {
    matches!(
        ty.kind(),
        TypeKind::I64
            | TypeKind::U8
            | TypeKind::F64
            | TypeKind::Bool
            | TypeKind::Char
            | TypeKind::String
            | TypeKind::Array(_)
            | TypeKind::Optional(_)
    )
}
