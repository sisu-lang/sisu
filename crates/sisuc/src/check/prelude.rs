//! The prelude: the functions every file sees without an import, `print`, `eprint`, `panic`
//! and `read_line`. No top-level function, class or constant takes one of their names. `todo`, a
//! keyword that panics as `panic` does, is checked here too.

use super::array::pair;
use super::expr::typed;
use super::{Checked, Checker, Poisoned, payload, type_of};
use crate::ast;
use crate::diagnostic::{DiagnosticKind, MismatchFix};
use crate::source::Span;
use crate::tir::{self, Stream, Type, TypeKind};

/// The prelude's names.
pub(super) const PRELUDE: [&str; 4] = ["print", "eprint", "panic", "read_line"];

impl Checker<'_> {
    /// A call of the prelude function `callee`; `None` when `callee` names none.
    pub(super) fn prelude_call(
        &mut self,
        span: Span,
        callee: &ast::Ident,
        args: &[ast::Arg],
    ) -> Option<Checked> {
        match callee.name.as_str() {
            "print" => Some(self.print(span, args, Stream::Stdout)),
            "eprint" => Some(self.print(span, args, Stream::Stderr)),
            // The panic is the callee's, not the parentheses' around the call.
            "panic" => Some(self.panic(Span::new(callee.span.start(), span.end()), args)),
            "read_line" => Some(self.read_line(span, args)),
            _ => None,
        }
    }

    /// `print(x)` or `eprint(x)`, by `stream`.
    fn print(&mut self, span: Span, args: &[ast::Arg], stream: Stream) -> Checked {
        let function = stream.name();
        // Not `value()`: a `unit` argument needs this code, not "expression has no value".
        let wrong = || {
            DiagnosticKind::PrintArgument {
                function,
                at: span,
                class: None,
            }
            .into()
        };
        let labeled = self.labels(args);
        let [arg] = args else {
            self.diagnostics.push(wrong());
            self.stray(args);
            return Err(Poisoned::error());
        };
        let checked = self.expr(&arg.value, None);
        let error = match type_of(&checked) {
            Some(ty) if matches!(payload(ty).kind(), TypeKind::Array(_)) => {
                DiagnosticKind::PrintArray {
                    function,
                    ty: self.type_name(ty),
                    optional: matches!(ty.kind(), TypeKind::Optional(_)),
                    at: arg.value.span,
                }
                .into()
            }
            Some(ty) if matches!(ty.kind(), TypeKind::Class(..) | TypeKind::Enum(..)) => {
                DiagnosticKind::PrintArgument {
                    function,
                    at: arg.value.span,
                    class: Some(self.type_name(ty)),
                }
                .into()
            }
            Some(ty) if ty == Type::UNIT || matches!(ty.kind(), TypeKind::Optional(_)) => wrong(),
            Some(Type::NEVER) => DiagnosticKind::PrintNever {
                function,
                at: arg.value.span,
            }
            .into(),
            _ => {
                let kind = checked
                    .ok()
                    .filter(|_| !labeled)
                    .map(|operand| tir::ExprKind::Print {
                        stream,
                        operand: Box::new(operand),
                    });
                return typed(span, Type::UNIT, labeled, kind);
            }
        };
        self.diagnostics.push(error);
        Err(Poisoned::error())
    }

    /// `panic(message)`, which never finishes.
    fn panic(&mut self, span: Span, args: &[ast::Arg]) -> Checked {
        let labeled = self.labels(args);
        let [arg] = args else {
            return self.argument_count("panic", 1, span, args);
        };
        let message = self.message(&arg.value);
        panics(span, labeled, message)
    }

    /// `todo` or `todo(message)`, at `span`, whose keyword is at `keyword`: it panics as `panic`
    /// does, with Rust's `todo!` messages, and always gets the warning that it is left in the
    /// program.
    pub(super) fn todo(&mut self, span: Span, keyword: Span, args: &[ast::Arg]) -> Checked {
        self.report(DiagnosticKind::TodoLeft { at: keyword });
        let labeled = self.labels(args);
        let text = |text: &str| tir::Expr {
            kind: tir::ExprKind::Str(text.to_string()),
            ty: Type::STRING,
            span: keyword,
        };
        let message = match args {
            [] => Ok(text("not yet implemented")),
            [arg] => self.message(&arg.value).map(|message| {
                let operands = vec![text("not yet implemented: "), message];
                self.spilled(keyword, Type::STRING, operands, |operands| {
                    let (lhs, rhs) = pair(operands);
                    tir::ExprKind::Concat { lhs, rhs }
                })
            }),
            _ => return self.argument_count("todo", 1, span, args),
        };
        panics(keyword, labeled, message)
    }

    /// `read_line()`, of type `string?`; it takes no arguments.
    fn read_line(&mut self, span: Span, args: &[ast::Arg]) -> Checked {
        if !args.is_empty() {
            return self.argument_count("read_line", 0, span, args);
        }
        let ty = Type::optional(Type::STRING);
        typed(span, ty, false, Some(tir::ExprKind::ReadLine))
    }

    /// `name`'s count error: it takes `want` arguments, and `args` is not that many.
    fn argument_count(
        &mut self,
        name: &str,
        want: usize,
        span: Span,
        args: &[ast::Arg],
    ) -> Checked {
        self.report(DiagnosticKind::ArgumentCount {
            name: name.to_string(),
            want,
            found: args.len(),
            at: span,
        });
        self.stray(args);
        Err(Poisoned::error())
    }

    /// A `panic` or `todo` message, an expression of type `string`. A value that interpolation
    /// prints, on one line, gets the fix that wraps it in a literal.
    pub(super) fn message(&mut self, arg: &ast::Expr) -> Checked {
        let checked = self.value(arg, Some(Type::STRING));
        let got = match type_of(&checked) {
            Some(Type::STRING) | None => return checked,
            Some(got) => got,
        };
        let printed = matches!(
            got,
            Type::I64 | Type::U8 | Type::F64 | Type::BOOL | Type::CHAR
        );
        self.report(DiagnosticKind::Mismatch {
            want: vec![self.type_name(Type::STRING)],
            got: self.type_name(got),
            at: arg.span,
            source: None,
            fix: (printed && self.one_line(arg.span))
                .then_some(MismatchFix::WrapInString(arg.span)),
            conversion: false,
        });
        Err(Poisoned::error())
    }
}

/// The panic with `message`, which reports at the start of `span`; `labeled` when its
/// arguments had a label.
fn panics(span: Span, labeled: bool, message: Checked) -> Checked {
    // A message that failed is the failure of this rule, so the call is `Error`.
    let failed = labeled || message.is_err();
    let kind = message
        .ok()
        .filter(|_| !labeled)
        .map(|message| tir::ExprKind::Panic(Box::new(message)));
    typed(span, Type::NEVER, failed, kind)
}
