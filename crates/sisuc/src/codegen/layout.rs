//! How a Sisu value sits in memory: the LLVM type of each Sisu type, the layouts of classes,
//! array blocks and string blocks, and the TBAA tags that describe them.

use std::cmp::Reverse;

use inkwell::AddressSpace;
use inkwell::context::Context;
use inkwell::types::{BasicTypeEnum, StructType};
use inkwell::values::{BasicValue, BasicValueEnum, IntValue, MetadataValue, PointerValue};

use super::{Codegen, POSITIONED};
use crate::tir::{Class, ClassId, Type, TypeKind};

/// The tag of a TBAA type `name` under `root`. Every type sits directly under the root, so LLVM
/// assumes that an access tagged with one never touches memory that an access tagged with
/// another does; codegen must keep that true.
pub(super) fn tbaa_tag<'ctx>(
    context: &'ctx Context,
    root: MetadataValue<'ctx>,
    name: &str,
) -> MetadataValue<'ctx> {
    let offset = context.i64_type().const_zero().into();
    let ty = context.metadata_node(&[context.metadata_string(name).into(), root.into(), offset]);
    context.metadata_node(&[ty.into(), ty.into(), offset])
}

/// A class's LLVM type, and where its fields sit in it.
pub(super) struct ClassLayout<'ctx> {
    pub(super) ty: StructType<'ctx>,
    /// `element[i]` is the struct element of declared field `i`; element 0 is the count.
    pub(super) element: Box<[u32]>,
    /// `field_tag[i]` is the TBAA tag of declared field `i`, named `<class>.<field>`.
    pub(super) field_tag: Box<[MetadataValue<'ctx>]>,
}

/// Where an array's elements sit in its block, from the target's data layout.
pub(super) struct ArrayLayout<'ctx> {
    /// The LLVM type of one element.
    pub(super) element: BasicTypeEnum<'ctx>,
    /// The first element's byte offset: the header's size rounded up to the element's
    /// alignment.
    pub(super) offset: u64,
    /// The ABI size of one element, which is also the distance between two elements.
    pub(super) size: u64,
    /// The block's alignment: the larger of the header's and the element's.
    pub(super) align: u64,
}

/// A string block's header, from the target's data layout. A block with capacity `cap` is
/// `size + cap` bytes, aligned to `align`.
pub(super) struct StringLayout<'ctx> {
    /// `{ i64 count, i64 len, i64 cap, [0 x i8] bytes }`: the bytes start where the header ends.
    header: StructType<'ctx>,
    /// The header's ABI size.
    pub(super) size: u64,
    /// The header's ABI alignment, which is the block's.
    pub(super) align: u32,
}

/// The header's fields, by index.
pub(super) const LEN: u32 = 1;
pub(super) const CAP: u32 = 2;
pub(super) const BYTES: u32 = 3;

