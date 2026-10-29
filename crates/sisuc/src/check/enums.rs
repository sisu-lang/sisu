//! Enum declarations: their variants and payloads, the names they cannot take, which enums no
//! value can be built of, the boxes that break a cycle, and which enums are counted.

use std::{fmt, mem};

use rustc_hash::{FxHashMap, FxHashSet};

use super::generic::{GenericSig, Site, mismatched};
use super::interface::Slot;
use super::unused_names::Used;
use super::{Checked, Checker, Declared, Definition, DiagnosticKind, Poisoned, own_args};
use crate::ast;
use crate::diagnostic::{BuiltIn, Unbuilt, VariantLabels, suggestion};
use crate::graph::components;
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, EnumId, Rank, Type, TypeArgs, TypeKind};

pub(super) struct EnumInfo {
    pub(super) name: ast::Ident,
    pub(super) type_params: Vec<Symbol>,
    /// Whether a type parameter is written with a bound.
    pub(super) bounded: bool,
    /// The bounds of each of `type_params`; empty when `bounded` is false.
    pub(super) bounds: Vec<Vec<super::Bound>>,
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
    /// For each field, the class and enum types written in its type, each with its span.
    pub(super) inner: Vec<Vec<(Type, Span)>>,
    /// The payload's labels, one for each field.
    pub(super) labels: Vec<Option<Symbol>>,
}

/// The name milestone 8's prelude takes, which no item, variant or binding may take, and what
/// it is there.
pub(super) fn reserved_name(name: &str) -> Option<BuiltIn> {
    match name {
        "Option" | "Result" => Some(BuiltIn::Type),
        "Some" | "Ok" | "Err" => Some(BuiltIn::Constructor),
        _ => None,
    }
}

/// The enum declarations `ty` names where a value of it holds them inline: `ty` itself, or its
/// optional's payload, and the type arguments of each enum it names, through optionals. With
/// `open`, only those named with an argument that carries a parameter: `C<T>`, not `Wrap3<i64>`.
fn inline_enums(ty: Type, open: bool) -> Vec<usize> {
    match ty.kind() {
        TypeKind::Optional(payload) => inline_enums(payload, open),
        TypeKind::Enum(id, args) => {
            let named = (!open || ty.has_param()).then_some(id.0);
            let args = args.get().iter().flat_map(|&arg| inline_enums(arg, open));
            named.into_iter().chain(args).collect()
        }
        _ => Vec::new(),
    }
}

/// Whether `ty` names, where `inline_enums` looks, the enum `e` applied to its own `count`
/// parameters in order: `Tree<T>` in `Tree`, or `G<T>` in `G`'s `Wrap2<G<T>>`.
fn names_itself(ty: Type, e: usize, count: usize) -> bool {
    match ty.kind() {
        TypeKind::Optional(payload) => names_itself(payload, e, count),
        TypeKind::Enum(id, args) => {
            let args = args.get();
            let own = |(i, arg): (usize, &Type)| matches!(arg.kind(), TypeKind::Param(_, j) if j as usize == i);
            (id.0 == e && args.len() == count && args.iter().enumerate().all(own))
                || args.iter().any(|&arg| names_itself(arg, e, count))
        }
        _ => false,
    }
}

