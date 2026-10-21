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
use crate::tir::{self, EnumId, Type, TypeArgs, TypeKind};

pub(super) struct EnumInfo {
    pub(super) name: ast::Ident,
    pub(super) type_params: Vec<Symbol>,
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
            self.body
                .type_params
                .clone_from(&self.enums[id.0].type_params);
            let fields = payload
                .iter()
                .map(|field| self.resolve(&field.ty))
                .collect();
            self.body.type_params.clear();
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
        let receiver = Type::intern(TypeKind::Enum(
            id,
            super::own_args(&self.enums[id.0].type_params),
        ));
        for method in &e.methods {
            let func = tir::FuncId(self.signatures.len());
            if self.first_definition(&mut first, Definition::Method, Some(owner), &method.name) {
                self.enums[id.0]
                    .methods
                    .insert(method.name.name, Declared::Known(func));
            }
            self.signature(
                method,
                format!("{owner}.{}", method.name.name),
                &e.type_params,
            );
            declared.push((method, Some(receiver)));
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
        let ty = Type::intern(TypeKind::Enum(id, TypeArgs::NONE));
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
                    type_params: info.type_params.iter().map(Symbol::to_string).collect(),
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
                .any(|f| f.boxed || f.ty.may_be_counted(&lowered));
            lowered[e].counted = counted;
        }
        self.never_built(cyclic);
        lowered
    }

    /// Reports each enum no value can be built of. Which classes and enums can be built is a
    /// least fixed point over them all, with the arguments they are given: a class can when
    /// every field's type can, an enum when every field of some variant can, and every other
    /// type always can. A class in `cyclic` counts as built, and so does an enum with no
    /// variant, since each has its error already. Each component of the rest that needs no
    /// other is reported once, at its first enum; a type that only needs one of those is not.
    fn never_built(&mut self, cyclic: &[bool]) {
        let mut fix = Buildable::new(self, cyclic);
        fix.run();
        let classes = self.classes.len();
        // What each declaration no value can be built of needs that none can be built of.
        let mut needs = vec![Vec::new(); fix.slots.len()];
        for (n, wants) in needs.iter_mut().enumerate() {
            if !fix.built[fix.root(n)] {
                for &ty in fix.slots[n].iter().flatten().flatten() {
                    fix.blame(ty, wants);
                }
            }
        }
        let component = components(&needs);
        let mut leaves = vec![true; needs.len()];
        for (n, wants) in needs.iter().enumerate() {
            if wants.iter().any(|&w| component[w] != component[n]) {
                leaves[component[n]] = false;
            }
        }
        let mut reported = vec![false; needs.len()];
        for e in 0..self.enums.len() {
            let n = classes + e;
            if fix.built[fix.root(n)] || !leaves[component[n]] || reported[component[n]] {
                continue;
            }
            reported[component[n]] = true;
            let chain = self.chain(n, &fix);
            let name = &self.enums[e].name;
            self.report(DiagnosticKind::NeverConstructed {
                name: name.name.to_string(),
                at: name.span,
                cause: Unbuilt::Chain(chain),
            });
        }
    }

    /// The chain of fields from the enum at node `start`, through types none of which can be
    /// built by `fix`: each link's holder (`E.V` or `K.f`) and the type held, with the arguments
    /// the chain gives it. At an enum it takes the first variant's first such field, at a class
    /// the first such field, and it ends at the first type it meets again.
    fn chain(&self, start: usize, fix: &Buildable) -> Vec<(String, String)> {
        let classes = self.classes.len();
        let params = &self.enums[start - classes].type_params;
        // A field named `name` of type `ty`, if that type cannot be built: its name, its type's
        // name, and its node and arguments.
        let unbuilt = |(name, ty): (&str, Type)| {
            let (n, args) = match ty.kind() {
                TypeKind::Class(id, args) => (id.0, args),
                TypeKind::Enum(id, args) => (classes + id.0, args),
                _ => return None,
            };
            let key = fix.key_of(n, args);
            (!fix.built[key]).then(|| (name.to_string(), self.type_name(ty), (n, args), key))
        };
        let mut chain = Vec::new();
        let mut on_chain = vec![false; fix.keys.len()];
        let (mut at, mut args) = (start, super::own_args(params));
        let mut key = fix.root(start);
        while !std::mem::replace(&mut on_chain[key], true) {
            let with = args.get();
            let held = |ty: &Option<Type>| ty.map(|ty| ty.subst(with));
            let (holder, field) = if at < classes {
                let class = &self.classes[at];
                let mut fields = class
                    .fields
                    .iter()
                    .filter_map(|f| Some((f.name.name.as_str(), held(&f.ty)?)));
                (&class.name, fields.find_map(unbuilt))
            } else {
                let e = &self.enums[at - classes];
                let mut fields = e.variants.first().into_iter().flat_map(|v| {
                    let name = v.name.name.as_str();
                    v.fields
                        .iter()
                        .filter_map(move |ty| Some((name, held(ty)?)))
                });
                (&e.name, fields.find_map(unbuilt))
            };
            let (name, ty, next, next_key) =
                field.expect("a type no value can be built of should need another such type");
            chain.push((format!("{}.{name}", holder.name), ty));
            ((at, args), key) = (next, next_key);
        }
        chain
    }
}

