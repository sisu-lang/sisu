//! Struct declarations: their names, fields and methods. A struct has the shape of a class, so its
//! table holds a `ClassInfo` for each, indexed by `StructId`.

use rustc_hash::{FxHashMap, FxHashSet};

use super::interface::Slot;
use super::reports::Reported;
use super::{
    Checker, ClassInfo, Declared, Definition, DiagnosticKind, FieldInfo, FieldType, SelfType,
};
use crate::ast;
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, Rank, StructId, Type, TypeArgs, TypeKind};

/// The names that an `extend` block of `items` takes as its target.
pub(super) fn extended(items: &[&ast::Item]) -> FxHashSet<Symbol> {
    let target = |extend: &ast::Extend| match &extend.target.kind {
        ast::TypeKind::Name(name) => Some(*name),
        ast::TypeKind::Applied(applied) => Some(applied.name),
        _ => None,
    };
    (items.iter())
        .filter_map(|item| match item {
            ast::Item::Extend(extend) => target(extend),
            _ => None,
        })
        .collect()
}

/// Whether the struct `s` has a form that a later task builds: type parameters, a conformance
/// list, a generic method, or an `extend` block of its name in `extended`.
fn waits(s: &ast::Struct, extended: &FxHashSet<Symbol>) -> bool {
    !s.type_params.is_empty()
        || !s.conforms.is_empty()
        || extended.contains(&s.name.name)
        || (s.members.iter())
            .any(|m| matches!(m, ast::Member::Method(f) if !f.type_params.is_empty()))
}

impl Checker<'_> {
    /// Pass 1 for the struct `s`: gives it a `StructId` and records its name, as for a class.
    /// Returns how many methods it declares. A form in `waits` is E0083 at the keyword until a
    /// later task builds it. Its name is poisoned, so its uses give no error of their own.
    pub(super) fn collect_struct(
        &mut self,
        s: &ast::Struct,
        extended: &FxHashSet<Symbol>,
        first: &mut FxHashMap<Symbol, (Span, Definition)>,
        rejected: &mut Vec<(Definition, Symbol, Reported)>,
    ) -> usize {
        let name = &s.name;
        if waits(s, extended) {
            let reported = self.error(DiagnosticKind::ReservedWord {
                word: "struct",
                at: s.keyword,
            });
            if !self.built_in(name) {
                self.first_callable(first, rejected, Definition::Struct, name);
                rejected.push((Definition::Struct, name.name, reported));
            }
            return 0;
        }
        let id = StructId(self.structs.len());
        self.structs
            .push(ClassInfo::new(name.clone(), Vec::new(), false));
        if !self.built_in(name) && self.first_callable(first, rejected, Definition::Struct, name) {
            self.names.add_struct(name.name, id);
        }
        (s.members.iter())
            .filter(|m| matches!(m, ast::Member::Method(_)))
            .count()
    }

    /// Pass 2 for the struct `s`, if `collect_struct` gave it the id `next`: its fields and the
    /// signatures of its methods, which take a plain `self`. A field and a method share a
    /// namespace. Returns whether it took the id.
    pub(super) fn struct_signatures<'a>(
        &mut self,
        s: &'a ast::Struct,
        next: usize,
        declared: &mut Vec<(&'a ast::Function, SelfType)>,
    ) -> bool {
        if self
            .structs
            .get(next)
            .is_none_or(|info| info.name.span != s.name.span)
        {
            return false;
        }
        let id = StructId(next);
        let owner = s.name.name.as_str();
        let receiver = Type::intern(TypeKind::Struct(id, TypeArgs::NONE));
        self.body.self_type = SelfType::Known(receiver);
        let mut first = FxHashMap::default();
        for member in &s.members {
            match member {
                ast::Member::Field(field) => {
                    let from = self.queued();
                    let FieldType { ty, inner, shown } = self.resolve_field(&field.ty, true);
                    let queued = from..self.queued();
                    if self.first_definition(
                        &mut first,
                        Definition::Field,
                        Some(owner),
                        &field.name,
                    ) {
                        let at = self.structs[id.0].fields.len();
                        self.keep_at(queued, Slot::StructField(id, at));
                        self.structs[id.0].add_field(FieldInfo {
                            name: field.name.clone(),
                            ty_span: field.ty.span,
                            inner,
                            ty,
                            shown,
                            // A struct field changes as its holder does.
                            mutable: false,
                        });
                    }
                }
                ast::Member::Method(method) => {
                    let func = tir::FuncId(self.signatures.len());
                    if self.first_definition(
                        &mut first,
                        Definition::Method,
                        Some(owner),
                        &method.name,
                    ) {
                        (self.structs[id.0].methods)
                            .insert(method.name.name, Declared::Known(func));
                    }
                    let name = format!("{owner}.{}", method.name.name);
                    self.signature(method, name, &[], Vec::new());
                    declared.push((method, SelfType::Known(receiver)));
                }
            }
        }
        for method in &s.broken_methods {
            let reported = self.proof(method.span); // proof: parser mark
            (self.structs[id.0].methods).insert(method.name, Declared::Poisoned(reported));
        }
        self.body.self_type = SelfType::Absent;
        true
    }
}

/// The struct, unless a field's type is `Error`.
pub(super) fn lower_struct(info: &ClassInfo) -> Option<tir::Struct> {
    let fields = (info.fields.iter())
        .map(|f| {
            Some(tir::Payload {
                ty: f.ty?,
                label: Some(f.name.name.to_string()),
                boxed: false,
                rank: Rank::Plain,
            })
        })
        .collect::<Option<_>>()?;
    Some(tir::Struct {
        name: info.name.name.to_string(),
        fields,
    })
}