impl<'ctx> Codegen<'ctx, '_> {
    /// The LLVM type of a value of `ty`; `None` for `unit` and `never`, which have no value.
    pub(super) fn llvm_type(&self, ty: Type) -> Option<BasicTypeEnum<'ctx>> {
        match ty.kind() {
            TypeKind::I64 => Some(self.context.i64_type().into()),
            TypeKind::U8 => Some(self.context.i8_type().into()),
            TypeKind::F64 => Some(self.context.f64_type().into()),
            TypeKind::Bool => Some(self.context.bool_type().into()),
            TypeKind::Char => Some(self.context.i32_type().into()),
            TypeKind::Unit | TypeKind::Never => None,
            // An object, an array or a string is a pointer to its block.
            TypeKind::String | TypeKind::Class(_) | TypeKind::Array(_) => {
                Some(self.context.ptr_type(AddressSpace::default()).into())
            }
            TypeKind::Enum(id) => Some(self.enum_layout(id).ty.into()),
            TypeKind::Optional(payload) => match self
                .llvm_type(payload)
                .expect("the checker should reject an optional of `unit` or `never`, so its payload has a value type")
            {
                // A string, class or array optional is the block's pointer, `null` for `None`.
                ptr @ BasicTypeEnum::PointerType(_) => Some(ptr),
                // A scalar or enum optional is `{ i1, T }`: whether it holds a value, then the
                // value.
                value => {
                    let flag = self.context.bool_type().into();
                    Some(self.context.struct_type(&[flag, value], false).into())
                }
            },
        }
    }

    /// The `{ i1, T }` type of the optional `ty`, or `None` when `ty` is a pointer optional.
    fn optional_pair(&self, ty: Type) -> Option<StructType<'ctx>> {
        match self.llvm_type(ty) {
            Some(BasicTypeEnum::StructType(pair)) => Some(pair),
            _ => None,
        }
    }

    /// The `None` of the optional `ty`: `null`, or `{ false, 0 }`.
    pub(super) fn empty(&self, ty: Type) -> BasicValueEnum<'ctx> {
        self.llvm_type(ty)
            .expect("the checker should reject an optional of `unit` or `never`, so it has an LLVM type")
            .const_zero()
    }

    /// Whether `value`, of the optional `ty`, holds a value: a pointer that is not `null`, or a
    /// pair's flag.
    pub(super) fn has_value(
        &self,
        ty: Type,
        value: BasicValueEnum<'ctx>,
        name: &str,
    ) -> IntValue<'ctx> {
        if self.optional_pair(ty).is_none() {
            return self
                .builder
                .build_is_not_null(value.into_pointer_value(), name)
                .expect(POSITIONED);
        }
        self.builder
            .build_extract_value(value.into_struct_value(), 0, name)
            .expect("a scalar or enum optional should be lowered to a `{ flag, value }` pair")
            .into_int_value()
    }

    /// The optional `ty` that holds `value` when `flag` is set. A pointer optional is the
    /// pointer itself, which is never `null`, so only a constant `true` flag can build one.
    pub(super) fn optional(
        &self,
        ty: Type,
        flag: IntValue<'ctx>,
        value: BasicValueEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let Some(pair) = self.optional_pair(ty) else {
            if flag.get_zero_extended_constant() != Some(1) {
                unreachable!("no site builds a pointer optional from a run-time flag");
            }
            return value;
        };
        let pair = self
            .builder
            .build_insert_value(pair.const_zero(), flag, 0, "some")
            .expect("a scalar or enum optional should be lowered to a `{ flag, value }` pair");
        let pair = self
            .builder
            .build_insert_value(pair, value, 1, "wrap")
            .expect("a scalar or enum optional should be lowered to a `{ flag, value }` pair");
        pair.as_basic_value_enum()
    }

    /// The value that `value`, of the optional `ty`, holds; meaningful only when `has_value`.
    pub(super) fn optional_value(
        &self,
        ty: Type,
        value: BasicValueEnum<'ctx>,
        name: &str,
    ) -> BasicValueEnum<'ctx> {
        if self.optional_pair(ty).is_none() {
            return value;
        }
        self.builder
            .build_extract_value(value.into_struct_value(), 1, name)
            .expect("a scalar or enum optional should be lowered to a `{ flag, value }` pair")
    }

    /// Lays out every enum, then every class: a class's layout needs the sizes of its enum
    /// fields, while an enum's payload holds a class as a pointer. A class is
    /// `{ i64 count, fields... }`, its fields sorted by alignment.
    pub(super) fn lay_out_types(&mut self) {
        self.lay_out_enums();
        // Every class type exists before any body is set, so a field can name any class.
        let types: Vec<StructType> = self
            .program
            .classes
            .iter()
            .map(|class| {
                self.context
                    .opaque_struct_type(&format!("sisu.{}", class.name))
            })
            .collect();
        let layouts: Vec<_> = self
            .program
            .classes
            .iter()
            .map(|class| self.class_body(class))
            .collect();
        for ((ty, (body, element)), class) in
            types.into_iter().zip(layouts).zip(&self.program.classes)
        {
            ty.set_body(&body, false);
            let field_tag = class
                .fields
                .iter()
                .map(|field| {
                    let name = format!("{}.{}", class.name, field.name);
                    tbaa_tag(self.context, self.tbaa_root, &name)
                })
                .collect();
            self.classes.push(ClassLayout {
                ty,
                element,
                field_tag,
            });
        }
    }

    /// The body of `class`, the count and then its fields, and the struct element of each
    /// declared field. Fields go largest alignment first, so that less padding sits between them.
    fn class_body(&self, class: &Class) -> (Vec<BasicTypeEnum<'ctx>>, Box<[u32]>) {
        let fields: Vec<BasicTypeEnum> = class
            .fields
            .iter()
            .map(|field| {
                self.llvm_type(field.ty)
                    .expect("the checker should give every class field a value type, never `unit` or `never`")
            })
            .collect();
        // Largest alignment first; `sort_by_key` is stable, so ties keep declared order.
        let mut order: Vec<usize> = (0..fields.len()).collect();
        order.sort_by_key(|&i| Reverse(self.target_data.get_abi_alignment(&fields[i])));
        let body = std::iter::once(self.context.i64_type().into())
            .chain(order.iter().map(|&i| fields[i]))
            .collect();
        let mut element = vec![0; fields.len()].into_boxed_slice();
        for (k, &i) in order.iter().enumerate() {
            element[i] = u32::try_from(k + 1).expect(
                "a class should have fewer than 2^32 fields, since field indexes are `u32` in LLVM",
            );
        }
        (body, element)
    }

    /// The ABI size and alignment of an object of class `id`, from the target's data layout.
    pub(super) fn class_layout(&self, id: ClassId) -> (u64, u32) {
        let ty = &self.classes[id.0].ty;
        (
            self.target_data.get_abi_size(ty),
            self.target_data.get_abi_alignment(ty),
        )
    }

    /// `load`, tagged with `tag`: `header_tag`, `element_tag` or a field's tag.
    pub(super) fn load_tagged(
        &self,
        ty: BasicTypeEnum<'ctx>,
        ptr: PointerValue<'ctx>,
        name: &str,
        tag: MetadataValue<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let value = self.builder.build_load(ty, ptr, name).expect(POSITIONED);
        let load = value
            .as_instruction_value()
            .expect("a load should be an instruction");
        load.set_metadata(tag, self.tbaa_kind)
            .expect("a TBAA tag should be a metadata node");
        value
    }

    /// `store`, tagged with `tag`: `header_tag`, `element_tag` or a field's tag.
    pub(super) fn store_tagged(
        &self,
        ptr: PointerValue<'ctx>,
        value: BasicValueEnum<'ctx>,
        tag: MetadataValue<'ctx>,
    ) {
        let store = self.builder.build_store(ptr, value).expect(POSITIONED);
        store
            .set_metadata(tag, self.tbaa_kind)
            .expect("a TBAA tag should be a metadata node");
    }

    /// The count of the counted block `object`: the `i64` at offset 0, tagged with `header_tag`.
    pub(super) fn load_count(&self, object: PointerValue<'ctx>, name: &str) -> IntValue<'ctx> {
        self.load_tagged(
            self.context.i64_type().into(),
            object,
            name,
            self.header_tag,
        )
        .into_int_value()
    }

    /// Stores `count` as the count of the counted block `object`, tagged with `header_tag`.
    pub(super) fn store_count(&self, object: PointerValue<'ctx>, count: IntValue<'ctx>) {
        self.store_tagged(object, count.into(), self.header_tag);
    }

    /// Gives the block `sisu_alloc` just returned a count of 1, untagged: nothing has read the
    /// block yet.
    pub(super) fn init_count(&self, object: PointerValue<'ctx>) {
        self.store(object, self.context.i64_type().const_int(1, false).into());
    }

    /// The header: the count, then the length.
    fn header(&self) -> StructType<'ctx> {
        let i64_type = self.context.i64_type().into();
        self.context.struct_type(&[i64_type, i64_type], false)
    }

    pub(super) fn array_layout(&self, element: Type) -> ArrayLayout<'ctx> {
        let header = self.header();
        let ty = self
            .llvm_type(element)
            .expect("the checker should reject an array of `unit` or `never`, so its element has a value type");
        let align = u64::from(self.target_data.get_abi_alignment(&ty));
        ArrayLayout {
            element: ty,
            offset: self
                .target_data
                .get_abi_size(&header)
                .next_multiple_of(align),
            size: self.target_data.get_abi_size(&ty),
            align: u64::from(self.target_data.get_abi_alignment(&header)).max(align),
        }
    }

    /// `offset + len * size`: the size of a block whose length `len` passed the size checks,
    /// or is a literal's.
    pub(super) fn block_size(
        &self,
        layout: &ArrayLayout<'ctx>,
        len: IntValue<'ctx>,
    ) -> IntValue<'ctx> {
        let i64_type = self.context.i64_type();
        let elements = self
            .builder
            .build_int_nuw_mul(len, i64_type.const_int(layout.size, false), "elements")
            .expect(POSITIONED);
        self.builder
            .build_int_nuw_add(elements, i64_type.const_int(layout.offset, false), "size")
            .expect(POSITIONED)
    }

    /// The address of the header's length.
    pub(super) fn len_ptr(&self, array: PointerValue<'ctx>) -> PointerValue<'ctx> {
        self.builder
            .build_struct_gep(self.header(), array, 1, "len")
            .expect("the array header should have the length at index 1")
    }

    pub(super) fn string_layout(&self) -> StringLayout<'ctx> {
        let i64_type = self.context.i64_type().into();
        let bytes = self.context.i8_type().array_type(0).into();
        let header = self
            .context
            .struct_type(&[i64_type, i64_type, i64_type, bytes], false);
        StringLayout {
            header,
            size: self.target_data.get_abi_size(&header),
            align: self.target_data.get_abi_alignment(&header),
        }
    }

    /// The address of header field `index` of `string`.
    pub(super) fn string_field(
        &self,
        string: PointerValue<'ctx>,
        index: u32,
    ) -> PointerValue<'ctx> {
        self.builder
            .build_struct_gep(self.string_layout().header, string, index, "field")
            .expect("the string header should have the length, capacity and bytes at 1, 2 and 3")
    }
}
