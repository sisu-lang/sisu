//! Structs at run time. A struct value is `%sisu.<Name> = { fields... }`, its fields inline in
//! declaration order. It has no count and no identity: a copy copies the fields. This module
//! owns building a value, the address of a field, and the bodies of the struct helpers, which
//! `define_helpers` reaches through `define_struct_helper`.

use inkwell::values::{BasicValueEnum, FunctionValue, PointerValue};

use super::Codegen;
use super::rc::Helper;
use crate::tir::StructId;

impl<'ctx> Codegen<'ctx, '_> {
    /// The address of field `index` of the value of struct `id` at `address`.
    pub(super) fn struct_field_ptr(
        &self,
        id: StructId,
        address: PointerValue<'ctx>,
        index: usize,
    ) -> PointerValue<'ctx> {
        let index = u32::try_from(index).expect("a field index should fit in a `u32`");
        self.builder
            .build_struct_gep(self.structs[id.0], address, index, "field")
            .expect("a struct type should have an element for each field")
    }

    /// A value of struct `id` whose fields, in declaration order, hold `values`.
    pub(super) fn struct_value(
        &self,
        id: StructId,
        values: &[BasicValueEnum<'ctx>],
    ) -> BasicValueEnum<'ctx> {
        let ty = self.structs[id.0];
        let slot = self.entry_alloca(ty.into(), "struct");
        for (index, &value) in values.iter().enumerate() {
            self.store(self.struct_field_ptr(id, slot, index), value);
        }
        self.value_at(ty.into(), slot, "struct")
    }

    /// The body of `@sisu_rc.<kind>.<S>`: `retain` or `release` of each counted field of the
    /// value at the parameter, through `retain_at` or `release_at`.
    pub(super) fn define_struct_helper(
        &self,
        kind: Helper,
        id: StructId,
        function: FunctionValue<'ctx>,
    ) {
        let (value, done) = self.enum_entry(function);
        let fields = &self.program.structs[id.0].fields;
        for (index, field) in fields.iter().enumerate() {
            if !field
                .ty
                .is_counted(&self.program.enums, &self.program.structs)
            {
                continue;
            }
            let place = self.struct_field_ptr(id, value, index);
            match kind {
                Helper::Retain => self.retain_at(place, field.ty),
                Helper::Release => self.release_at(place, field.ty),
                // The checker reports E0159 for `==` on a struct until Task 4 generates it.
                _ => unreachable!("`helper` should queue for a struct only `retain` or `release`"),
            }
        }
        self.branch_to(done);
    }
}
