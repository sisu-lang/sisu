//! Enum declarations: their variants and payloads, the names they cannot take, which enums no
//! value can be built of, the boxes that break a cycle, and which enums are counted.

use std::{fmt, mem};

use rustc_hash::{FxHashMap, FxHashSet};

use super::alias::Through;
use super::generic::{GenericSig, Site, written_args};
use super::interface::Slot;
use super::mut_args::Callee;
use super::reports::Reported;
use super::unused_names::Used;
use super::{
    Checked, Checker, ClassInfo, Declared, Definition, DiagnosticKind, Expectation, Origin,
    Poisoned, SelfType, own_args,
};
use crate::ast;
use crate::diagnostic::{BuiltIn, Unbuilt, VariantLabels, suggestion};
use crate::graph::components;
use crate::source::{Span, removal_end};
use crate::symbol::Symbol;
use crate::tir::{self, EnumId, Rank, Type, TypeArgs, TypeKind};

pub(super) struct EnumInfo {
    pub(super) name: ast::Ident,
    pub(super) type_params: Vec<Symbol>,
    /// Whether a type parameter is written with a bound.
    pub(super) bounded: bool,
    /// The bounds of each of `type_params`; empty when `bounded` is false.
    pub(super) bounds: Vec<super::Bounds>,
    /// The first variant of each name, in declaration order.
    pub(super) variants: Vec<VariantInfo>,
    /// Each variant's index in `variants`, by name.
    pub(super) index: FxHashMap<Symbol, usize>,
    /// The first method of each name that no variant takes, and every broken one.
    pub(super) methods: FxHashMap<Symbol, Declared<tir::FuncId>>,
    /// The associated types the declaration's body chooses.
    pub(super) assoc: std::rc::Rc<super::Choices>,
}

pub(super) struct VariantInfo {
    pub(super) name: ast::Ident,
    /// The payload's field types; `None` is `Error`.
    pub(super) fields: Vec<Option<Type>>,
    /// Where each field's type is written.
    pub(super) spans: Vec<Span>,
    /// Each field's type as the payload writes it, when it names an alias and the enum takes no
    /// type parameter.
    pub(super) shown: Vec<Option<std::rc::Rc<str>>>,
    /// For each field, the class and enum types written in its type, each with its span.
    pub(super) inner: Vec<Vec<(Type, Span)>>,
    /// The payload's labels, one for each field.
    pub(super) labels: Vec<Option<Symbol>>,
    /// The token of the report of labels that are neither all present nor all absent.
    pub(super) mixed: Option<Reported>,
}

/// The name milestone 8's prelude takes, which no item, variant or binding may take, and what
/// it is there.
pub(super) fn reserved_name(name: Symbol) -> Option<BuiltIn> {
    match name {
        Symbol::OPTION | Symbol::RESULT => Some(BuiltIn::Type),
        Symbol::SOME | Symbol::OK | Symbol::ERR => Some(BuiltIn::Constructor),
        _ => None,
    }
}

/// The enum and struct declarations `ty` names where a value of it holds them inline: `ty`
/// itself, or its optional's payload, and the type arguments of each enum or struct it names,
/// through optionals. Each is a node of `lower_types`: an enum by its id, and a struct after the
/// `enums` enums. With `open`, only those named with an argument that carries a parameter:
/// `C<T>`, not `Wrap3<i64>`. A declaration comes once for each type that names it, in the order
/// of the first visit. Each type is read once, whatever number of paths lead to it.
fn inline_enums(ty: Type, open: bool, enums: usize) -> Vec<usize> {
    fn collect(
        ty: Type,
        (open, enums): (bool, usize),
        seen: &mut FxHashSet<Type>,
        named: &mut Vec<usize>,
    ) {
        if !seen.insert(ty) {
            return;
        }
        #[cfg(test)]
        TYPE_WALK_STEPS.set(TYPE_WALK_STEPS.get() + 1);
        let (node, args) = match ty.kind() {
            TypeKind::Optional(payload) => return collect(payload, (open, enums), seen, named),
            TypeKind::Enum(id, args) => (id.0, args),
            TypeKind::Struct(id, args) => (enums + id.0, args),
            _ => return,
        };
        if !open || ty.has_param() {
            named.push(node);
        }
        for &arg in args.get() {
            collect(arg, (open, enums), seen, named);
        }
    }
    let mut named = Vec::new();
    collect(ty, (open, enums), &mut FxHashSet::default(), &mut named);
    named
}

/// Whether `ty` names, where `inline_enums` looks, the enum or struct at node `e` applied to its
/// own `count` parameters in order: `Tree<T>` in `Tree`, or `G<T>` in `G`'s `Wrap2<G<T>>`. The
/// nodes are those of `inline_enums`, with the structs after the `enums` enums. Each type is read
/// once.
fn names_itself(ty: Type, (e, count): (usize, usize), enums: usize) -> bool {
    fn find(
        ty: Type,
        (e, count, enums): (usize, usize, usize),
        seen: &mut FxHashSet<Type>,
    ) -> bool {
        // A type seen before gave `false`: a `true` ends the whole search.
        if !seen.insert(ty) {
            return false;
        }
        #[cfg(test)]
        TYPE_WALK_STEPS.set(TYPE_WALK_STEPS.get() + 1);
        let (node, args) = match ty.kind() {
            TypeKind::Optional(payload) => return find(payload, (e, count, enums), seen),
            TypeKind::Enum(id, args) => (id.0, args.get()),
            TypeKind::Struct(id, args) => (enums + id.0, args.get()),
            _ => return false,
        };
        let own = |(i, arg): (usize, &Type)| matches!(arg.kind(), TypeKind::Param(_, j) if j as usize == i);
        (node == e && args.len() == count && args.iter().enumerate().all(own))
            || args.iter().any(|&arg| find(arg, (e, count, enums), seen))
    }
    find(ty, (e, count, enums), &mut FxHashSet::default())
}

/// The `Rank` of a payload or field of type `ty` of the enum or struct at node `e` of
/// `lower_types`, which has `count` type parameters. `declared` gives the component of each node.
fn rank(ty: Type, (e, count): (usize, usize), declared: &[usize], enums: usize) -> Rank {
    if !ty.has_param() {
        Rank::Plain
    } else if names_itself(ty, (e, count), enums) {
        Rank::Own
    } else if inline_enums(ty, true, enums)
        .iter()
        .any(|&n| declared[n] == declared[e])
    {
        Rank::Open
    } else if inline_enums(ty, false, enums)
        .iter()
        .any(|&n| declared[n] == declared[e])
    {
        Rank::Late
    } else if ty.has_assoc() {
        Rank::Assoc
    } else {
        Rank::Never
    }
}