/// Which declarations can be built, given which of their type parameters' arguments can: the
/// least fixed point over keys, each a declaration (a class's node is its `ClassId`, an
/// enum's follows the classes) and a flag for each of its parameters.
struct Buildable {
    classes: usize,
    /// By node, its slots, each of which builds it when all its field types can be built: a
    /// class's fields, or each variant's payload. A class on a cycle of fields, and an enum with
    /// no variant, have one empty slot.
    slots: Vec<Vec<Vec<Option<Type>>>>,
    /// By node, how many type parameters it has.
    params: Vec<usize>,
    /// The most keys there can be: for each declaration, 2 to the power of its parameters.
    bound: usize,
    ids: FxHashMap<(usize, Vec<bool>), usize>,
    keys: Vec<(usize, Vec<bool>)>,
    built: Vec<bool>,
    /// By key, the slots of other keys that wait for it to be built.
    waiting: Vec<Vec<(usize, usize)>>,
    /// Slots of keys to try: a key and the index of one of its node's slots.
    queue: Vec<(usize, usize)>,
}

impl Buildable {
    fn new(checker: &Checker<'_>, cyclic: &[bool]) -> Buildable {
        let mut slots = Vec::new();
        let mut params = Vec::new();
        for (c, class) in checker.classes.iter().enumerate() {
            let fields = class.fields.iter().map(|f| f.ty);
            slots.push(vec![if cyclic[c] {
                Vec::new()
            } else {
                fields.collect()
            }]);
            params.push(class.type_params.len());
        }
        for info in &checker.enums {
            let variants = info.variants.iter().map(|v| v.fields.clone());
            slots.push(if info.variants.is_empty() {
                vec![Vec::new()]
            } else {
                variants.collect()
            });
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
            params: params.clone(),
            bound,
            ids: FxHashMap::default(),
            keys: Vec::new(),
            built: Vec::new(),
            waiting: Vec::new(),
            queue: Vec::new(),
        };
        for (n, &p) in params.iter().enumerate() {
            fix.key(n, vec![true; p]);
        }
        fix
    }

    /// The key of `node` with `flags`, interned, its slots queued, when it is new.
    fn key(&mut self, node: usize, flags: Vec<bool>) -> usize {
        if let Some(&key) = self.ids.get(&(node, flags.clone())) {
            return key;
        }
        assert!(
            self.keys.len() < self.bound,
            "the can-be-built keys should be a declaration and its parameters' flags"
        );
        let key = self.keys.len();
        self.ids.insert((node, flags.clone()), key);
        self.keys.push((node, flags));
        self.built.push(false);
        self.waiting.push(Vec::new());
        self.queue
            .extend((0..self.slots[node].len()).map(|s| (key, s)));
        key
    }

    /// The key of `node` with every parameter's argument buildable, which `new` interned.
    fn root(&self, node: usize) -> usize {
        self.ids[&(node, vec![true; self.params[node]])]
    }

    /// The key `node` has when given `args`, which `run` interned if a field holds it. A
    /// type parameter in `args` is a chain's start's, whose arguments are all buildable.
    fn key_of(&self, node: usize, args: TypeArgs) -> usize {
        let flags = args.get().iter().map(|&arg| self.reached(arg)).collect();
        self.ids[&(node, flags)]
    }

    /// Whether `ty` was found buildable, a type parameter's argument always.
    fn reached(&self, ty: Type) -> bool {
        let (node, args) = match ty.kind() {
            TypeKind::Class(id, args) => (id.0, args),
            TypeKind::Enum(id, args) => (self.classes + id.0, args),
            _ => return true,
        };
        self.built[self.key_of(node, args)]
    }

    /// Adds to `wants` each declaration `ty` names that cannot be built with any arguments.
    fn blame(&self, ty: Type, wants: &mut Vec<usize>) {
        let (node, args) = match ty.kind() {
            TypeKind::Class(id, args) => (id.0, args),
            TypeKind::Enum(id, args) => (self.classes + id.0, args),
            _ => return,
        };
        if !self.built[self.root(node)] {
            wants.push(node);
        }
        for &arg in args.get() {
            self.blame(arg, wants);
        }
    }

    /// Whether `ty` can be built, where a type parameter's argument can when `flags` says so.
    /// Each declaration `ty` names that cannot be built yet is added to `consulted`.
    fn buildable(&mut self, ty: Option<Type>, flags: &[bool], consulted: &mut Vec<usize>) -> bool {
        let Some(ty) = ty else { return true };
        let (node, args) = match ty.kind() {
            TypeKind::Param(_, i) => return flags[i as usize],
            TypeKind::Class(id, args) => (id.0, args),
            TypeKind::Enum(id, args) => (self.classes + id.0, args),
            _ => return true,
        };
        let args = args
            .get()
            .iter()
            .map(|&arg| self.buildable(Some(arg), flags, consulted))
            .collect();
        let key = self.key(node, args);
        if !self.built[key] {
            consulted.push(key);
        }
        self.built[key]
    }

    fn run(&mut self) {
        while let Some((key, s)) = self.queue.pop() {
            if self.built[key] {
                continue;
            }
            let (node, flags) = self.keys[key].clone();
            let mut consulted = Vec::new();
            // Every field is tried, so that every key a field reaches is interned and tried.
            let mut all = true;
            for ty in self.slots[node][s].clone() {
                all &= self.buildable(ty, &flags, &mut consulted);
            }
            if all {
                self.built[key] = true;
                let waiting = std::mem::take(&mut self.waiting[key]);
                self.queue.extend(waiting);
            } else {
                for c in consulted {
                    self.waiting[c].push((key, s));
                }
            }
        }
    }
}
