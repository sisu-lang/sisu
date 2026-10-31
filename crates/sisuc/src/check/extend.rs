//! `extend` blocks: their targets, their methods, and the methods of one name that a class or an
//! enum holds in its body and in plain blocks.

use rustc_hash::FxHashMap;

use super::generic::named_params;
use super::interface::Bound;
use super::{Checker, Choices, Declared, Definition, DiagnosticKind, Holder, Symbol, UNKNOWN};
use crate::ast;
use crate::conformance::{bind, overlap};
use crate::diagnostic::{BlockNeed, CannotExtendForm, Earlier, Owner as MethodOwner};
use crate::lexer::{TokenKind, lex};
use crate::source::Span;
use crate::tir::{self, Type, TypeKind};

/// A definition of a method name, among those of a class or an enum: where its name is, its
/// kind, and the target and type parameters of the block that holds it; none for the body.
type Def = (Span, Definition, Option<(Type, usize)>);

/// An `extend` block whose target is a class or an enum.
pub(super) struct Extension {
    pub(super) holder: Holder,
    /// The `extend` keyword through the target, as written: where W0012 reports an unused block.
    pub(super) header: Span,
    /// The target, as written.
    pub(super) written: Span,
    pub(super) target: Type,
    /// The block's type parameters, which are the `Param`s of `target`, numbered from 0.
    pub(super) type_params: Vec<Symbol>,
    /// The bounds of each of `type_params`.
    pub(super) bounds: Vec<Vec<Bound>>,
    /// The interface of an `extend T: I` block, and where it is written.
    pub(super) interface: Option<(Bound, Span)>,
    /// The associated types the block chooses.
    pub(super) assoc: std::rc::Rc<Choices>,
    /// The first method of each name.
    pub(super) methods: FxHashMap<Symbol, tir::FuncId>,
    /// Whether the type already conforms to the interface, so that the block's methods answer no
    /// call: its conformance is reported (E0153).
    pub(super) duplicate: bool,
}

