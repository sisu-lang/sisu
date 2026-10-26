//! The ways to write an optional value: `Some(x)`, `Option.Some(x)`, `Option.None` and
//! `Option<T>.None`. The prelude declares `Option`, but the optional type is built in, so none of
//! these makes an enum value.

use super::generic::{GenericSig, Site, mark, mismatched};
use super::{Checked, Checker, DiagnosticKind, Poisoned, optional_of};
use crate::ast;
use crate::diagnostic::{Definition, suggestion};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, Type, TypeKind};

impl Checker<'_> {
    /// `Some(args)`, which `site` writes, or `Option.Some(args)` when `member` is its `Some`:
    /// the call of `Some<T>(x: T) -> T?`, which converts `x` to the optional once.
    pub(super) fn some(
        &mut self,
        span: Span,
        site: Site<'_>,
        member: Option<&ast::Ident>,
        args: &[ast::Arg],
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
        let (arg, found) = match site.expected.map(|ty| (ty, ty.kind())) {
            // An expected `T?` fixes `T`, so a literal in the argument takes that type: the
            // generic call would type an `if` of integer literals by their default first.
            Some((want, TypeKind::Optional(payload))) if site.written.is_empty() => {
                (mark(self.operand(&args[0].value, Some(payload))), want)
            }
            _ => {
                let param = Symbol::intern("T");
                let ty = Type::intern(TypeKind::Param(param, 0));
                let sig = GenericSig {
                    name: site.name,
                    variant: member,
                    declared: &[param],
                    owner: &[],
                    params: &[Some(ty)],
                    ret: Some(optional_of(ty)),
                    bounds: &[],
                };
                let written = (site.written, site.angles);
                let Some((args, _, Some(found))) =
                    self.generic_call(span, &sig, written, args, site.expected)
                else {
                    return Err(Poisoned::error());
                };
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
    ) -> Checked {
        match (member.name.as_str(), call) {
            ("Some", Some((args, _))) => self.some(span, site, Some(member), args),
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
