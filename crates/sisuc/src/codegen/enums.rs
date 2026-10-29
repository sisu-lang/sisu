//! Enums at run time. An enum value is a tag and its variant's payload, held inline:
//! `%sisu.<Name> = { tag, [n x iA] }`, with one struct per variant, `%sisu.<Name>.<Variant>`,
//! laid over the payload storage. A field that closes a cycle holds a box instead,
//! `%sisu.box.<F> = { i64 count, %sisu.<F> }`. A boxed field of type `F` with k ≥ 1 `?` layers
//! is k − 1 flags around a box pointer that is `null` for the innermost `None`: `{ i1, ptr }`
//! for an `F??`. When a flag is clear, the storage inside it is zero. An `Option`-shaped enum
//! has one variant with no payload and one with one field whose storage has a niche. Such an
//! enum is that field's storage, with the payload-free variant in the niche's empty value
//! (`Shape`): `enum Chain { End  Next(Chain) }` is the pointer to its box, `null` for `End`.
//! This module owns the layouts, building a variant, and the bodies of the enum helpers, which
//! `define_helpers` reaches through `define_enum_helper`.
//!
//! Boxes free from pending lists, not by recursion. Releasing a value of an enum `F` whose
//! component has boxes keeps one list head per member of the component on its own stack.
//! `walk.<F>` releases the value's counted parts; a box whose count reaches zero is pushed onto
//! its enum's list, linked through its count word, which is dead by then. `drain` then pops
//! boxes until every list is empty: it walks each box's value, which may push more, and frees
//! the box. A field held inline whose enum is in the same component, such as `Two<Expr>.Both`'s
//! `Expr` in `enum Expr { Pair(Two<Expr>) }`, is walked in place with the same lists, so its
//! depth is bounded by the inline nesting inside one box. So the stack stays the same for any
//! depth and shape of value.

use std::num::NonZeroU32;
use std::rc::Rc;

use inkwell::AddressSpace;
use inkwell::IntPredicate;
use inkwell::basic_block::BasicBlock;
use inkwell::types::{ArrayType, BasicTypeEnum, IntType, StructType};
use inkwell::values::{
    BasicMetadataValueEnum, BasicValueEnum, FunctionValue, IntValue, PhiValue, PointerValue,
};

use super::layout::{HeapBlock, Niche, Storage, next_niche};
use super::rc::{Helper, param};
use super::{Codegen, POSITIONED, enum_of};
use crate::mir::{Place, Projection};
use crate::tir::{Enum, EnumId, LocalId, Payload, Type, TypeArgs, TypeKind};
use rustc_hash::FxHashMap;

/// An enum's LLVM types.
pub(super) struct EnumLayout<'ctx> {
    /// `{ tag, [n x iA] }`, or `{ tag }` when no variant has a field. `iA` is the integer type
    /// as wide as the largest payload ABI alignment `a`, and `n` the largest payload ABI size
    /// rounded up to whole `a`s, so the storage fits every payload at its alignment. An
    /// `Option`-shaped enum is the storage of its field's niche instead (`shape`).
    pub(super) ty: BasicTypeEnum<'ctx>,
    /// `i8`, `i16` or `i32`, the smallest that holds the variant count. Tag `v` is variant `v`.
    /// An `Option`-shaped enum stores no tag: `load_tag` gives it from the niche.
    pub(super) tag: IntType<'ctx>,
    /// Indexed by variant: `{ fields... }`, a field's LLVM type being its type's, or `ptr` when
    /// boxed. Opaque for an `Option`-shaped enum, which lays no payload over a storage.
    pub(super) payloads: Vec<StructType<'ctx>>,
    /// `%sisu.box.<Name> = { i64 count, %sisu.<Name> }`, a box that holds a value of this enum.
    pub(super) boxed: StructType<'ctx>,
    /// For an `Option`-shaped enum, where it keeps its payload-free variant.
    pub(super) shape: Option<Shape>,
    /// The niche of an optional of this enum: the tag after the last variant. For an
    /// `Option`-shaped enum, it is the next value of its field's niche. `None` when that does
    /// not fit.
    pub(super) niche: Option<Niche>,
}

/// The layout of an `Option`-shaped enum: two variants, one with no payload and one with one
/// field whose storage has a niche. A value is that field's storage, and the payload-free
/// variant is the niche's empty value, in either order of the variants.
#[derive(Clone, Copy)]
pub(super) struct Shape {
    /// The index of the variant with no payload.
    pub(super) empty: usize,
    /// The niche of the other variant's field: a box pointer's `null` when the field is boxed.
    pub(super) field: Niche,
}

impl Shape {
    /// The index of the variant with the field.
    fn payload(self) -> usize {
        1 - self.empty
    }
}

/// The width in bits of the tag of an enum of `count` variants: 8, 16 or 32, the smallest
/// that holds the variant count. The tags past the last variant are spare, and an optional of
/// the enum keeps its `None` in the first of them (`layout::niche`).
pub(super) fn tag_bits(count: usize) -> u32 {
    if count <= 1 << 8 {
        8
    } else if count <= 1 << 16 {
        16
    } else {
        32
    }
}

/// The enum a boxed field holds: its type is `F` or `F?`.
pub(super) fn boxed_enum(ty: Type) -> EnumId {
    ty.inline_enum()
        .expect("the checker should box only a field of an enum type or its optional")
}

/// The payload of `ty` when it is an optional of an optional: a boxed field of that type keeps a
/// flag around the storage of its payload.
pub(super) fn nested(ty: Type) -> Option<Type> {
    match ty.kind() {
        TypeKind::Optional(payload) if matches!(payload.kind(), TypeKind::Optional(_)) => {
            Some(payload)
        }
        _ => None,
    }
}

/// An enum's component with boxes: its members in declaration order, and the index of the enum
/// among them.
pub(super) struct Component {
    pub(super) members: Rc<[EnumId]>,
    pub(super) index: usize,
}

/// The component of each enum of `enums`, when it has boxes, built in one pass over `enums`.
pub(super) fn components(enums: &[Enum]) -> Vec<Option<Component>> {
    #[cfg(test)]
    super::MEMBER_SCANS.set(super::MEMBER_SCANS.get() + enums.len());
    let mut by_cycle: FxHashMap<usize, Vec<EnumId>> = FxHashMap::default();
    let slots: Vec<Option<(usize, usize)>> = enums
        .iter()
        .enumerate()
        .map(|(i, e)| {
            e.cycle.map(|cycle| {
                let members = by_cycle.entry(cycle).or_default();
                members.push(EnumId(i));
                (cycle, members.len() - 1)
            })
        })
        .collect();
    let shared: FxHashMap<usize, Rc<[EnumId]>> = by_cycle
        .into_iter()
        .map(|(cycle, members)| (cycle, members.into()))
        .collect();
    slots
        .into_iter()
        .map(|slot| {
            slot.map(|(cycle, index)| Component {
                members: Rc::clone(&shared[&cycle]),
                index,
            })
        })
        .collect()
}

/// The enums `e` holds inline: each unboxed payload field of an enum type or its optional.
fn held_inline(e: &Enum) -> impl Iterator<Item = EnumId> + '_ {
    e.variants
        .iter()
        .flat_map(|variant| &variant.fields)
        .filter(|field| !field.boxed)
        .filter_map(|field| field.ty.inline_enum())
}

