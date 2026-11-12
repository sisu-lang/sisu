//! Structs at run time. A struct value is `%sisu.<Name> = { fields... }`, its fields inline in
//! declaration order. It has no count and no identity: a copy copies the fields. A field that
//! closes a cycle holds a box instead, `%sisu.box.<Name> = { i64 count, %sisu.<Name> }`, as an
//! enum's payload field does (`codegen/enums.rs`). This module owns building a value and the
//! address of a field. The struct helpers are an enum's with one variant: `define_inline_helper`.

use inkwell::types::StructType;
use inkwell::values::{BasicValueEnum, PointerValue};

use super::Codegen;
use crate::tir::{Inline, StructId};

/// A struct's LLVM types.
pub(super) struct StructLayout<'ctx> {
    /// `%sisu.<Name>`, its fields in declaration order; a boxed field is its box storage.
    pub(super) ty: StructType<'ctx>,
    /// `%sisu.box.<Name> = { i64 count, %sisu.<Name> }`, a box that holds a value of this
    /// struct.
    pub(super) boxed: StructType<'ctx>,
}

impl<'ctx> Codegen<'ctx, '_> {
    /// The address of field `index` of the value of struct `id` at `address`: the box storage
    /// of a boxed field.
    pub(super) fn struct_field_ptr(
        &self,
        id: StructId,
        address: PointerValue<'ctx>,
        index: usize,
    ) -> PointerValue<'ctx> {
        let index = u32::try_from(index).expect("a field index should fit in a `u32`");
        self.builder
            .build_struct_gep(self.structs[id.0].ty, address, index, "field")
            .expect("a struct type should have an element for each field")
    }

    /// A value of struct `id` whose fields, in declaration order, hold `values`. The slot is zeroed
    /// first, as `variant` does, so a boxed `F?` field given `None` keeps its zero storage. A boxed
    /// field's value moves into a new box with count 1 (`store_boxed`).
    pub(super) fn struct_value(
        &self,
        id: StructId,
        values: &[BasicValueEnum<'ctx>],
    ) -> BasicValueEnum<'ctx> {
        let ty = self.structs[id.0].ty;
        let slot = self.entry_alloca(ty.into(), "struct");
        let fields = &self.program.structs[id.0].fields;
        if fields.iter().any(|field| field.boxed) {
            self.store_zero(slot, ty.into());
        }
        for (index, (&value, field)) in values.iter().zip(fields).enumerate() {
            let place = self.field_slot(slot, Inline::Struct(id), 0, index);
            if field.boxed {
                self.store_boxed(place, value, field.ty, None);
            } else {
                self.store(place, value);
            }
        }
        self.value_at(ty.into(), slot, "struct")
    }
}
