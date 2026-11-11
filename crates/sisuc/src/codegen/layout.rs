//! How a Sisu value sits in memory: the LLVM type of each Sisu type, the layouts of classes,
//! array blocks and string blocks, the count word (`load_count`, `store_count`, `init_count`),
//! the optional accessors (`empty`, `has_value`, `optional`, `optional_value`) and the niches
//! they read (`niche`), the three words of an array or string value (`value_block`, `value_data`,
//! `value_len`, `make_value`, `load_words`), the size of every heap block (`heap_size`), and the
//! TBAA tags that describe them.

use std::cmp::Reverse;
use std::num::NonZeroU32;

use inkwell::context::Context;
use inkwell::types::{BasicTypeEnum, StructType};
use inkwell::values::{
    BasicValue, BasicValueEnum, InstructionValue, IntValue, MetadataValue, PointerValue,
};
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

impl ArrayLayout<'_> {
    /// The most bytes a block of this layout can have: `Layout::from_size_align` needs the size
    /// rounded up to the alignment to fit in an `isize`.
    fn byte_limit(&self) -> u64 {
        i64::MAX.cast_unsigned() - self.align + 1
    }

    /// The most elements a block of this layout can have: the header and that many elements
    /// are `byte_limit` bytes at most. A block that has this many elements cannot take another.
    pub(super) fn largest(&self) -> u64 {
        (self.byte_limit() - self.offset) / self.size
    }
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

/// The string header's fields, by index.
pub(super) const LEN: u32 = 1;
pub(super) const CAP: u32 = 2;
pub(super) const BYTES: u32 = 3;

/// A heap block, named by what its size depends on. `heap_size` gives its size and alignment.
pub(super) enum HeapBlock<'a, 'ctx> {
    /// An object of the class.
    Class(ClassId),
    /// An array block with room for `cap` elements.
    Array {
        layout: &'a ArrayLayout<'ctx>,
        cap: IntValue<'ctx>,
    },
    /// A string block of capacity `cap`.
    String { cap: IntValue<'ctx> },
    /// A box of the enum.
    Box(EnumId),
    /// The counted box that holds a value of this type in an `any` value: see `value_box`.
    Value(Type),
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
    /// The three words of an array or string value: the `block` is `null` when empty, and the
    /// other words are then zero. It has no other spare value.
    Value,
    /// An integer of this many bits: an `i32` for a `char`, an `i8` for a `bool`.
    Int(u32),
    /// The layout of the enum, with the empty value in its tag.
    Tag(EnumId),
    /// The pair of an `any` value: the data pointer is `null` when empty, and the table pointer
    /// is then `null` too. It has no other spare value.
    Pair,
}

/// Whether a value of `ty` is the three words of an array or a string: `{ block, data, len }`.
/// So is the optional of one, which keeps its `None` in a `null` block. The optional of such an
/// optional is a `{ flag, value }` pair.
pub(super) fn three_words(ty: Type) -> bool {
    let words = |ty: Type| matches!(ty.kind(), TypeKind::String | TypeKind::Array(_));
    if let TypeKind::Optional(payload) = ty.kind() {
        words(payload)
    } else {
        words(ty)
    }
}

/// The niche of an optional of a value whose own niche is `niche`: the next spare value, when it
/// fits the storage. A pointer has no spare value past `null`.
pub(super) fn next_niche(program: &Program, niche: Niche) -> Option<Niche> {
    let Niche { storage, empty } = niche;
    let empty = empty + 1;
    let fits = match storage {
        Storage::Pointer | Storage::Value | Storage::Pair => false,
        Storage::Int(bits) => empty < 1 << bits,
        Storage::Tag(id) => empty < 1 << tag_bits(program.enums[id.0].variants.len()),
    };
    fits.then_some(Niche { storage, empty })
}

/// The largest value, in bytes, that codegen keeps as an LLVM value. A larger one stays in memory
/// (`Codegen::by_memory`). Measured with an enum of `i64` fields, in a loop and in a recursion, at
/// this size both ways run at the same speed. Below it, memory runs slower: up to 3.9 times when
/// calls do not inline. Above it, an LLVM value builds slower with no gain at run time: 2.5 times
/// at 256 bytes and 11 times at 1 KiB, in a program of 300 functions. It must stay at 24 or more,
/// because the words of a string or an array (`three_words`) are read from an LLVM value. Other
/// aggregates can use this rule when they land (ADR 0046).
const LARGEST_LLVM_VALUE: u64 = 256;

impl<'ctx> Codegen<'ctx, '_> {
    /// Whether a value of the LLVM type `ty` stays in memory: the `BasicValueEnum` of such a value
    /// is a pointer to it, with its type in `memory_values`. LLVM splits a first-class aggregate
    /// into one value for each element, which costs time and memory for each byte of a large
    /// enum. Instruction selection crashes at 65 536 elements.
    pub(super) fn by_memory(&self, ty: BasicTypeEnum<'ctx>) -> bool {
        ty.is_struct_type() && self.target_data.get_abi_size(&ty) > LARGEST_LLVM_VALUE
    }

