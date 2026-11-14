//! Struct declarations: their names, fields and methods. A struct has the shape of a class, so its
//! table holds a `ClassInfo` for each, indexed by `StructId`.

use rustc_hash::FxHashMap;

use super::generic::Site;
use super::reports::Reported;
use super::{Checked, Checker, ClassInfo, Declared, Definition, Holder, Poisoned};
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

    /// `Struct.member`, or `Struct.member(args)` when `call` gives the arguments and the span of
    /// their parentheses. `site` writes the struct `id`, or an alias of it, and `member` is one of
    /// its methods. Then this reports the method named through its type. `typed` is whether the
    /// member is written with type arguments. `None` for any other member, which the value check
    /// reports. A method of an `extend` block is one of them.
    pub(super) fn struct_path(
        &mut self,
        id: StructId,
        member: &ast::Ident,
        call: Option<(&[ast::Arg], Span)>,
        (site, typed): (Site<'_>, bool),
    ) -> Option<Checked> {
        let args = call.map_or(&[][..], |(args, _)| args);
        let reported = match *self.structs[id.0].methods.get(&member.name)? {
            Declared::Known(func) => {
                let type_name = site.name.name;
                let site = (site, type_name.as_str(), typed);
                self.method_through_type(func, Holder::Struct(id), member, site, args)
            }
            // The method's own error is reported.
            Declared::Poisoned(reported) => {
                self.stray(args);
                reported
            }
        };
        Some(Err(Poisoned::untyped(reported)))
    }
}