/// What a message calls the enum of the variant `member`, which `site` writes: the qualifier as
/// the user wrote it, which may be an alias. A bare variant, as `Ok`, has none: `site` writes the
/// variant, so the enum's own name, `enum_name`, stands.
fn qualifier(site: Site<'_>, member: &ast::Ident, enum_name: Symbol) -> Symbol {
    // `Checker::cannot_infer` (generic.rs) tests for a bare variant in the same way; change both.
    if site.name.span == member.span {
        enum_name
    } else {
        site.name.name
    }
}

impl Checker<'_> {
    /// Whether `name` is one milestone 8's prelude takes; reports it when it is. The prelude's
    /// own declarations take them without a report.
    pub(super) fn reserved(&mut self, name: &ast::Ident) -> bool {
        if name.span.in_prelude() {
            return false;
        }
        let Some(kind) = reserved_name(name.name) else {
            return false;
        };
        self.error(DiagnosticKind::BuiltInName {
            name: name.name.to_string(),
            at: name.span,
            kind,
        });
        true
    }

    /// Checks the variants of `e`, enum `id`, and resolves their payload types and its method
    /// signatures, adding each method to `declared` with `self`'s type. Its variants and methods
    /// share a namespace; each payload's labels have one of their own.
    pub(super) fn enum_signatures<'a>(
        &mut self,
        e: &'a ast::Enum,
        id: EnumId,
        declared: &mut Vec<(&'a ast::Function, SelfType)>,
    ) {
        let owner = e.name.name.as_str();
        if e.variants.is_empty() {
            self.error(DiagnosticKind::EmptyEnum {
                name: owner.to_string(),
                at: e.name.span,
            });
        }
        let receiver = Type::intern(TypeKind::Enum(id, own_args(&self.enums[id.0].type_params)));
        self.body.self_type = SelfType::Known(receiver);
        self.body.set_type_params(&self.enums[id.0].type_params);
        let bounds = self.declare_bounds(0, Vec::new(), &e.type_params);
        if self.enums[id.0].bounded {
            self.enums[id.0].bounds = bounds;
        }
        // The choices come before the payloads and the signatures, which may name them. The
        // scope stays for the payloads of all variants.
        self.enums[id.0].assoc = self.declare_choices(&e.assoc, owner);
        let mut first = FxHashMap::default();
        for variant in &e.variants {
            let name = &variant.name;
            if name.name.as_str().starts_with(char::is_uppercase) {
                self.reserved(name);
            } else {
                self.error(DiagnosticKind::VariantCase {
                    name: name.name.to_string(),
                    at: name.span,
                });
            }
            let payload = variant.payload.as_deref().unwrap_or_default();
            let labeled = |field: &ast::PayloadField| field.label.is_some();
            let mixed = (payload.iter())
                .find(|field| labeled(field) != labeled(&payload[0]))
                .map(|field| {
                    self.error(DiagnosticKind::MixedLabels {
                        variant: name.name.to_string(),
                        at: field
                            .label
                            .as_ref()
                            .map_or(field.ty.span, |label| label.span),
                    })
                });
            let mut labels = FxHashMap::default();
            for label in payload.iter().filter_map(|field| field.label.as_ref()) {
                self.first_definition(&mut labels, Definition::Field, Some(owner), label);
            }
            // The checks that each field's type queues.
            let mut queued = Vec::new();
            let (mut fields, mut inner, mut shown) = (Vec::new(), Vec::new(), Vec::new());
            for field in payload {
                let from = self.queued();
                let written = self.resolve_field(&field.ty, e.type_params.is_empty());
                queued.push(from..self.queued());
                fields.push(written.ty);
                inner.push(written.inner);
                shown.push(written.shown);
            }
            if self.first_definition(&mut first, Definition::Variant, Some(owner), name) {
                let variant = self.enums[id.0].variants.len();
                for (i, queued) in queued.into_iter().enumerate() {
                    self.keep_at(queued, Slot::Payload(id, variant, i));
                }
                let info = &mut self.enums[id.0];
                info.index.insert(name.name, info.variants.len());
                info.variants.push(VariantInfo {
                    name: name.clone(),
                    fields,
                    spans: payload.iter().map(|field| field.ty.span).collect(),
                    shown,
                    inner,
                    labels: payload
                        .iter()
                        .map(|field| field.label.as_ref().map(|label| label.name))
                        .collect(),
                    mixed,
                });
            }
        }
        self.body.set_type_params(&[]);
        for method in &e.methods {
            let func = tir::FuncId(self.signatures.len());
            if self.first_definition(&mut first, Definition::Method, Some(owner), &method.name) {
                self.enums[id.0]
                    .methods
                    .insert(method.name.name, Declared::Known(func));
            }
            let bounds = self.enums[id.0].bounds.clone();
            self.signature(
                method,
                format!("{owner}.{}", method.name.name),
                &e.type_params,
                bounds,
            );
            declared.push((method, SelfType::Known(receiver)));
        }
        for method in &e.broken_methods {
            let reported = self.proof(method.span); // proof: parser mark
            self.enums[id.0]
                .methods
                .insert(method.name, Declared::Poisoned(reported));
        }
        self.body.self_type = SelfType::Absent;
        self.body.assoc = std::rc::Rc::default();
    }

    /// `Enum.member`, or `Enum.member(args)` when `call` gives the arguments and the span of their
    /// parentheses, where `Enum` is the enum `id`, which `site` writes: the variant `member`
    /// built, or an error. Through an alias, the variant has the type that the alias names, and
    /// the alias's parameters are the type arguments that the arguments bind.
    pub(super) fn enum_path(
        &mut self,
        span: Span,
        id: EnumId,
        member: &ast::Ident,
        call: Option<(&[ast::Arg], Span)>,
        site: Site<'_>,
        through: Option<&Through>,
    ) -> Checked {
        let info = &self.enums[id.0];
        let enum_name = qualifier(site, member, info.name.name);
        let enum_name = enum_name.as_str();
        let Some(&variant) = info.index.get(&member.name) else {
            let reported = match info.methods.get(&member.name) {
                Some(&Declared::Known(func)) => {
                    // An argument at a `mut` parameter gets the help of a place.
                    let muts = self.signatures[func.0].muts.clone();
                    self.stamp_mut_params(call.map_or(&[], |(args, _)| args), &muts);
                    self.error(DiagnosticKind::MemberKind {
                        name: member.name.to_string(),
                        enum_name: enum_name.to_string(),
                        method: true,
                        at: member.span,
                    })
                }
                // The method's own error is reported.
                Some(&Declared::Poisoned(reported)) => reported,
                None => {
                    let variants = info
                        .variants
                        .iter()
                        .map(|v| (v.name.name.as_str(), Definition::Variant));
                    let suggestion = suggestion(member.name.as_str(), variants);
                    self.error(DiagnosticKind::UnknownVariant {
                        enum_name: enum_name.to_string(),
                        name: member.name.to_string(),
                        at: member.span,
                        suggestion,
                    })
                }
            };
            self.stray(call.map_or(&[], |(args, _)| args));
            return Err(Poisoned::untyped(reported));
        };
        self.record(Used::Variant(id, variant), span);
        if let Some((args, _)) = call {
            self.refuse_variant_mut((id, variant), member, site, args);
        }
        let info = &self.enums[id.0];
        // Built only for a diagnostic.
        let path = format_args!(
            "{}.{}",
            qualifier(site, member, info.name.name),
            member.name
        );
        let v = &info.variants[variant];
        let ty = Type::intern(TypeKind::Enum(id, TypeArgs::NONE));
        let generic = !info.type_params.is_empty();
        let args = match call {
            _ if generic && call.is_some() != v.fields.is_empty() => {
                return self.generic_variant_args(span, (id, variant), member, call, site, through);
            }
            None if v.fields.is_empty() => Vec::new(),
            Some((args, _)) if !v.fields.is_empty() => {
                // Checking the arguments needs the whole checker, so the field types are copied.
                let built = if v.labels.iter().all(Option::is_some) {
                    let named: Vec<_> = v
                        .labels
                        .iter()
                        .flatten()
                        .copied()
                        .zip(v.fields.iter().copied())
                        .collect();
                    self.construct(span, &path, &named, args, |_| Origin::None)
                } else if v.labels.iter().all(Option::is_none) {
                    let fields = v.fields.clone();
                    self.positional(span, (&path, id, variant), &fields, args)
                } else {
                    // Mixed labels are reported at the declaration.
                    let reported = v.mixed.expect("mixed labels should be reported");
                    self.stray(args);
                    return Err(Poisoned::untyped(reported));
                };
                built.map_err(|reported| Poisoned::typed(ty, reported))?
            }
            Some((args, parens)) => {
                // The fix deletes the parentheses, so what they hold (even a comment) makes it
                // unsafe.
                let inside = parens.start()..parens.end();
                let reported = self.error(DiagnosticKind::VariantForm {
                    path: path.to_string(),
                    payload: false,
                    held: !args.is_empty() || self.comment_in(&[inside]),
                    at: parens,
                });
                self.stray(args);
                return Err(Poisoned::untyped(reported));
            }
            None => {
                let reported = self.error(DiagnosticKind::VariantForm {
                    path: path.to_string(),
                    payload: true,
                    held: false,
                    at: member.span,
                });
                return Err(Poisoned::untyped(reported));
            }
        };
        Ok(self.spilled(span, ty, args, |args| tir::ExprKind::Variant {
            id,
            variant,
            args,
        }))
    }

    /// E0167 for each `mut` argument of `Enum.member(args)`, a variant of the enum `id`.
    fn refuse_variant_mut(
        &mut self,
        (id, variant): (EnumId, usize),
        member: &ast::Ident,
        site: Site<'_>,
        args: &[ast::Arg],
    ) {
        if args.iter().any(|arg| arg.mut_keyword.is_some()) {
            let info = &self.enums[id.0];
            let shown = format!(
                "{}.{}",
                qualifier(site, member, info.name.name),
                member.name
            );
            let labels = &info.variants[variant].labels;
            let labeled = !labels.is_empty() && labels.iter().all(Option::is_some);
            let callee = Callee::Construct {
                shown: &shown,
                class: false,
                labeled,
            };
            self.refuse_mut(args, callee);
        }
    }

    /// `Enum.member` or `Enum.member(args)`, the variant `variant` of the generic enum `id`
    /// written in its form, `call` holding the arguments when it has a payload: the variant
    /// built with the type arguments its arguments and `site` bind, or an error.
    fn generic_variant_args(
        &mut self,
        span: Span,
        (id, variant): (EnumId, usize),
        member: &ast::Ident,
        call: Option<(&[ast::Arg], Span)>,
        site: Site<'_>,
        through: Option<&Through>,
    ) -> Checked {
        let info = &self.enums[id.0];
        let v = &info.variants[variant];
        // Built only for a diagnostic.
        let path = format_args!(
            "{}.{}",
            qualifier(site, member, info.name.name),
            member.name
        );
        let (declared, bounds, ret, fields) = match through {
            None => (
                info.type_params.clone(),
                info.bounds.clone(),
                Type::intern(TypeKind::Enum(id, own_args(&info.type_params))),
                v.fields.clone(),
            ),
            // The payload has the types that the alias's arguments for the enum's parameters
            // make them.
            Some(through) => {
                let TypeKind::Enum(_, list) = through.ty.kind() else {
                    unreachable!("an alias that builds a variant should name an enum");
                };
                let fields = v.fields.clone();
                let fields = (fields.into_iter())
                    .map(|ty| ty.and_then(|ty| self.subst(ty, list.get())))
                    .collect();
                (through.params.clone(), Vec::new(), through.ty, fields)
            }
        };
        let v = &self.enums[id.0].variants[variant];
        let (labels, mixed) = (v.labels.clone(), v.mixed);
        let args = call.map_or(&[][..], |(args, _)| args);
        let shape = if labels.iter().all(Option::is_some) && !labels.is_empty() {
            let names: Vec<_> = labels.iter().flatten().copied().collect();
            self.shaped(span, &path, &names, args).map(|()| None)
        } else if labels.iter().all(Option::is_none) {
            self.positional_shape(span, &path, fields.len(), args)
        } else {
            // Mixed labels are reported at the declaration.
            self.stray(args);
            return Err(Poisoned::untyped(
                mixed.expect("mixed labels should be reported"),
            ));
        };
        let labeled = match shape {
            Ok(labeled) => labeled,
            Err(reported) => {
                let fixed = self.counted_result(Some(ret), &declared, &[], &bounds, site);
                return Err(Poisoned::new(fixed, reported));
            }
        };
        let sig = GenericSig {
            name: site.name,
            variant: Some(member),
            declared: &declared,
            owner: &[],
            params: &fields,
            ret: Some(ret),
            bounds: &bounds,
            mut_site: None,
            function: None,
        };
        // An alias's type arguments, when the call wrote them, are applied already.
        let written = (
            if through.is_some() { &[] } else { site.written },
            site.angles,
        );
        let (args, list, ty) = self.generic_call(span, &sig, written, args, site.expected)?;
        // The bounds that the alias's body puts on its parameters, on what they are bound to.
        let held = through.map_or(Ok(()), |t| self.use_checks(&t.checks, list, span));
        let ty = self.built_type(ty, held, span)?;
        // A mismatched argument is one that failed.
        let refused = labeled.or_else(|| held.err());
        match args.into_iter().collect::<Result<Vec<_>, Poisoned>>() {
            Ok(args) if refused.is_none() => Ok(self.spilled(span, ty, args, |args| {
                tir::ExprKind::Variant { id, variant, args }
            })),
            args => {
                let reported = (args.err().map(|failed| failed.reported()))
                    .or(refused)
                    .expect("a variant that fails should have a failed part");
                // No argument settles a parameter in a call with a failed part.
                let written = written_args(site, list.get(), 0);
                self.failed_build((held.is_ok(), reported), (ret, declared.len()), written)
            }
        }
    }

    /// Reports the labels of `path(args)`, a variant whose payload declares none, and a count
    /// of `args` that is not `want`. `Err` with the token of the count when it is wrong, else the
    /// token of the labels' report, when there was a label.
    pub(super) fn positional_shape(
        &mut self,
        span: Span,
        path: &dyn fmt::Display,
        want: usize,
        args: &[ast::Arg],
    ) -> Result<Option<Reported>, Reported> {
        // Each `label: ` and the white space after it, which the fix deletes. A comment between
        // the label and the `:`, or after the `:`, stays: the cut stops at it, so a comment before
        // the `:` makes two cuts.
        let comments = self.source.comments;
        let mut labels = Vec::new();
        for arg in args {
            let (Some(label), Some(colon)) = (&arg.label, arg.colon) else {
                continue;
            };
            let before = removal_end(comments, label.span.end(), colon.start());
            let after = removal_end(comments, colon.end(), arg.value.span.start());
            if before == colon.start() {
                labels.push(Span::new(label.span.start(), after));
            } else {
                labels.push(Span::new(label.span.start(), before));
                labels.push(Span::new(colon.start(), after));
            }
        }
        let labeled = (args.iter().find_map(|arg| arg.label.as_ref())).map(|first| {
            self.error(DiagnosticKind::LabeledArgument {
                at: first.span,
                variant: Some(VariantLabels {
                    path: path.to_string(),
                    labels,
                }),
            })
        });
        if want != args.len() {
            self.stray(args);
            return Err(match self.misread_in(args) {
                Some(reported) => reported,
                None => self.error(DiagnosticKind::ArgumentCount {
                    name: path.to_string(),
                    want,
                    found: args.len(),
                    at: span,
                }),
            });
        }
        Ok(labeled)
    }

    /// The arguments of `path(args)`, the variant `variant` of the enum `id`, whose payload
    /// declares no labels, checked against its `fields` in order. `Err` with the token of a
    /// mistake, here or in an argument.
    fn positional(
        &mut self,
        span: Span,
        (path, id, variant): (&dyn fmt::Display, EnumId, usize),
        fields: &[Option<Type>],
        args: &[ast::Arg],
    ) -> Result<Vec<tir::Expr>, Reported> {
        let labeled = self.positional_shape(span, path, fields.len(), args)?;
        // A mismatched argument is one that failed. Each argument is checked, so this collects
        // into a `Vec` before it gives up.
        let checked: Vec<_> = (args.iter().zip(fields).enumerate())
            .map(|(i, (arg, field))| {
                // A mismatch shows a field's alias as written.
                let text = self.enums[id.0].variants[variant].shown[i].clone();
                if let (Some(want), Some(text)) = (*field, text) {
                    self.show(arg.value.span, want, &text);
                }
                let origin = Origin::Payload {
                    enum_: Origin::id(id.0),
                    variant: Origin::id(variant),
                    field: Origin::id(i),
                };
                self.expect(&arg.value, Expectation::or_poisoned(*field, origin))
            })
            .collect();
        match (
            checked.into_iter().collect::<Result<Vec<_>, Poisoned>>(),
            labeled,
        ) {
            (Ok(checked), None) => Ok(checked),
            (Err(failed), _) => Err(failed.reported()),
            (Ok(_), Some(reported)) => Err(reported),
        }
    }

    /// The prelude's `Result` and the variant it declares for `name`, when `name` is `Ok` or
    /// `Err`, which a program writes without the enum.
    pub(super) fn result_variant(&self, name: Symbol) -> Option<(EnumId, usize)> {
        if !matches!(name, Symbol::OK | Symbol::ERR) {
            return None;
        }
        let Some(Declared::Known(id)) = self.names.enum_id(Symbol::RESULT) else {
            return None;
        };
        Some((id, *self.enums[id.0].index.get(&name)?))
    }

    /// The enums with a variant `name`, by name, in declaration order. A duplicate or broken
    /// enum has no variants anyone can name.
    pub(super) fn variant_of(&self, name: Symbol) -> Vec<String> {
        self.enums
            .iter()
            .enumerate()
            .filter(|(i, e)| {
                matches!(self.names.enum_id(e.name.name), Some(Declared::Known(id)) if id.0 == *i)
                    && e.variants.iter().any(|v| v.name.name == name)
            })
            .map(|(_, e)| e.name.name.to_string())
            .collect()
    }

    /// Lowers every enum and every struct, with each payload or field's `Rank` from its declared
    /// type, and lays them out (`tir::layout_types`).
    pub(super) fn lower_types(&self) -> (Vec<tir::Enum>, Vec<tir::Struct>) {
        // The declarations each enum's and struct's generic fields name, inline (see
        // `Rank::Late`), the structs after the enums.
        let enums = self.enums.len();
        let named = |fields: &mut dyn Iterator<Item = Type>| -> Vec<usize> {
            let generic = fields.filter(|ty| ty.has_param());
            generic
                .flat_map(|ty| inline_enums(ty, false, enums))
                .collect()
        };
        let enum_names = self.enums.iter().map(|info| {
            named(
                &mut info
                    .variants
                    .iter()
                    .flat_map(|v| v.fields.iter().copied().flatten()),
            )
        });
        let struct_names =
            (self.structs.iter()).map(|info| named(&mut info.fields.iter().filter_map(|f| f.ty)));
        let declared = components(&enum_names.chain(struct_names).collect::<Vec<_>>());
        let mut lowered: Vec<tir::Enum> = (self.enums.iter().enumerate())
            .map(|(e, info)| tir::Enum {
                name: info.name.name.to_string(),
                span: info.name.span,
                instance: false,
                type_params: info.type_params.iter().map(Symbol::to_string).collect(),
                variants: (info.variants.iter())
                    .map(|v| tir::Variant {
                        name: v.name.name.to_string(),
                        fields: (v.fields.iter().zip(&v.labels))
                            .map(|(&ty, label)| {
                                // An `Error` field's error stops the program before codegen.
                                let ty = ty.unwrap_or(Type::NEVER);
                                let own = (e, info.type_params.len());
                                let rank = rank(ty, own, &declared, enums);
                                tir::Payload {
                                    ty,
                                    label: label.as_ref().map(Symbol::to_string),
                                    boxed: false,
                                    rank,
                                }
                            })
                            .collect(),
                    })
                    .collect(),
                counted: false,
                cycle: None,
                reaches: false,
            })
            .collect();
        let mut structs: Vec<tir::Struct> = (self.structs.iter().enumerate())
            .map(|(s, info)| tir::Struct {
                name: info.name.name.to_string(),
                span: info.name.span,
                instance: false,
                type_params: info.type_params.iter().map(ToString::to_string).collect(),
                fields: (info.fields.iter())
                    .map(|f| {
                        // An `Error` field's error stops the program before codegen.
                        let ty = f.ty.unwrap_or(Type::NEVER);
                        let own = (enums + s, info.type_params.len());
                        tir::Payload {
                            ty,
                            label: Some(f.name.name.to_string()),
                            boxed: false,
                            rank: rank(ty, own, &declared, enums),
                        }
                    })
                    .collect(),
                counted: false,
                cycle: None,
                reaches: false,
            })
            .collect();
        tir::layout_types(&mut lowered, &mut structs);
        (lowered, structs)
    }

    /// Reports each enum, struct or class no value can be built of. Which can be built is a
    /// least fixed point over them all, with the arguments they are given. A class or a struct
    /// can when every field's type can, and an enum when every field of some variant can. Every
    /// other type always can. A class in `cyclic` counts as built, and so does an enum with no
    /// variant, since each has its error already. The fixed point finds what each one needs.
    /// Each component of the rest that needs no other is reported once. The report is at its
    /// first enum or struct, or else its first class; a type that only needs one of those is not.
    /// Returns, by node (the classes, the enums, then the structs), whether it is in a component
    /// reported here, or is a class in `cyclic`: whether it has an error of that kind of its own.
    pub(super) fn never_built(&mut self, cyclic: &[bool]) -> Vec<bool> {
        let mut fix = Buildable::new(self, cyclic);
        fix.run();
        let classes = self.classes.len();
        let nodes = fix.slots.len();
        let unbuilt = |fix: &Buildable, n: usize| !fix.built[n];
        let needs: Vec<Vec<usize>> = (0..nodes)
            .map(|n| {
                if unbuilt(&fix, n) {
                    fix.needs(n)
                } else {
                    Vec::new()
                }
            })
            .collect();
        let component = components(&needs);
        let mut leaves = vec![true; nodes];
        for (n, wants) in needs.iter().enumerate() {
            if wants.iter().any(|&w| component[w] != component[n]) {
                leaves[component[n]] = false;
            }
        }
        let mut reported = vec![false; nodes];
        for n in (classes..nodes).chain(0..classes) {
            if !unbuilt(&fix, n) || !leaves[component[n]] || reported[component[n]] {
                continue;
            }
            reported[component[n]] = true;
            let (links, first_struct) = self.chain(n, &mut fix);
            let name = self.node_info(n).0;
            self.error(DiagnosticKind::NeverConstructed {
                name: name.name.to_string(),
                at: name.span,
                cause: Unbuilt::Chain {
                    links,
                    first_struct,
                },
            });
        }
        (0..nodes)
            .map(|n| reported[component[n]] || n < classes && cyclic[n])
            .collect()
    }

    /// The chain of fields from the enum, class or struct at node `start`, through types none of
    /// which can be built by `fix`. It gives each link's holder (`E.V` or `K.f`) and the type held,
    /// with the arguments the chain gives it. At an enum it takes the first variant's first such
    /// field, and at a class or a struct the first such field. It ends at the first type it meets
    /// again. It also gives the first struct on the chain, if there is one, with its arguments.
    fn chain(&self, start: usize, fix: &mut Buildable) -> (Vec<(String, String)>, Option<String>) {
        let classes = self.classes.len();
        let params = self.node_info(start).1;
        // A field named `name` of type `ty`, if that type cannot be built: its name, its type's
        // name, and its node and arguments.
        let mut unbuilt = |(name, ty): (&str, Type)| {
            let (n, args) = fix.node_of(ty)?;
            let key = fix.key_of(n, args);
            (!fix.built[key]).then(|| (name.to_string(), self.type_name(ty), (n, args), key))
        };
        let mut chain = Vec::new();
        let mut first_struct = None;
        // Asking for a key's flags can add keys to `fix`, so this is a set.
        let mut on_chain = FxHashSet::default();
        let (mut at, mut args) = (start, own_args(params));
        let mut key = start;
        while on_chain.insert(key) {
            let with = args.get();
            let held = |ty: &Option<Type>| ty.and_then(|ty| self.subst(ty, with));
            let (holder, field) = if let Some(class) = self.class_node(at) {
                let mut fields = class
                    .fields
                    .iter()
                    .filter_map(|f| Some((f.name.name.as_str(), held(&f.ty)?)));
                (&class.name, fields.find_map(&mut unbuilt))
            } else {
                let e = &self.enums[at - classes];
                let mut fields = e.variants.first().into_iter().flat_map(|v| {
                    let name = v.name.name.as_str();
                    v.fields
                        .iter()
                        .filter_map(move |ty| Some((name, held(ty)?)))
                });
                (&e.name, fields.find_map(&mut unbuilt))
            };
            let (name, ty, next, next_key) =
                field.expect("a type no value can be built of should need another such type");
            if first_struct.is_none()
                && let Some(s) = at.checked_sub(classes + self.enums.len())
            {
                let ty = Type::intern(TypeKind::Struct(tir::StructId(s), args));
                first_struct = Some(self.type_name(ty));
            }
            chain.push((format!("{}.{name}", holder.name), ty));
            ((at, args), key) = (next, next_key);
        }
        (chain, first_struct)
    }

    /// The name and type parameters of node `n` of `never_built`: a class, an enum, then a struct.
    fn node_info(&self, n: usize) -> (&ast::Ident, &[Symbol]) {
        if let Some(info) = self.class_node(n) {
            (&info.name, &info.type_params)
        } else {
            let info = &self.enums[n - self.classes.len()];
            (&info.name, &info.type_params)
        }
    }

    /// The class or struct at node `n` of `never_built`, which have the same table; `None` for
    /// an enum.
    fn class_node(&self, n: usize) -> Option<&ClassInfo> {
        let enums = self.classes.len() + self.enums.len();
        match n.checked_sub(enums) {
            Some(s) => Some(&self.structs[s]),
            None => self.classes.get(n),
        }
    }

    /// What a value of each enum, class and struct may hold. The nodes are the enums, then the
    /// classes, then the structs; a field holds the node its array and optional wrappers end at.
    /// A field of the poison type marks its node poison; a class field marks an enum or a struct
    /// `class`. Each fact then spreads to every node that holds a marked one: `class` through
    /// enums and structs only, `poison` through every node.
    pub(super) fn holds_table(&self) -> HoldsTable {
        let enums = self.enums.len();
        let structs = enums + self.classes.len();
        let mut poison = vec![false; structs + self.structs.len()];
        // A class node never takes `class`: it is a class object itself.
        let mut class = vec![false; poison.len()];
        // Who holds each node.
        let mut holders = vec![Vec::new(); poison.len()];
        let enum_fields = self.enums.iter().enumerate().flat_map(|(i, e)| {
            e.variants
                .iter()
                .flat_map(move |v| v.fields.iter().map(move |&ty| (i, ty)))
        });
        let class_fields = self
            .classes
            .iter()
            .enumerate()
            .flat_map(|(i, c)| c.fields.iter().map(move |f| (enums + i, f.ty)));
        let struct_fields = (self.structs.iter().enumerate())
            .flat_map(|(i, s)| s.fields.iter().map(move |f| (structs + i, f.ty)));
        let mut seen = FxHashSet::default();
        for (holder, ty) in enum_fields.chain(class_fields).chain(struct_fields) {
            let Some(ty) = ty else {
                poison[holder] = true;
                continue;
            };
            // A field holds its type's node and the nodes of its type arguments, through array
            // and optional wrappers. It reads each type once, a wrapper too, whatever number of
            // paths lead to it.
            seen.clear();
            let mut types = vec![ty];
            while let Some(ty) = types.pop() {
                if !seen.insert(ty) {
                    continue;
                }
                #[cfg(test)]
                TYPE_WALK_STEPS.set(TYPE_WALK_STEPS.get() + 1);
                match ty.kind() {
                    TypeKind::Array(inner) | TypeKind::Optional(inner) => types.push(inner),
                    TypeKind::Class(id, args) => {
                        class[holder] |= !(enums..structs).contains(&holder);
                        holders[enums + id.0].push(holder);
                        types.extend_from_slice(args.get());
                    }
                    TypeKind::Enum(id, args) => {
                        holders[id.0].push(holder);
                        types.extend_from_slice(args.get());
                    }
                    TypeKind::Struct(id, args) => {
                        holders[structs + id.0].push(holder);
                        types.extend_from_slice(args.get());
                    }
                    _ => {}
                }
            }
        }
        mark_holders(&mut poison, &holders);
        mark_holders(&mut class, &holders);
        let holds = |i: usize| Holds {
            class: class[i],
            poison: poison[i],
        };
        HoldsTable {
            enums: (0..enums).map(holds).collect(),
            structs: (structs..poison.len()).map(holds).collect(),
            classes: poison[enums..structs].to_vec(),
            cache: std::cell::RefCell::default(),
        }
    }

    /// What a value of `ty` may hold, with its type arguments, through array and optional
    /// wrappers. Each type is decided once for the whole check, a wrapper too: `holds_table` is
    /// set before the first body is checked and never changes.
    pub(super) fn holds(&self, mut ty: Type) -> Holds {
        let mut wrappers = Vec::new();
        let held = loop {
            if let Some(&held) = self.holds_table.cache.borrow().get(&ty) {
                break held;
            }
            #[cfg(test)]
            TYPE_WALK_STEPS.set(TYPE_WALK_STEPS.get() + 1);
            if let TypeKind::Array(inner) | TypeKind::Optional(inner) = ty.kind() {
                wrappers.push(ty);
                ty = inner;
                continue;
            }
            let held = self.holds_of(ty);
            wrappers.push(ty);
            break held;
        };
        self.holds_table
            .cache
            .borrow_mut()
            .extend(wrappers.into_iter().map(|wrapper| (wrapper, held)));
        held
    }

    /// `holds` for a type that is no array or optional.
    fn holds_of(&self, ty: Type) -> Holds {
        let (own, args) = match ty.kind() {
            TypeKind::Enum(id, args) => (self.holds_table.enums[id.0], args),
            TypeKind::Struct(id, args) => (self.holds_table.structs[id.0], args),
            TypeKind::Class(id, args) => {
                let poison = self.holds_table.classes[id.0];
                (
                    Holds {
                        class: true,
                        poison,
                    },
                    args,
                )
            }
            _ => return Holds::default(),
        };
        args.get().iter().fold(own, |held, &arg| {
            let arg = self.holds(arg);
            Holds {
                class: held.class || arg.class,
                poison: held.poison || arg.poison,
            }
        })
    }
}

