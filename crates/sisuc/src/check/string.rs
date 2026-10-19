//! `string`: interpolation, `+` that joins two, a chain of comparisons over them, `len()`,
//! `chars()`, `bytes()` and `words()`, and the unsafe fix to interpolate a value that `+` or
//! `string(x)` meets.
//! Conversions: `i64(x)`, `u8(x)`, `char(x)` and `f64(x)` between the scalar types, and
//! `string(cs)` from a `[char]`.

use std::fmt::Write;

use super::array::pair;
use super::expr::{local, no_member, path, typed};
use super::{Checked, Checker, Poisoned, block_expr, type_of};
use crate::ast::{self, CompareOp, ExprKind};
use crate::diagnostic::{
    Definition, Diagnostic, DiagnosticKind, InterpolateHelp, MismatchFix, MismatchSource, Operand,
};
use crate::source::Span;
use crate::tir::{self, Type, TypeKind};

impl Checker<'_> {
    /// `l + r` over two `string`s, which `mismatched` says were reported against.
    pub(super) fn concat(
        &mut self,
        span: Span,
        l: Checked,
        r: Checked,
        mismatched: bool,
    ) -> Checked {
        // A join with an `Error` operand is `Error`: that operand may have been meant as a
        // number, and a `char` or an expected type beside the join would report it again.
        let known = !mismatched && type_of(&l).is_some() && type_of(&r).is_some();
        let (Ok(l), Ok(r)) = (l, r) else {
            return Err(Poisoned::of(known.then_some(Type::STRING)));
        };
        Ok(self.spilled(span, Type::STRING, vec![l, r], |operands| {
            let (lhs, rhs) = pair(operands);
            tir::ExprKind::Concat { lhs, rhs }
        }))
    }

    /// A string literal with interpolations. Each part is checked with no expected type, so that
    /// a `unit` one is reported here, and must be of a type that interpolation writes as text. A
    /// part that never finishes makes the whole never finish.
    pub(super) fn interpolate(&mut self, span: Span, parts: &[ast::Part]) -> Checked {
        let mut ty = Type::STRING;
        let mut checked = Vec::with_capacity(parts.len());
        let mut formats = Vec::new();
        for (i, ast::Part { value: part, spec }) in parts.iter().enumerate() {
            let value = self.expr(part, None);
            let help = match type_of(&value).map(Type::kind) {
                None
                | Some(
                    TypeKind::I64
                    | TypeKind::U8
                    | TypeKind::F64
                    | TypeKind::Bool
                    | TypeKind::Char
                    | TypeKind::String,
                ) => {
                    if let (Some(spec), Some(found)) = (spec, type_of(&value)) {
                        formats.push(self.format(i, part, spec, found));
                    }
                    checked.push(value);
                    continue;
                }
                Some(TypeKind::Never) => {
                    ty = Type::NEVER;
                    checked.push(value);
                    continue;
                }
                Some(TypeKind::Optional(payload))
                    if !matches!(payload.kind(), TypeKind::Array(_)) =>
                {
                    Some(InterpolateHelp::Unwrap)
                }
                Some(TypeKind::Array(_) | TypeKind::Optional(_)) => Some(InterpolateHelp::ForLoop),
                Some(TypeKind::Unit | TypeKind::Class(_) | TypeKind::Enum(_)) => None,
            };
            let found = type_of(&value).expect("a part reported here should have a type");
            self.report(DiagnosticKind::CannotInterpolate {
                ty: self.type_name(found),
                help,
                at: part.span,
            });
            checked.push(Err(Poisoned::error()));
        }
        match checked.into_iter().collect::<Result<Vec<_>, _>>() {
            Ok(parts) => Ok(
                self.spilled(span, ty, parts, |parts| tir::ExprKind::Interpolate {
                    parts,
                    formats,
                }),
            ),
            Err(_) => Err(Poisoned::typed(ty)),
        }
    }

    /// The format of the part `i`, `part`, of type `ty`, that `spec` gives. Without an alignment,
    /// a number pads on the left and any other value on the right. Only an `f64` takes a
    /// precision.
    fn format(
        &mut self,
        i: usize,
        part: &ast::Expr,
        spec: &ast::FormatSpec,
        ty: Type,
    ) -> tir::Format {
        let numeric = matches!(ty, Type::I64 | Type::U8 | Type::F64);
        let pad = spec.width.map(|width| tir::Pad {
            align: spec.align.unwrap_or(if numeric {
                tir::Align::Right
            } else {
                tir::Align::Left
            }),
            width,
        });
        let mut precision = spec.precision;
        if precision.is_some() && ty != Type::F64 {
            self.report(DiagnosticKind::PrecisionNeedsF64 {
                ty: self.type_name(ty),
                at: spec.span,
                fix: (ty == Type::I64).then_some(part.span),
            });
            precision = None;
        }
        tir::Format {
            part: i,
            pad,
            precision,
        }
    }

    /// The operand `e` of the `+` over `operands`, of type `got`, which is not the `string` the
    /// other operand makes it; that operand gets a secondary label. A value that interpolation
    /// prints gets the unsafe fix to interpolate it, unless the `+` is a `compound` assignment's or
    /// a constant's, a comment between the operands would go with the text the unsafe fix removes,
    /// or an operand spans a line break, which a string literal cannot hold.
    pub(super) fn concat_mismatch(
        &self,
        e: &ast::Expr,
        got: Type,
        operands: &[&ast::Expr],
        compound: bool,
    ) -> Diagnostic {
        let printed = matches!(
            got,
            Type::I64 | Type::U8 | Type::F64 | Type::BOOL | Type::CHAR | Type::STRING
        );
        let [lhs, rhs] = operands else {
            unreachable!("`+` should have two operands");
        };
        let between = lhs.span.end()..rhs.span.start();
        let commented = self.source.comments.iter().any(|c| between.contains(c));
        let lines = operands.iter().any(|e| !self.one_line(e.span));
        // A literal in parentheses starts at its `(`, and is wrapped whole like any other value.
        let operand = |e: &ast::Expr| Operand {
            span: e.span,
            literal: matches!(e.kind, ExprKind::Str(_) | ExprKind::Interpolate(_))
                && self.source.quotes.binary_search(&e.span.start()).is_ok(),
        };
        let unsafe_fix =
            !(compound || self.body.in_constant.is_some() || !printed || commented || lines);
        let other = if e.span == lhs.span {
            rhs.span
        } else {
            lhs.span
        };
        DiagnosticKind::Mismatch {
            want: vec![self.type_name(Type::STRING)],
            got: self.type_name(got),
            at: e.span,
            source: Some(MismatchSource::Concat(other)),
            fix: unsafe_fix.then(|| MismatchFix::Interpolate {
                lhs: operand(lhs),
                rhs: operand(rhs),
            }),
            conversion: false,
        }
        .into()
    }

    /// Whether `span` holds no line break, which a string literal cannot hold. A comment inside
    /// it would end at one, so it holds no comment either, and fits inside `{}`.
    pub(super) fn one_line(&self, span: Span) -> bool {
        let next = self.source.breaks.partition_point(|&b| b < span.start());
        self.source
            .breaks
            .get(next)
            .is_none_or(|&b| b >= span.end())
    }

    /// A chain over the `string`s `operands`, which codegen compares two at a time. Two make
    /// one `Compare`; more bind each operand once, in nested blocks that stop at the first pair
    /// that fails. `a < b < c` is
    /// `(block (let #0 a) (let #1 b) (&& (< #0 #1) (block (let #2 c) (< #1 #2))))`.
    pub(super) fn string_chain(
        &mut self,
        span: Span,
        operands: Vec<tir::Expr>,
        ops: &[CompareOp],
    ) -> tir::Expr {
        if let [op] = ops {
            return self.spilled(span, Type::BOOL, operands, |operands| {
                tir::ExprKind::Compare {
                    operands,
                    ops: vec![*op],
                }
            });
        }
        let mut lets = Vec::new();
        let reads: Vec<_> = operands
            .into_iter()
            .map(|init| {
                let read = (self.fresh(Type::STRING), init.span);
                lets.push(tir::Stmt::Let {
                    local: read.0,
                    init,
                });
                read
            })
            .collect();
        let read = |i: usize| local(reads[i].0, Type::STRING, reads[i].1);
        let pair = |i: usize| tir::Expr {
            kind: tir::ExprKind::Compare {
                operands: vec![read(i), read(i + 1)],
                ops: vec![ops[i]],
            },
            ty: Type::BOOL,
            span,
        };
        // From the innermost block out: each binds the right operand of the pair it tests.
        let mut rest = pair(ops.len() - 1);
        for i in (1..ops.len()).rev() {
            let bind = lets.pop().expect("each operand should have its `let`");
            let inner = block_expr(vec![bind], Some(rest), Type::BOOL, span);
            rest = and(pair(i - 1), inner, span);
        }
        block_expr(lets, Some(rest), Type::BOOL, span)
    }

    /// `receiver.method(args)` on a `string`: `len()`, `chars()`, `bytes()` and `words()`.
    pub(super) fn string_method(
        &mut self,
        span: Span,
        receiver: Checked,
        method: &ast::Ident,
        args: &[ast::Arg],
    ) -> Checked {
        let (ty, node): (_, fn(Box<tir::Expr>) -> tir::ExprKind) = match method.name.as_str() {
            "len" => (Type::I64, tir::ExprKind::Len),
            "chars" => (Type::array(Type::CHAR), tir::ExprKind::Chars),
            "bytes" => (Type::array(Type::U8), tir::ExprKind::Bytes),
            "words" => (Type::array(Type::STRING), tir::ExprKind::Words),
            _ => {
                let owner = self.type_name(Type::STRING);
                let d = no_member(
                    Definition::Method,
                    owner,
                    method,
                    ["len", "chars", "bytes", "words"],
                );
                return self.rejected(d, args);
            }
        };
        if !args.is_empty() {
            let d = DiagnosticKind::ArgumentCount {
                name: method.name.to_string(),
                want: 0,
                found: args.len(),
                at: span,
            };
            return self.rejected(d.into(), args);
        }
        let kind = receiver.ok().map(|r| node(Box::new(r)));
        typed(span, ty, false, kind)
    }

    /// `to(arg)`: `i64(x)` takes a `u8`, a `char` or an `f64`; `u8(x)`, `char(x)` and `f64(x)`
    /// take an `i64`; and `string(x)` takes a `[char]`. The argument of a scalar conversion
    /// expects no type, so a literal one is an `i64`; `string([])` is the empty string. `u8(x)`,
    /// `char(x)` and `i64(x)` from an `f64` can fail, so they give an optional, and in a constant
    /// they are an error, typed as `to`.
    pub(super) fn convert(
        &mut self,
        span: Span,
        name: Span,
        to: Type,
        args: &[ast::Arg],
    ) -> Checked {
        let labeled = self.labels(args);
        let [arg] = args else {
            self.report(DiagnosticKind::ArgumentCount {
                name: to.to_string(),
                want: 1,
                found: args.len(),
                at: span,
            });
            self.stray(args);
            return Err(Poisoned::error());
        };
        let chars = [Type::array(Type::CHAR)];
        let from: &[Type] = match to {
            Type::I64 => &[Type::U8, Type::CHAR, Type::F64],
            Type::STRING => &chars,
            _ => &[Type::I64],
        };
        let checked = self.value(&arg.value, (to == Type::STRING).then_some(chars[0]));
        let fails = matches!(to, Type::U8 | Type::CHAR)
            || (to == Type::I64 && type_of(&checked) == Some(Type::F64));
        // The head, `to(`, and the tail, `)`, are the text a rewrite of the call deletes.
        let head = span.start()..arg.value.span.start();
        let tail = arg.value.span.end()..span.end();
        let d = match type_of(&checked) {
            Some(ty) if ty == to => {
                let grouped = !single_term(&arg.value);
                // The removal deletes the head, from the name when the value stays grouped, and
                // the tail unless it does.
                let deleted = if grouped {
                    name.start()..head.end
                } else {
                    head.clone()
                };
                let commented = self
                    .source
                    .comments
                    .iter()
                    .any(|c| deleted.contains(c) || (!grouped && tail.contains(c)));
                DiagnosticKind::AlreadyConverted {
                    value: path(&arg.value).or_else(|| literal(&arg.value)),
                    ty: self.type_name(to),
                    at: arg.value.span,
                    conversion: span,
                    name,
                    grouped,
                    labeled,
                    commented,
                }
                .into()
            }
            // The one type `x` should have is offered a conversion to it, or for a `string`,
            // interpolation of the value.
            Some(ty) if !from.contains(&ty) => match *from {
                [only] if to == Type::STRING => {
                    self.string_call_mismatch(span, &arg.value, ty, only, &[head, tail])
                }
                [only] => self.mismatch(arg.value.span, Some(&arg.value), only, ty),
                _ => DiagnosticKind::Mismatch {
                    want: from.iter().map(|&ty| self.type_name(ty)).collect(),
                    got: self.type_name(ty),
                    at: arg.value.span,
                    source: None,
                    fix: None,
                    conversion: false,
                }
                .into(),
            },
            _ if fails && self.body.in_constant.is_some() => {
                let fix = self.body.in_constant.as_ref().and_then(|site| {
                    let arg = checked.as_ref().ok()?;
                    site.literal(span, to, arg)
                });
                self.report(DiagnosticKind::ConversionInConstant {
                    to: self.type_name(to),
                    at: span,
                    fix,
                });
                return Err(Poisoned::typed(to));
            }
            _ => {
                let node = if to == Type::STRING {
                    tir::ExprKind::FromChars
                } else {
                    tir::ExprKind::Convert
                };
                let kind = checked
                    .ok()
                    .filter(|_| !labeled)
                    .map(|arg| node(Box::new(arg)));
                let ty = if fails { Type::optional(to) } else { to };
                return typed(span, ty, labeled, kind);
            }
        };
        self.diagnostics.push(d);
        Err(Poisoned::error())
    }

    /// `string(arg)` where `arg`, of type `got`, is not a `want`. A value that interpolation
    /// prints is offered `"{arg}"` instead, unless a comment sits in `deleted`, the text the
    /// rewrite replaces, or the argument spans a line break, which a string literal cannot hold.
    fn string_call_mismatch(
        &self,
        call: Span,
        arg: &ast::Expr,
        got: Type,
        want: Type,
        deleted: &[std::ops::Range<usize>],
    ) -> Diagnostic {
        let printed = matches!(
            got,
            Type::I64 | Type::U8 | Type::F64 | Type::BOOL | Type::CHAR
        );
        let commented = self
            .source
            .comments
            .iter()
            .any(|c| deleted.iter().any(|d| d.contains(c)));
        if !printed || commented || !self.one_line(arg.span) {
            return self.mismatch(arg.span, Some(arg), want, got);
        }
        DiagnosticKind::Mismatch {
            want: vec![self.type_name(want)],
            got: self.type_name(got),
            at: arg.span,
            source: None,
            fix: Some(MismatchFix::InterpolateCall {
                call,
                arg: arg.span,
            }),
            conversion: false,
        }
        .into()
    }
}