impl<'a> Checker<'_> {
    /// Resolves the target and the interface of `extend`, then the signatures of its methods, and
    /// adds each method to `declared` with `self`'s type. The block is recorded when its target
    /// is a class or an enum that the checker can take, and its interface, if it writes one, is an
    /// interface that it can take. Otherwise its methods get signatures, so that the ids of the
    /// functions after them stay in place, and `declared` holds them with an unknown receiver:
    /// their bodies are not checked, since the block's own error says what is wrong.
    pub(super) fn extend_signatures(
        &mut self,
        extend: &'a ast::Extend,
        declared: &mut Vec<(&'a ast::Function, Option<Type>)>,
    ) {
        let params = self.type_params(&[], &extend.type_params);
        self.body.set_type_params(&params);
        // `Self` is the target in the bounds of the block's parameters, in the interface the block
        // names, and in its methods. A target that writes an associated type of a parameter is
        // reported, and needs the bounds of the parameters to find it, so it comes after them.
        let reported_late = extend.target.names_member();
        let queued = self.queued();
        let early = if reported_late {
            None
        } else {
            self.extend_target(extend)
        };
        self.body.self_type = Some(early.unwrap_or(UNKNOWN));
        let bounds = self.declare_bounds(0, Vec::new(), &extend.type_params);
        let target = if reported_late {
            self.extend_target(extend)
        } else {
            // The checks of the target's arguments waited with no bound of the block in scope.
            self.rescope(queued);
            early
        };
        self.body.self_type = Some(target.unwrap_or(UNKNOWN));
        let interface = (extend.interface.as_ref()).map(|ty| {
            let bound = self.bound(ty, target)?;
            // The block's parameters could not be found from the receiver (see `extend_target`).
            if bound.args.get().iter().any(|arg| arg.has_assoc()) {
                self.cannot_extend_assoc(ty);
                return None;
            }
            Some((bound, ty.bare))
        });
        let assoc = std::rc::Rc::new(Choices::new(if extend.interface.is_some() {
            self.resolve_choices(&extend.assoc, &extend.target.to_string())
        } else {
            Vec::new()
        }));
        let unused = target.is_some_and(|target| self.unused_params(extend, target));
        let failed = unused || matches!(interface, Some(None));
        let usable = target.filter(|_| !failed);
        let receiver = usable.unwrap_or(UNKNOWN);
        let holder = usable.and_then(method_holder);
        self.body.self_type = Some(receiver);
        self.body.assoc = std::rc::Rc::clone(&assoc);
        let name = self.type_name(receiver);
        let at = self.extensions.len();
        let mut methods = FxHashMap::default();
        let mut first = FxHashMap::default();
        for method in &extend.methods {
            let func = tir::FuncId(self.signatures.len());
            if self.first_definition(&mut first, Definition::Method, Some(&name), &method.name)
                && let Some(holder) = holder
            {
                methods.insert(method.name.name, func);
                self.extension_methods
                    .entry((holder, method.name.name))
                    .or_default()
                    .push((at, func));
                if matches!(holder, Holder::Array | Holder::I64) {
                    self.builtin_methods.push((holder, method.name.name, at));
                }
            }
            let signature = format!("{name}.{}", method.name.name);
            self.signature(method, signature, &extend.type_params, bounds.clone());
            if usable.is_some() {
                self.signatures[func.0].extension = Some(at);
            }
            if let Some(Some((bound, _))) = interface {
                self.signatures[func.0].conformance = Some((bound.interface, bound.args));
            }
            if let (Some(mut_self), Some(holder)) = (&method.mut_self, holder) {
                let owner = method_owner(holder, &name);
                self.report(DiagnosticKind::MutSelf {
                    keyword: mut_self.keyword,
                    receiver: mut_self.receiver,
                    safe: mut_self.annotation.is_none(),
                    owner,
                });
            }
            declared.push((method, Some(receiver)));
        }
        self.body.set_type_params(&[]);
        self.body.self_type = None;
        self.body.assoc = std::rc::Rc::default();
        if let (Some(target), Some(holder)) = (usable, holder) {
            for method in &extend.broken_methods {
                self.broken_extension_methods.insert((holder, method.name));
            }
            self.extensions.push(Extension {
                holder,
                header: extend.keyword.to(extend.target.bare),
                written: extend.target.bare,
                target,
                type_params: params,
                bounds,
                interface: interface.flatten(),
                assoc,
                methods,
                duplicate: false,
            });
            self.record_block_bounds(at);
        } else {
            self.record_failed_block(extend, target);
        }
    }

    /// Records what a failed block may have given. When the target is a type that the checker
    /// can take, the block may have given methods to that type. Else it may have given methods of
    /// its names to any type.
    fn record_failed_block(&mut self, extend: &ast::Extend, target: Option<Type>) {
        if let Some(holder) = target.and_then(method_holder) {
            let name = self.holder_name(holder);
            self.broken_extensions.insert(name);
        } else {
            // No target: its methods may belong to any type (see `Checker.rejected_methods`).
            let names = (extend.methods.iter().map(|m| m.name.name))
                .chain(extend.broken_methods.iter().map(|m| m.name));
            self.rejected_methods.extend(names);
        }
    }

    /// What the type parameters of the block `extension` stand for when its target covers `ty`.
    /// `None` when it does not cover `ty`. Its bounds are not checked.
    pub(super) fn covers(&self, extension: usize, ty: Type) -> Option<tir::TypeArgs> {
        let ext = &self.extensions[extension];
        if ext.type_params.is_empty() {
            return (ext.target == ty).then_some(tir::TypeArgs::NONE);
        }
        bind(ext.target, ext.type_params.len(), ty)
    }

    /// The type arguments of the owner of the method `func`, which a call on a receiver of type
    /// `ty` gives its first type parameters: its class's or enum's, or what the parameters of
    /// its `extend` block stand for.
    pub(super) fn owner_args(&self, func: tir::FuncId, ty: Type) -> tir::TypeArgs {
        if let Some(at) = self.signatures[func.0].extension {
            return (self.covers(at, ty))
                .expect("the block of a method that a call found should cover the receiver");
        }
        match ty.kind() {
            TypeKind::Class(_, args) | TypeKind::Enum(_, args) => args,
            _ => unreachable!("only a class or an enum has methods"),
        }
    }

    /// Whether the bounds of the block `extension` hold for `stands`, what its type parameters
    /// stand for.
    pub(super) fn block_holds(&mut self, extension: usize, stands: tir::TypeArgs) -> bool {
        self.first_failed_bound(extension, stands).is_none()
    }

    /// The first bound of the block `extension` that fails for `stands`, as the place of its type
    /// parameter and the bound.
    fn first_failed_bound(
        &mut self,
        extension: usize,
        stands: tir::TypeArgs,
    ) -> Option<(usize, Bound)> {
        if self.extensions[extension].bounds.iter().all(Vec::is_empty) {
            return None;
        }
        let bounds = self.extensions[extension].bounds.clone();
        for (i, bounds) in bounds.iter().enumerate() {
            for &b in bounds {
                let args = self.subst_args(b.args, stands.get());
                if !self.conforms(stands.get()[i], b.interface, args) {
                    return Some((i, b));
                }
            }
        }
        None
    }

    /// Reports the first bound of the block `extension` that fails for a receiver of type `ty`,
    /// at the call of `method` that starts at `span`: the receiver's type arguments do not meet it.
    pub(super) fn report_block_bound(
        &mut self,
        extension: usize,
        ty: Type,
        span: Span,
        method: &str,
    ) {
        let stands = self
            .covers(extension, ty)
            .expect("a block that fails its bounds should cover the receiver");
        let (i, bound) = (self.first_failed_bound(extension, stands))
            .expect("a block that fails its bounds should have a bound that fails");
        let need = BlockNeed {
            method: method.to_string(),
            stands: self.type_name(stands.get()[i]),
        };
        self.check_bound_of(span, stands.get()[i], bound, stands.get(), Some(need));
    }

    /// Reports each type parameter of `extend` that its target, `target`, does not name. The fix
    /// takes the list out, when it holds one parameter that the rest of the block does not name,
    /// and no comment is in it. Whether there was one.
    fn unused_params(&mut self, extend: &ast::Extend, target: Type) -> bool {
        let params = &extend.type_params;
        let named = named_params(std::iter::once(Some(target)), params.len());
        let written = extend.target.to_string();
        // The text from `extend` to the target, which the fix writes as one space.
        let header = Span::new(extend.keyword.end(), extend.target.span.start());
        let mut unused = false;
        for (param, _) in params.iter().zip(&named).filter(|(_, named)| !**named) {
            let remove = (params.len() == 1 && !self.names_in_block(extend, param.name.name))
                .then_some(header)
                .filter(|header| !self.comment_in(&[header.range()]));
            self.report(DiagnosticKind::CannotExtend {
                written: param.name.name.to_string(),
                at: param.name.span,
                form: CannotExtendForm::Unused {
                    target: written.clone(),
                    remove,
                },
            });
            unused = true;
        }
        unused
    }

    /// Whether `name` is written in `extend` after its type parameters, outside strings and
    /// comments. Text that does not lex alone is held to name it.
    fn names_in_block(&self, extend: &ast::Extend, name: Symbol) -> bool {
        let rest = self.source.slice(extend.angles.end()..extend.span.end());
        lex(rest).map_or(true, |lexed| {
            (lexed.tokens.iter())
                .any(|token| matches!(token.kind, TokenKind::Ident(n) if n == name))
        })
    }

    /// The type that the target of `extend` names. It is a class or an enum. In a plain block of
    /// the prelude with no `mut self` method, it can be an array or `i64`. Reports a target that
    /// is none of these, and gives `None` for it.
    fn extend_target(&mut self, extend: &ast::Extend) -> Option<Type> {
        let ty = &extend.target;
        if self.is_interface_name(ty) {
            let name = match &ty.kind {
                ast::TypeKind::Applied(applied) => applied.name,
                ast::TypeKind::Name(name) => *name,
                _ => unreachable!("an interface name should be a name or an applied name"),
            };
            // Following the advice of `InterfaceAsType` would give `CannotExtend`, so it has none.
            self.report(DiagnosticKind::InterfaceAsType {
                name: name.to_string(),
                written: None,
                at: ty.bare,
            });
            return None;
        }
        let resolved = self.resolve(ty)?;
        let form = match resolved.kind() {
            TypeKind::Class(..) | TypeKind::Enum(..) if !resolved.has_assoc() => {
                return Some(resolved);
            }
            // An associated type in the target: the block's parameters could not be found from
            // the receiver, and `overlap` and `find` compare types structurally.
            TypeKind::Class(..) | TypeKind::Enum(..) | TypeKind::Assoc(..) => {
                self.cannot_extend_assoc(ty);
                return None;
            }
            TypeKind::Array(_) | TypeKind::I64
                if ty.bare.in_prelude()
                    && extend.interface.is_none()
                    && extend.methods.iter().all(|m| m.mut_self.is_none()) =>
            {
                return Some(resolved);
            }
            TypeKind::Param(..) => CannotExtendForm::TypeParam,
            _ => CannotExtendForm::BuiltIn,
        };
        self.report(DiagnosticKind::CannotExtend {
            written: ty.to_string(),
            at: ty.bare,
            form,
        });
        None
    }

    /// The name of the class or enum `holder`; for a built-in type, a name no item can take.
    pub(super) fn holder_name(&self, holder: Holder) -> Symbol {
        match holder {
            Holder::Class(c) => self.classes[c.0].name.name,
            Holder::Enum(e) => self.enums[e.0].name.name,
            Holder::Array => Symbol::intern("[]"),
            Holder::I64 => Symbol::intern("i64"),
        }
    }

    /// Reports each method that a class or an enum holds in its body and in plain `extend`
    /// blocks, or in two plain blocks whose targets overlap, by one name, and each plain block's
    /// method named like a field or a variant. They share a namespace; the one that starts last
    /// is the one reported, against the first earlier one that covers a type with it. The
    /// methods of a conformance block are another namespace: the block's. Only the blocks that
    /// hold a method of the name are read, and a block that extends one instantiation meets the
    /// others through a table, so the work follows the methods and not the pairs of blocks, but
    /// for a block with type parameters, which meets each earlier block of the name.
    ///
    /// That is the second recorded exception to "A pass is linear in the program" (user,
    /// 2026-10-25; ruling F9 (d)): `n` plain blocks with parameters that hold a method of one name
    /// make `n` overlap tests for the last, `n²` in all. An index on the whole target would
    /// remove it, and waits until a program needs it.
    pub(super) fn extension_clashes(&mut self) {
        let mut reports = Vec::new();
        for (&(holder, name), list) in &self.extension_methods {
            let owner = self.holder_name(holder).to_string();
            let plain = list
                .iter()
                .filter(|&&(at, _)| self.extensions[at].interface.is_none());
            // Each definition of the name: where it is, its kind, and the target it covers, with
            // its parameters. The body's has none: it covers every instantiation.
            let mut defs: Vec<Def> = self
                .body_definition(holder, name)
                .into_iter()
                .map(|(span, kind)| (span, kind, None))
                .collect();
            defs.extend(plain.map(|&(at, f)| {
                let ext = &self.extensions[at];
                let target = Some((ext.target, ext.type_params.len()));
                (self.signatures[f.0].span, Definition::Method, target)
            }));
            defs.sort_by_key(|(span, ..)| span.start());
            // The first earlier definition, the first of each exact target, and the places of
            // the earlier ones that have parameters.
            let mut exact: FxHashMap<Type, usize> = FxHashMap::default();
            let mut generic: Vec<(usize, Type, usize)> = Vec::new();
            let mut universal = None;
            for (i, &(span, kind, target)) in defs.iter().enumerate() {
                let earlier = match target {
                    None => (i > 0).then_some(0),
                    Some((ty, 0)) => {
                        let overlapping = (generic.iter())
                            .find(|&&(_, other, params)| {
                                overlap(&[ty], 0, &[other], params).is_some()
                            })
                            .map(|&(g, ..)| g);
                        let hits = [universal, exact.get(&ty).copied(), overlapping];
                        hits.into_iter().flatten().min()
                    }
                    // The recorded exception of the doc comment: the block meets every earlier
                    // definition of the name.
                    Some((ty, params)) => (0..i).find(|&g| match defs[g].2 {
                        None => true,
                        Some((other, others)) => overlap(&[ty], params, &[other], others).is_some(),
                    }),
                };
                match target {
                    None => {
                        universal.get_or_insert(i);
                    }
                    Some((ty, 0)) => {
                        exact.entry(ty).or_insert(i);
                    }
                    Some((ty, params)) => generic.push((i, ty, params)),
                }
                if let Some(first) = earlier {
                    let (first, first_kind, _) = defs[first];
                    reports.push(super::defined_twice(
                        kind,
                        &ast::Ident { name, span },
                        Earlier::At {
                            span: first,
                            kind: first_kind,
                            owner: Some(owner.clone()),
                        },
                    ));
                }
            }
        }
        for report in reports {
            self.report(report);
        }
    }

    /// The method, field or variant `name` of `holder`'s own declaration, and where its name is.
    fn body_definition(&self, holder: Holder, name: Symbol) -> Option<(Span, Definition)> {
        match holder {
            Holder::Array | Holder::I64 => None,
            Holder::Class(c) => {
                let info = &self.classes[c.0];
                match info.methods.get(&name) {
                    Some(&Declared::Known(f)) => {
                        Some((self.signatures[f.0].span, Definition::Method))
                    }
                    _ => info
                        .field(name)
                        .map(|i| (info.fields[i].name.span, Definition::Field)),
                }
            }
            Holder::Enum(e) => {
                let info = &self.enums[e.0];
                match info.methods.get(&name) {
                    Some(&Declared::Known(f)) => {
                        Some((self.signatures[f.0].span, Definition::Method))
                    }
                    _ => info
                        .index
                        .get(&name)
                        .map(|&v| (info.variants[v].name.span, Definition::Variant)),
                }
            }
        }
    }
}

/// The class or enum that `ty` is: what holds its conformances. Only these have any, so the
/// walks of the conformances never meet a built-in type here.
pub(super) fn holder_of(ty: Type) -> Option<Holder> {
    match ty.kind() {
        TypeKind::Class(id, _) => Some(Holder::Class(id)),
        TypeKind::Enum(id, _) => Some(Holder::Enum(id)),
        _ => None,
    }
}

/// `holder_of`, or the array or `i64` that `ty` is: what holds its methods, which the prelude's
/// `extend` blocks give.
pub(super) fn method_holder(ty: Type) -> Option<Holder> {
    match ty.kind() {
        TypeKind::Array(_) => Some(Holder::Array),
        TypeKind::I64 => Some(Holder::I64),
        _ => holder_of(ty),
    }
}

/// What owns the methods of `holder`, whose type is named `name`, in the report of a `mut self`.
fn method_owner(holder: Holder, name: &str) -> MethodOwner {
    match holder {
        Holder::Class(_) => MethodOwner::Class,
        Holder::Enum(_) => MethodOwner::Enum(name.to_string()),
        Holder::Array | Holder::I64 => {
            unreachable!("the prelude should write no `mut self` on a built-in type")
        }
    }
}