impl<'ctx> Codegen<'ctx, '_> {
    pub(super) fn enum_layout(&self, id: EnumId) -> &EnumLayout<'ctx> {
        &self.enums[id.0]
    }

    /// The index of `member` among `members`, the members of one component, read from the table
    /// that `components` builds.
    ///
    /// It checks in constant time that `member` is in that component. The `unsafe` index of
    /// `head` relies on this check.
    fn member_position(&self, members: &Rc<[EnumId]>, member: EnumId) -> usize {
        #[cfg(test)]
        super::MEMBER_SCANS.set(super::MEMBER_SCANS.get() + 1);
        let component = self.components[member.0]
            .as_ref()
            .expect("an enum that a component links to should be in a component with boxes");
        assert!(
            Rc::ptr_eq(&component.members, members),
            "the checker should box only a field whose enum is in the declaring enum's component"
        );
        component.index
    }

    /// Builds every enum's layout. First it names each enum's type and its payload structs, so
    /// that a payload can name any enum. Then it gives each enum its shape and niche, its
    /// payloads' bodies and its body. These need the layouts of the enums it holds inline. Thus
    /// an enum held inline is laid out before its holder, in post-order over the inline fields,
    /// with an explicit stack. `tir::layout_enums` boxes a field on every cycle, so the inline
    /// fields form no cycle and the walk ends.
    pub(super) fn lay_out_enums(&mut self) {
        let program = self.program;
        let context = self.context;
        self.enums = program
            .enums
            .iter()
            .map(|e| {
                let tag = NonZeroU32::new(tag_bits(e.variants.len()))
                    .and_then(|bits| context.custom_width_int_type(bits).ok())
                    .expect("a tag should be an `i8`, an `i16` or an `i32`, which LLVM has");
                EnumLayout {
                    ty: context
                        .opaque_struct_type(&format!("sisu.{}", e.name))
                        .into(),
                    tag,
                    payloads: e
                        .variants
                        .iter()
                        .map(|v| context.opaque_struct_type(&format!("sisu.{}.{}", e.name, v.name)))
                        .collect(),
                    boxed: context.opaque_struct_type(&format!("sisu.box.{}", e.name)),
                    shape: None,
                    niche: None,
                }
            })
            .collect();
        let ptr_type = context.ptr_type(AddressSpace::default());
        let i64_type = context.i64_type();
        // A box on a pending list holds the next box's address in the word `load_count` reads as
        // its count.
        assert!(
            self.target_data.get_abi_size(&ptr_type) <= self.target_data.get_abi_size(&i64_type),
            "a pointer should fit the 8-byte count word that links a box into a pending list"
        );
        let mut done = vec![false; program.enums.len()];
        for root in 0..program.enums.len() {
            // `(id, true)` once the enums `id` holds inline are queued before it.
            let mut stack = vec![(root, false)];
            while let Some((id, held_queued)) = stack.pop() {
                if done[id] {
                    continue;
                }
                if held_queued {
                    self.lay_out_enum(id);
                    done[id] = true;
                    continue;
                }
                stack.push((id, true));
                stack.extend(held_inline(&program.enums[id]).map(|held| (held.0, false)));
            }
        }
        for layout in &self.enums {
            layout.boxed.set_body(&[i64_type.into(), layout.ty], false);
        }
    }

    /// Lays out enum `id`, whose inline-held enums are laid out. An `Option`-shaped enum gets
    /// its shape and type; another enum gets its payloads' bodies and its body. Then each gets
    /// its niche.
    fn lay_out_enum(&mut self, id: usize) {
        let program = self.program;
        let e = &program.enums[id];
        let shape = self.option_shape(e);
        if let Some(shape) = shape {
            self.enums[id].ty = self.niche_type(shape.field.storage);
        } else {
            for (variant, &payload) in e.variants.iter().zip(&self.enums[id].payloads) {
                let fields: Vec<_> = variant
                    .fields
                    .iter()
                    .map(|field| {
                        if field.boxed {
                            self.box_storage(field.ty)
                        } else {
                            self.llvm_type(field.ty).expect(
                                "the checker should give every payload field a value type, never `unit` or `never`",
                            )
                        }
                    })
                    .collect();
                payload.set_body(&fields, false);
            }
            self.set_enum_body(id);
        }
        // A tagged enum's last tag is its last variant's: the tag after it is the next value.
        let last = Niche {
            storage: Storage::Tag(EnumId(id)),
            empty: u64::try_from(e.variants.len()).expect("a variant count should fit in a `u64`")
                - 1,
        };
        let niche = next_niche(program, shape.map_or(last, |shape| shape.field));
        let layout = &mut self.enums[id];
        layout.shape = shape;
        layout.niche = niche;
    }

    /// The shape of `e` when it is `Option`-shaped: two variants, one with no payload and one
    /// with one field that has a niche. A boxed `F` is a box pointer, never `null`; a boxed `F?`
    /// keeps its `None` in that `null`, so it has no niche.
    fn option_shape(&self, e: &Enum) -> Option<Shape> {
        let [first, second] = e.variants.as_slice() else {
            return None;
        };
        let (empty, field) = match (first.fields.as_slice(), second.fields.as_slice()) {
            ([], [field]) => (0, field),
            ([field], []) => (1, field),
            _ => return None,
        };
        let field = if field.boxed {
            (!matches!(field.ty.kind(), TypeKind::Optional(_))).then_some(Niche {
                storage: Storage::Pointer,
                empty: 0,
            })?
        } else {
            self.niche(field.ty)?
        };
        Some(Shape { empty, field })
    }

    /// The struct type of enum `id`, which is not `Option`-shaped.
    fn tagged_type(&self, id: EnumId) -> StructType<'ctx> {
        self.enums[id.0].ty.into_struct_type()
    }

    /// Sets the body of enum `id`, whose payload structs are all sized.
    fn set_enum_body(&self, id: usize) {
        let layout = &self.enums[id];
        let ty = layout.ty.into_struct_type();
        let (size, align) = layout
            .payloads
            .iter()
            .fold((0, 1), |(size, align), payload| {
                (
                    size.max(self.target_data.get_abi_size(payload)),
                    align.max(self.target_data.get_abi_alignment(payload)),
                )
            });
        let tag = layout.tag.into();
        if size == 0 {
            ty.set_body(&[tag], false);
            return;
        }
        let unit = NonZeroU32::new(align * 8)
            .and_then(|bits| self.context.custom_width_int_type(bits).ok())
            .expect("an ABI alignment should be a positive number of bytes that LLVM has an integer type for");
        let units = u32::try_from(size.div_ceil(u64::from(align)))
            .expect("a payload should be smaller than 2^32 units of its alignment");
        ty.set_body(&[tag, unit.array_type(units).into()], false);
    }

    /// The address of the tag of the enum `id` at `value_addr`.
    fn tag_ptr(&self, id: EnumId, value_addr: PointerValue<'ctx>) -> PointerValue<'ctx> {
        self.builder
            .build_struct_gep(self.tagged_type(id), value_addr, 0, "tag")
            .expect("an enum's struct should have the tag at element 0")
    }

    /// The tag of the enum `id` at `value_addr`. An `Option`-shaped enum has the tag of its
    /// payload-free variant when its storage holds the niche's empty value.
    pub(super) fn load_tag(&self, id: EnumId, value_addr: PointerValue<'ctx>) -> IntValue<'ctx> {
        let layout = &self.enums[id.0];
        if let Some(shape) = layout.shape {
            let present = self.holds_at(shape.field, value_addr, "present");
            let tag = |variant| {
                let variant =
                    u64::try_from(variant).expect("a variant index should fit in a `u64`");
                layout.tag.const_int(variant, false)
            };
            return self
                .builder
                .build_select(present, tag(shape.payload()), tag(shape.empty), "tag")
                .expect(POSITIONED)
                .into_int_value();
        }
        self.builder
            .build_load(self.enums[id.0].tag, self.tag_ptr(id, value_addr), "tag")
            .expect(POSITIONED)
            .into_int_value()
    }

    /// The slot that holds payload field `field` of variant `variant` in the enum `id` at
    /// `value_addr`, through the variant's struct laid over the payload storage: a box pointer
    /// for a boxed field. An `Option`-shaped enum's field is the value itself, and a `bool`
    /// there is an `i8`: `payload_value` reads it, never a load of an `i1`.
    pub(super) fn field_slot(
        &self,
        value_addr: PointerValue<'ctx>,
        id: EnumId,
        variant: usize,
        field: usize,
    ) -> PointerValue<'ctx> {
        let layout = &self.enums[id.0];
        if layout.shape.is_some() {
            return value_addr;
        }
        let storage = self
            .builder
            .build_struct_gep(self.tagged_type(id), value_addr, 1, "payload")
            .expect("an enum with a payload field should have its payload storage at element 1");
        let index = u32::try_from(field).expect(
            "a variant should have fewer than 2^32 fields, since field indexes are `u32` in LLVM",
        );
        self.builder
            .build_struct_gep(layout.payloads[variant], storage, index, "field")
            .expect("the checker should only emit a payload field index that the variant has")
    }

    /// The storage of a boxed field of type `ty`: a box pointer, inside one flag for each layer
    /// past the first.
    fn box_storage(&self, ty: Type) -> BasicTypeEnum<'ctx> {
        match nested(ty) {
            Some(payload) => {
                let fields = [self.context.bool_type().into(), self.box_storage(payload)];
                self.context.struct_type(&fields, false).into()
            }
            None => self.context.ptr_type(AddressSpace::default()).into(),
        }
    }

    /// The address of the box pointer in the storage at `slot` of a boxed field of type `ty`,
    /// inside its flags.
    fn box_slot(&self, mut slot: PointerValue<'ctx>, mut ty: Type) -> PointerValue<'ctx> {
        while let Some(payload) = nested(ty) {
            slot = self.layer_place(ty, slot, 1);
            ty = payload;
        }
        slot
    }

    /// The address of element `index` of the storage at `slot` of a boxed field of the optional
    /// of an optional `ty`: its flag at 0, the storage of its payload at 1.
    pub(super) fn layer_place(
        &self,
        ty: Type,
        slot: PointerValue<'ctx>,
        index: u32,
    ) -> PointerValue<'ctx> {
        self.builder
            .build_struct_gep(
                self.box_storage(ty).into_struct_type(),
                slot,
                index,
                "layer",
            )
            .expect("a boxed field's layer should be a `{ flag, storage }` pair")
    }

    /// The flag of the layer at `slot` of a boxed field of the optional of an optional `ty`.
    pub(super) fn load_layer_flag(&self, ty: Type, slot: PointerValue<'ctx>) -> IntValue<'ctx> {
        self.builder
            .build_load(
                self.context.bool_type(),
                self.layer_place(ty, slot, 0),
                "flag",
            )
            .expect(POSITIONED)
            .into_int_value()
    }

    /// The box pointer of boxed payload field `field` of variant `variant` in the enum `id` at
    /// `value_addr`: `null` for a `None` at any layer of an `F?`, `F??` and so on, and in the
    /// zero value that a `Move` of a flagged local leaves.
    pub(super) fn load_box(
        &self,
        value_addr: PointerValue<'ctx>,
        id: EnumId,
        variant: usize,
        field: usize,
    ) -> PointerValue<'ctx> {
        let ty = self.program.enums[id.0].variants[variant].fields[field].ty;
        self.builder
            .build_load(
                self.context.ptr_type(AddressSpace::default()),
                self.box_slot(self.field_slot(value_addr, id, variant, field), ty),
                "box",
            )
            .expect(POSITIONED)
            .into_pointer_value()
    }

    /// The address of the value in `boxed`, a box of the enum `inner`.
    pub(super) fn box_value(&self, inner: EnumId, boxed: PointerValue<'ctx>) -> PointerValue<'ctx> {
        self.builder
            .build_struct_gep(self.enums[inner.0].boxed, boxed, 1, "boxed")
            .expect("a box's struct should have the value at element 1")
    }

    /// The address of the value of payload field `field` of variant `variant` in the enum `id`
    /// at `value_addr`: the field's slot when it is inline, the value in its box when it is a
    /// boxed `F`. Never for a boxed `F?`, whose box may be `null`: `field_place` gives its
    /// storage, and `payload_value` reads it.
    pub(super) fn payload_place(
        &self,
        value_addr: PointerValue<'ctx>,
        id: EnumId,
        variant: usize,
        field: usize,
    ) -> PointerValue<'ctx> {
        let payload = &self.program.enums[id.0].variants[variant].fields[field];
        if !payload.boxed {
            return self.field_slot(value_addr, id, variant, field);
        }
        assert!(
            !matches!(payload.ty.kind(), TypeKind::Optional(_)),
            "a boxed `F?` field should have no place, only the storage that `field_place` gives"
        );
        let boxed = self.load_box(value_addr, id, variant, field);
        self.box_value(boxed_enum(payload.ty), boxed)
    }

    /// The address of payload field `field` of variant `variant` in the enum `id` at
    /// `value_addr`, and whether it is the storage of a boxed `F?`, whose box may be `null`:
    /// `payload_place`'s address for every other field.
    pub(super) fn field_place(
        &self,
        value_addr: PointerValue<'ctx>,
        id: EnumId,
        variant: usize,
        field: usize,
    ) -> (PointerValue<'ctx>, bool) {
        let payload = &self.program.enums[id.0].variants[variant].fields[field];
        if payload.boxed && matches!(payload.ty.kind(), TypeKind::Optional(_)) {
            return (self.field_slot(value_addr, id, variant, field), true);
        }
        (self.payload_place(value_addr, id, variant, field), false)
    }

    /// The address of the value inside the optional `ty` at `address`, its payload type, and
    /// whether it is still the storage of a boxed field, as `boxed` says `address` is. Meaningful
    /// only after a test that the optional holds a value.
    pub(super) fn some_place(
        &self,
        address: PointerValue<'ctx>,
        ty: Type,
        boxed: bool,
    ) -> (PointerValue<'ctx>, Type, bool) {
        let TypeKind::Optional(payload) = ty.kind() else {
            unreachable!("mir should project `some` only out of an optional")
        };
        if !boxed {
            return (self.optional_value_place(ty, address), payload, false);
        }
        if nested(ty).is_some() {
            return (self.layer_place(ty, address, 1), payload, true);
        }
        let boxed = self
            .builder
            .build_load(
                self.context.ptr_type(AddressSpace::default()),
                address,
                "box",
            )
            .expect(POSITIONED)
            .into_pointer_value();
        (self.box_value(boxed_enum(ty), boxed), payload, false)
    }

    /// Whether the optional `ty` at `address` holds a value; `boxed` says whether `address` is
    /// the storage of a boxed field: its flag, or a box pointer that is not `null`.
    pub(super) fn present_at(
        &self,
        address: PointerValue<'ctx>,
        ty: Type,
        boxed: bool,
    ) -> IntValue<'ctx> {
        if boxed && nested(ty).is_some() {
            return self.load_layer_flag(ty, address);
        }
        // A boxed `F?` is its box pointer, whatever the layout of a value of `F?` is.
        let llvm_ty = if boxed {
            self.context.ptr_type(AddressSpace::default()).into()
        } else {
            self.llvm_type(ty)
                .expect("the checker should give every optional a value type")
        };
        let value = self
            .builder
            .build_load(llvm_ty, address, "optional")
            .expect(POSITIONED);
        if boxed {
            return self
                .builder
                .build_is_not_null(value.into_pointer_value(), "some")
                .expect(POSITIONED);
        }
        self.has_value(ty, value, "some")
    }

    /// The value of payload field `field` of variant `variant` in the enum `id` at
    /// `value_addr`, borrowed: not retained. A boxed `F` is the value in its box; a boxed `F?`
    /// is `None` when its box is `null` and the value in its box otherwise, and never reads
    /// through a `null` box.
    pub(super) fn payload_value(
        &self,
        value_addr: PointerValue<'ctx>,
        id: EnumId,
        variant: usize,
        field: usize,
    ) -> BasicValueEnum<'ctx> {
        let payload = &self.program.enums[id.0].variants[variant].fields[field];
        let ty = payload.ty;
        let llvm_ty = self.llvm_type(ty).expect(
            "the checker should give every payload field a value type, never `unit` or `never`",
        );
        let (true, TypeKind::Optional(_)) = (payload.boxed, ty.kind()) else {
            let place = self.payload_place(value_addr, id, variant, field);
            let layout = &self.enums[id.0];
            if layout.shape.is_some() && !payload.boxed {
                let stored = self
                    .builder
                    .build_load(layout.ty, place, "field")
                    .expect(POSITIONED);
                return self.narrow(ty, stored, "field");
            }
            return self
                .builder
                .build_load(llvm_ty, place, "field")
                .expect(POSITIONED);
        };
        self.boxed_value(self.field_slot(value_addr, id, variant, field), ty)
    }

    /// The value of a boxed field of the optional `ty` whose storage is at `slot`, borrowed.
    /// Each layer past the first is its flag around the value of the storage inside it.
    fn boxed_value(&self, slot: PointerValue<'ctx>, ty: Type) -> BasicValueEnum<'ctx> {
        if let Some(payload) = nested(ty) {
            let flag = self.load_layer_flag(ty, slot);
            let value = self.boxed_value(self.layer_place(ty, slot, 1), payload);
            return self.optional(ty, flag, value);
        }
        let TypeKind::Optional(inner) = ty.kind() else {
            unreachable!("only a boxed optional field should be read by its storage")
        };
        let llvm_ty = self.llvm_type(ty).expect(
            "the checker should give every payload field a value type, never `unit` or `never`",
        );
        let boxed = self
            .builder
            .build_load(self.context.ptr_type(AddressSpace::default()), slot, "box")
            .expect(POSITIONED)
            .into_pointer_value();
        let function = self.current_function();
        let (absent, some) = (
            self.current_block(),
            self.context.append_basic_block(function, "box.some"),
        );
        let done = self.context.append_basic_block(function, "box.done");
        let null = self.builder.build_is_null(boxed, "null").expect(POSITIONED);
        self.branch_on(null, done, some);
        self.builder.position_at_end(some);
        let inner_ty = self
            .llvm_type(inner)
            .expect("the checker should box only a field of an enum type or its optional");
        let value = self
            .builder
            .build_load(inner_ty, self.box_value(boxed_enum(ty), boxed), "boxed")
            .expect(POSITIONED);
        let present = self.optional(ty, self.context.bool_type().const_int(1, false), value);
        self.branch_to(done);
        self.builder.position_at_end(done);
        self.phi(llvm_ty, &[(self.empty(ty), absent), (present, some)])
    }

    /// The value of variant `variant` of enum `id` with the evaluated `values` as its fields:
    /// the tag and each field stored into a scratch slot, which is loaded whole. The slot is
    /// zeroed first, so that the whole load never reads a byte that no store gave a value, and
    /// a boxed `F?` field given `None` keeps its `null`. A boxed field's value moves into a new
    /// box with count 1, built into its token's block when `reuse` gives it one.
    pub(super) fn variant(
        &self,
        id: EnumId,
        variant: usize,
        values: &[BasicValueEnum<'ctx>],
        reuse: &[Option<LocalId>],
    ) -> BasicValueEnum<'ctx> {
        let layout = self.enum_layout(id);
        if let Some(shape) = layout.shape {
            return self.shaped_variant(id, shape, variant, values, reuse);
        }
        let (ty, tag) = (layout.ty.into_struct_type(), layout.tag);
        let slot = self.entry_alloca(ty.into(), "variant");
        self.store(slot, ty.const_zero().into());
        let tag_value = u64::try_from(variant).expect("a variant index should fit in a `u64`");
        self.store(
            self.tag_ptr(id, slot),
            tag.const_int(tag_value, false).into(),
        );
        let fields = &self.program.enums[id.0].variants[variant].fields;
        debug_assert_eq!(
            reuse.len(),
            values.len(),
            "a construction should carry one reuse entry per field"
        );
        for (field, ((&value, payload), &token)) in values.iter().zip(fields).zip(reuse).enumerate()
        {
            let place = self.field_slot(slot, id, variant, field);
            if payload.boxed {
                self.store_boxed(place, value, payload.ty, token);
            } else {
                self.store(place, value);
            }
        }
        self.builder
            .build_load(ty, slot, "variant")
            .expect(POSITIONED)
    }

    /// `variant` of the `Option`-shaped enum `id`. The payload-free variant is the empty value
    /// of its field's niche. The other variant is the field's storage: a `bool` widened to
    /// `i8`, or a boxed field's value in a new box, as `variant` builds it.
    fn shaped_variant(
        &self,
        id: EnumId,
        shape: Shape,
        variant: usize,
        values: &[BasicValueEnum<'ctx>],
        reuse: &[Option<LocalId>],
    ) -> BasicValueEnum<'ctx> {
        if variant == shape.empty {
            return self.empty_value(shape.field);
        }
        let payload = &self.program.enums[id.0].variants[variant].fields[0];
        let (&[value], &[token]) = (values, reuse) else {
            unreachable!("the field variant of an `Option`-shaped enum should have one field")
        };
        if payload.boxed {
            let ptr_type = self.context.ptr_type(AddressSpace::default());
            let slot = self.entry_alloca(ptr_type.into(), "variant");
            self.store_boxed(slot, value, payload.ty, token);
            return self
                .builder
                .build_load(ptr_type, slot, "variant")
                .expect(POSITIONED);
        }
        self.widen(payload.ty, value, "variant")
    }

    /// Stores at `place` a new box that holds `value`, of type `F`, or of `F?` when present. An
    /// `F??` stores its flag, and its payload inside it when present; the zeroed slot keeps a
    /// clear flag's storage zero. With a `token`, the box's block comes from
    /// `@sisu_rc.reuse.<F>`, and a flagged token's slot is left `null`.
    fn store_boxed(
        &self,
        place: PointerValue<'ctx>,
        value: BasicValueEnum<'ctx>,
        ty: Type,
        token: Option<LocalId>,
    ) {
        if let Some(payload) = nested(ty) {
            let flag = self.has_value(ty, value, "flag");
            self.store(self.layer_place(ty, place, 0), flag.into());
            let function = self.current_function();
            let some = self.context.append_basic_block(function, "layer.some");
            let done = self.context.append_basic_block(function, "layer.done");
            self.branch_on(flag, some, done);
            self.builder.position_at_end(some);
            let inner = self.optional_value(ty, value, "layer");
            self.store_boxed(self.layer_place(ty, place, 1), inner, payload, token);
            self.branch_to(done);
            self.builder.position_at_end(done);
            return;
        }
        let (value, present) = match ty.kind() {
            TypeKind::Optional(_) => {
                let function = self.current_function();
                let some = self.context.append_basic_block(function, "box.some");
                let done = self.context.append_basic_block(function, "box.done");
                self.branch_on(self.has_value(ty, value, "flag"), some, done);
                self.builder.position_at_end(some);
                (self.optional_value(ty, value, "payload"), Some(done))
            }
            _ => (value, None),
        };
        let inner = boxed_enum(ty);
        let boxed = match token {
            Some(token) => {
                let boxed = self
                    .builder
                    .build_call(
                        self.helper(Helper::Reuse, self.types[token.0]),
                        &[self.load_local(token).into()],
                        "box",
                    )
                    .expect(POSITIONED)
                    .try_as_basic_value()
                    .basic()
                    .expect("`reuse` should return a pointer, as `helper` declares it");
                if self.flagged[token.0] {
                    let (slot, ty) = self.slot(token);
                    self.store(slot, ty.const_zero());
                }
                boxed
            }
            None => self.alloc_box(inner),
        }
        .into_pointer_value();
        self.init_count(boxed);
        self.store(self.box_value(inner, boxed), value);
        self.store(place, boxed.into());
        if let Some(done) = present {
            self.branch_to(done);
            self.builder.position_at_end(done);
        }
    }

    /// A scratch slot holding the enum `value`, for a helper that takes its address.
    pub(super) fn enum_slot(&self, value: BasicValueEnum<'ctx>) -> PointerValue<'ctx> {
        let slot = self.entry_alloca(value.get_type(), "enum");
        self.store(slot, value);
        slot
    }

    /// The members of the enum `id`'s component, in declaration order, when the component has
    /// boxes; `None` otherwise.
    pub(super) fn component(&self, id: EnumId) -> Option<Rc<[EnumId]>> {
        let component = self.components[id.0].as_ref()?;
        Some(Rc::clone(&component.members))
    }

    /// The enum of payload field `payload` of the enum `id`, whose component has boxes, when the
    /// field is an inline link: not boxed, and of an enum in that component, or its optional
    /// under any number of `?`s.
    pub(super) fn inline_link(&self, id: EnumId, payload: &Payload) -> Option<EnumId> {
        let enums = &self.program.enums;
        (payload.ty.inline_enum())
            .filter(|g| !payload.boxed && enums[g.0].cycle == enums[id.0].cycle)
    }

    /// The body of `@sisu_rc.<kind>.<E>`.
    pub(super) fn define_enum_helper(
        &self,
        kind: Helper,
        id: EnumId,
        function: FunctionValue<'ctx>,
    ) {
        match kind {
            Helper::Retain => self.define_enum_retain(id, function),
            Helper::Release => self.define_enum_release(id, function),
            Helper::Walk => self.define_walk(id, function),
            Helper::Drain => self.define_drain(id, function),
            Helper::Eq => self.define_enum_eq(id, function),
            Helper::EqComponent => self.define_eq_component(id, function),
            Helper::Unbox => self.define_unbox(id, function),
            Helper::Free => self.define_free(id, function),
            Helper::Reuse => self.define_reuse(id, function),
            Helper::Drop | Helper::Unique => {
                unreachable!("an enum should never be queued for a `drop` or `unique` helper")
            }
            Helper::Show | Helper::ShowComponent => {
                unreachable!("`define_helpers` should define every `show` helper itself")
            }
        }
    }

    /// Starts a helper that returns nothing: returns its first parameter, the value's address,
    /// and the block that returns, and leaves the builder in the entry block.
    fn enum_entry(&self, function: FunctionValue<'ctx>) -> (PointerValue<'ctx>, BasicBlock<'ctx>) {
        let entry = self.context.append_basic_block(function, "entry");
        let done = self.context.append_basic_block(function, "done");
        self.builder.position_at_end(done);
        self.builder.build_return(None).expect(POSITIONED);
        self.builder.position_at_end(entry);
        (param(function), done)
    }

    /// `@sisu_rc.retain.<E>(ptr)`: switches on the tag of the value at `ptr`, and retains each
    /// counted field of its variant: a box through `@sisu_rc.retain`, which skips `null`, any
    /// other field through `retain_at`.
    fn define_enum_retain(&self, id: EnumId, function: FunctionValue<'ctx>) {
        let (value, done) = self.enum_entry(function);
        self.for_each_counted_field(id, value, done, |variant, field, payload| {
            if payload.boxed {
                let boxed = self.load_box(value, id, variant, field);
                self.call(self.rc_retain, &[boxed.into()]);
            } else {
                self.retain_at(self.payload_place(value, id, variant, field), payload.ty);
            }
        });
    }

    /// `@sisu_rc.release.<E>(ptr)`: releases each counted part of the value at `ptr`. Without
    /// boxes, it switches on the tag and releases each counted field of the variant through
    /// `release_at`. With boxes, it walks the value with one empty pending list per member of
    /// `E`'s component, held in its own frame, then drains them. Releasing the zero value, which
    /// a `Move` of a flagged local leaves in a slot, releases nothing: it is variant 0 with
    /// every field zero, and `release` and `walk` skip a `null` string, object, array or box,
    /// an optional whose flag is clear, and a zero enum in turn.
    fn define_enum_release(&self, id: EnumId, function: FunctionValue<'ctx>) {
        let (value, done) = self.enum_entry(function);
        let Some(members) = self.component(id) else {
            self.for_each_counted_field(id, value, done, |variant, field, payload| {
                self.release_at(self.payload_place(value, id, variant, field), payload.ty);
            });
            return;
        };
        let heads_type = self.heads_type(members.len());
        let heads = self.entry_alloca_heads(heads_type);
        let walk = self.helper(
            Helper::Walk,
            Type::intern(TypeKind::Enum(id, TypeArgs::NONE)),
        );
        self.call(walk, &[value.into(), heads.into()]);
        let drain = self.helper(
            Helper::Drain,
            Type::intern(TypeKind::Enum(members[0], TypeArgs::NONE)),
        );
        self.call(drain, &[heads.into()]);
        self.branch_to(done);
    }

    /// The pending lists of a component of `members` enums or classes: one head per member, in
    /// declaration order, each `null` when its list is empty.
    ///
    /// It is an array, so LLVM builds it in constant time and prints it in constant size. A
    /// struct type with one element for each member would cost a ring of `n` members `n` steps
    /// at each use.
    pub(super) fn heads_type(&self, members: usize) -> ArrayType<'ctx> {
        let members = u32::try_from(members)
            .expect("a component should have fewer than 2^32 members, as it has as many heads");
        self.context
            .ptr_type(AddressSpace::default())
            .array_type(members)
    }

    /// A new frame slot for the heads of a component, with every head `null`.
    ///
    /// One `memset` of constant size zeroes it. A store of a constant of `heads_type` would
    /// become one store for each member, in each helper that makes the slot.
    pub(super) fn entry_alloca_heads(&self, heads_type: ArrayType<'ctx>) -> PointerValue<'ctx> {
        let heads = self.entry_alloca(heads_type.into(), "heads");
        let size = self.target_data.get_abi_size(&heads_type);
        let align = self.target_data.get_abi_alignment(&heads_type);
        self.builder
            .build_memset(
                heads,
                align,
                self.context.i8_type().const_zero(),
                self.context.i64_type().const_int(size, false),
            )
            .expect("the alignment of an array of pointers should be a power of two");
        heads
    }

    /// The address of the head of member `index`'s list in `heads`, of type `heads_type`.
    #[expect(
        unsafe_code,
        reason = "inkwell marks GEP unsafe for a wrong index list; this GEP takes the two indexes of an array"
    )]
    pub(super) fn head(
        &self,
        heads_type: ArrayType<'ctx>,
        heads: PointerValue<'ctx>,
        index: usize,
    ) -> PointerValue<'ctx> {
        let index = u32::try_from(index)
            .expect("a component should have fewer than 2^32 members, as it has as many heads");
        let i32_type = self.context.i32_type();
        let indexes = [
            i32_type.const_zero(),
            i32_type.const_int(u64::from(index), false),
        ];
        // SAFETY: the first index steps over the one array that `heads` points to. The second is
        // below its length, because every caller passes the index of a member of the component
        // that `heads_type` counts. `member_position` asserts this for an index found from an
        // enum. The other callers take the index from an `enumerate` over the members, or from
        // `ClassCycles`, which indexes a class in its own cycle. So the index list is valid for
        // the array type and `inbounds` is sound.
        unsafe {
            self.builder
                .build_in_bounds_gep(heads_type, heads, &indexes, "head")
                .expect(POSITIONED)
        }
    }

    /// `@sisu_rc.walk.<F>(value, heads)`: switches on the tag of the value at `value`, and
    /// releases each counted field of its variant. A boxed field that is not `null` loses 1
    /// from its box's count, and at zero the box becomes the head of its enum's list in
    /// `heads`, linked to the old head through its count word, the word `load_count` reads.
    /// Nothing else reaches a listed box, so no RC check reads that word. An inline link, when
    /// present, is walked in place with the same `heads`. Every other counted field goes through
    /// `release_at`, which may drain another component's lists in a frame of its own.
    fn define_walk(&self, id: EnumId, function: FunctionValue<'ctx>) {
        let (value, done) = self.enum_entry(function);
        let heads = function
            .get_nth_param(1)
            .expect("the generated `walk` helper should be declared with two parameters")
            .into_pointer_value();
        let members = self
            .component(id)
            .expect("only an enum whose component has boxes should get a `walk` helper");
        let heads_type = self.heads_type(members.len());
        let ptr_type = self.context.ptr_type(AddressSpace::default());
        self.for_each_counted_field(id, value, done, |variant, field, payload| {
            let block = |name| self.context.append_basic_block(function, name);
            if !payload.boxed {
                let place = self.payload_place(value, id, variant, field);
                let Some(inner) = self.inline_link(id, payload) else {
                    self.release_at(place, payload.ty);
                    return;
                };
                let walk = self.helper(
                    Helper::Walk,
                    Type::intern(TypeKind::Enum(inner, TypeArgs::NONE)),
                );
                // A link under `?`s is walked when every layer has a value.
                let next = block("link.next");
                let (mut ty, mut place) = (payload.ty, place);
                while let TypeKind::Optional(contents) = ty.kind() {
                    let some = block("link");
                    let link = self.load_link(ty, place);
                    self.branch_on(self.has_value(ty, link, "flag"), some, next);
                    self.builder.position_at_end(some);
                    place = self.optional_value_place(ty, place);
                    ty = contents;
                }
                self.call(walk, &[place.into(), heads.into()]);
                self.branch_to(next);
                self.builder.position_at_end(next);
                return;
            }
            let (live, last, next) = (block("box"), block("box.last"), block("box.next"));
            let boxed = self.load_box(value, id, variant, field);
            let null = self.builder.build_is_null(boxed, "null").expect(POSITIONED);
            self.branch_on(null, next, live);
            self.builder.position_at_end(live);
            self.decrement(boxed, last, next);
            self.builder.position_at_end(last);
            let index = self.member_position(&members, boxed_enum(payload.ty));
            let head = self.head(heads_type, heads, index);
            let old = self
                .builder
                .build_load(ptr_type, head, "old")
                .expect(POSITIONED);
            self.store(boxed, old);
            self.store(head, boxed.into());
            self.branch_to(next);
            self.builder.position_at_end(next);
        });
    }

    /// `@sisu_rc.drain.<F0>(heads)`, for the component whose first member is `F0`: while some
    /// list in `heads` is not empty, takes the head box of the first such list, makes the box
    /// its count word (the word `load_count` reads) links to the new head, walks the box's
    /// value, which may list more boxes, and frees the box. It runs in one frame, however many
    /// boxes it frees.
    fn define_drain(&self, first: EnumId, function: FunctionValue<'ctx>) {
        let members = self
            .component(first)
            .expect("only the first member of a component with boxes should get a `drain` helper");
        let heads = param(function);
        let heads_type = self.heads_type(members.len());
        let ptr_type = self.context.ptr_type(AddressSpace::default());
        let block = |name| self.context.append_basic_block(function, name);
        let (entry, next) = (block("entry"), block("next"));
        self.builder.position_at_end(entry);
        self.branch_to(next);
        self.builder.position_at_end(next);
        for (index, &member) in members.iter().enumerate() {
            let head = self.head(heads_type, heads, index);
            let boxed = self
                .builder
                .build_load(ptr_type, head, "box")
                .expect(POSITIONED)
                .into_pointer_value();
            let (take, empty) = (block("take"), block("empty"));
            let null = self.builder.build_is_null(boxed, "null").expect(POSITIONED);
            self.branch_on(null, empty, take);
            self.builder.position_at_end(take);
            let following = self
                .builder
                .build_load(ptr_type, boxed, "following")
                .expect(POSITIONED);
            self.store(head, following);
            let walk = self.helper(
                Helper::Walk,
                Type::intern(TypeKind::Enum(member, TypeArgs::NONE)),
            );
            self.call(walk, &[self.box_value(member, boxed).into(), heads.into()]);
            let (size, align) = self.heap_size(&HeapBlock::Box(member));
            self.call(self.free, &[boxed.into(), size.into(), align.into()]);
            self.branch_to(next);
            self.builder.position_at_end(empty);
        }
        self.builder.build_return(None).expect(POSITIONED);
    }

    /// `@sisu_rc.unbox.<F>(slot, out) -> ptr`: `slot` holds a box of `F`, never `null`. Copies
    /// the box's value to `out` and stores `null` into `slot`. A box with count 1 is returned,
    /// its value moved out. A shared box keeps its value: `out` is retained, the box loses the
    /// slot's reference, and `null` is returned. Its count was at least 2, so it stays above 0.
    fn define_unbox(&self, id: EnumId, function: FunctionValue<'ctx>) {
        let nth = |n| {
            function
                .get_nth_param(n)
                .expect("the generated `unbox` helper should be declared with two parameters")
                .into_pointer_value()
        };
        let (slot, out) = (nth(0), nth(1));
        let ptr_type = self.context.ptr_type(AddressSpace::default());
        let block = |name| self.context.append_basic_block(function, name);
        let (entry, unique, shared) = (block("entry"), block("unique"), block("shared"));
        self.builder.position_at_end(entry);
        let boxed = self
            .builder
            .build_load(ptr_type, slot, "box")
            .expect(POSITIONED)
            .into_pointer_value();
        let value_ty = self.enums[id.0].ty;
        let value = self
            .builder
            .build_load(value_ty, self.box_value(id, boxed), "value")
            .expect(POSITIONED);
        self.store(out, value);
        self.store(slot, ptr_type.const_null().into());
        let count = self.load_count(boxed, "count");
        let one = self.int_compare(
            IntPredicate::EQ,
            count,
            self.context.i64_type().const_int(1, false),
        );
        self.branch_on(one, unique, shared);
        self.builder.position_at_end(unique);
        self.builder.build_return(Some(&boxed)).expect(POSITIONED);
        self.builder.position_at_end(shared);
        let ty = Type::intern(TypeKind::Enum(id, TypeArgs::NONE));
        self.call(self.helper(Helper::Retain, ty), &[out.into()]);
        let minus_one = self
            .context
            .i64_type()
            .const_int((-1_i64).cast_unsigned(), false);
        self.add_to_count(boxed, minus_one);
        self.builder
            .build_return(Some(&ptr_type.const_null()))
            .expect(POSITIONED);
    }

    /// A new block for a box of `F`, from `sisu_alloc`.
    fn alloc_box(&self, id: EnumId) -> BasicValueEnum<'ctx> {
        let (size, align) = self.heap_size(&HeapBlock::Box(id));
        self.builder
            .build_call(self.alloc, &[size.into(), align.into()], "box")
            .expect(POSITIONED)
            .try_as_basic_value()
            .basic()
            .expect("`sisu_alloc` should return a pointer, as the runtime declares it")
    }

    /// `@sisu_rc.reuse.<F>(token) -> ptr`: `token` when it holds a box of `F`, whose value was
    /// moved out, and a new block from `sisu_alloc` when it is `null`.
    fn define_reuse(&self, id: EnumId, function: FunctionValue<'ctx>) {
        let block = |name| self.context.append_basic_block(function, name);
        let (entry, held, empty) = (block("entry"), block("held"), block("empty"));
        self.builder.position_at_end(entry);
        let token = param(function);
        let null = self.builder.build_is_null(token, "null").expect(POSITIONED);
        self.branch_on(null, empty, held);
        self.builder.position_at_end(held);
        self.builder.build_return(Some(&token)).expect(POSITIONED);
        self.builder.position_at_end(empty);
        let boxed = self.alloc_box(id);
        self.builder.build_return(Some(&boxed)).expect(POSITIONED);
    }

    /// `@sisu_rc.free.<F>(token)`: frees the box of `F` that `token` holds, whose value was
    /// moved out; nothing when it is `null`.
    fn define_free(&self, id: EnumId, function: FunctionValue<'ctx>) {
        let (boxed, done) = self.helper_entry(function);
        let (size, align) = self.heap_size(&HeapBlock::Box(id));
        self.call(self.free, &[boxed.into(), size.into(), align.into()]);
        self.branch_to(done);
    }

    /// `take_apart`, straight-line (see `Statement::TakeApart`): unboxes each token's box,
    /// outermost first, into a slot of its own; moves each binding out of its place, zeroing the
    /// source when its value keeps a rest; releases each rest; then zeroes the scrutinee's slot
    /// when it is flagged, as a move does.
    pub(super) fn take_apart(
        &self,
        scrutinee: LocalId,
        tokens: &[(LocalId, Place)],
        bindings: &[(LocalId, Place)],
        rest: &[Place],
    ) {
        let mut unboxed = Unboxed {
            child: FxHashMap::default(),
            out: vec![None],
        };
        for (token, place) in tokens {
            let Some((&Projection::Payload { variant, field }, outer)) =
                place.projections.split_last()
            else {
                unreachable!("lowering should make a token only of a payload field")
            };
            let (parent, ty, _) = self.unboxed_at(scrutinee, outer, &unboxed);
            let slot = self.field_slot(parent, enum_of(ty), variant, field);
            let inner = self.types[token.0];
            let value_ty = self
                .llvm_type(inner)
                .expect("a token's enum should have a value type");
            let out = self.entry_alloca(value_ty, "unboxed");
            let boxed = self
                .builder
                .build_call(
                    self.helper(Helper::Unbox, inner),
                    &[slot.into(), out.into()],
                    "token",
                )
                .expect(POSITIONED)
                .try_as_basic_value()
                .basic()
                .expect("`unbox` should return a pointer, as `helper` declares it");
            self.store(self.slot(*token).0, boxed);
            unboxed.insert(&place.projections, out);
        }
        // Each rest's address, and whether each value keeps a rest.
        let mut kept = vec![false; unboxed.out.len()];
        let rest: Vec<(PointerValue<'ctx>, Type)> = (rest.iter())
            .map(|place| {
                let (address, ty, value) = self.unboxed_at(scrutinee, &place.projections, &unboxed);
                kept[value] = true;
                (address, ty)
            })
            .collect();
        for (binding, place) in bindings {
            let (address, _, value) = self.unboxed_at(scrutinee, &place.projections, &unboxed);
            let (slot, ty) = self.slot(*binding);
            let moved = self
                .builder
                .build_load(ty, address, "moved")
                .expect(POSITIONED);
            self.store(slot, moved);
            if kept[value] {
                self.store(address, ty.const_zero());
            }
        }
        for (address, ty) in rest {
            self.release_at(address, ty);
        }
        // No `--rc-checks` moved mark is set: `check_not_moved` (rc.rs) skips enums.
        if self.flagged[scrutinee.0] {
            let (slot, ty) = self.slot(scrutinee);
            self.store(slot, ty.const_zero());
        }
    }

    /// The address of the value at `projections` from `scrutinee`, its type, and the node in
    /// `unboxed` of the value that holds it: the scrutinee, node 0, or a token's box, whose value
    /// is in that token's slot.
    fn unboxed_at(
        &self,
        scrutinee: LocalId,
        projections: &[Projection],
        unboxed: &Unboxed<'ctx>,
    ) -> (PointerValue<'ctx>, Type, usize) {
        let (mut address, _) = self.slot(scrutinee);
        let mut ty = self.types[scrutinee.0];
        let mut node = Some(0);
        let mut value = 0;
        for projection in projections {
            // A `some` here is inline: the box of a boxed `F?` is never taken apart.
            let &Projection::Payload { variant, field } = projection else {
                (address, ty, _) = self.some_place(address, ty, false);
                continue;
            };
            let id = enum_of(ty);
            ty = self.program.enums[id.0].variants[variant].fields[field].ty;
            node = node.and_then(|node| unboxed.child(node, variant, field));
            address = match node.and_then(|node| Some((node, unboxed.out[node]?))) {
                Some((node, out)) => {
                    value = node;
                    out
                }
                None => self.field_slot(address, id, variant, field),
            };
        }
        (address, ty, value)
    }

    /// `@sisu_rc.eq.<E>(a, b) -> i1`: with boxes, `eq_component` of `E`'s component, given
    /// `E`'s index in it. Without, `false` when the tags differ, else whether each pair of the
    /// variant's fields is `==`, stopping at the first that is not. It neither retains nor
    /// releases.
    fn define_enum_eq(&self, id: EnumId, function: FunctionValue<'ctx>) {
        let block = |name| self.context.append_basic_block(function, name);
        let nth = |n| {
            function
                .get_nth_param(n)
                .expect("the generated `eq` helper should be declared with two parameters")
                .into_pointer_value()
        };
        let (a, b) = (nth(0), nth(1));
        let entry = block("entry");
        if let Some(members) = self.component(id) {
            self.builder.position_at_end(entry);
            let component = self.helper(
                Helper::EqComponent,
                Type::intern(TypeKind::Enum(members[0], TypeArgs::NONE)),
            );
            let args = [self.member_index(&members, id).into(), a.into(), b.into()];
            let equal = self.call_component(component, &args);
            self.builder.build_return(Some(&equal)).expect(POSITIONED);
            return;
        }
        let fields = block("fields");
        let (equal, unequal) = self.eq_returns(function);
        self.builder.position_at_end(entry);
        let tag = self.load_tag(id, a);
        let same = self.int_compare(IntPredicate::EQ, tag, self.load_tag(id, b));
        self.branch_on(same, fields, unequal);
        self.builder.position_at_end(fields);
        self.for_each_field(
            id,
            tag,
            equal,
            |_| true,
            |variant, field, _| {
                let pair = self.fields_equal((id, variant, field), a, b);
                let next = block("next");
                self.branch_on(pair, next, unequal);
                self.builder.position_at_end(next);
            },
        );
    }

    /// Whether payload field `field` of variant `variant` is `==` in the values of the enum `id`
    /// at `a` and `b`, which are of that variant. An enum field is read by address, and a field
    /// of any other type by value.
    fn fields_equal(
        &self,
        (id, variant, field): (EnumId, usize, usize),
        a: PointerValue<'ctx>,
        b: PointerValue<'ctx>,
    ) -> IntValue<'ctx> {
        let ty = self.program.enums[id.0].variants[variant].fields[field].ty;
        if let TypeKind::Enum(..) = ty.kind() {
            return self.equal_at(
                ty,
                self.payload_place(a, id, variant, field),
                self.payload_place(b, id, variant, field),
            );
        }
        let value = |at| self.payload_value(at, id, variant, field);
        self.equal(ty, value(a), value(b))
    }

    /// A call of `@sisu_rc.eq_component.<F0>` with `args`: `kind`, `a` and `b`.
    fn call_component(
        &self,
        component: FunctionValue<'ctx>,
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> IntValue<'ctx> {
        self.builder
            .build_call(component, args, "eq")
            .expect(POSITIONED)
            .try_as_basic_value()
            .basic()
            .expect("the generated `eq_component` helper should return a `bool`")
            .into_int_value()
    }

    /// The value of the inline link of type `ty`, `G` or `G?`, at `place`.
    pub(super) fn load_link(&self, ty: Type, place: PointerValue<'ctx>) -> BasicValueEnum<'ctx> {
        let llvm_ty = self
            .llvm_type(ty)
            .expect("an inline link should be of an enum type or its optional");
        self.builder
            .build_load(llvm_ty, place, "link")
            .expect(POSITIONED)
    }

    /// The enum and the two values' addresses of link `field` of variant `variant`, a box or an
    /// inline link, in the values of the enum `id` at `a` and `b`. Branches to `same` when the
    /// pair is equal at once, and to `unequal` when only one is present, and leaves the builder
    /// where both values are present: two boxes go through `boxes_differ`, and two inline `G?`s
    /// are compared by presence first, so an absent one's payload is never read. Each address
    /// points into the values at `a` and `b`, never into a copy, so a pending pair stays valid.
    fn link_pair(
        &self,
        (id, variant, field): (EnumId, usize, usize),
        a: PointerValue<'ctx>,
        b: PointerValue<'ctx>,
        same: BasicBlock<'ctx>,
        unequal: BasicBlock<'ctx>,
    ) -> (EnumId, PointerValue<'ctx>, PointerValue<'ctx>) {
        let payload = &self.program.enums[id.0].variants[variant].fields[field];
        let ty = payload.ty;
        let inner = (ty.inline_enum()).expect("a link should be of an enum type or its optional");
        if payload.boxed {
            let (ty, box_a, box_b) = match nested(ty) {
                None => (
                    ty,
                    self.load_box(a, id, variant, field),
                    self.load_box(b, id, variant, field),
                ),
                Some(_) => self.layer_boxes(ty, a, b, (id, variant, field), same, unequal),
            };
            self.boxes_differ(ty, box_a, box_b, same, unequal);
            return (
                inner,
                self.box_value(inner, box_a),
                self.box_value(inner, box_b),
            );
        }
        let (place_a, place_b) = (
            self.payload_place(a, id, variant, field),
            self.payload_place(b, id, variant, field),
        );
        // Under each `?` the flags are compared first, so an absent layer's payload is never read.
        let (mut ty, mut place_a, mut place_b) = (ty, place_a, place_b);
        while let TypeKind::Optional(contents) = ty.kind() {
            let (link_a, link_b) = (self.load_link(ty, place_a), self.load_link(ty, place_b));
            let (fa, fb) = (
                self.has_value(ty, link_a, "flag"),
                self.has_value(ty, link_b, "flag"),
            );
            let function = self.current_function();
            let block = |name| self.context.append_basic_block(function, name);
            let (flags, both) = (block("link.flags"), block("link.both"));
            let flags_equal = self.int_compare(IntPredicate::EQ, fa, fb);
            self.branch_on(flags_equal, flags, unequal);
            self.builder.position_at_end(flags);
            self.branch_on(fa, both, same);
            self.builder.position_at_end(both);
            (place_a, place_b) = (
                self.optional_value_place(ty, place_a),
                self.optional_value_place(ty, place_b),
            );
            ty = contents;
        }
        (inner, place_a, place_b)
    }

    /// The box pointers of boxed field `field` of variant `variant` of the enum `id` at `a` and
    /// `b`, of the optional of an optional `ty`, after its flags, with the type of the box, `F?`:
    /// at each layer past the first, unequal flags go to `unequal`, and two clear ones to `same`.
    fn layer_boxes(
        &self,
        mut ty: Type,
        a: PointerValue<'ctx>,
        b: PointerValue<'ctx>,
        (id, variant, field): (EnumId, usize, usize),
        same: BasicBlock<'ctx>,
        unequal: BasicBlock<'ctx>,
    ) -> (Type, PointerValue<'ctx>, PointerValue<'ctx>) {
        let (mut slot_a, mut slot_b) = (
            self.field_slot(a, id, variant, field),
            self.field_slot(b, id, variant, field),
        );
        while let Some(payload) = nested(ty) {
            let (fa, fb) = (
                self.load_layer_flag(ty, slot_a),
                self.load_layer_flag(ty, slot_b),
            );
            let function = self.current_function();
            let block = |name| self.context.append_basic_block(function, name);
            let (flags, both) = (block("layer.flags"), block("layer.both"));
            self.branch_on(self.int_compare(IntPredicate::EQ, fa, fb), flags, unequal);
            self.builder.position_at_end(flags);
            self.branch_on(fa, both, same);
            self.builder.position_at_end(both);
            (slot_a, slot_b) = (
                self.layer_place(ty, slot_a, 1),
                self.layer_place(ty, slot_b, 1),
            );
            ty = payload;
        }
        let ptr_type = self.context.ptr_type(AddressSpace::default());
        let load = |slot| {
            self.builder
                .build_load(ptr_type, slot, "box")
                .expect(POSITIONED)
                .into_pointer_value()
        };
        (ty, load(slot_a), load(slot_b))
    }

    /// `compare_at_once` on the boxes `a` and `b` of a boxed field of type `ty`, `F` or `F?`,
    /// which branches to `same` or `unequal`; only an `F?` box is `null`, for `None`.
    fn boxes_differ(
        &self,
        ty: Type,
        a: PointerValue<'ctx>,
        b: PointerValue<'ctx>,
        same: BasicBlock<'ctx>,
        unequal: BasicBlock<'ctx>,
    ) {
        let nullable = matches!(ty.kind(), TypeKind::Optional(_));
        self.compare_at_once(self.reflexive(ty), nullable, a, b, same, unequal);
    }

    /// The `kind` of `member` in its component: its index, as an `i32`.
    pub(super) fn member_index(&self, members: &Rc<[EnumId]>, member: EnumId) -> IntValue<'ctx> {
        self.kind(self.member_position(members, member))
    }

    /// The `kind` of the member at `index` in a component, as an `i32`.
    pub(super) fn kind(&self, index: usize) -> IntValue<'ctx> {
        let index = u64::try_from(index).expect("a member index should fit in a `u64`");
        self.context.i32_type().const_int(index, false)
    }

    /// `@sisu_rc.eq_component.<F0>(kind, a, b) -> i1`: `==` on the values at `a` and `b` of
    /// member `kind` of the component whose first member is `F0`. It switches on `kind`, then
    /// compares as `eq` does. It compares a link, a box or an inline link (`inline_link`) by
    /// the values it holds: see `link_pair`. It neither retains nor releases. It compares depth
    /// first. In each variant it compares the fields in order and stops at the first
    /// difference, as a recursive `==` does. A link that is the last field loops: `kind`, `a`
    /// and `b` become that link's enum and its two values. Before any other link, it pushes
    /// where the value's comparison goes on (`push_pending`). Then it loops into the link.
    /// When a value is done, it pops that place and goes on there. So it compares any depth
    /// and shape of value in one frame. Every address it holds points into the compared
    /// values, which the caller keeps alive.
    fn define_eq_component(&self, first: EnumId, function: FunctionValue<'ctx>) {
        let members = self.component(first).expect(
            "only the first member of a component with boxes should get an `eq_component` helper",
        );
        let block = |name| self.context.append_basic_block(function, name);
        let (entry, top, pop, dispatch) = (
            block("entry"),
            block("loop"),
            block("pop"),
            block("dispatch"),
        );
        let nth = |n| {
            function.get_nth_param(n).expect(
                "the generated `eq_component` helper should be declared with three parameters",
            )
        };
        self.builder.position_at_end(entry);
        let pending = self.empty_pending();
        let (equal, unequal) = self.pending_returns(function, &pending);
        self.builder.position_at_end(entry);
        self.branch_to(top);
        let ptr_type = self.context.ptr_type(AddressSpace::default()).into();
        let types: [BasicTypeEnum<'ctx>; 3] = [self.context.i32_type().into(), ptr_type, ptr_type];
        let phis = |at, name| {
            self.builder.position_at_end(at);
            types.map(|ty| self.builder.build_phi(ty, name).expect(POSITIONED))
        };
        let (current, popped) = (phis(top, "current"), phis(dispatch, "popped"));
        for (phi, n) in current.iter().zip(0..) {
            phi.add_incoming(&[(&nth(n), entry)]);
        }
        self.builder.position_at_end(top);
        let starts: Vec<_> = members
            .iter()
            .map(|m| block(&self.program.enums[m.0].name))
            .collect();
        let cases: Vec<_> = starts
            .iter()
            .enumerate()
            .skip(1)
            .map(|(index, &start)| (self.kind(index), start))
            .collect();
        // `kind` is always a member's index, so the first member can be the default.
        let kind = current[0].as_basic_value().into_int_value();
        self.builder
            .build_switch(kind, starts[0], &cases)
            .expect(POSITIONED);
        let eq = EqLoop {
            members,
            top,
            current,
            pop,
            unequal,
            popped,
        };
        let mut resumes = Vec::new();
        for (&id, &start) in eq.members.iter().zip(&starts) {
            self.builder.position_at_end(start);
            let a = current[1].as_basic_value().into_pointer_value();
            let b = current[2].as_basic_value().into_pointer_value();
            // A member whose `Eq` is written is compared by its `eq`, wherever a link meets it.
            let ty = Type::intern(TypeKind::Enum(id, TypeArgs::NONE));
            if let Some(method) = self.written_eq(ty) {
                let borrowed = &self.mir.functions[method.0].borrowed;
                let same = self.call_written_eq(method, ty, [a, b], borrowed);
                self.branch_on(same, pop, unequal);
                continue;
            }
            let tag = self.load_tag(id, a);
            let same = self.int_compare(IntPredicate::EQ, tag, self.load_tag(id, b));
            let fields = block("fields");
            self.branch_on(same, fields, unequal);
            self.builder.position_at_end(fields);
            for (variant, at) in self.switch_variants(id, tag, pop, |_| true) {
                self.builder.position_at_end(at);
                self.compare_variant(&eq, &pending, &mut resumes, (id, variant));
            }
        }
        self.builder.position_at_end(pop);
        self.pop_pending(&pending, equal, dispatch, popped);
        self.builder.position_at_end(dispatch);
        let Some((&default, rest)) = resumes.split_first() else {
            // No variant has a link that is not its last field, so nothing is ever pushed.
            self.builder.build_unreachable().expect(POSITIONED);
            return;
        };
        let cases: Vec<_> = (1..)
            .zip(rest)
            .map(|(index, &at)| (self.context.i32_type().const_int(index, false), at))
            .collect();
        let resume = popped[0].as_basic_value().into_int_value();
        self.builder
            .build_switch(resume, default, &cases)
            .expect(POSITIONED);
    }

    /// Whether payload field `payload` of the enum `id` is a link: a box, or an inline link.
    pub(super) fn is_link(&self, id: EnumId, payload: &Payload) -> bool {
        payload.boxed || self.inline_link(id, payload).is_some()
    }

    /// Compares the fields of variant `variant` of the enum `id`, in the loop `eq`, from the
    /// builder's block, in field order. Before each link that is not the last field, it pushes
    /// the index of a new block in `resumes`, which goes on from the next field, and loops into
    /// the link. A last field that is a link loops with no push. Else the value is done and
    /// goes to `pop`.
    fn compare_variant(
        &self,
        eq: &EqLoop<'ctx>,
        pending: &Pending<'ctx>,
        resumes: &mut Vec<BasicBlock<'ctx>>,
        (id, variant): (EnumId, usize),
    ) {
        let function = self.current_function();
        let pointer = |phi: PhiValue<'ctx>| phi.as_basic_value().into_pointer_value();
        let (mut a, mut b) = (pointer(eq.current[1]), pointer(eq.current[2]));
        let payloads = &self.program.enums[id.0].variants[variant].fields;
        let looped = payloads
            .iter()
            .enumerate()
            .next_back()
            .filter(|(_, payload)| self.is_link(id, payload))
            .map(|(field, _)| field);
        for (field, payload) in payloads.iter().enumerate() {
            if looped == Some(field) {
                continue;
            }
            if !self.is_link(id, payload) {
                let next = self.context.append_basic_block(function, "next");
                let pair = self.fields_equal((id, variant, field), a, b);
                self.branch_on(pair, next, eq.unequal);
                self.builder.position_at_end(next);
                continue;
            }
            let resume = self.context.append_basic_block(function, "resume");
            let index = u64::try_from(resumes.len()).expect("a resume index should fit in a `u64`");
            resumes.push(resume);
            let index = self.context.i32_type().const_int(index, false).into();
            self.push_pending(pending, [index, a.into(), b.into()]);
            self.enter_link(eq, (id, variant, field), a, b);
            self.builder.position_at_end(resume);
            (a, b) = (pointer(eq.popped[1]), pointer(eq.popped[2]));
        }
        match looped {
            Some(field) => self.enter_link(eq, (id, variant, field), a, b),
            None => self.branch_to(eq.pop),
        }
    }

    /// Loops into link `field` of variant `variant` in the values of the enum `id` at `a` and
    /// `b`, when `link_pair` does not decide the pair at once: equal goes to `pop`.
    fn enter_link(
        &self,
        eq: &EqLoop<'ctx>,
        link: (EnumId, usize, usize),
        a: PointerValue<'ctx>,
        b: PointerValue<'ctx>,
    ) {
        let (inner, value_a, value_b) = self.link_pair(link, a, b, eq.pop, eq.unequal);
        let from = self.current_block();
        let values: [BasicValueEnum<'ctx>; 3] = [
            self.member_index(&eq.members, inner).into(),
            value_a.into(),
            value_b.into(),
        ];
        for (phi, value) in eq.current.iter().zip(values) {
            phi.add_incoming(&[(&value, from)]);
        }
        self.branch_to(eq.top);
    }

    /// The blocks of an `eq_component` helper that return `true` and `false`. Both go to one
    /// block that frees the buffer of `pending` when a push allocated it, then returns.
    fn pending_returns(
        &self,
        function: FunctionValue<'ctx>,
        pending: &Pending<'ctx>,
    ) -> (BasicBlock<'ctx>, BasicBlock<'ctx>) {
        let block = |name| self.context.append_basic_block(function, name);
        let (equal, unequal, finish) = (block("equal"), block("unequal"), block("finish"));
        for at in [equal, unequal] {
            self.builder.position_at_end(at);
            self.branch_to(finish);
        }
        self.builder.position_at_end(finish);
        let bool_type = self.context.bool_type();
        let result = self.phi(
            bool_type,
            &[
                (bool_type.const_int(1, false), equal),
                (bool_type.const_zero(), unequal),
            ],
        );
        self.free_pending(pending);
        self.builder.build_return(Some(&result)).expect(POSITIONED);
        (equal, unequal)
    }

    /// No pending pairs: no top pair held, and a buffer that is `null`, with capacity and
    /// length 0, so that a comparison that never holds two pairs at once allocates nothing.
    pub(super) fn empty_pending(&self) -> Pending<'ctx> {
        let i64_type = self.context.i64_type();
        let ptr_type = self.context.ptr_type(AddressSpace::default());
        let bool_type = self.context.bool_type();
        let pending = Pending {
            top: self.entry_alloca(self.pending_pair().into(), "pending.top"),
            held: self.entry_alloca(bool_type.into(), "pending.held"),
            buffer: self.entry_alloca(ptr_type.into(), "pending"),
            cap: self.entry_alloca(i64_type.into(), "pending.cap"),
            len: self.entry_alloca(i64_type.into(), "pending.len"),
        };
        self.store(pending.held, bool_type.const_zero().into());
        self.store(pending.buffer, ptr_type.const_null().into());
        self.store(pending.cap, i64_type.const_zero().into());
        self.store(pending.len, i64_type.const_zero().into());
        pending
    }

    /// The `i64` in the `Pending` slot `slot`.
    fn load_pending(&self, slot: PointerValue<'ctx>) -> IntValue<'ctx> {
        self.builder
            .build_load(self.context.i64_type(), slot, "pending")
            .expect(POSITIONED)
            .into_int_value()
    }

    /// Whether `pending` holds a top pair.
    fn pending_held(&self, pending: &Pending<'ctx>) -> IntValue<'ctx> {
        self.builder
            .build_load(self.context.bool_type(), pending.held, "held")
            .expect(POSITIONED)
            .into_int_value()
    }

    /// The buffer pointer of `pending`.
    fn pending_buffer(&self, pending: &Pending<'ctx>) -> PointerValue<'ctx> {
        let ptr_type = self.context.ptr_type(AddressSpace::default());
        self.builder
            .build_load(ptr_type, pending.buffer, "buffer")
            .expect(POSITIONED)
            .into_pointer_value()
    }

    /// The address of pair `index` in the buffer `buffer`.
    #[expect(
        unsafe_code,
        reason = "inkwell marks GEP unsafe for a wrong index list; this GEP takes one `i64` index"
    )]
    fn pending_entry(
        &self,
        buffer: PointerValue<'ctx>,
        index: IntValue<'ctx>,
    ) -> PointerValue<'ctx> {
        // SAFETY: the one `i64` index steps over whole pairs and never into an aggregate, so
        // it is valid for the pair type. `inbounds` is sound because every caller passes an
        // index below the buffer's capacity: a push grows a full buffer first, and a pop takes
        // an index below the length.
        unsafe {
            self.builder
                .build_in_bounds_gep(self.pending_pair(), buffer, &[index], "pair")
                .expect(POSITIONED)
        }
    }

    /// The `kind`, `a` and `b` of the pending pair at `at`.
    fn load_pair(&self, at: PointerValue<'ctx>) -> [BasicValueEnum<'ctx>; 3] {
        let pair = self.pending_pair();
        let ptr_type = self.context.ptr_type(AddressSpace::default()).into();
        let types: [BasicTypeEnum<'ctx>; 3] = [self.context.i32_type().into(), ptr_type, ptr_type];
        let mut index = 0;
        types.map(|ty| {
            let field = self
                .builder
                .build_struct_gep(pair, at, index, "part")
                .expect("a pending pair should have three elements");
            index += 1;
            self.builder
                .build_load(ty, field, "part")
                .expect(POSITIONED)
        })
    }

    /// Stores `pair`, its `kind`, `a` and `b`, as the pending pair at `at`.
    fn store_pair(&self, at: PointerValue<'ctx>, pair: [BasicValueEnum<'ctx>; 3]) {
        for (index, value) in (0..).zip(pair) {
            let field = self
                .builder
                .build_struct_gep(self.pending_pair(), at, index, "part")
                .expect("a pending pair should have three elements");
            self.store(field, value);
        }
    }

    /// Pushes `pair` onto `pending`. It becomes the top pair, which lives in the helper's
    /// frame; a top pair held before moves to the buffer (`push_buffer`). So the buffer
    /// grows only when the comparison waits on two pairs at once.
    pub(super) fn push_pending(&self, pending: &Pending<'ctx>, pair: [BasicValueEnum<'ctx>; 3]) {
        let function = self.current_function();
        let (spill, set) = (
            self.context.append_basic_block(function, "pending.spill"),
            self.context.append_basic_block(function, "pending.set"),
        );
        self.branch_on(self.pending_held(pending), spill, set);
        self.builder.position_at_end(spill);
        self.push_buffer(pending, self.load_pair(pending.top));
        self.branch_to(set);
        self.builder.position_at_end(set);
        self.store_pair(pending.top, pair);
        let held = self.context.bool_type().const_int(1, false);
        self.store(pending.held, held.into());
    }

    /// Pushes `pair` onto the buffer of `pending`. A full buffer grows first: the first push
    /// allocates room for `FIRST_PENDING` pairs, and each later one doubles it.
    fn push_buffer(&self, pending: &Pending<'ctx>, pair: [BasicValueEnum<'ctx>; 3]) {
        let function = self.current_function();
        let block = |name| self.context.append_basic_block(function, name);
        let (grow, first, double, put) = (
            block("pending.grow"),
            block("pending.first"),
            block("pending.double"),
            block("pending.put"),
        );
        let i64_type = self.context.i64_type();
        let (len, cap) = (
            self.load_pending(pending.len),
            self.load_pending(pending.cap),
        );
        self.branch_on(self.int_compare(IntPredicate::EQ, len, cap), grow, put);
        self.builder.position_at_end(grow);
        let buffer = self.pending_buffer(pending);
        let null = self
            .builder
            .build_is_null(buffer, "null")
            .expect(POSITIONED);
        self.branch_on(null, first, double);
        self.builder.position_at_end(first);
        let first_cap = i64_type.const_int(FIRST_PENDING, false);
        let (size, align) = self.heap_size(&HeapBlock::Pending { cap: first_cap });
        let allocated = self.call_pointer(self.alloc, &[size.into(), align.into()]);
        self.store(pending.buffer, allocated.into());
        self.store(pending.cap, first_cap.into());
        self.branch_to(put);
        self.builder.position_at_end(double);
        let doubled = self
            .builder
            .build_int_nuw_mul(cap, i64_type.const_int(2, false), "cap")
            .expect(POSITIONED);
        let (old_size, align) = self.heap_size(&HeapBlock::Pending { cap });
        let (new_size, _) = self.heap_size(&HeapBlock::Pending { cap: doubled });
        let args = [
            buffer.into(),
            old_size.into(),
            align.into(),
            new_size.into(),
        ];
        let grown = self.call_pointer(self.realloc(), &args);
        self.store(pending.buffer, grown.into());
        self.store(pending.cap, doubled.into());
        self.branch_to(put);
        self.builder.position_at_end(put);
        self.store_pair(self.pending_entry(self.pending_buffer(pending), len), pair);
        let one = i64_type.const_int(1, false);
        let more = self
            .builder
            .build_int_nuw_add(len, one, "len")
            .expect(POSITIONED);
        self.store(pending.len, more.into());
    }

    /// Pops the last pair of `pending` into the `popped` phis of block `resumed`, and branches
    /// there: the top pair when one is held, else the buffer's last. Branches to `empty` when
    /// `pending` holds no pair.
    pub(super) fn pop_pending(
        &self,
        pending: &Pending<'ctx>,
        empty: BasicBlock<'ctx>,
        resumed: BasicBlock<'ctx>,
        popped: [PhiValue<'ctx>; 3],
    ) {
        let function = self.current_function();
        let block = |name| self.context.append_basic_block(function, name);
        let (from_top, check, from_buffer) =
            (block("pop.top"), block("pop.check"), block("pop.buffer"));
        let take = |pair: [BasicValueEnum<'ctx>; 3]| {
            let from = self.current_block();
            for (phi, value) in popped.iter().zip(pair) {
                phi.add_incoming(&[(&value, from)]);
            }
            self.branch_to(resumed);
        };
        self.branch_on(self.pending_held(pending), from_top, check);
        self.builder.position_at_end(from_top);
        self.store(pending.held, self.context.bool_type().const_zero().into());
        take(self.load_pair(pending.top));
        self.builder.position_at_end(check);
        let len = self.load_pending(pending.len);
        let zero = self.context.i64_type().const_zero();
        self.branch_on(
            self.int_compare(IntPredicate::EQ, len, zero),
            empty,
            from_buffer,
        );
        self.builder.position_at_end(from_buffer);
        let one = self.context.i64_type().const_int(1, false);
        let last = self
            .builder
            .build_int_nuw_sub(len, one, "last")
            .expect(POSITIONED);
        self.store(pending.len, last.into());
        take(self.load_pair(self.pending_entry(self.pending_buffer(pending), last)));
    }

    /// Frees the buffer of `pending` when a push allocated it.
    pub(super) fn free_pending(&self, pending: &Pending<'ctx>) {
        let function = self.current_function();
        let (free, done) = (
            self.context.append_basic_block(function, "pending.free"),
            self.context.append_basic_block(function, "pending.done"),
        );
        let buffer = self.pending_buffer(pending);
        let null = self
            .builder
            .build_is_null(buffer, "null")
            .expect(POSITIONED);
        self.branch_on(null, done, free);
        self.builder.position_at_end(free);
        let cap = self.load_pending(pending.cap);
        let (size, align) = self.heap_size(&HeapBlock::Pending { cap });
        self.call(self.free, &[buffer.into(), size.into(), align.into()]);
        self.branch_to(done);
        self.builder.position_at_end(done);
    }

    /// A call of `callee`, a runtime function that returns a pointer, with `args`.
    fn call_pointer(
        &self,
        callee: FunctionValue<'ctx>,
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> PointerValue<'ctx> {
        self.builder
            .build_call(callee, args, "pending")
            .expect(POSITIONED)
            .try_as_basic_value()
            .basic()
            .expect(
                "the runtime's allocators should return a pointer, as the runtime declares them",
            )
            .into_pointer_value()
    }

    /// `for_each_field` over the counted fields, boxes included, of the value at `value`.
    fn for_each_counted_field(
        &self,
        id: EnumId,
        value: PointerValue<'ctx>,
        done: BasicBlock<'ctx>,
        each: impl FnMut(usize, usize, &Payload),
    ) {
        let enums = &self.program.enums;
        self.for_each_field(
            id,
            self.load_tag(id, value),
            done,
            |payload| payload.boxed || payload.ty.is_counted(enums),
            each,
        );
    }

    /// Switches on `tag`, of enum `id`. Each variant with a field that `wanted` picks gets a
    /// block that runs `each(variant, field, payload)` for each picked field in order, then
    /// branches to `done`; every other tag branches to `done` at once.
    fn for_each_field(
        &self,
        id: EnumId,
        tag: IntValue<'ctx>,
        done: BasicBlock<'ctx>,
        wanted: impl Fn(&Payload) -> bool,
        mut each: impl FnMut(usize, usize, &Payload),
    ) {
        let e = &self.program.enums[id.0];
        for (variant, block) in self.switch_variants(id, tag, done, &wanted) {
            self.builder.position_at_end(block);
            for (field, payload) in e.variants[variant].fields.iter().enumerate() {
                if wanted(payload) {
                    each(variant, field, payload);
                }
            }
            self.branch_to(done);
        }
    }

    /// Switches on `tag`, of enum `id`: each variant with a field that `wanted` picks goes to a
    /// new block, returned with the variant's index; every other tag goes to `done`.
    fn switch_variants(
        &self,
        id: EnumId,
        tag: IntValue<'ctx>,
        done: BasicBlock<'ctx>,
        wanted: impl Fn(&Payload) -> bool,
    ) -> Vec<(usize, BasicBlock<'ctx>)> {
        let function = self.current_function();
        let e = &self.program.enums[id.0];
        let arms: Vec<_> = (0_u64..)
            .zip(&e.variants)
            .enumerate()
            .filter(|(_, (_, variant))| variant.fields.iter().any(&wanted))
            .map(|(index, (value, variant))| {
                let block = self.context.append_basic_block(function, &variant.name);
                (index, self.enums[id.0].tag.const_int(value, false), block)
            })
            .collect();
        let cases: Vec<_> = arms
            .iter()
            .map(|&(_, value, block)| (value, block))
            .collect();
        self.builder
            .build_switch(tag, done, &cases)
            .expect(POSITIONED);
        arms.into_iter()
            .map(|(variant, _, block)| (variant, block))
            .collect()
    }
}