/// A string literal, for the message, in Sisu's escapes: no other literal is named.
fn literal(e: &ast::Expr) -> Option<String> {
    let ExprKind::Str(text) = &e.kind else {
        return None;
    };
    let mut quoted = String::from('"');
    for c in text.chars() {
        match c {
            '\\' | '"' | '{' | '}' => quoted.extend(['\\', c]),
            '\n' => quoted.push_str("\\n"),
            '\t' => quoted.push_str("\\t"),
            '\r' => quoted.push_str("\\r"),
            '\0' => quoted.push_str("\\0"),
            c if c.is_control() => write!(quoted, "\\u{{{:X}}}", u32::from(c))
                .expect("writing to a `String` should never fail"),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    Some(quoted)
}

/// Whether `e` is one term as written: a name, a literal, a call, a field or index read, or an
/// expression in parentheses. Anything else needs its parentheses once it sits beside other code.
/// A parenthesized node is the only one whose span reaches past its parts.
fn single_term(e: &ast::Expr) -> bool {
    match &e.kind {
        ExprKind::Int { .. }
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Char(_)
        | ExprKind::Str(_)
        | ExprKind::Interpolate(_)
        | ExprKind::Name(_)
        | ExprKind::SelfValue
        | ExprKind::NoneLit
        | ExprKind::Call { .. }
        | ExprKind::Todo { .. }
        | ExprKind::Field { .. }
        | ExprKind::MethodCall { .. }
        | ExprKind::Index { .. }
        | ExprKind::ArrayLit(_)
        | ExprKind::ArrayFill { .. } => true,
        ExprKind::Binary { lhs, rhs, .. } | ExprKind::Coalesce { lhs, rhs } => {
            e.span != lhs.span.to(rhs.span)
        }
        // A `bool`, which no conversion holds: never reached, and grouping is always right.
        ExprKind::Is { .. } | ExprKind::Compare { .. } => false,
        ExprKind::Unary { operand, .. } => e.span.end() != operand.span.end(),
        ExprKind::Block(block) => e.span != block.span,
        // The keyword starts it, unless parentheses do.
        ExprKind::Match { keyword, .. } => e.span.start() != keyword.start(),
        ExprKind::If {
            then_block,
            else_block,
            ..
        }
        | ExprKind::IfLet {
            then_block,
            else_block,
            ..
        } => e.span.end() != else_block.as_ref().unwrap_or(then_block).span.end(),
    }
}

/// `lhs && rhs`, at `span`.
fn and(lhs: tir::Expr, rhs: tir::Expr, span: Span) -> tir::Expr {
    tir::Expr {
        kind: tir::ExprKind::Binary {
            op: tir::BinaryOp::And,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        },
        ty: Type::BOOL,
        span,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::{TokenKind, lex};

    /// Beside other code, a `match` needs its parentheses, and a parenthesized one has them.
    #[test]
    fn a_match_is_one_term_only_in_parentheses() {
        let keyword = Span::new(1, 6);
        let scrutinee = ast::Expr {
            kind: ExprKind::Name(crate::symbol::Symbol::intern("x")),
            span: Span::new(7, 8),
        };
        for (span, single) in [(Span::new(1, 20), false), (Span::new(0, 21), true)] {
            let e = ast::Expr {
                kind: ExprKind::Match {
                    keyword,
                    scrutinee: Box::new(scrutinee.clone()),
                    arms: Vec::new(),
                    close: span,
                },
                span,
            };
            assert_eq!(single_term(&e), single, "{span:?}");
        }
    }

    /// The name of a literal is Sisu that lexes back to the literal's text.
    #[test]
    fn literal_names_lex_back() {
        for text in [
            "hi!",
            "{}",
            "\\\"",
            "a\nb\tc\rd\0e",
            "\u{7}\u{1B}\u{7F}\u{85}",
            "é😀",
        ] {
            let e = ast::Expr {
                kind: ExprKind::Str(text.to_string()),
                span: Span::new(0, 0),
            };
            let name = literal(&e).expect("a string literal has a name");
            let lexed = lex(&name).expect("the name should lex");
            assert_eq!(
                lexed.tokens[0].kind,
                TokenKind::Str(text.to_string()),
                "{name}"
            );
            assert!(lexed.diagnostics.is_empty(), "{name}");
        }
    }
}