    /// The value of type `ty` that stays at `place`, which nothing writes while the value is in
    /// use.
    pub(super) fn in_memory(
        &self,
        place: PointerValue<'ctx>,
        ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        self.memory_values.borrow_mut().insert(place, ty);
        place.into()
    }

    /// The type of `value` when it stays in memory, as `in_memory` says.
    pub(super) fn memory_type(&self, value: BasicValueEnum<'ctx>) -> Option<BasicTypeEnum<'ctx>> {
        let BasicValueEnum::PointerValue(place) = value else {
            return None;
        };
        self.memory_values.borrow().get(&place).copied()
    }

    /// The value of type `ty` at `place`, which nothing writes or frees before the value's last
    /// use.
    pub(super) fn value_at(
        &self,
        ty: BasicTypeEnum<'ctx>,
        place: PointerValue<'ctx>,
        name: &str,
    ) -> BasicValueEnum<'ctx> {
        if self.by_memory(ty) {
            return self.in_memory(place, ty);
        }
        self.builder.build_load(ty, place, name).expect(POSITIONED)
    }

    /// The value of type `ty` at `place`, which can change or go after this: a load, or a copy
    /// into a scratch slot.
    pub(super) fn copied_at(
        &self,
        ty: BasicTypeEnum<'ctx>,
        place: PointerValue<'ctx>,
        name: &str,
    ) -> BasicValueEnum<'ctx> {
        if self.by_memory(ty) {
            let slot = self.entry_alloca(ty, name);
            self.copy_memory(slot, place, ty);
            return self.in_memory(slot, ty);
        }
        self.builder.build_load(ty, place, name).expect(POSITIONED)
    }

    /// Copies the value of type `ty` at `from` to `to`, which is the same place or does not
    /// overlap it. A copy to the same place emits nothing: Valgrind on ARM64 reports a `memcpy`
    /// onto itself as an overlap.
    pub(super) fn copy_memory(
        &self,
        to: PointerValue<'ctx>,
        from: PointerValue<'ctx>,
        ty: BasicTypeEnum<'ctx>,
    ) {
        if to == from {
            return;
        }
        let align = self.target_data.get_abi_alignment(&ty);
        let size = self
            .context
            .i64_type()
            .const_int(self.target_data.get_abi_size(&ty), false);
        self.builder
            .build_memcpy(to, align, from, align, size)
            .expect("the alignment of a type should be a power of two");
    }

    /// Stores the zero value of the LLVM type `ty` at `place`.
    pub(super) fn store_zero(&self, place: PointerValue<'ctx>, ty: BasicTypeEnum<'ctx>) {
        if !self.by_memory(ty) {
            self.store(place, ty.const_zero());
            return;
        }
        let i64_type = self.context.i64_type();
        self.builder
            .build_memset(
                place,
                self.target_data.get_abi_alignment(&ty),
                self.context.i8_type().const_zero(),
                i64_type.const_int(self.target_data.get_abi_size(&ty), false),
            )
            .expect("the alignment of a type should be a power of two");
    }

    /// A scratch slot of type `ty` that holds its zero value.
    fn zeroed_slot(&self, ty: BasicTypeEnum<'ctx>, name: &str) -> PointerValue<'ctx> {
        let slot = self.entry_alloca(ty, name);
        self.store_zero(slot, ty);
        slot
    }

    /// The LLVM type of a value of `ty`; `None` for `unit` and `never`, which have no value.
    pub(super) fn llvm_type(&self, ty: Type) -> Option<BasicTypeEnum<'ctx>> {
        match ty.kind() {
            TypeKind::I64 => Some(self.context.i64_type().into()),
            TypeKind::U8 => Some(self.context.i8_type().into()),
            TypeKind::F64 => Some(self.context.f64_type().into()),
            TypeKind::Bool => Some(self.context.bool_type().into()),
            TypeKind::Char => Some(self.context.i32_type().into()),
            TypeKind::Unit | TypeKind::Never => None,
            // An object is a pointer to its block.
            TypeKind::Class(..) => Some(self.context.ptr_type(AddressSpace::default()).into()),
            TypeKind::Struct(..) => panic!("a struct lands in Task 3"),
            // An array or a string is three words: its block, its data and its length.
            TypeKind::String | TypeKind::Array(_) => Some(self.value_type().into()),
            TypeKind::Enum(id, _) => Some(self.enum_layout(id).ty),
            // The pointer to the data, then the pointer to the table.
            TypeKind::Any(..) => Some(self.any_type().into()),
            TypeKind::Param(..) => {
                panic!("mono should replace every type parameter before codegen")
            }
            TypeKind::Assoc(..) => {
                panic!("mono should replace every associated type before codegen")
            }
            // An optional with a niche is its payload's storage, and any other is `{ i1, T }`:
            // whether it holds a value, then the value.
            TypeKind::Optional(payload) => {
                #[cfg(test)]
                crate::tir::LAYOUT_STEPS.set(crate::tir::LAYOUT_STEPS.get() + 1);
                // Each borrow ends before the recursive call, which borrows the cache again.
                let known = self.optional_types.borrow().get(&ty).copied();
                if known.is_some() {
                    return known;
                }
                let laid_out: BasicTypeEnum<'ctx> = match self.niche(payload) {
                    Some(niche) => self.niche_type(niche.storage),
                    None => self.optional_pair(payload).into(),
                };
                self.optional_types.borrow_mut().insert(ty, laid_out);
                Some(laid_out)
            }
        }
    }

    /// The niche of an optional of `payload`, if its storage has a spare value. It is `null`
    /// for a class, and a `null` block for a string or an array, 0x110000 for a `char` and 2 for a
    /// `bool` in an `i8`.
    /// For an enum, it is the enum's own niche (`EnumLayout::niche`). An optional with a niche
    /// gives the next value of its niche (`next_niche`).
    ///
    /// The answer for each type is kept: an optional of `n` layers asks for the niche of each
    /// layer below it, so a walk with no memory reads the layers quadratically.
    pub(super) fn niche(&self, payload: Type) -> Option<Niche> {
        #[cfg(test)]
        crate::tir::LAYOUT_STEPS.set(crate::tir::LAYOUT_STEPS.get() + 1);
        // Each borrow ends before the recursive call, which borrows the cache again.
        let known = self.niches.borrow().get(&payload).copied();
        if let Some(niche) = known {
            return niche;
        }
        let niche = self.find_niche(payload);
        self.niches.borrow_mut().insert(payload, niche);
        niche
    }

    /// `niche`, with no memory.
    fn find_niche(&self, payload: Type) -> Option<Niche> {
        let (storage, empty) = match payload.kind() {
            TypeKind::Class(..) => (Storage::Pointer, 0),
            TypeKind::String | TypeKind::Array(_) => (Storage::Value, 0),
            TypeKind::Any(..) => (Storage::Pair, 0),
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
            Storage::Value => self.value_type().into(),
            Storage::Int(bits) => NonZeroU32::new(bits)
                .and_then(|bits| self.context.custom_width_int_type(bits).ok())
                .expect("a niche should be an `i8` or an `i32`, which LLVM has")
                .into(),
            Storage::Tag(id) => self.enum_layout(id).ty,
            Storage::Pair => self.any_type().into(),
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
        if let Some(niche) = self.optional_niche(ty) {
            return self.empty_value(niche);
        }
        let pair = self.optional_pair(Self::payload_of(ty)).into();
        if self.by_memory(pair) {
            self.in_memory(self.zeroed_slot(pair, "none"), pair)
        } else {
            pair.const_zero()
        }
    }

    /// The empty value of `niche` in its storage. The empty value of an enum's tag has zero
    /// storage after it. For an enum with no field, it is the tag.
    pub(super) fn empty_value(&self, Niche { storage, empty }: Niche) -> BasicValueEnum<'ctx> {
        match storage {
            Storage::Pointer | Storage::Value | Storage::Pair => {
                self.niche_type(storage).const_zero()
            }
            Storage::Int(_) => self
                .niche_type(storage)
                .into_int_type()
                .const_int(empty, false)
                .into(),
            Storage::Tag(id) => {
                let layout = self.enum_layout(id);
                if layout.is_tag() {
                    return layout.tag.const_int(empty, false).into();
                }
                if self.by_memory(layout.ty) {
                    let slot = self.zeroed_slot(layout.ty, "empty");
                    self.store(
                        self.tag_ptr(id, slot),
                        layout.tag.const_int(empty, false).into(),
                    );
                    return self.in_memory(slot, layout.ty);
                }
                let ty = layout.ty.into_struct_type();
                let fields: Vec<BasicValueEnum<'_>> =
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
        let niche = self.optional_niche(ty);
        if self.memory_type(value).is_some() {
            let place = value.into_pointer_value();
            return match niche {
                Some(niche) => self.holds_at(niche, place, name),
                None => self
                    .builder
                    .build_load(
                        self.context.bool_type(),
                        self.pair_field(ty, place, 0),
                        name,
                    )
                    .expect(POSITIONED)
                    .into_int_value(),
            };
        }
        let Some(niche) = niche else {
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
        // The `block` is the first word of an array or string value, and it alone says whether
        // the value is there.
        if let Storage::Value = niche.storage {
            return self
                .builder
                .build_is_not_null(self.load_pointer(place), name)
                .expect(POSITIONED);
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
            Storage::Value => {
                return self
                    .builder
                    .build_is_not_null(self.value_block(value), name)
                    .expect(POSITIONED);
            }
            Storage::Pair => {
                let data = self
                    .builder
                    .build_extract_value(value.into_struct_value(), 0, "data")
                    .expect("an `any` value should be a pair of pointers")
                    .into_pointer_value();
                return self
                    .builder
                    .build_is_not_null(data, name)
                    .expect(POSITIONED);
            }
            Storage::Int(_) => value.into_int_value(),
            Storage::Tag(id) if self.enum_layout(id).is_tag() => value.into_int_value(),
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
            let pair = self.optional_pair(payload);
            if self.by_memory(pair.into()) {
                let slot = self.entry_alloca(pair.into(), "some");
                self.store(self.pair_field(ty, slot, 0), flag.into());
                self.store(self.pair_field(ty, slot, 1), value);
                return self.in_memory(slot, pair.into());
            }
            let pair = pair.const_zero();
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
        let chosen = self
            .builder
            .build_select(flag, value, self.empty(ty), "wrap")
            .expect(POSITIONED);
        match self.memory_type(value) {
            Some(stored) => self.in_memory(chosen.into_pointer_value(), stored),
            None => chosen,
        }
    }

    /// The address of element `index` of the `{ flag, value }` pair of the optional `ty` at
    /// `place`.
    fn pair_field(&self, ty: Type, place: PointerValue<'ctx>, index: u32) -> PointerValue<'ctx> {
        self.builder
            .build_struct_gep(
                self.optional_pair(Self::payload_of(ty)),
                place,
                index,
                "pair",
            )
            .expect("an optional with no niche should be a `{ flag, value }` pair")
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
            if self.memory_type(value).is_some() {
                let payload = self
                    .llvm_type(Self::payload_of(ty))
                    .expect("the payload of an optional should have a value type");
                let place = self.pair_field(ty, value.into_pointer_value(), 1);
                return self.value_at(payload, place, name);
            }
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
        let types: Vec<StructType<'_>> = self
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
        let fields: Vec<BasicTypeEnum<'_>> = class
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
        if self.memory_type(value).is_some() {
            self.store(ptr, value);
            return;
        }
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

    /// The array header, as the string header is: the count, the length, then the capacity.
    fn header(&self) -> StructType<'ctx> {
        let i64_type = self.context.i64_type().into();
        self.context
            .struct_type(&[i64_type, i64_type, i64_type], false)
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
            HeapBlock::Value(ty) => abi(&self.value_box(ty)),
            // A capacity that reaches here passed the size checks, or is a literal's or an
            // existing block's.
            HeapBlock::Array { layout, cap } => {
                let elements = self
                    .builder
                    .build_int_nuw_mul(cap, i64_type.const_int(layout.size, false), "elements")
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
    /// program's and is also the capacity of the new block: panics at `span` when `len` is
    /// negative or the block would be too large.
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
        let largest = i64_type.const_int(layout.byte_limit(), false);
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

    /// The address of the header's capacity.
    pub(super) fn cap_ptr(&self, array: PointerValue<'ctx>) -> PointerValue<'ctx> {
        self.builder
            .build_struct_gep(self.header(), array, 2, "cap")
            .expect("the array header should have the capacity at index 2")
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

    /// The type of an array or a string value: the pointer to its block, the pointer to its first
    /// element or byte, and its length. A value that covers its whole block has `data` at the
    /// first element and `len` equal to the header's `len`.
    pub(super) fn value_type(&self) -> StructType<'ctx> {
        let ptr = self.context.ptr_type(AddressSpace::default()).into();
        self.context
            .struct_type(&[ptr, ptr, self.context.i64_type().into()], false)
    }

    /// The part `index` of the array or string value `value`.
    fn value_part(
        &self,
        value: BasicValueEnum<'ctx>,
        index: u32,
        name: &str,
    ) -> BasicValueEnum<'ctx> {
        self.builder
            .build_extract_value(value.into_struct_value(), index, name)
            .expect("an array or string value should have three words")
    }

    /// The block of the array or string value `value`.
    pub(super) fn value_block(&self, value: BasicValueEnum<'ctx>) -> PointerValue<'ctx> {
        self.value_part(value, 0, "block").into_pointer_value()
    }

    /// The first element or byte of the array or string value `value`.
    pub(super) fn value_data(&self, value: BasicValueEnum<'ctx>) -> PointerValue<'ctx> {
        self.value_part(value, 1, "data").into_pointer_value()
    }

    /// The length of the array or string value `value`.
    pub(super) fn value_len(&self, value: BasicValueEnum<'ctx>) -> IntValue<'ctx> {
        self.value_part(value, 2, "len").into_int_value()
    }

    /// The array or string value with the words `block`, `data` and `len`.
    pub(super) fn make_value(
        &self,
        block: PointerValue<'ctx>,
        data: PointerValue<'ctx>,
        len: IntValue<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let words: [BasicValueEnum<'ctx>; 3] = [block.into(), data.into(), len.into()];
        let mut value = self.value_type().get_undef().as_basic_value_enum();
        for (index, word) in (0..).zip(words) {
            value = self
                .builder
                .build_insert_value(value.into_struct_value(), word, index, "value")
                .expect("an array or string value should have three words")
                .as_basic_value_enum();
        }
        value
    }

    /// The object that `value`, of the counted type `ty`, is: the pointer itself for a class, and
    /// the block of an array or a string. `null` for the `None` of an optional.
    pub(super) fn counted_pointer(
        &self,
        ty: Type,
        value: BasicValueEnum<'ctx>,
    ) -> PointerValue<'ctx> {
        if three_words(ty) {
            self.value_block(value)
        } else {
            value.into_pointer_value()
        }
    }

    /// The address of word `index` of the array or string value at `place`.
    pub(super) fn words_ptr(&self, place: PointerValue<'ctx>, index: u32) -> PointerValue<'ctx> {
        self.builder
            .build_struct_gep(self.value_type(), place, index, "word")
            .expect("an array or string value should have three words")
    }

    /// Marks `len`, a load of a length, `!range [0, 2^63)` and `!noundef`: a length is never
    /// negative (see `header_len`).
    pub(super) fn mark_len(&self, len: IntValue<'ctx>) {
        let i64_type = self.context.i64_type();
        let load: InstructionValue<'ctx> = len
            .as_instruction_value()
            .expect("a load should be an instruction");
        // The upper bound is exclusive and wraps, so `1 << 63` is 2^63.
        let range = self.context.metadata_node(&[
            i64_type.const_zero().into(),
            i64_type.const_int(1 << 63, false).into(),
        ]);
        load.set_metadata(range, self.context.get_kind_id("range"))
            .expect("a range should be a metadata node");
        load.set_metadata(
            self.context.metadata_node(&[]),
            self.context.get_kind_id("noundef"),
        )
        .expect("an empty node should be a metadata node");
    }

    /// Word `index` of the array or string value at `place`, named `name` and loaded with the
    /// TBAA tag `tag` when given. The length word, 2, is marked as `mark_len` does: a length that
    /// is loaded from anywhere carries the facts of a length.
    pub(super) fn load_word(
        &self,
        place: PointerValue<'ctx>,
        index: u32,
        name: &str,
        tag: Option<MetadataValue<'ctx>>,
    ) -> BasicValueEnum<'ctx> {
        let ty: BasicTypeEnum<'ctx> = if index == 2 {
            self.context.i64_type().into()
        } else {
            self.context.ptr_type(AddressSpace::default()).into()
        };
        let at = self.words_ptr(place, index);
        let word = match tag {
            Some(tag) => self.load_tagged(ty, at, name, tag),
            None => self.builder.build_load(ty, at, name).expect(POSITIONED),
        };
        if index == 2 {
            self.mark_len(word.into_int_value());
        }
        word
    }

    /// The array or string value at `place`, as three loads, so that its length carries `!range`
    /// and `!noundef`. Each load has the TBAA tag `tag`, when `place` has one.
    pub(super) fn load_words(
        &self,
        place: PointerValue<'ctx>,
        tag: Option<MetadataValue<'ctx>>,
    ) -> BasicValueEnum<'ctx> {
        let block = self.load_word(place, 0, "block", tag).into_pointer_value();
        let data = self.load_word(place, 1, "data", tag).into_pointer_value();
        let len = self.load_word(place, 2, "len", tag).into_int_value();
        self.make_value(block, data, len)
    }

    /// The value of type `ty` at `place`, with the TBAA tag `tag` when given. An array or a string
    /// is loaded by `load_words`.
    pub(super) fn load_place(
        &self,
        ty: Type,
        place: PointerValue<'ctx>,
        name: &str,
        tag: Option<MetadataValue<'ctx>>,
    ) -> BasicValueEnum<'ctx> {
        if three_words(ty) {
            return self.load_words(place, tag);
        }
        let llvm_ty = self
            .llvm_type(ty)
            .expect("the checker should give every field a value type, never `unit` or `never`");
        if self.by_memory(llvm_ty) {
            return self.copied_at(llvm_ty, place, name);
        }
        match tag {
            Some(tag) => self.load_tagged(llvm_ty, place, name, tag),
            None => self
                .builder
                .build_load(llvm_ty, place, name)
                .expect(POSITIONED),
        }
    }
}
