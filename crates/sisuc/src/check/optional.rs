//! The ways to write an optional value: `Some(x)`, `Option.Some(x)`, `Option.None` and
//! `Option<T>.None`. The prelude declares `Option`, but the optional type is built in, so none of
//! these makes an enum value.

use super::alias::Through;
use super::generic::{GenericSig, Site, mark, mismatched};
use super::{Checked, Checker, DiagnosticKind, Poisoned, optional_of, type_of};
use crate::ast;
use crate::diagnostic::{Definition, suggestion};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, Type, TypeKind};

impl Checker<'_> {
    /// `Some(args)`, which `site` writes, or `Option.Some(args)` when `member` is its `Some`:
    /// the call of `Some<T>(x: T) -> T?`, which converts `x` to the optional once. Through an alias
    /// of an optional type that takes parameters, the call binds them: `through` says its type.
    pub(super) fn some(
        &mut self,
        span: Span,
        site: Site<'_>,
        member: Option<&ast::Ident>,
        args: &[ast::Arg],
        through: Option<&Through>,
    ) -> Checked {
        // Built only for a diagnostic.
        let path = if let Some(member) = member {
            format_args!("{}.{}", site.name.name, member.name)
        } else {
            format_args!("{}", site.name.name)
        };
        let Some(labeled) = self.positional_shape(span, &path, 1, args) else {
            return Err(Poisoned::error());
        };
        let open = through.filter(|t| !t.params.is_empty());
        // An alias `type M<T> = T?` is `Option` over its parameter: an expected optional fixes
        // the parameter as it fixes the one of `Some`.
        let plain = open.is_none_or(|t| {
            matches!(t.ty.kind(), TypeKind::Optional(p)
                if t.params.len() == 1 && p == Type::intern(TypeKind::Param(t.params[0], 0)))
        });
        let hinted = self.hinted(span, None).1.is_some();
        let (arg, found) = match site.expected.map(|ty| (ty, ty.kind())) {
            // An expected `T?` fixes `T`, so a literal in the argument takes that type: the
            // generic call would type an `if` of integer literals by their default first.
            Some((want, TypeKind::Optional(payload)))
                if site.written.is_empty() && !want.holds_hole() && plain =>
            {
                let e = &args[0].value;
                // A hinted `Some` gives its hint to the argument (see `Body::hint_at`).
                let outer_hint = self.pass_hint(Some(e.span), hinted.then_some(payload));
                let checked = self.value(e, Some(payload));
                self.body.hint_at = outer_hint;
                match type_of(&checked) {
                    // An `any` argument wider than an `any` hint keeps its type: the join converts
                    // the other values to it.
                    Some(got)
                        if hinted && got != payload && self.any_join(payload, got) == Some(got) =>
                    {
                        (checked, optional_of(got))
                    }
                    _ => (mark(self.settled(e, checked, Some(payload))), want),
                }
            }
            _ => {
                let param = Symbol::intern("T");
                let ty = Type::intern(TypeKind::Param(param, 0));
                let (declared, payload, ret) = match open {
                    Some(t) => match t.ty.kind() {
                        TypeKind::Optional(payload) => (t.params.clone(), payload, t.ty),
                        _ => unreachable!("an alias that builds `Some` should name an optional"),
                    },
                    None => (vec![param], ty, optional_of(ty)),
                };
                let sig = GenericSig {
                    name: site.name,
                    variant: member,
                    declared: &declared,
                    owner: &[],
                    params: &[Some(payload)],
                    ret: Some(ret),
                    bounds: &[],
                };
                let written = (site.written, site.angles);
                let Some((args, list, Some(found))) =
                    self.generic_call(span, &sig, written, args, site.expected)
                else {
                    return Err(Poisoned::error());
                };
                if let Some(t) = open
                    && self.use_checks(&t.checks, list, span) != Some(true)
                {
                    return Err(Poisoned::error());
                }
                let arg = (args.into_iter().next())
                    .expect("`Some` should have the one argument it checked");
                (arg, found)
            }
        };
        let failed = labeled || mismatched(&arg);
        match arg {
            Ok(inner) if !failed => Ok(tir::Expr {
                kind: tir::ExprKind::Wrap(Box::new(inner)),
                ty: found,
                span,
            }),
            _ => Err(Poisoned::of((!failed).then_some(found))),
        }
    }

    /// `Option.member`, or `Option.member(args)` when `call` gives the arguments and the span of
    /// their parentheses, where `site` writes `Option` with the type argument it may have: the
    /// optional built, or an error.
    pub(super) fn option_path(
        &mut self,
        span: Span,
        site: Site<'_>,
        member: &ast::Ident,
        call: Option<(&[ast::Arg], Span)>,
        through: Option<&Through>,
    ) -> Checked {
        match (member.name.as_str(), call) {
            ("Some", Some((args, _))) => self.some(span, site, Some(member), args, through),
            ("None", None) if let Some(t) = through.filter(|t| !t.params.is_empty()) => {
                let sig = GenericSig {
                    name: site.name,
                    variant: Some(member),
                    declared: &t.params,
                    owner: &[],
                    params: &[],
                    ret: Some(t.ty),
                    bounds: &[],
                };
                let written = (site.written, site.angles);
                let Some((_, list, Some(found))) =
                    self.generic_call(span, &sig, written, &[], site.expected)
                else {
                    return Err(Poisoned::error());
                };
                if self.use_checks(&t.checks, list, span) != Some(true) {
                    return Err(Poisoned::error());
                }
                self.none(span, Some(found))
            }
            ("None", None) => {
                let mut expected = site.expected;
                if !site.written.is_empty() {
                    let param = [Symbol::intern("T")];
                    let Some(args) = self.type_args(site.name, &param, site.written, site.angles)
                    else {
                        return Err(Poisoned::error());
                    };
                    expected = Some(optional_of(args.get()[0]));
                }
                self.none(span, expected)
            }
            ("Some" | "None", _) => {
                let payload = member.name.as_str() == "Some";
                let (held, at) = match call {
                    Some((args, parens)) => (
                        !args.is_empty() || self.comment_in(&[parens.range()]),
                        parens,
                    ),
                    None => (false, member.span),
                };
                self.report(DiagnosticKind::VariantForm {
                    path: format!("{}.{}", site.name.name, member.name),
                    payload,
                    held,
                    at,
                });
                self.stray(call.map_or(&[], |(args, _)| args));
                Err(Poisoned::error())
            }
            _ => {
                let variants = [("None", Definition::Variant), ("Some", Definition::Variant)];
                self.report(DiagnosticKind::UnknownVariant {
                    enum_name: site.name.name.to_string(),
                    name: member.name.to_string(),
                    at: member.span,
                    suggestion: suggestion(member.name.as_str(), variants),
                });
                self.stray(call.map_or(&[], |(args, _)| args));
                Err(Poisoned::error())
            }
        }
    }
}
