//! How a Sisu value sits in memory: the LLVM type of each Sisu type, the layouts of classes,
//! array blocks and string blocks, the count word (`load_count`, `store_count`, `init_count`),
//! the optional accessors (`empty`, `has_value`, `optional`, `optional_value`) and the niches
//! they read (`niche`), the size of every heap block (`heap_size`), and the TBAA tags that
//! describe them.

use std::cmp::Reverse;
use std::num::NonZeroU32;

use inkwell::context::Context;
use inkwell::types::{BasicTypeEnum, StructType};
use inkwell::values::{BasicValue, BasicValueEnum, IntValue, MetadataValue, PointerValue};
use inkwell::{AddressSpace, IntPredicate};

use super::enums::tag_bits;
use super::{Codegen, POSITIONED};
use crate::source::Span;
use crate::tir::{Class, ClassId, EnumId, Program, Type, TypeKind};

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
    size: u64,
    /// The header's ABI alignment, which is the block's.
    pub(super) align: u32,
}

/// The string header's fields, by index.
pub(super) const LEN: u32 = 1;
pub(super) const CAP: u32 = 2;
pub(super) const BYTES: u32 = 3;

/// A heap block, named by what its size depends on. `heap_size` gives its size and alignment.
pub(super) enum HeapBlock<'a, 'ctx> {
    /// An object of the class.
    Class(ClassId),
    /// An array block of `len` elements.
    Array {
        layout: &'a ArrayLayout<'ctx>,
        len: IntValue<'ctx>,
    },
    /// A string block of capacity `cap`.
    String { cap: IntValue<'ctx> },
    /// A box of the enum.
    Box(EnumId),
    /// The pending pairs of an `eq_component` helper, with room for `cap` of them.
    Pending { cap: IntValue<'ctx> },
}

/// Where an optional keeps its `None`: a bit pattern that its payload never holds, in the
/// payload's own storage. An optional with no niche is a `{ flag, value }` pair.
#[derive(Clone, Copy)]
pub(super) struct Niche {
    pub(super) storage: Storage,
    /// The bit pattern of the empty value.
    pub(super) empty: u64,
}

/// The storage that holds a niche.
#[derive(Clone, Copy)]
pub(super) enum Storage {
    /// The pointer to a block, `null` when empty. It has no other spare value.
    Pointer,
    /// An integer of this many bits: an `i32` for a `char`, an `i8` for a `bool`.
    Int(u32),
    /// The layout of the enum, with the empty value in its tag.
    Tag(EnumId),
}

/// The niche of an optional of a value whose own niche is `niche`: the next spare value, when it
/// fits the storage. A pointer has no spare value past `null`.
pub(super) fn next_niche(program: &Program, niche: Niche) -> Option<Niche> {
    let Niche { storage, empty } = niche;
    let empty = empty + 1;
    let fits = match storage {
        Storage::Pointer => false,
        Storage::Int(bits) => empty < 1 << bits,
        Storage::Tag(id) => empty < 1 << tag_bits(program.enums[id.0].variants.len()),
    };
    fits.then_some(Niche { storage, empty })
}

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
            TypeKind::String | TypeKind::Class(..) | TypeKind::Array(_) => {
                Some(self.context.ptr_type(AddressSpace::default()).into())
            }
            TypeKind::Enum(id, _) => Some(self.enum_layout(id).ty),
            TypeKind::Param(..) => {
                panic!("mono should replace every type parameter before codegen")
            }
            // An optional with a niche is its payload's storage, and any other is `{ i1, T }`:
            // whether it holds a value, then the value.
            TypeKind::Optional(payload) => match self.niche(payload) {
                Some(niche) => Some(self.niche_type(niche.storage)),
                None => Some(self.optional_pair(payload).into()),
            },
        }
    }

    /// The niche of an optional of `payload`, if its storage has a spare value: `null` for a
    /// string, a class or an array, 0x110000 for a `char`, 2 for a `bool` in an `i8`, and the
    /// enum's own niche (`EnumLayout::niche`) for an enum. An optional with a niche gives the
    /// next value of its niche (`next_niche`).
    pub(super) fn niche(&self, payload: Type) -> Option<Niche> {
        let (storage, empty) = match payload.kind() {
            TypeKind::String | TypeKind::Class(..) | TypeKind::Array(_) => (Storage::Pointer, 0),
            TypeKind::Char => (Storage::Int(32), 0x11_0000),
            TypeKind::Bool => (Storage::Int(8), 2),
            TypeKind::Enum(id, _) => return self.enum_layout(id).niche,
            TypeKind::Optional(inner) => return next_niche(self.program, self.niche(inner)?),
            _ => return None,
        };
        Some(Niche { storage, empty })
    }

    /// The niche of the optional `ty`, if it has one.
    fn optional_niche(&self, ty: Type) -> Option<Niche> {
        self.niche(Self::payload_of(ty))
    }

    /// The LLVM type of `storage`.
    pub(super) fn niche_type(&self, storage: Storage) -> BasicTypeEnum<'ctx> {
        match storage {
            Storage::Pointer => self.context.ptr_type(AddressSpace::default()).into(),
            Storage::Int(bits) => NonZeroU32::new(bits)
                .and_then(|bits| self.context.custom_width_int_type(bits).ok())
                .expect("a niche should be an `i8` or an `i32`, which LLVM has")
                .into(),
            Storage::Tag(id) => self.enum_layout(id).ty,
        }
    }

    /// The `{ i1, T }` type of an optional of `payload` with no niche.
    fn optional_pair(&self, payload: Type) -> StructType<'ctx> {
        let value = self
            .llvm_type(payload)
            .expect("the checker should reject an optional of `unit` or `never`, so its payload has a value type");
        let flag = self.context.bool_type().into();
        self.context.struct_type(&[flag, value], false)
    }

    /// The payload type of the optional `ty`.
    fn payload_of(ty: Type) -> Type {
        let TypeKind::Optional(payload) = ty.kind() else {
            unreachable!("only an optional should be read as an optional")
        };
        payload
    }

    /// The `None` of the optional `ty`: its niche's empty value, or `{ false, 0 }`.
    pub(super) fn empty(&self, ty: Type) -> BasicValueEnum<'ctx> {
        match self.optional_niche(ty) {
            Some(niche) => self.empty_value(niche),
            None => self.optional_pair(Self::payload_of(ty)).const_zero().into(),
        }
    }

    /// The empty value of `niche` in its storage. The empty value of an enum's tag has zero
    /// storage after it.
    pub(super) fn empty_value(&self, Niche { storage, empty }: Niche) -> BasicValueEnum<'ctx> {
        match storage {
            Storage::Pointer => self.niche_type(storage).const_zero(),
            Storage::Int(_) => self
                .niche_type(storage)
                .into_int_type()
                .const_int(empty, false)
                .into(),
            Storage::Tag(id) => {
                let layout = self.enum_layout(id);
                let ty = layout.ty.into_struct_type();
                let fields: Vec<BasicValueEnum> =
                    std::iter::once(layout.tag.const_int(empty, false).into())
                        .chain(
                            ty.get_field_types()
                                .into_iter()
                                .skip(1)
                                .map(BasicTypeEnum::const_zero),
                        )
                        .collect();
                ty.const_named_struct(&fields).into()
            }
        }
    }

    /// Whether `value`, of the optional `ty`, holds a value: a pointer that is not `null`, bits
    /// that are not the niche's empty value, or a pair's flag.
    pub(super) fn has_value(
        &self,
        ty: Type,
        value: BasicValueEnum<'ctx>,
        name: &str,
    ) -> IntValue<'ctx> {
        let Some(niche) = self.optional_niche(ty) else {
            return self
                .builder
                .build_extract_value(value.into_struct_value(), 0, name)
                .expect("an optional with no niche should be a `{ flag, value }` pair")
                .into_int_value();
        };
        self.holds(niche, value, name)
    }

    /// Whether the storage of `niche` at `place` holds a value. An enum's tag is loaded alone:
    /// a load of the whole enum copies all its storage.
    pub(super) fn holds_at(
        &self,
        niche: Niche,
        place: PointerValue<'ctx>,
        name: &str,
    ) -> IntValue<'ctx> {
        if let Storage::Tag(id) = niche.storage {
            let tag = self.load_tag(id, place);
            let empty = tag.get_type().const_int(niche.empty, false);
            return self.int_compare(IntPredicate::NE, tag, empty);
        }
        let value = self
            .builder
            .build_load(self.niche_type(niche.storage), place, "niche")
            .expect(POSITIONED);
        self.holds(niche, value, name)
    }

    /// Whether `value`, in the storage of `niche`, is not the niche's empty value.
    fn holds(
        &self,
        Niche { storage, empty }: Niche,
        value: BasicValueEnum<'ctx>,
        name: &str,
    ) -> IntValue<'ctx> {
        let bits = match storage {
            Storage::Pointer => {
                return self
                    .builder
                    .build_is_not_null(value.into_pointer_value(), name)
                    .expect(POSITIONED);
            }
            Storage::Int(_) => value.into_int_value(),
            Storage::Tag(_) => self
                .builder
                .build_extract_value(value.into_struct_value(), 0, "tag")
                .expect("an enum's struct should have the tag at element 0")
                .into_int_value(),
        };
        let empty = bits.get_type().const_int(empty, false);
        self.int_compare(IntPredicate::NE, bits, empty)
    }

    /// The optional `ty` that holds `value` when `flag` is set. With a niche, `value` keeps
    /// its bits, a `bool` widened to `i8`, and a clear `flag` selects the empty value.
    pub(super) fn optional(
        &self,
        ty: Type,
        flag: IntValue<'ctx>,
        value: BasicValueEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let payload = Self::payload_of(ty);
        if self.optional_niche(ty).is_none() {
            let pair = self.optional_pair(payload).const_zero();
            let pair = self
                .builder
                .build_insert_value(pair, flag, 0, "some")
                .expect("an optional with no niche should be a `{ flag, value }` pair");
            let pair = self
                .builder
                .build_insert_value(pair, value, 1, "wrap")
                .expect("an optional with no niche should be a `{ flag, value }` pair");
            return pair.as_basic_value_enum();
        }
        let value = self.widen(payload, value, "wrap");
        if flag.get_zero_extended_constant() == Some(1) {
            return value;
        }
        self.builder
            .build_select(flag, value, self.empty(ty), "wrap")
            .expect(POSITIONED)
    }

    /// The value that `value`, of the optional `ty`, holds; meaningful only when `has_value`.
    /// With a niche it is `value`'s bits, a `bool` narrowed back from `i8`.
    pub(super) fn optional_value(
        &self,
        ty: Type,
        value: BasicValueEnum<'ctx>,
        name: &str,
    ) -> BasicValueEnum<'ctx> {
        if self.optional_niche(ty).is_none() {
            return self
                .builder
                .build_extract_value(value.into_struct_value(), 1, name)
                .expect("an optional with no niche should be a `{ flag, value }` pair");
        }
        self.narrow(Self::payload_of(ty), value, name)
    }

    /// `value`, of type `payload`, in the storage of its niche: a `bool` widened to `i8`.
    pub(super) fn widen(
        &self,
        payload: Type,
        value: BasicValueEnum<'ctx>,
        name: &str,
    ) -> BasicValueEnum<'ctx> {
        if payload != Type::BOOL {
            return value;
        }
        self.builder
            .build_int_z_extend(value.into_int_value(), self.context.i8_type(), name)
            .expect(POSITIONED)
            .into()
    }

    /// The value of type `payload` that `stored`, in the storage of its niche, holds: a `bool`
    /// narrowed back from `i8`.
    pub(super) fn narrow(
        &self,
        payload: Type,
        stored: BasicValueEnum<'ctx>,
        name: &str,
    ) -> BasicValueEnum<'ctx> {
        if payload != Type::BOOL {
            return stored;
        }
        self.builder
            .build_int_truncate(stored.into_int_value(), self.context.bool_type(), name)
            .expect(POSITIONED)
            .into()
    }

    /// The address of the value in the optional `ty` at `place`: `place` itself with a niche,
    /// else element 1 of the pair. Meaningful only when it has a value. A `bool?` keeps its
    /// `bool` as an `i8`, so never for a `bool?`: `optional_value` reads its value.
    pub(super) fn optional_value_place(
        &self,
        ty: Type,
        place: PointerValue<'ctx>,
    ) -> PointerValue<'ctx> {
        debug_assert!(
            Self::payload_of(ty) != Type::BOOL,
            "a `bool?`'s value should be read with `optional_value`, never loaded as an `i1`"
        );
        if self.optional_niche(ty).is_some() {
            return place;
        }
        self.builder
            .build_struct_gep(self.optional_pair(Self::payload_of(ty)), place, 1, "value")
            .expect("an optional pair should have its value at element 1")
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

    /// The array header: the count, then the length.
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

    /// The size and alignment of `block`, as the `i64`s that `sisu_alloc`, `sisu_realloc` and
    /// `sisu_free` take. Every one of those calls takes them from here, so a block is freed
    /// with the layout it was allocated with; only `[v; n]`'s alloc takes its size from
    /// `checked_array_size`, the same formula with checks.
    pub(super) fn heap_size(
        &self,
        block: &HeapBlock<'_, 'ctx>,
    ) -> (IntValue<'ctx>, IntValue<'ctx>) {
        let i64_type = self.context.i64_type();
        let abi = |ty: &StructType<'ctx>| {
            (
                i64_type.const_int(self.target_data.get_abi_size(ty), false),
                i64_type.const_int(u64::from(self.target_data.get_abi_alignment(ty)), false),
            )
        };
        match *block {
            HeapBlock::Class(id) => abi(&self.classes[id.0].ty),
            HeapBlock::Box(inner) => abi(&self.enums[inner.0].boxed),
            // A length that reaches here passed the size checks, or is a literal's or an
            // existing block's.
            HeapBlock::Array { layout, len } => {
                let elements = self
                    .builder
                    .build_int_nuw_mul(len, i64_type.const_int(layout.size, false), "elements")
                    .expect(POSITIONED);
                let size = self
                    .builder
                    .build_int_nuw_add(elements, i64_type.const_int(layout.offset, false), "size")
                    .expect(POSITIONED);
                (size, i64_type.const_int(layout.align, false))
            }
            HeapBlock::String { cap } => {
                let layout = self.string_layout();
                let size = self
                    .builder
                    .build_int_nuw_add(i64_type.const_int(layout.size, false), cap, "size")
                    .expect(POSITIONED);
                (size, i64_type.const_int(u64::from(layout.align), false))
            }
            // A buffer this large holds a pair for each link of values that exist in memory.
            HeapBlock::Pending { cap } => {
                let (entry, align) = abi(&self.pending_pair());
                let size = self
                    .builder
                    .build_int_nuw_mul(cap, entry, "size")
                    .expect(POSITIONED);
                (size, align)
            }
        }
    }

    /// `{ i32 kind, ptr a, ptr b }`: a pair of values that an `eq_component` helper still has
    /// to compare, with the member index of their enum.
    pub(super) fn pending_pair(&self) -> StructType<'ctx> {
        let ptr_type = self.context.ptr_type(AddressSpace::default()).into();
        self.context
            .struct_type(&[self.context.i32_type().into(), ptr_type, ptr_type], false)
    }

    /// `heap_size`'s array formula with checks, for `[v; n]`, whose length `len` is the
    /// program's: panics at `span` when `len` is negative or the block would be too large.
    pub(super) fn checked_array_size(
        &self,
        layout: &ArrayLayout<'ctx>,
        len: IntValue<'ctx>,
        span: Span,
    ) -> (IntValue<'ctx>, IntValue<'ctx>) {
        let i64_type = self.context.i64_type();
        let (size, offset) = (
            i64_type.const_int(layout.size, false),
            i64_type.const_int(layout.offset, false),
        );
        let negative = self.int_compare(IntPredicate::SLT, len, i64_type.const_zero());
        self.panic_if(negative, span, "array length is negative");
        let (elements, wrapped) = self.overflowing("llvm.umul.with.overflow", len, size);
        let (total, carried) = self.overflowing("llvm.uadd.with.overflow", elements, offset);
        // `Layout::from_size_align` needs the size rounded up to the alignment to fit in
        // an `isize`.
        let largest = i64_type.const_int(i64::MAX.cast_unsigned() - layout.align + 1, false);
        let above = self.int_compare(IntPredicate::UGT, total, largest);
        let too_large = [wrapped, carried, above]
            .into_iter()
            .reduce(|a, b| self.builder.build_or(a, b, "too_large").expect(POSITIONED))
            .expect("the size checks should have three flags");
        self.panic_if(too_large, span, "array is too large");
        (total, i64_type.const_int(layout.align, false))
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
