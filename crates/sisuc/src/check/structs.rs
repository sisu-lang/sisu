//! Struct declarations: their names, fields and methods. A struct has the shape of a class, so its
//! table holds a `ClassInfo` for each, indexed by `StructId`.

use rustc_hash::FxHashMap;

use super::reports::Reported;
use super::{Checker, ClassInfo, Definition};
use crate::ast;
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::StructId;

impl Checker<'_> {
    /// Pass 1 for the struct `s`: gives it a `StructId` and records its name and its type
    /// parameters, as for a class. Returns how many methods it declares.
    pub(super) fn collect_struct(
        &mut self,
        s: &ast::Struct,
        first: &mut FxHashMap<Symbol, (Span, Definition)>,
        rejected: &mut Vec<(Definition, Symbol, Reported)>,
    ) -> usize {
        let name = &s.name;
        let type_params = self.type_params(&[], &s.type_params);
        let bounded = s.type_params.iter().any(|p| !p.bounds.is_empty());
        let id = StructId(self.structs.len());
        self.structs
            .push(ClassInfo::new(name.clone(), type_params, bounded));
        if !self.built_in(name) && self.first_callable(first, rejected, Definition::Struct, name) {
            self.names.add_struct(name.name, id);
        }
        (s.members.iter())
            .filter(|m| matches!(m, ast::Member::Method(_)))
            .count()
    }
}