/// The blocks and phis of an `eq_component` helper's loop that each variant's comparison uses.
struct EqLoop<'ctx> {
    members: Rc<[EnumId]>,
    /// The loop's head, and its `kind`, `a` and `b` phis.
    top: BasicBlock<'ctx>,
    current: [PhiValue<'ctx>; 3],
    /// Where a value whose fields are all compared goes.
    pop: BasicBlock<'ctx>,
    unequal: BasicBlock<'ctx>,
    /// The resume index, `a` and `b` of a popped pair.
    popped: [PhiValue<'ctx>; 3],
}

/// The first capacity of a buffer of pending pairs, in pairs.
const FIRST_PENDING: u64 = 8;

/// The slots of the pending pairs of an `eq_component` helper, in its entry block, which
/// `mem2reg` makes phis: the top pair and whether it is held, then the buffer of the pairs
/// below it, `null` until the first spill, its capacity and its length.
pub(super) struct Pending<'ctx> {
    top: PointerValue<'ctx>,
    held: PointerValue<'ctx>,
    buffer: PointerValue<'ctx>,
    cap: PointerValue<'ctx>,
    len: PointerValue<'ctx>,
}

/// A `take_apart`'s token places as a trie of their prefixes, so that finding a place's token
/// costs one lookup per projection: node 0 is the scrutinee, and a node's child is keyed by the
/// node and the payload projection that reaches it.
struct Unboxed<'ctx> {
    child: FxHashMap<(usize, usize, usize), usize>,
    /// The slot each node's token's box went to, for a node that is a token's place.
    out: Vec<Option<PointerValue<'ctx>>>,
}

impl<'ctx> Unboxed<'ctx> {
    fn child(&self, node: usize, variant: usize, field: usize) -> Option<usize> {
        #[cfg(test)]
        super::TAKE_APART_PROBES.set(super::TAKE_APART_PROBES.get() + 1);
        self.child.get(&(node, variant, field)).copied()
    }

    /// Records that the box at `projections`, a token's place, went to `out`.
    fn insert(&mut self, projections: &[Projection], out: PointerValue<'ctx>) {
        let mut node = 0;
        for projection in projections {
            // An inline `some` stays at its optional's node.
            let &Projection::Payload { variant, field } = projection else {
                continue;
            };
            #[cfg(test)]
            super::TAKE_APART_PROBES.set(super::TAKE_APART_PROBES.get() + 1);
            let next = self.out.len();
            node = *self.child.entry((node, variant, field)).or_insert(next);
            if node == next {
                self.out.push(None);
            }
        }
        self.out[node] = Some(out);
    }
}