impl Checker<'_> {
    /// Whether `name` is one milestone 8's prelude takes; reports it when it is. The prelude's
    /// own declarations take them without a report.
    pub(super) fn reserved(&mut self, name: &ast::Ident) -> bool {
        if name.span.in_prelude() {
            return false;
        }
        let Some(kind) = reserved_name(name.name.as_str()) else {
            return false;
        };
        self.report(DiagnosticKind::BuiltInName {
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
        declared: &mut Vec<(&'a ast::Function, Option<Type>)>,
    ) {
        let owner = e.name.name.as_str();
        if e.variants.is_empty() {
            self.report(DiagnosticKind::EmptyEnum {
                name: owner.to_string(),
                at: e.name.span,
            });
        }
        let receiver = Type::intern(TypeKind::Enum(id, own_args(&self.enums[id.0].type_params)));
        self.body.self_type = Some(receiver);
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
                self.report(DiagnosticKind::VariantCase {
                    name: name.name.to_string(),
                    at: name.span,
                });
            }
            let payload = variant.payload.as_deref().unwrap_or_default();
            let labeled = |field: &ast::PayloadField| field.label.is_some();
            if let Some(field) = payload
                .iter()
                .find(|field| labeled(field) != labeled(&payload[0]))
            {
                self.report(DiagnosticKind::MixedLabels {
                    variant: name.name.to_string(),
                    at: field
                        .label
                        .as_ref()
                        .map_or(field.ty.span, |label| label.span),
                });
            }
            let mut labels = FxHashMap::default();
            for label in payload.iter().filter_map(|field| field.label.as_ref()) {
                self.first_definition(&mut labels, Definition::Field, Some(owner), label);
            }
            // The checks that each field's type queues.
            let mut queued = Vec::new();
            let (fields, inner): (Vec<_>, Vec<_>) = payload
                .iter()
                .map(|field| {
                    let from = self.queued();
                    let resolved = self.resolve_written(&field.ty);
                    queued.push(from..self.queued());
                    resolved
                })
                .unzip();
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
                    inner,
                    labels: payload
                        .iter()
                        .map(|field| field.label.as_ref().map(|label| label.name))
                        .collect(),
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
            declared.push((method, Some(receiver)));
        }
        for method in &e.broken_methods {
            self.enums[id.0]
                .methods
                .insert(method.name, Declared::Broken);
        }
        self.body.self_type = None;
        self.body.assoc = std::rc::Rc::default();
    }

    /// `Enum.member`, or `Enum.member(args)` when `call` gives the arguments and the span of their
    /// parentheses, where `Enum` is the enum `id`, which `site` writes: the variant `member`
    /// built, or an error.
    pub(super) fn enum_path(
        &mut self,
        span: Span,
        id: EnumId,
        member: &ast::Ident,
        call: Option<(&[ast::Arg], Span)>,
        site: Site<'_>,
    ) -> Checked {
        let info = &self.enums[id.0];
        let enum_name = info.name.name.as_str();
        let Some(&variant) = info.index.get(&member.name) else {
            match info.methods.get(&member.name) {
                Some(Declared::Known(_)) => self.report(DiagnosticKind::MemberKind {
                    name: member.name.to_string(),
                    enum_name: enum_name.to_string(),
                    method: true,
                    at: member.span,
                }),
                // The method's own error is reported.
                Some(Declared::Broken) => {}
                None => {
                    let variants = info
                        .variants
                        .iter()
                        .map(|v| (v.name.name.as_str(), Definition::Variant));
                    let suggestion = suggestion(member.name.as_str(), variants);
                    self.report(DiagnosticKind::UnknownVariant {
                        enum_name: enum_name.to_string(),
                        name: member.name.to_string(),
                        at: member.span,
                        suggestion,
                    });
                }
            }
            self.stray(call.map_or(&[], |(args, _)| args));
            return Err(Poisoned::error());
        };
        self.record(Used::Variant(id, variant), span);
        let info = &self.enums[id.0];
        let enum_name = info.name.name.as_str();
        // Built only for a diagnostic.
        let path = format_args!("{enum_name}.{}", member.name);
        let v = &info.variants[variant];
        let ty = Type::intern(TypeKind::Enum(id, TypeArgs::NONE));
        let generic = !info.type_params.is_empty();
        let args = match call {
            _ if generic && call.is_some() != v.fields.is_empty() => {
                return self.generic_variant_args(span, (id, variant), member, call, site);
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
                    self.construct(span, &path, &named, args)
                } else if v.labels.iter().all(Option::is_none) {
                    let fields = v.fields.clone();
                    self.positional(span, &path, &fields, args)
                } else {
                    // Mixed labels are reported at the declaration.
                    self.stray(args);
                    Err(true)
                };
                match built {
                    Ok(args) => args,
                    Err(failed) => return Err(Poisoned::of((!failed).then_some(ty))),
                }
            }
            Some((args, parens)) => {
                // The fix deletes the parentheses, so what they hold (even a comment) makes it
                // unsafe.
                let inside = parens.start()..parens.end();
                self.report(DiagnosticKind::VariantForm {
                    path: path.to_string(),
                    payload: false,
                    held: !args.is_empty() || self.comment_in(&[inside]),
                    at: parens,
                });
                self.stray(args);
                return Err(Poisoned::error());
            }
            None => {
                self.report(DiagnosticKind::VariantForm {
                    path: path.to_string(),
                    payload: true,
                    held: false,
                    at: member.span,
                });
                return Err(Poisoned::error());
            }
        };
        Ok(self.spilled(span, ty, args, |args| tir::ExprKind::Variant {
            id,
            variant,
            args,
        }))
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
    ) -> Checked {
        let info = &self.enums[id.0];
        let v = &info.variants[variant];
        let enum_name = info.name.name;
        // Built only for a diagnostic.
        let path = format_args!("{enum_name}.{}", member.name);
        let (declared, fields, labels) =
            (info.type_params.clone(), v.fields.clone(), v.labels.clone());
        let bounds = info.bounds.clone();
        let args = call.map_or(&[][..], |(args, _)| args);
        let labeled = if labels.iter().all(Option::is_some) && !labels.is_empty() {
            let names: Vec<_> = labels.iter().flatten().copied().collect();
            if !self.shaped(span, &path, &names, args) {
                return Err(Poisoned::error());
            }
            false
        } else if labels.iter().all(Option::is_none) {
            match self.positional_shape(span, &path, fields.len(), args) {
                Some(labeled) => labeled,
                None => return Err(Poisoned::error()),
            }
        } else {
            // Mixed labels are reported at the declaration.
            self.stray(args);
            return Err(Poisoned::error());
        };
        let sig = GenericSig {
            name: site.name,
            variant: Some(member),
            declared: &declared,
            owner: &[],
            params: &fields,
            ret: Some(Type::intern(TypeKind::Enum(id, own_args(&declared)))),
            bounds: &bounds,
        };
        let written = (site.written, site.angles);
        let Some((args, _, Some(ty))) = self.generic_call(span, &sig, written, args, site.expected)
        else {
            return Err(Poisoned::error());
        };
        let failed = labeled || args.iter().any(mismatched);
        match args.into_iter().collect() {
            Ok(args) if !failed => Ok(self.spilled(span, ty, args, |args| {
                tir::ExprKind::Variant { id, variant, args }
            })),
            _ => Err(Poisoned::of((!failed).then_some(ty))),
        }
    }

    /// Reports the labels of `path(args)`, a variant whose payload declares none, and a count
    /// of `args` that is not `want`. `None` when the count is wrong, else whether there was a
    /// label.
    pub(super) fn positional_shape(
        &mut self,
        span: Span,
        path: &dyn fmt::Display,
        want: usize,
        args: &[ast::Arg],
    ) -> Option<bool> {
        // Each `label: ` and the space after it, which the fix deletes.
        let labels: Vec<_> = args
            .iter()
            .filter_map(|arg| {
                let label = arg.label.as_ref()?;
                Some(Span::new(label.span.start(), arg.value.span.start()))
            })
            .collect();
        let labeled = !labels.is_empty();
        if let Some(first) = args.iter().find_map(|arg| arg.label.as_ref()) {
            self.report(DiagnosticKind::LabeledArgument {
                at: first.span,
                variant: Some(VariantLabels {
                    path: path.to_string(),
                    labels,
                }),
            });
        }
        if want != args.len() {
            self.stray(args);
            if !self.misread_in(args) {
                self.report(DiagnosticKind::ArgumentCount {
                    name: path.to_string(),
                    want,
                    found: args.len(),
                    at: span,
                });
            }
            return None;
        }
        Some(labeled)
    }

    /// The arguments of `path(args)`, a variant whose payload declares no labels, checked
    /// against its `fields` in order. `Err` holds whether a mistake was reported here, rather
    /// than in an argument.
    fn positional(
        &mut self,
        span: Span,
        path: &dyn fmt::Display,
        fields: &[Option<Type>],
        args: &[ast::Arg],
    ) -> Result<Vec<tir::Expr>, bool> {
        let Some(labeled) = self.positional_shape(span, path, fields.len(), args) else {
            return Err(true);
        };
        let (checked, mismatched): (Vec<_>, Vec<_>) = args
            .iter()
            .zip(fields)
            .map(|(arg, field)| self.operand(&arg.value, *field))
            .unzip();
        let failed = labeled || mismatched.contains(&true);
        match checked.into_iter().collect() {
            Ok(args) if !failed => Ok(args),
            _ => Err(failed),
        }
    }

    /// The prelude's `Result` and the variant it declares for `name`, when `name` is `Ok` or
    /// `Err`, which a program writes without the enum.
    pub(super) fn result_variant(&self, name: Symbol) -> Option<(EnumId, usize)> {
        if !matches!(name.as_str(), "Ok" | "Err") {
            return None;
        }
        let Some(&Declared::Known(id)) = self.enum_ids.get(&Symbol::intern("Result")) else {
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
                matches!(self.enum_ids.get(&e.name.name), Some(Declared::Known(id)) if id.0 == *i)
                    && e.variants.iter().any(|v| v.name.name == name)
            })
            .map(|(_, e)| e.name.name.to_string())
            .collect()
    }

    /// Lowers every enum, with each payload field's `Rank` from its declared type, and lays them
    /// out (`tir::layout_enums`).
    pub(super) fn lower_enums(&self) -> Vec<tir::Enum> {
        // The declarations each enum's generic fields name, inline (see `Rank::Late`); a
        // program without a generic enum has no such field and skips the graph.
        let declared = if self.enums.iter().any(|info| !info.type_params.is_empty()) {
            let named: Vec<Vec<usize>> = (self.enums.iter())
                .map(|info| {
                    let fields = info.variants.iter().flat_map(|v| v.fields.iter().flatten());
                    let generic = fields.filter(|ty| ty.has_param());
                    generic.flat_map(|&ty| inline_enums(ty, false)).collect()
                })
                .collect();
            components(&named)
        } else {
            Vec::new()
        };
        let mut lowered: Vec<tir::Enum> = (self.enums.iter().enumerate())
            .map(|(e, info)| tir::Enum {
                name: info.name.name.to_string(),
                type_params: info.type_params.iter().map(Symbol::to_string).collect(),
                variants: (info.variants.iter())
                    .map(|v| tir::Variant {
                        name: v.name.name.to_string(),
                        fields: (v.fields.iter().zip(&v.labels))
                            .map(|(&ty, label)| {
                                // An `Error` field's error stops the program before codegen.
                                let ty = ty.unwrap_or(Type::NEVER);
                                let rank = if !ty.has_param() {
                                    Rank::Plain
                                } else if names_itself(ty, e, info.type_params.len()) {
                                    Rank::Own
                                } else if inline_enums(ty, true)
                                    .iter()
                                    .any(|&n| declared[n] == declared[e])
                                {
                                    Rank::Open
                                } else if inline_enums(ty, false)
                                    .iter()
                                    .any(|&n| declared[n] == declared[e])
                                {
                                    Rank::Late
                                } else if ty.has_assoc() {
                                    Rank::Assoc
                                } else {
                                    Rank::Never
                                };
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
            })
            .collect();
        tir::layout_enums(&mut lowered);
        lowered
    }

    /// Reports each enum or class no value can be built of. Which classes and enums can be
    /// built is a least fixed point over them all, with the arguments they are given: a class
    /// can when every field's type can, an enum when every field of some variant can, and every
    /// other type always can. A class in `cyclic` counts as built, and so does an enum with no
    /// variant, since each has its error already. Each component of the rest that needs no
    /// other, by what the fixed point found each one to need, is reported once, at its first
    /// enum, or else its first class; a type that only needs one of those is not. Returns, by
    /// node (the classes, then the enums), whether it is in a component reported here, or is a
    /// class in `cyclic`: whether it has an error of that kind of its own.
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
            let chain = self.chain(n, &mut fix);
            let name = if n < classes {
                &self.classes[n].name
            } else {
                &self.enums[n - classes].name
            };
            self.report(DiagnosticKind::NeverConstructed {
                name: name.name.to_string(),
                at: name.span,
                cause: Unbuilt::Chain(chain),
            });
        }
        (0..nodes)
            .map(|n| reported[component[n]] || n < classes && cyclic[n])
            .collect()
    }

    /// The chain of fields from the enum or class at node `start`, through types none of which
    /// can be built by `fix`: each link's holder (`E.V` or `K.f`) and the type held, with the
    /// arguments the chain gives it. At an enum it takes the first variant's first such field,
    /// at a class the first such field, and it ends at the first type it meets again.
    fn chain(&self, start: usize, fix: &mut Buildable) -> Vec<(String, String)> {
        let classes = self.classes.len();
        let params = if start < classes {
            &self.classes[start].type_params
        } else {
            &self.enums[start - classes].type_params
        };
        // A field named `name` of type `ty`, if that type cannot be built: its name, its type's
        // name, and its node and arguments.
        let mut unbuilt = |(name, ty): (&str, Type)| {
            let (n, args) = match ty.kind() {
                TypeKind::Class(id, args) => (id.0, args),
                TypeKind::Enum(id, args) => (classes + id.0, args),
                _ => return None,
            };
            let key = fix.key_of(n, args);
            (!fix.built[key]).then(|| (name.to_string(), self.type_name(ty), (n, args), key))
        };
        let mut chain = Vec::new();
        // Asking for a key's flags can add keys to `fix`, so this is a set.
        let mut on_chain = FxHashSet::default();
        let (mut at, mut args) = (start, own_args(params));
        let mut key = start;
        while on_chain.insert(key) {
            let with = args.get();
            let held = |ty: &Option<Type>| ty.and_then(|ty| self.subst(ty, with));
            let (holder, field) = if at < classes {
                let class = &self.classes[at];
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
            chain.push((format!("{}.{name}", holder.name), ty));
            ((at, args), key) = (next, next_key);
        }
        chain
    }

    /// What a value of each enum and class may hold. The nodes are the enums, then the classes;
    /// a field holds the node its array and optional wrappers end at. A field of the poison type
    /// marks its node poison; a class field marks an enum `class`. Each fact then spreads to every
    /// node that holds a marked one: `class` through enums only, `poison` through every node.
    pub(super) fn holds_table(&self) -> HoldsTable {
        let enums = self.enums.len();
        let mut poison = vec![false; enums + self.classes.len()];
        let mut class = vec![false; enums];
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
        for (holder, ty) in enum_fields.chain(class_fields) {
            let Some(ty) = ty else {
                poison[holder] = true;
                continue;
            };
            // A field holds its type's node and the nodes of its type arguments.
            let mut types = vec![ty];
            while let Some(ty) = types.pop() {
                match unwrapped(ty).kind() {
                    TypeKind::Class(id, args) => {
                        if let Some(c) = class.get_mut(holder) {
                            *c = true;
                        }
                        holders[enums + id.0].push(holder);
                        types.extend_from_slice(args.get());
                    }
                    TypeKind::Enum(id, args) => {
                        holders[id.0].push(holder);
                        types.extend_from_slice(args.get());
                    }
                    _ => {}
                }
            }
        }
        spread(&mut poison, &holders);
        spread(&mut class, &holders);
        HoldsTable {
            enums: (0..enums)
                .map(|i| Holds {
                    class: class[i],
                    poison: poison[i],
                })
                .collect(),
            classes: poison.split_off(enums),
        }
    }

    /// What a value of `ty` may hold, with its type arguments.
    pub(super) fn holds(&self, ty: Type) -> Holds {
        let (own, args) = match unwrapped(ty).kind() {
            TypeKind::Enum(id, args) => (self.holds_table.enums[id.0], args),
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

/// `Holds` for each enum, by `EnumId`, and whether each class, by `ClassId`, holds the poison
/// type.
#[derive(Default)]
pub(super) struct HoldsTable {
    enums: Vec<Holds>,
    classes: Vec<bool>,
}

/// `ty` without its array and optional wrappers.
fn unwrapped(mut ty: Type) -> Type {
    while let TypeKind::Array(inner) | TypeKind::Optional(inner) = ty.kind() {
        ty = inner;
    }
    ty
}

/// Marks every node that holds a marked one, through `holders`; a holder past the end of
/// `marked` is not a node of its fact.
fn spread(marked: &mut [bool], holders: &[Vec<usize>]) {
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
            let fields = class.fields.iter().map(|f| f.ty);
            let fields: Vec<_> = fields.filter(|&ty| may_wait(ty)).collect();
            slots.push(if cyclic[c] || fields.is_empty() {
                Vec::new()
            } else {
                vec![fields]
            });
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

    /// The key `node` has when given `args`. A type parameter in `args` is a chain's start's,
    /// whose arguments are all buildable.
    fn key_of(&mut self, node: usize, args: TypeArgs) -> usize {
        let flags = args.get().iter().map(|&arg| self.reached(arg)).collect();
        self.key(node, flags)
    }

    /// Whether `ty` can be built, a type parameter's argument always.
    fn reached(&mut self, ty: Type) -> bool {
        let (node, args) = match ty.kind() {
            TypeKind::Class(id, args) => (id.0, args),
            TypeKind::Enum(id, args) => (self.classes + id.0, args),
            _ => return true,
        };
        let key = self.key_of(node, args);
        self.built[key]
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
        let mut deps = Vec::new();
        for ty in self.slots[node].clone().into_iter().flatten() {
            self.buildable(ty, &flags, &mut deps);
        }
        deps
    }

    /// Whether `ty` can be built, where a type parameter's argument can when `flags` says so.
    /// Each key `ty` needs that cannot be built yet is added to `consulted`: an application's,
    /// and its arguments' only when the application cannot be built.
    fn buildable(&mut self, ty: Option<Type>, flags: &[bool], consulted: &mut Vec<usize>) -> bool {
        let Some(ty) = ty else { return true };
        let (node, args) = match ty.kind() {
            TypeKind::Param(_, i) => return flags[i as usize],
            TypeKind::Class(id, args) => (id.0, args),
            TypeKind::Enum(id, args) => (self.classes + id.0, args),
            _ => return true,
        };
        let mark = consulted.len();
        let args = args
            .get()
            .iter()
            .map(|&arg| self.buildable(Some(arg), flags, consulted))
            .collect();
        let key = self.key(node, args);
        if self.built[key] {
            consulted.truncate(mark);
        } else {
            consulted.push(key);
        }
        self.built[key]
    }

    /// Scans queued fields until none is left. A field that cannot be built yet waits for the
    /// keys it consulted: its own, and its arguments', whose building changes which key it
    /// holds. The first of them built queues it once more, so a field is scanned at most once
    /// for each key built that it consulted.
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
            let mut consulted = Vec::new();
            let buildable = self.buildable(self.slots[node][s][f], &flags, &mut consulted);
            self.keys[key].1 = flags;
            if buildable {
                let remaining = &mut self.pending[key][s].0;
                *remaining -= 1;
                if *remaining == 0 {
                    self.build(key);
                }
            } else {
                self.pending[key][s].1[f] = self.scans;
                for d in consulted {
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

/// Whether a field of type `ty` can wait to be built: only a class, an enum or a type parameter
/// can. `Buildable::buildable` finds any other type, and an error type, always buildable.
fn may_wait(ty: Option<Type>) -> bool {
    ty.is_some_and(|ty| {
        matches!(
            ty.kind(),
            TypeKind::Class(..) | TypeKind::Enum(..) | TypeKind::Param(..)
        )
    })
}

#[cfg(test)]
thread_local! {
    /// How many fields `Buildable::run` has scanned on this thread.
    pub(super) static SCANS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
