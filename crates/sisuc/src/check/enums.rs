//! Enum declarations: their variants and payloads, the names they cannot take, which enums no
//! value can be built of, the boxes that break a cycle, and which enums are counted.

use std::fmt;

use rustc_hash::FxHashMap;

use super::{Checked, Checker, Declared, Definition, DiagnosticKind, Poisoned};
use crate::ast;
use crate::diagnostic::{BuiltIn, Unbuilt, VariantLabels, suggestion};
use crate::graph::components;
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, EnumId, Type, TypeKind};

pub(super) struct EnumInfo {
    pub(super) name: ast::Ident,
    /// The first variant of each name, in declaration order.
    pub(super) variants: Vec<VariantInfo>,
    /// Each variant's index in `variants`, by name.
    pub(super) index: FxHashMap<Symbol, usize>,
    /// The first method of each name that no variant takes, and every broken one.
    pub(super) methods: FxHashMap<Symbol, Declared<tir::FuncId>>,
}

pub(super) struct VariantInfo {
    pub(super) name: ast::Ident,
    /// The payload's field types; `None` is `Error`.
    pub(super) fields: Vec<Option<Type>>,
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

impl Checker<'_> {
    /// Whether `name` is one milestone 8's prelude takes; reports it when it is.
    pub(super) fn reserved(&mut self, name: &ast::Ident) -> bool {
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
            let fields = payload
                .iter()
                .map(|field| self.resolve(&field.ty))
                .collect();
            if self.first_definition(&mut first, Definition::Variant, Some(owner), name) {
                let info = &mut self.enums[id.0];
                info.index.insert(name.name, info.variants.len());
                info.variants.push(VariantInfo {
                    name: name.clone(),
                    fields,
                    labels: payload
                        .iter()
                        .map(|field| field.label.as_ref().map(|label| label.name))
                        .collect(),
                });
            }
        }
        for method in &e.methods {
            let func = tir::FuncId(self.signatures.len());
            if self.first_definition(&mut first, Definition::Method, Some(owner), &method.name) {
                self.enums[id.0]
                    .methods
                    .insert(method.name.name, Declared::Known(func));
            }
            self.signature(method, format!("{owner}.{}", method.name.name));
            declared.push((method, Some(Type::intern(TypeKind::Enum(id)))));
        }
        for method in &e.broken_methods {
            self.enums[id.0]
                .methods
                .insert(method.name, Declared::Broken);
        }
    }

    /// `Enum.member`, or `Enum.member(args)` when `call` gives the arguments and the span of their
    /// parentheses, where `Enum` is the enum `id`: the variant `member` built, or an error.
    pub(super) fn enum_path(
        &mut self,
        span: Span,
        id: EnumId,
        member: &ast::Ident,
        call: Option<(&[ast::Arg], Span)>,
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
        // Built only for a diagnostic.
        let path = format_args!("{enum_name}.{}", member.name);
        let v = &info.variants[variant];
        let ty = Type::intern(TypeKind::Enum(id));
        let args = match call {
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
        if fields.len() != args.len() {
            self.report(DiagnosticKind::ArgumentCount {
                name: path.to_string(),
                want: fields.len(),
                found: args.len(),
                at: span,
            });
            self.stray(args);
            return Err(true);
        }
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

    /// Lowers every enum: boxes each payload field whose enum lies in the declaring enum's
    /// component of the holds-inline graph, and marks the enums with counted parts. Reports
    /// the enums no value can be built of; `cyclic` marks the classes on a cycle of fields,
    /// which `never_constructed` reports.
    pub(super) fn lower_enums(&mut self, cyclic: &[bool]) -> Vec<tir::Enum> {
        let held: Vec<Vec<usize>> = self
            .enums
            .iter()
            .map(|e| {
                let fields = e.variants.iter().flat_map(|v| v.fields.iter().flatten());
                fields
                    .copied()
                    .filter_map(Type::inline_enum)
                    .map(|id| id.0)
                    .collect()
            })
            .collect();
        let component = components(&held);
        let mut lowered: Vec<tir::Enum> = self
            .enums
            .iter()
            .enumerate()
            .map(|(e, info)| {
                let variants: Vec<tir::Variant> = info
                    .variants
                    .iter()
                    .map(|v| tir::Variant {
                        name: v.name.name.to_string(),
                        fields: v
                            .fields
                            .iter()
                            .map(|&ty| {
                                // An `Error` field's error stops the program before codegen.
                                let ty = ty.unwrap_or(Type::NEVER);
                                let boxed = ty
                                    .inline_enum()
                                    .is_some_and(|f| component[f.0] == component[e]);
                                tir::Payload { ty, boxed }
                            })
                            .collect(),
                    })
                    .collect();
                // Every member of a component of more than one enum holds another inline, so
                // the component has a box exactly when its every member has one.
                let boxed = variants.iter().flat_map(|v| &v.fields).any(|f| f.boxed);
                tir::Enum {
                    name: info.name.name.to_string(),
                    variants,
                    counted: false,
                    cycle: boxed.then_some(component[e]),
                }
            })
            .collect();
        // A component is numbered after every component it holds, and holds its own members
        // only through boxes, so counting in component order sees each inline enum counted.
        let mut order: Vec<usize> = (0..lowered.len()).collect();
        order.sort_by_key(|&e| component[e]);
        for e in order {
            let counted = lowered[e]
                .variants
                .iter()
                .flat_map(|v| &v.fields)
                .any(|f| f.boxed || f.ty.is_counted(&lowered));
            lowered[e].counted = counted;
        }
        self.never_built(cyclic);
        lowered
    }

    /// Reports each enum no value can be built of. Which classes and enums can be built is a
    /// least fixed point over them all: a class can when every field's type can, an enum when
    /// every field of some variant can, and every other type always can. A class in `cyclic`
    /// counts as built, and so does an enum with no variant, since each has its error already.
    /// Each component of the rest that needs no other is reported once, at its first enum; a
    /// type that only needs one of those is not.
    fn never_built(&mut self, cyclic: &[bool]) {
        let classes = self.classes.len();
        let node = |ty: &Option<Type>| match ty.map(Type::kind) {
            Some(TypeKind::Class(id)) => Some(id.0),
            Some(TypeKind::Enum(id)) => Some(classes + id.0),
            _ => None,
        };
        // Each class, and each variant, as (its type's node, the nodes it needs built).
        let mut slots: Vec<(usize, Vec<usize>)> = Vec::new();
        for (c, class) in self.classes.iter().enumerate() {
            let needs = if cyclic[c] {
                Vec::new()
            } else {
                class.fields.iter().filter_map(|f| node(&f.ty)).collect()
            };
            slots.push((c, needs));
        }
        for (e, info) in self.enums.iter().enumerate() {
            if info.variants.is_empty() {
                slots.push((classes + e, Vec::new()));
            }
            for variant in &info.variants {
                slots.push((
                    classes + e,
                    variant.fields.iter().filter_map(node).collect(),
                ));
            }
        }
        let nodes = classes + self.enums.len();
        let mut waiting = vec![Vec::new(); nodes];
        for (s, (_, needs)) in slots.iter().enumerate() {
            for &need in needs {
                waiting[need].push(s);
            }
        }
        let mut missing: Vec<usize> = slots.iter().map(|(_, needs)| needs.len()).collect();
        let mut built = vec![false; nodes];
        let mut ready: Vec<usize> = Vec::new();
        let complete = |s: usize, built: &mut Vec<bool>, ready: &mut Vec<usize>| {
            let owner = slots[s].0;
            if !built[owner] {
                built[owner] = true;
                ready.push(owner);
            }
        };
        for s in (0..slots.len()).filter(|&s| missing[s] == 0) {
            complete(s, &mut built, &mut ready);
        }
        while let Some(n) = ready.pop() {
            for &s in &waiting[n] {
                missing[s] -= 1;
                if missing[s] == 0 {
                    complete(s, &mut built, &mut ready);
                }
            }
        }
        // What each node no value can be built of needs that none can be built of.
        let mut needs = vec![Vec::new(); nodes];
        for (owner, wants) in &slots {
            if !built[*owner] {
                needs[*owner].extend(wants.iter().copied().filter(|&n| !built[n]));
            }
        }
        let component = components(&needs);
        let mut leaves = vec![true; nodes];
        for (n, wants) in needs.iter().enumerate() {
            if wants.iter().any(|&w| component[w] != component[n]) {
                leaves[component[n]] = false;
            }
        }
        let mut reported = vec![false; nodes];
        for e in 0..self.enums.len() {
            let n = classes + e;
            if built[n] || !leaves[component[n]] || reported[component[n]] {
                continue;
            }
            reported[component[n]] = true;
            let chain = self.chain(n, &built);
            let name = &self.enums[e].name;
            self.report(DiagnosticKind::NeverConstructed {
                name: name.name.to_string(),
                at: name.span,
                cause: Unbuilt::Chain(chain),
            });
        }
    }

    /// The chain of fields from the enum at node `start`, through types none of which can be
    /// `built`: each link's holder (`E.V` or `K.f`) and the type held. At an enum it takes the
    /// first variant's first such field, at a class the first such field, and it ends at the
    /// first type it meets again.
    fn chain(&self, start: usize, built: &[bool]) -> Vec<(String, String)> {
        let classes = self.classes.len();
        // A field named `name` of type `ty`, if that type cannot be built: its name, its type's
        // name and its node.
        let unbuilt = |(name, ty): (&str, Option<Type>)| {
            let ty = ty?;
            let n = match ty.kind() {
                TypeKind::Class(id) => id.0,
                TypeKind::Enum(id) => classes + id.0,
                _ => return None,
            };
            (!built[n]).then(|| (name.to_string(), self.type_name(ty), n))
        };
        let mut chain = Vec::new();
        let mut on_chain = vec![false; built.len()];
        let mut at = start;
        while !std::mem::replace(&mut on_chain[at], true) {
            let (holder, field) = if at < classes {
                let class = &self.classes[at];
                let mut fields = class.fields.iter().map(|f| (f.name.name.as_str(), f.ty));
                (&class.name, fields.find_map(unbuilt))
            } else {
                let e = &self.enums[at - classes];
                let mut fields = e.variants.first().into_iter().flat_map(|v| {
                    let name = v.name.name.as_str();
                    v.fields.iter().map(move |&ty| (name, ty))
                });
                (&e.name, fields.find_map(unbuilt))
            };
            let (name, held, next) =
                field.expect("a type no value can be built of should need another such type");
            chain.push((format!("{}.{name}", holder.name), held));
            at = next;
        }
        chain
    }
}