/// What a value may hold: a class object, or the poison type.
#[derive(Clone, Copy, Default)]
pub(super) struct Holds {
    pub(super) class: bool,
    pub(super) poison: bool,
}

/// `Holds` for each enum, by `EnumId`, and each struct, by `StructId`, and whether each class, by
/// `ClassId`, holds the poison type.
#[derive(Default)]
pub(super) struct HoldsTable {
    enums: Vec<Holds>,
    structs: Vec<Holds>,
    classes: Vec<bool>,
    /// What `Checker::holds` has decided, by type.
    cache: std::cell::RefCell<FxHashMap<Type, Holds>>,
}

/// `ty` without its array and optional wrappers.
pub(super) fn unwrapped(mut ty: Type) -> Type {
    while let TypeKind::Array(inner) | TypeKind::Optional(inner) = ty.kind() {
        // One step for each wrapper it removes, so a walk that unwraps before its seen set or
        // cache shows the work.
        #[cfg(test)]
        TYPE_WALK_STEPS.set(TYPE_WALK_STEPS.get() + 1);
        ty = inner;
    }
    ty
}

/// Marks every node that holds a marked one, through `holders`; a holder past the end of
/// `marked` is not a node of its fact.
fn mark_holders(marked: &mut [bool], holders: &[Vec<usize>]) {
    let mut work: Vec<usize> = (0..marked.len()).filter(|&i| marked[i]).collect();
    while let Some(held) = work.pop() {
        for &holder in &holders[held] {
            if let Some(m) = marked.get_mut(holder)
                && !*m
            {
                *m = true;
                work.push(holder);
            }
        }
    }
}

