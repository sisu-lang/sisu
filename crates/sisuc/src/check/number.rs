//! The methods on `i64` and `f64`, the constants on `f64`, the math written another way, and the
//! warning for a comparison with `f64.nan`.

use std::f64::consts;

use super::expr::{no_member, postfix, typed};
use super::generic::Site;
use super::{Checked, Checker, Declared, Poisoned, UNKNOWN, built_in_type};
use crate::ast;
use crate::diagnostic::{Definition, DiagnosticKind, MathFix, NanFix};
use crate::source::Span;
use crate::tir::{self, MathOp, Type, TypeKind};

/// Each method of `f64`: its operation and its parameter count. Every parameter is an `f64`. The
/// result is `bool` for the `is_` methods and `f64` for the rest. `i64`'s `min`, `max` and
/// `clamp` are the prelude's.
const METHODS: [(MathOp, usize); 25] = [
    (MathOp::Abs, 0),
    (MathOp::Sqrt, 0),
    (MathOp::Floor, 0),
    (MathOp::Ceil, 0),
    (MathOp::Trunc, 0),
    (MathOp::Round, 0),
    (MathOp::Sin, 0),
    (MathOp::Cos, 0),
    (MathOp::Tan, 0),
    (MathOp::Asin, 0),
    (MathOp::Acos, 0),
    (MathOp::Atan, 0),
    (MathOp::Atan2, 1),
    (MathOp::Exp, 0),
    (MathOp::Ln, 0),
    (MathOp::Log10, 0),
    (MathOp::Log2, 0),
    (MathOp::Pow, 1),
    (MathOp::Hypot, 1),
    (MathOp::Min, 1),
    (MathOp::Max, 1),
    (MathOp::Clamp, 2),
    (MathOp::IsNan, 0),
    (MathOp::IsFinite, 0),
    (MathOp::IsInfinite, 0),
];

/// The constants on `f64`: `f64.pi` is `PI`.
const CONSTANTS: [(&str, f64); 8] = [
    ("pi", consts::PI),
    ("tau", consts::TAU),
    ("e", consts::E),
    ("infinity", f64::INFINITY),
    ("nan", f64::NAN),
    ("max", f64::MAX),
    ("min_positive", f64::MIN_POSITIVE),
    ("epsilon", f64::EPSILON),
];

/// The name of the method `name` that `f64` has.
pub(super) fn method_name(name: &str) -> Option<&'static str> {
    METHODS
        .iter()
        .map(|(op, _)| op.name())
        .find(|op| *op == name)
}

/// The operation and parameter count of the method `name` of a receiver of type `ty`, if the
/// compiler writes one: only `f64` has any.
pub(super) fn method(ty: Type, name: &str) -> Option<(MathOp, usize)> {
    METHODS
        .iter()
        .find(|(op, _)| ty == Type::F64 && op.name() == name)
        .copied()
}

/// The constant that another language's name means: `PI`, `Infinity` and `inf` are `pi` and
/// `infinity`.
pub(super) fn constant_named(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    let lower = if lower == "inf" { "infinity" } else { &lower };
    CONSTANTS
        .iter()
        .map(|(constant, _)| *constant)
        .find(|constant| *constant == lower)
}

