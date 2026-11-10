//! The ways to write an optional value: `Some(x)`, `Option.Some(x)`, `Option.None` and
//! `Option<T>.None`. The prelude declares `Option`, but the optional type is built in, so none of
//! these makes an enum value.

use super::alias::Through;
use super::any::is_target;
use super::generic::{GenericSig, Site, mark, mismatched, written_args};
use super::mut_args::Callee;
use super::{Checked, Checker, DiagnosticKind, Expectation, Poisoned, optional_of, type_of};
use crate::ast;
use crate::diagnostic::{Definition, suggestion};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, Type, TypeArgs, TypeKind};

impl Checker<'_> {
    /// `Some(args)`, which `site` writes, or `Option.Some(args)` when `member` is its `Some`:
    /// the call of `Some<T>(x: T) -> T?`, which converts `x` to the optional once. `through` is the
    /// alias that the path names. An alias with no parameter fixes the type of the call. An alias
    /// that takes parameters, the `open` ones, has them bound by the call.
    pub(super) fn some(
        &mut self,
        span: Span,
        site: Site<'_>,
        member: Option<&ast::Ident>,
        args: &[ast::Arg],
        through: Option<&Through>,
    ) -> Checked {
        let checked = self.some_of(span, site, member, args, through);
        if checked.is_err() {
            self.body.failed_some_spans.insert(span);
        }
        checked
    }

    /// `some`, before it records a `Some` that gave an error.
    #[expect(
        clippy::too_many_lines,
        reason = "one match on the hint and the payload, so the cases of a `Some` read in one place"
    )]
    fn some_of(
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
        self.refuse_mut(
            args,
            Callee::Construct {
                shown: &path,
                class: false,
                labeled: false,
            },
        );
        let open = through.filter(|t| !t.params.is_empty());
        let Some(labeled) = self.positional_shape(span, &path, 1, args) else {
            let (declared, _, ret) = some_shape(open);
            let fixed = closed_alias(through)
                .or_else(|| self.counted_result(Some(ret), &declared, &[], &[], site));
            return Err(Poisoned::of(fixed));
        };
        // An alias `type M<T> = T?` is `Option` over its parameter: an expected optional fixes
        // the parameter as it fixes the one of `Some`.
        let plain = open.is_none_or(|t| {
            matches!(t.ty.kind(), TypeKind::Optional(p)
                if t.params.len() == 1 && p == Type::intern(TypeKind::Param(t.params[0], 0)))
        });
        let hinted = matches!(site.expected, Expectation::Hint { .. });
        let (arg, found, list) = match site.expected.ty().map(|ty| (ty, ty.kind())) {
            // An expected `T?` fixes `T`, so a literal in the argument takes that type: the
            // generic call would type an `if` of integer literals by their default first.
            Some((want, TypeKind::Optional(payload)))
                if site.written.is_empty() && !want.holds_hole() && plain =>
            {
                let e = &args[0].value;
                // A hinted `Some` gives its hint to the argument (see `Expectation::Hint`).
                let checked = if hinted {
                    let hint = Expectation::Hint {
                        want: payload,
                        siblings: payload,
                    };
                    self.value(e, hint)
                } else {
                    self.value(e, Expectation::Required(payload))
                };
                match type_of(&checked) {
                    // An object that does not convert to the hint keeps its own type: the join
                    // converts it to an `any` type, or the other values to it, or reports it. So
                    // does a `Some` of one, which kept its type. So does an optional beside an
                    // `any` type, which does not convert: the join reports the `Some`.
                    Some(got)
                        if hinted
                            && got != payload
                            && (matches!(
                                beneath(got).kind(),
                                TypeKind::Class(..) | TypeKind::Any(..)
                            ) || matches!(got.kind(), TypeKind::Optional(_))
                                && is_target(payload))
                            && !self.converts_to_any(got, payload) =>
                    {
                        #[cfg(test)]
                        super::reached("a hinted Some keeps its type");
                        (checked, optional_of(got), TypeArgs::NONE)
                    }
                    _ => (
                        mark(self.settled(e, checked, Expectation::Required(payload))),
                        want,
                        TypeArgs::NONE,
                    ),
                }
            }
            _ => {
                let (declared, payload, ret) = some_shape(open);
                let sig = GenericSig {
                    name: site.name,
                    variant: member,
                    declared: &declared,
                    owner: &[],
                    params: &[Some(payload)],
                    ret: Some(ret),
                    bounds: &[],
                    mut_site: None,
                };
                let written = (site.written, site.angles);
                let (args, list, found) =
                    self.generic_call(span, &sig, written, args, site.expected)?;
                let Some(found) = found else {
                    return Err(Poisoned::error());
                };
                if let Some(t) = open
                    && self.use_checks(&t.checks, list, span) != Some(true)
                {
                    return Err(Poisoned::error());
                }
                let arg = (args.into_iter().next())
                    .expect("`Some` should have the one argument it checked");
                (arg, found, list)
            }
        };
        let failed = labeled || mismatched(&arg);
        match arg {
            Ok(inner) if !failed => Ok(tir::Expr::new(
                tir::ExprKind::Wrap(Box::new(inner)),
                found,
                span,
            )),
            _ => {
                // No argument settles the payload in a call with a failed part.
                Err(Poisoned::of(self.failed_some(
                    (through, open),
                    site,
                    list.get(),
                )))
            }
        }
    }

    /// The type of a `Some` at `site`, through `through`, whose argument fails. `open` is
    /// `through` when it takes parameters. `list` is the type arguments that the call found. An
    /// alias with no open parameter fixes the type, as does a written list; else `None`, as no
    /// argument settles the payload in a call with a failed part.
    fn failed_some(
        &mut self,
        (through, open): (Option<&Through>, Option<&Through>),
        site: Site<'_>,
        list: &[Type],
    ) -> Option<Type> {
        closed_alias(through).or_else(|| {
            let (declared, _, ret) = some_shape(open);
            self.fixed_result(
                Some(ret),
                declared.len(),
                &[],
                &[],
                written_args(site, list, 0),
            )
        })
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
        match (member.name, call) {
            (Symbol::SOME, Some((args, _))) => self.some(span, site, Some(member), args, through),
            (Symbol::NONE, None) if let Some(t) = through.filter(|t| !t.params.is_empty()) => {
                let sig = GenericSig {
                    name: site.name,
                    variant: Some(member),
                    declared: &t.params,
                    owner: &[],
                    params: &[],
                    ret: Some(t.ty),
                    bounds: &[],
                    mut_site: None,
                };
                let written = (site.written, site.angles);
                let (_, list, found) =
                    self.generic_call(span, &sig, written, &[], site.expected)?;
                let Some(found) = found else {
                    return Err(Poisoned::error());
                };
                if self.use_checks(&t.checks, list, span) != Some(true) {
                    return Err(Poisoned::error());
                }
                self.none(span, Expectation::Required(found))
            }
            (Symbol::NONE, None) => {
                let mut expected = site.expected;
                if !site.written.is_empty() {
                    let param = [Symbol::intern("T")];
                    let Some(args) = self.type_args(site.name, &param, site.written, site.angles)
                    else {
                        return Err(Poisoned::error());
                    };
                    expected = Expectation::Required(optional_of(args.get()[0]));
                }
                self.none(span, expected)
            }
            (Symbol::SOME | Symbol::NONE, _) => {
                let payload = member.name == Symbol::SOME;
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

/// The type that `through`, an alias with no open parameter, names: it fixes the type of a `Some`
/// through it, whatever the arguments are.
fn closed_alias(through: Option<&Through>) -> Option<Type> {
    through.filter(|t| t.params.is_empty()).map(|t| t.ty)
}

/// `ty` without its optional layers.
fn beneath(ty: Type) -> Type {
    match ty.kind() {
        TypeKind::Optional(payload) => beneath(payload),
        _ => ty,
    }
}

/// The type parameters of `Some`, the type of its payload and its result: the alias's `open`
/// parameters and type, else `T`, `T` and `T?`.
fn some_shape(open: Option<&Through>) -> (Vec<Symbol>, Type, Type) {
    let Some(t) = open else {
        let param = Symbol::intern("T");
        let ty = Type::intern(TypeKind::Param(param, 0));
        return (vec![param], ty, optional_of(ty));
    };
    match t.ty.kind() {
        TypeKind::Optional(payload) => (t.params.clone(), payload, t.ty),
        _ => unreachable!("an alias that builds `Some` should name an optional"),
    }
}