/// Which declarations can be built, given which of their type parameters' arguments can: the
/// least fixed point over keys, each a declaration (a class's node is its `ClassId`, an
/// enum's follows the classes) and a flag for each of its parameters.
struct Buildable {
    classes: usize,
    /// The node of the first struct: the structs follow the enums.
    structs: usize,
    /// By node, its slots, each of which builds it when all its field types can be built: a
    /// class's fields, or each variant's payload, less the fields `may_wait` says are always
    /// built. A node built at once has none: a class on a cycle of fields, an enum with no
    /// variant, and a node with a slot that keeps no field.
    slots: Vec<Vec<Vec<Option<Type>>>>,
    /// The most keys there can be: for each declaration, 2 to the power of its parameters.
    bound: usize,
    /// The keys other than the roots, by declaration and flags. A root is a declaration with
    /// every flag set, and `new` adds them first, so a node's root key is the node.
    ids: FxHashMap<(usize, Vec<bool>), usize>,
    keys: Vec<(usize, Vec<bool>)>,
    built: Vec<bool>,
    /// By key, for each of its node's slots, how many of its fields are not yet known to be
    /// buildable, and for each field the number of the scan whose waits are live: 0 when none
    /// is, as the field is buildable or queued. Empty for a key built when it was added.
    pending: Vec<Vec<(usize, Vec<usize>)>>,
    /// By key, the fields of other keys that wait for it to be built: the key, its slot, the
    /// field's index, and the number of the scan that registered the wait.
    waiting: Vec<Vec<(usize, usize, usize, usize)>>,
    /// How many fields have been scanned.
    scans: usize,
    /// Whether `run` is scanning.
    scanning: bool,
    /// Fields of keys to try: a key, the index of one of its node's slots, and a field's.
    queue: Vec<(usize, usize, usize)>,
}

