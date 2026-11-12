//! The prelude: the functions every file sees without an import, `print`, `eprint`, `panic`,
//! `read_line` and `dbg`. No top-level function, class or constant takes one of their names.
//! `todo`, a keyword that panics as `panic` does, is checked here too.

use super::array::pair;
use super::expr::{path, read_jump, typed, unlabeled};
use super::mut_args::Callee;
use super::reports::Reported;
use super::{Checked, Checker, Expectation, Origin, Poisoned, type_of};
use crate::ast;
use crate::diagnostic::{DiagnosticKind, Expected, MismatchFix, PrintProblem};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, Stream, Type, TypeKind};

/// The prelude's names.
pub(super) const PRELUDE: &[&str] = &["print", "eprint", "panic", "read_line", "dbg"];

/// Whether the prelude function `name` takes arguments: only `read_line` takes none.
pub(super) fn takes_arguments(name: &str) -> bool {
    name != "read_line"
}

impl Checker<'_> {
    /// Whether `name` is a function of the program or of the prelude.
    pub(super) fn is_function(&self, name: Symbol) -> bool {
        self.names.function(name).is_some() || PRELUDE.contains(&name.as_str())
    }

    /// A call of the prelude function `callee`; `None` when `callee` names none.
    pub(super) fn prelude_call(
        &mut self,
        span: Span,
        callee: &ast::Ident,
        args: &[ast::Arg],
        expected: Expectation,
    ) -> Option<Checked> {
        match callee.name.as_str() {
            "print" => Some(self.print(span, args, Stream::Stdout)),
            "eprint" => Some(self.print(span, args, Stream::Stderr)),
            "dbg" => Some(self.dbg(span, args, expected)),
            // The panic is the callee's, not the parentheses' around the call.
            "panic" => Some(self.panic(Span::new(callee.span.start(), span.end()), args)),
            "read_line" => Some(self.read_line(span, args)),
            _ => None,
        }
    }

    /// `print(x)` or `eprint(x)`, by `stream`.
    fn print(&mut self, span: Span, args: &[ast::Arg], stream: Stream) -> Checked {
        let (arg, checked, labeled) = match self.shown(
            span,
            args,
            stream.name(),
            (Expectation::None, Some(Type::UNIT)),
        ) {
            Ok(shown) => shown,
            Err(failed) => return failed,
        };
        self.warn_if_optional(&arg.value, &checked);
        if let Some(ty) = type_of(&checked) {
            self.record_shown(ty, self.lang.display);
        }
        let kind = unlabeled(checked, labeled).map(|operand| tir::ExprKind::Print {
            stream,
            operand: Box::new(operand),
        });
        typed(span, Type::UNIT, false, kind)
    }

    /// `dbg(x)`, of the type of `x`, which it checks against `expected`. It does not warn when
    /// `x` is an optional: `dbg` is meant to be run.
    fn dbg(&mut self, span: Span, args: &[ast::Arg], expected: Expectation) -> Checked {
        let shown = self.shown(span, args, "dbg", (expected, None));
        let (arg, checked, labeled) = match shown {
            Ok(shown) => shown,
            Err(failed) => return failed,
        };
        // No argument settles `T` in a call with a failed part.
        let ty = match &checked {
            Ok(value) => value.ty,
            Err(failed) => return Err(Poisoned::untyped(failed.reported())),
        };
        self.dbg_shown.push((self.body.owner, ty, span));
        // `dbg<T>(x: T) -> T` has the type of its argument.
        let kind = unlabeled(checked, labeled).map(|operand| tir::ExprKind::Dbg {
            at: arg.value.span,
            text: self.source.text(arg.value.span).to_string(),
            operand: Box::new(operand),
        });
        typed(span, ty, labeled.is_some(), kind)
    }

    /// The argument of a call of `function` (`print`, `eprint` or `dbg`) at `span`, checked with
    /// `expected`, with whether the arguments had a label. Every type shows through `Display` but
    /// `unit` and `never`. `Err` is the call's own result when its argument is wrong, which this
    /// has reported. `context` holds two parts: the type `expected` that the argument must have,
    /// and the type `ret` of the call when it fails. `dbg` has the type of its argument, so its
    /// `ret` is none.
    fn shown<'a>(
        &mut self,
        span: Span,
        args: &'a [ast::Arg],
        function: &'static str,
        (expected, ret): (Expectation, Option<Type>),
    ) -> Result<(&'a ast::Arg, Checked, Option<Reported>), Checked> {
        // Not `value()`: a `unit` argument needs this code, not "expression has no value".
        let wrong = |problem, at| DiagnosticKind::PrintArgument {
            function,
            problem,
            at,
        };
        let labeled = self.labels(args);
        let [arg] = args else {
            let reported = self.diagnostics.error(wrong(PrintProblem::Count, span));
            self.stray(args);
            return Err(Err(Poisoned::new(ret, reported)));
        };
        self.refuse_mut(args, Callee::Unnamed(&function));
        // The argument is an operand, not a value part of the value, so a hint does not reach it.
        let checked = read_jump(self.expr(&arg.value, expected.plain()));
        let error = match type_of(&checked) {
            Some(Type::UNIT) => wrong(PrintProblem::NoValue, arg.value.span),
            Some(Type::NEVER) => DiagnosticKind::PrintNever {
                function,
                at: self.never_at(arg.value.span),
            },
            _ => return Ok((arg, checked, labeled)),
        };
        Err(Err(Poisoned::new(ret, self.diagnostics.error(error))))
    }

    /// Warns when `checked`, the check of the printed `value`, is an optional: the text may be
    /// `None`. Only the type of `value` itself counts, not a type that it holds.
    pub(super) fn warn_if_optional(&mut self, value: &ast::Expr, checked: &Checked) {
        // `Some(x)` is never `None`, also when it gave an error.
        if self.body.failed_some_spans.contains(&value.span)
            || matches!(
                checked,
                Ok(tir::Expr {
                    kind: tir::ExprKind::Wrap(_),
                    ..
                })
            )
        {
            return;
        }
        if matches!(
            type_of(checked).map(Type::kind),
            Some(TypeKind::Optional(_))
        ) {
            self.warn(DiagnosticKind::PrintsOptional {
                value: path(value),
                at: value.span,
            });
        }
    }

    /// `panic(message)`, which never finishes.
    fn panic(&mut self, span: Span, args: &[ast::Arg]) -> Checked {
        self.body.exits += 1;
        let labeled = self.labels(args);
        let [arg] = args else {
            return self.argument_count("panic", 1, span, args, Type::NEVER);
        };
        self.refuse_mut(args, Callee::Unnamed(&"panic"));
        let message = self.message(&arg.value);
        panics(span, labeled, message)
    }

    /// `todo` or `todo(message)`, at `span`, whose keyword is at `keyword`: it panics as `panic`
    /// does, with Rust's `todo!` messages, and always gets the warning that it is left in the
    /// program.
    pub(super) fn todo(&mut self, span: Span, keyword: Span, args: &[ast::Arg]) -> Checked {
        self.body.exits += 1;
        self.body.todo = true;
        self.warn(DiagnosticKind::TodoLeft { at: keyword });
        let labeled = self.labels(args);
        let text = |text: &str| {
            tir::Expr::new(tir::ExprKind::Str(text.to_string()), Type::STRING, keyword)
        };
        if args.len() == 1 {
            self.refuse_mut(args, Callee::Unnamed(&"todo"));
        }
        let message = match args {
            [] => Ok(text("not yet implemented")),
            [arg] => self.message(&arg.value).map(|message| {
                let operands = vec![text("not yet implemented: "), message];
                self.spilled(keyword, Type::STRING, operands, |operands| {
                    let (lhs, rhs) = pair(operands);
                    tir::ExprKind::Concat { lhs, rhs }
                })
            }),
            _ => return self.argument_count("todo", 1, span, args, Type::NEVER),
        };
        panics(keyword, labeled, message)
    }

    /// `read_line()`, of type `string?`; it takes no arguments.
    fn read_line(&mut self, span: Span, args: &[ast::Arg]) -> Checked {
        if !args.is_empty() {
            return self.argument_count("read_line", 0, span, args, Type::optional(Type::STRING));
        }
        let ty = Type::optional(Type::STRING);
        typed(span, ty, false, Ok(tir::ExprKind::ReadLine))
    }

    /// `name`'s count error: it takes `want` arguments, and `args` is not that many. The call has
    /// the type `ret`.
    fn argument_count(
        &mut self,
        name: &str,
        want: usize,
        span: Span,
        args: &[ast::Arg],
        ret: Type,
    ) -> Checked {
        let reported = self.error(DiagnosticKind::ArgumentCount {
            name: name.to_string(),
            want,
            found: args.len(),
            at: span,
        });
        self.stray(args);
        Err(Poisoned::typed(ret, reported))
    }

    /// A `panic` or `todo` message, an expression of type `string`. A value that interpolation
    /// prints, on one line, gets the fix that wraps it in a literal.
    pub(super) fn message(&mut self, arg: &ast::Expr) -> Checked {
        let checked = self.value(arg, Expectation::Required(Type::STRING, Origin::None));
        let got = match type_of(&checked) {
            Some(Type::STRING) | None => return checked,
            Some(got) => got,
        };
        let printed = matches!(
            got,
            Type::I64 | Type::U8 | Type::F64 | Type::BOOL | Type::CHAR
        );
        let reported = self.error(DiagnosticKind::Mismatch {
            want: Expected::Types(vec![self.type_name(Type::STRING)]),
            got: self.type_name(got),
            at: arg.span,
            origin: None,
            fix: (printed && self.one_line(arg.span))
                .then_some(MismatchFix::WrapInString(arg.span)),
            conversion: false,
            layers: false,
        });
        Err(Poisoned::untyped(reported))
    }
}

/// The panic with `message`, which reports at the start of `span`; `labeled` when its
/// arguments had a label.
fn panics(span: Span, labeled: Option<Reported>, message: Checked) -> Checked {
    let kind = unlabeled(message, labeled).map(|message| tir::ExprKind::Panic(Box::new(message)));
    typed(span, Type::NEVER, false, kind)
}