impl Checker<'_> {
    /// `base.member`, or `base.member(args)` when `call` gives the arguments and the span of their
    /// parentheses, where `base` names an enum, whatever local tried to take its name: the enum's
    /// variant `member`, or an error. Otherwise, where `base` names no local:
    /// - a built-in type: the `f64` constant `member`, or, for any other member, an error whose
    ///   candidates are that type's constants (only `f64` has any);
    /// - `math` or `Math`, or `f64` in a call: the math idiom of another language, reported.
    ///
    /// `None` when `base` is no such name, or `member` is no constant or method of the idiom.
    pub(super) fn type_path(
        &mut self,
        span: Span,
        base: &ast::Expr,
        member: &ast::Ident,
        call: Option<(&[ast::Arg], Span)>,
        expected: Option<Type>,
    ) -> Option<Checked> {
        let (ident, written, angles) = match &base.kind {
            ast::ExprKind::Name(name) => (
                ast::Ident {
                    name: *name,
                    span: base.span,
                },
                &[][..],
                None,
            ),
            ast::ExprKind::Applied {
                name,
                types,
                angles,
            } => (name.clone(), &types[..], Some(*angles)),
            _ => return None,
        };
        let name = ident.name;
        // A local cannot take an enum's name, so the enum wins over one that tried.
        match self.enum_ids.get(&name) {
            Some(&Declared::Known(id)) => {
                let generic = !self.enums[id.0].type_params.is_empty();
                if let Some(angles) = angles.filter(|_| !generic) {
                    self.not_generic(name, angles);
                }
                let site = Site {
                    name: &ident,
                    written,
                    angles: angles.unwrap_or(base.span),
                    expected,
                };
                return Some(self.enum_path(span, id, member, call, site));
            }
            Some(Declared::Broken) => {
                self.stray(call.map_or(&[], |(args, _)| args));
                return Some(Err(Poisoned::error()));
            }
            None if name.as_str() == "Option" => {
                let site = Site {
                    name: &ident,
                    written,
                    angles: angles.unwrap_or(base.span),
                    expected,
                };
                return Some(self.option_path(span, site, member, call));
            }
            None => {}
        }
        // Any other base with type arguments is checked as a value, which reports them.
        if angles.is_some() || self.lookup(name).is_some() {
            return None;
        }
        let written = base.span.to(member.span);
        let declared = self.constants.contains_key(&name)
            || self.function_ids.contains_key(&name)
            || self.class_ids.contains_key(&name)
            || self.interface_ids.contains_key(&name);
        if matches!(name.as_str(), "math" | "Math") && !declared {
            if let Some((args, _)) = call {
                return self.math_call(written, member, args);
            }
            // A bare method has no value to write; a constant's name maps to `f64`'s.
            let fix = match method_name(member.name.as_str()) {
                Some(_) => None,
                None => Some(MathFix::Constant {
                    name: constant_named(member.name.as_str())?,
                }),
            };
            self.report_math(written, fix);
            return Some(Err(Poisoned::error()));
        }
        let ty = built_in_type(name.as_str())?;
        if let Some((args, _)) = call {
            return if ty == Type::F64 {
                self.math_call(written, member, args)
            } else {
                None
            };
        }
        let constants: &[(&str, f64)] = if ty == Type::F64 { &CONSTANTS } else { &[] };
        let Some(&(_, x)) = constants
            .iter()
            .find(|(constant, _)| *constant == member.name.as_str())
        else {
            let names = constants.iter().map(|(constant, _)| *constant);
            let d = no_member(Definition::Constant, ty.to_string(), member, names);
            self.diagnostics.push(d);
            return Some(Err(Poisoned::error()));
        };
        Some(Ok(tir::Expr {
            kind: tir::ExprKind::Float(x),
            ty: Type::F64,
            span,
        }))
    }

    /// Reports the math idiom written at `written`, with `fix` if it has one.
    fn report_math(&mut self, written: Span, fix: Option<MathFix>) {
        self.report(DiagnosticKind::MathIdiom { written, fix });
    }

    /// `written(args)`, where `written` is `math.member`, `Math.member` or `f64.member`: the
    /// idiom when `member` is a method of `f64`, else `None`. The fix is `receiver.member(rest)`,
    /// offered only where that surely checks and the rewrite deletes no comment: the first
    /// argument checks and has the method, and the others fit its parameters. Each argument is
    /// checked once.
    fn math_call(
        &mut self,
        written: Span,
        member: &ast::Ident,
        args: &[ast::Arg],
    ) -> Option<Checked> {
        let name = method_name(member.name.as_str())?;
        let fix = self.method_fix(written, name, args);
        self.report_math(written, fix);
        Some(Err(Poisoned::error()))
    }

    /// The fix of `written(args)`, the call of the method `name`, or `None` where it is no
    /// safe rewrite. Checks each argument once.
    fn method_fix(
        &mut self,
        written: Span,
        name: &'static str,
        args: &[ast::Arg],
    ) -> Option<MathFix> {
        let labeled = args.iter().any(|arg| arg.label.is_some());
        let Some((first, rest)) = args.split_first().filter(|_| !labeled) else {
            self.stray(args);
            return None;
        };
        let receiver = self.expr(&first.value, Some(UNKNOWN));
        let Some((ty, params)) = receiver
            .ok()
            .and_then(|r| Some((r.ty, self.built_in_arity(r.ty, name)?)))
        else {
            self.stray(rest);
            return None;
        };
        let (receiver, next) = (first.value.span, rest.first().map(|arg| arg.value.span));
        let deleted = [
            written.start()..receiver.start(),
            receiver.end()..next.map_or_else(|| receiver.end(), Span::start),
        ];
        let arguments = self.number_arguments(ty, params, rest);
        if !arguments || self.comment_in(&deleted) {
            return None;
        }
        let wrap = !postfix(&first.value);
        let (open, close) = if wrap { ("(", ")") } else { ("", "") };
        let text = self.source.file.slice(receiver.range());
        let tail = rest.last().map_or("", |last| {
            self.source
                .file
                .slice(rest[0].value.span.start()..last.value.span.end())
        });
        Some(MathFix::Method {
            name,
            form: format!("{open}{text}{close}.{name}({tail})"),
            receiver,
            next,
            wrap,
        })
    }

    /// Checks `rest`, the arguments after a receiver of type `ty` of a method with `params`
    /// parameters, each once. Whether the method takes them: as many as it has, each of type `ty`
    /// (an integer literal for an `f64`), and none failed.
    pub(super) fn number_arguments(&mut self, ty: Type, params: usize, rest: &[ast::Arg]) -> bool {
        let checked: Vec<_> = rest
            .iter()
            .map(|arg| self.expr(&arg.value, Some(ty)))
            .collect();
        rest.len() == params
            && checked
                .iter()
                .all(|checked| checked.as_ref().is_ok_and(|checked| checked.ty == ty))
    }

    /// Warns about `lhs == rhs`, or `!=` when `negated`, when a side is `f64.nan`: no number
    /// equals it, so the answer is fixed. Each side is its source and its checked form. An
    /// `f64` on the other side is tested with `is_nan()`; an `f64?` is not, as the call would
    /// not check it. A side that failed, or has another type, is reported elsewhere.
    pub(super) fn nan_comparison(
        &mut self,
        negated: bool,
        lhs: (&ast::Expr, &Checked),
        rhs: (&ast::Expr, &Checked),
    ) {
        // `f64.nan` is the only member that checks to a `nan`.
        let is_nan = |(e, checked): (&ast::Expr, &Checked)| {
            matches!(e.kind, ast::ExprKind::Field { .. })
                && matches!(checked, Ok(tir::Expr { kind: tir::ExprKind::Float(x), .. }) if x.is_nan())
        };
        let other = if is_nan(lhs) {
            rhs
        } else if is_nan(rhs) {
            lhs
        } else {
            return;
        };
        let fix = match other.1.as_ref().ok().map(|checked| checked.ty) {
            Some(Type::F64) => Some(NanFix {
                other: other.0.span,
                wrap: !postfix(other.0),
            }),
            Some(ty) if matches!(ty.kind(), TypeKind::Optional(Type::F64)) => None,
            _ => return,
        };
        let at = lhs.0.span.to(rhs.0.span);
        self.body.nans += 1;
        self.diagnostics
            .push(DiagnosticKind::NanComparison { at, negated, fix }.into());
    }

    /// `receiver.method(args)` on an `f64`, or on an `i64` that the prelude gives no method of
    /// that name (see `method_call`), `ty`. Each argument is checked with `ty` as its expected
    /// type, so `x.pow(2)` makes the `2` an `f64`.
    pub(super) fn number_method(
        &mut self,
        span: Span,
        receiver: Checked,
        ty: Type,
        method: &ast::Ident,
        args: &[ast::Arg],
    ) -> Checked {
        // An `i64` reaches here when the prelude gives it no method of the name.
        let Some((op, params)) = self::method(ty, method.name.as_str()) else {
            let owner = self.type_name(ty);
            let names: Vec<&str> = if ty == Type::F64 {
                METHODS.iter().map(|(op, _)| op.name()).collect()
            } else {
                self.prelude_method_names(ty)
            };
            return self.rejected(no_member(Definition::Method, owner, method, names), args);
        };
        if args.len() != params {
            let d = DiagnosticKind::ArgumentCount {
                name: method.name.to_string(),
                want: params,
                found: args.len(),
                at: span,
            };
            return self.rejected(d.into(), args);
        }
        let labeled = self.labels(args);
        let (checked, mismatched): (Vec<_>, Vec<_>) = args
            .iter()
            .map(|arg| self.operand(&arg.value, Some(ty)))
            .unzip();
        let result = match op {
            MathOp::IsNan | MathOp::IsFinite | MathOp::IsInfinite => Type::BOOL,
            _ => Type::F64,
        };
        let failed = labeled || mismatched.contains(&true);
        let operands: Result<Vec<_>, Poisoned> = std::iter::once(receiver).chain(checked).collect();
        // Never an owned reference, so no operand needs `spill`.
        let kind = operands
            .ok()
            .filter(|_| !failed)
            .map(|operands| tir::ExprKind::Math { op, operands });
        typed(span, result, failed, kind)
    }
}