impl Buildable {
    fn new(checker: &Checker<'_>, cyclic: &[bool]) -> Buildable {
        let mut slots = Vec::new();
        let mut params = Vec::new();
        for (c, class) in checker.classes.iter().enumerate() {
            slots.push(fields_slot(class, cyclic[c]));
            params.push(class.type_params.len());
        }
        for info in &checker.enums {
            let waits = |v: &VariantInfo| v.fields.iter().any(|&ty| may_wait(ty));
            slots.push(
                if !info.variants.is_empty() && info.variants.iter().all(waits) {
                    let fields = |v: &VariantInfo| {
                        v.fields
                            .iter()
                            .copied()
                            .filter(|&ty| may_wait(ty))
                            .collect()
                    };
                    info.variants.iter().map(fields).collect()
                } else {
                    Vec::new()
                },
            );
            params.push(info.type_params.len());
        }
        // A struct is built as a class is, and no struct is on a cycle of `cyclic`.
        for info in &checker.structs {
            slots.push(fields_slot(info, false));
            params.push(info.type_params.len());
        }
        let bound = params
            .iter()
            .map(|&p| {
                u32::try_from(p)
                    .ok()
                    .and_then(|p| 1usize.checked_shl(p))
                    .unwrap_or(usize::MAX)
            })
            .fold(0, usize::saturating_add);
        let mut fix = Buildable {
            classes: checker.classes.len(),
            structs: checker.classes.len() + checker.enums.len(),
            slots,
            bound,
            ids: FxHashMap::default(),
            keys: Vec::new(),
            built: Vec::new(),
            pending: Vec::new(),
            waiting: Vec::new(),
            scans: 0,
            scanning: false,
            queue: Vec::new(),
        };
        for (n, &p) in params.iter().enumerate() {
            let key = fix.push(n, vec![true; p]);
            debug_assert_eq!(
                key, n,
                "a node's root key should be the node, as roots come first"
            );
        }
        fix
    }

    /// The key of `node` with `flags`, interned when it is new. Outside `run`, a new key is
    /// run at once, so every key is final. No earlier key waits for it, so it changes none.
    fn key(&mut self, node: usize, flags: Vec<bool>) -> usize {
        if flags.iter().all(|&f| f) {
            return node;
        }
        let id = (node, flags);
        if let Some(&key) = self.ids.get(&id) {
            return key;
        }
        let (node, flags) = id;
        let key = self.push(node, flags.clone());
        self.ids.insert((node, flags), key);
        if !self.scanning {
            self.run();
        }
        key
    }

    /// Adds the key of `node` with `flags`, and queues its slots unless one is empty.
    fn push(&mut self, node: usize, flags: Vec<bool>) -> usize {
        assert!(
            self.keys.len() < self.bound,
            "the can-be-built keys should be a declaration and its parameters' flags"
        );
        let key = self.keys.len();
        self.keys.push((node, flags));
        let slots = &self.slots[node];
        // Nothing waits for a key built at once.
        let built = slots.is_empty();
        self.built.push(built);
        self.waiting.push(Vec::new());
        if built {
            self.pending.push(Vec::new());
            return key;
        }
        self.pending.push(
            slots
                .iter()
                .map(|fields| (fields.len(), vec![0; fields.len()]))
                .collect(),
        );
        for (s, fields) in slots.iter().enumerate() {
            self.queue.extend((0..fields.len()).map(|f| (key, s, f)));
        }
        key
    }

    /// The node of the class, enum or struct `ty`, and its arguments; `None` for another type.
    fn node_of(&self, ty: Type) -> Option<(usize, TypeArgs)> {
        match ty.kind() {
            TypeKind::Class(id, args) => Some((id.0, args)),
            TypeKind::Enum(id, args) => Some((self.classes + id.0, args)),
            TypeKind::Struct(id, args) => Some((self.structs + id.0, args)),
            _ => None,
        }
    }

    /// The key `node` has when given `args`. A type parameter in `args` is a chain's start's,
    /// whose arguments are all buildable.
    fn key_of(&mut self, node: usize, args: TypeArgs) -> usize {
        self.key_in(node, args, &mut FxHashMap::default())
    }

    /// `key_of`, where `known` holds whether each type read so far can be built: each type is
    /// read once, whatever number of paths lead to it.
    fn key_in(&mut self, node: usize, args: TypeArgs, known: &mut FxHashMap<Type, bool>) -> usize {
        let flags = (args.get().iter())
            .map(|&arg| self.reached(arg, known))
            .collect();
        self.key(node, flags)
    }

    /// Whether `ty` can be built, a type parameter's argument always.
    fn reached(&mut self, ty: Type, known: &mut FxHashMap<Type, bool>) -> bool {
        if let Some(&built) = known.get(&ty) {
            return built;
        }
        #[cfg(test)]
        TYPE_WALK_STEPS.set(TYPE_WALK_STEPS.get() + 1);
        let Some((node, args)) = self.node_of(ty) else {
            return true;
        };
        let key = self.key_in(node, args, known);
        let built = self.built[key];
        known.insert(ty, built);
        built
    }

    /// The declarations no value can be built of that the slots of `key` need, once the fixed
    /// point is reached: an argument such a slot needs is among the keys it needs, unless the
    /// type given it can be built.
    fn needs(&mut self, key: usize) -> Vec<usize> {
        let deps = self.deps(key);
        deps.into_iter()
            .map(|k| self.keys[k].0)
            .filter(|&node| !self.built[node])
            .collect()
    }

    /// The keys, none of them built, that the slots of `key` need.
    fn deps(&mut self, key: usize) -> Vec<usize> {
        let (node, flags) = self.keys[key].clone();
        let mut walk = Walk::default();
        for ty in self.slots[node].clone().into_iter().flatten() {
            self.buildable(ty, &flags, &mut walk);
        }
        walk.consulted
    }

    /// Whether `ty` can be built, where a type parameter's argument can when `flags` says so.
    /// Each key `ty` needs that cannot be built yet is added to `walk.consulted`: an application's,
    /// and its arguments' only when the application cannot be built. `walk` is for one top-level
    /// call, where the flags and the keys built do not change. So it reads each type once,
    /// whatever number of paths lead to it. It adds each key once for each type.
    fn buildable(&mut self, ty: Option<Type>, flags: &[bool], walk: &mut Walk) -> bool {
        let Some(ty) = ty else { return true };
        let built = self.verdict(ty, flags, &mut walk.verdicts);
        consult(ty, &walk.verdicts, &mut walk.seen, &mut walk.consulted);
        built
    }

    /// Whether `ty` can be built, as `buildable` says it, with the key of each application read.
    fn verdict(
        &mut self,
        ty: Type,
        flags: &[bool],
        verdicts: &mut FxHashMap<Type, (bool, usize, TypeArgs)>,
    ) -> bool {
        if let Some(&(built, ..)) = verdicts.get(&ty) {
            return built;
        }
        #[cfg(test)]
        TYPE_WALK_STEPS.set(TYPE_WALK_STEPS.get() + 1);
        let (node, args) = match ty.kind() {
            TypeKind::Param(_, i) => return flags[i as usize],
            _ => match self.node_of(ty) {
                Some(node) => node,
                None => return true,
            },
        };
        let given = (args.get().iter())
            .map(|&arg| self.verdict(arg, flags, verdicts))
            .collect();
        let key = self.key(node, given);
        let built = self.built[key];
        verdicts.insert(ty, (built, key, args));
        built
    }

    /// Scans queued fields until none is left. A field that cannot be built yet waits for the
    /// keys it consulted: its own, and its arguments', whose building changes which key it
    /// holds. The first of them built queues it once more, so a field is scanned at most once
    /// for each key built that it consulted. A key that two of its types share waits twice,
    /// and the first wait that is live queues the field.
    fn run(&mut self) {
        self.scanning = true;
        while let Some((key, s, f)) = self.queue.pop() {
            if self.built[key] {
                continue;
            }
            self.scans += 1;
            #[cfg(test)]
            SCANS.set(SCANS.get() + 1);
            // Taken, not cloned, for the scan: `buildable` reads no key's flags.
            let (node, flags) = (self.keys[key].0, mem::take(&mut self.keys[key].1));
            let mut walk = Walk::default();
            let buildable = self.buildable(self.slots[node][s][f], &flags, &mut walk);
            self.keys[key].1 = flags;
            if buildable {
                let remaining = &mut self.pending[key][s].0;
                *remaining -= 1;
                if *remaining == 0 {
                    self.build(key);
                }
            } else {
                self.pending[key][s].1[f] = self.scans;
                for d in walk.consulted {
                    self.waiting[d].push((key, s, f, self.scans));
                }
            }
        }
        self.scanning = false;
    }

    /// Marks `key` built, and queues each field waiting for it whose wait is live.
    fn build(&mut self, key: usize) {
        self.built[key] = true;
        for (k, s, f, scan) in mem::take(&mut self.waiting[key]) {
            let live = &mut self.pending[k][s].1[f];
            if *live == scan {
                *live = 0;
                self.queue.push((k, s, f));
            }
        }
    }
}

/// Adds to `consulted` the key of each application in `ty` that cannot be built and is not under
/// one that can. The keys of its arguments come first. `seen` holds the types it has added.
fn consult(
    ty: Type,
    verdicts: &FxHashMap<Type, (bool, usize, TypeArgs)>,
    seen: &mut FxHashSet<Type>,
    consulted: &mut Vec<usize>,
) {
    let Some(&(built, key, args)) = verdicts.get(&ty) else {
        return;
    };
    if built || !seen.insert(ty) {
        return;
    }
    #[cfg(test)]
    TYPE_WALK_STEPS.set(TYPE_WALK_STEPS.get() + 1);
    for &arg in args.get() {
        consult(arg, verdicts, seen, consulted);
    }
    consulted.push(key);
}

/// What one top-level call of `Buildable::buildable` has read: the scan of a field, or the fields
/// of a key's slots.
#[derive(Default)]
struct Walk {
    /// By type, whether it can be built and the key of its application.
    verdicts: FxHashMap<Type, (bool, usize, TypeArgs)>,
    /// The applications whose keys are in `consulted`.
    seen: FxHashSet<Type>,
    /// The keys, none of them built, that the types read need.
    consulted: Vec<usize>,
}

/// Whether a field of type `ty` can wait to be built: only a class, an enum, a struct or a type
/// parameter can. `Buildable::buildable` finds any other type, and an error type, always buildable.
fn may_wait(ty: Option<Type>) -> bool {
    ty.is_some_and(|ty| {
        matches!(
            ty.kind(),
            TypeKind::Class(..) | TypeKind::Enum(..) | TypeKind::Struct(..) | TypeKind::Param(..)
        )
    })
}

/// The one slot of a class or a struct, with the fields `may_wait` keeps; none for a class in
/// `cyclic` or with no such field, which is built at once.
fn fields_slot(info: &ClassInfo, cyclic: bool) -> Vec<Vec<Option<Type>>> {
    let fields: Vec<_> = (info.fields.iter().map(|f| f.ty))
        .filter(|&ty| may_wait(ty))
        .collect();
    if cyclic || fields.is_empty() {
        Vec::new()
    } else {
        vec![fields]
    }
}

#[cfg(test)]
thread_local! {
    /// How many fields `Buildable::run` has scanned on this thread.
    pub(super) static SCANS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many types the checker's type walks have visited on this thread: `Buildable`'s
    /// `verdict`, `consult` and `reached`, `holds_table`, `holds`, `inline_enums`, `names_itself`,
    /// `unwrapped` and `cycle::named`. Each ticks once for each type it reads. `unwrapped` ticks
    /// once for each wrapper it removes, so that a walk that unwraps before its seen set or cache
    /// shows the work. A walk visits each distinct type of its unit once. A field that names a
    /// chain again starts a new unit: `m` fields that name one alias of `k` distinct types still
    /// cost `m * k`.
    pub(super) static TYPE_WALK_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
