//! Enums at run time. An enum value is a tag and its variant's payload, held inline:
//! `%sisu.<Name> = { tag, [n x iA] }`, with one struct per variant, `%sisu.<Name>.<Variant>`,
//! laid over the payload storage. A field that closes a cycle holds a box instead,
//! `%sisu.box.<F> = { i64 count, %sisu.<F> }`. This module owns the layouts, building a variant,
//! and the bodies of the enum helpers, which `define_helpers` reaches through
//! `define_enum_helper`.
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

use inkwell::AddressSpace;
use inkwell::IntPredicate;
use inkwell::basic_block::BasicBlock;
use inkwell::types::{BasicTypeEnum, IntType, StructType};
use inkwell::values::{
    BasicMetadataValueEnum, BasicValueEnum, FunctionValue, IntValue, PhiValue, PointerValue,
};

use super::layout::HeapBlock;
use super::rc::{Helper, param};
use super::{Codegen, POSITIONED, enum_of};
use crate::mir::{Place, Projection};
use crate::tir::{Enum, EnumId, LocalId, Payload, Type, TypeArgs, TypeKind};
use rustc_hash::FxHashMap;

/// An enum's LLVM types.
pub(super) struct EnumLayout<'ctx> {
    /// `{ tag, [n x iA] }`, or `{ tag }` when no variant has a field. `iA` is the integer type
    /// as wide as the largest payload ABI alignment `a`, and `n` the largest payload ABI size
    /// rounded up to whole `a`s, so the storage fits every payload at its alignment.
    pub(super) ty: StructType<'ctx>,
    /// `i8`, `i16` or `i32`, the smallest that holds the variant count. Tag `v` is variant `v`.
    pub(super) tag: IntType<'ctx>,
    /// Indexed by variant: `{ fields... }`, a field's LLVM type being its type's, or `ptr` when
    /// boxed.
    pub(super) payloads: Vec<StructType<'ctx>>,
    /// `%sisu.box.<Name> = { i64 count, %sisu.<Name> }`, a box that holds a value of this enum.
    pub(super) boxed: StructType<'ctx>,
}

/// The enum a boxed field holds: its type is `F` or `F?`.
fn boxed_enum(ty: Type) -> EnumId {
    ty.inline_enum()
        .expect("the checker should box only a field of an enum type or its optional")
}

/// The index of `member` in a component of `members`.
fn member_position(members: &[EnumId], member: EnumId) -> usize {
    members.iter().position(|&m| m == member).expect(
        "the checker should box only a field whose enum is in the declaring enum's component",
    )
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

    /// Builds every enum's layout: each enum's type and its payload structs, named first so
    /// that a payload can name any enum, then each payload's body, then each enum's body. An
    /// enum's body needs its payloads' sizes, so an enum held inline gets its body before its
    /// holder: post-order over the inline fields, with an explicit stack. `tir::layout_enums`
    /// boxes a field on every cycle, so the inline fields form no cycle and the walk ends.
    pub(super) fn lay_out_enums(&mut self) {
        let program = self.program;
        let context = self.context;
        self.enums = program
            .enums
            .iter()
            .map(|e| {
                let count = e.variants.len();
                let tag = if count <= 1 << 8 {
                    context.i8_type()
                } else if count <= 1 << 16 {
                    context.i16_type()
                } else {
                    context.i32_type()
                };
                EnumLayout {
                    ty: context.opaque_struct_type(&format!("sisu.{}", e.name)),
                    tag,
                    payloads: e
                        .variants
                        .iter()
                        .map(|v| context.opaque_struct_type(&format!("sisu.{}.{}", e.name, v.name)))
                        .collect(),
                    boxed: context.opaque_struct_type(&format!("sisu.box.{}", e.name)),
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
        for (e, layout) in program.enums.iter().zip(&self.enums) {
            for (variant, payload) in e.variants.iter().zip(&layout.payloads) {
                let fields: Vec<_> = variant
                    .fields
                    .iter()
                    .map(|field| {
                        if field.boxed {
                            ptr_type.into()
                        } else {
                            self.llvm_type(field.ty).expect(
                                "the checker should give every payload field a value type, never `unit` or `never`",
                            )
                        }
                    })
                    .collect();
                payload.set_body(&fields, false);
            }
        }
        let mut done = vec![false; program.enums.len()];
        for root in 0..program.enums.len() {
            // `(id, true)` once the enums `id` holds inline are queued before it.
            let mut stack = vec![(root, false)];
            while let Some((id, held_queued)) = stack.pop() {
                if done[id] {
                    continue;
                }
                if held_queued {
                    self.set_enum_body(id);
                    done[id] = true;
                    continue;
                }
                stack.push((id, true));
                stack.extend(held_inline(&program.enums[id]).map(|held| (held.0, false)));
            }
        }
        for layout in &self.enums {
            layout
                .boxed
                .set_body(&[i64_type.into(), layout.ty.into()], false);
        }
    }

    /// Sets the body of enum `id`, whose payload structs are all sized.
    fn set_enum_body(&self, id: usize) {
        let layout = &self.enums[id];
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
            layout.ty.set_body(&[tag], false);
            return;
        }
        let unit = NonZeroU32::new(align * 8)
            .and_then(|bits| self.context.custom_width_int_type(bits).ok())
            .expect("an ABI alignment should be a positive number of bytes that LLVM has an integer type for");
        let units = u32::try_from(size.div_ceil(u64::from(align)))
            .expect("a payload should be smaller than 2^32 units of its alignment");
        layout
            .ty
            .set_body(&[tag, unit.array_type(units).into()], false);
    }

    /// The address of the tag of the enum `id` at `value_addr`.
    fn tag_ptr(&self, id: EnumId, value_addr: PointerValue<'ctx>) -> PointerValue<'ctx> {
        self.builder
            .build_struct_gep(self.enums[id.0].ty, value_addr, 0, "tag")
            .expect("an enum's struct should have the tag at element 0")
    }

    /// The tag of the enum `id` at `value_addr`.
    pub(super) fn load_tag(&self, id: EnumId, value_addr: PointerValue<'ctx>) -> IntValue<'ctx> {
        self.builder
            .build_load(self.enums[id.0].tag, self.tag_ptr(id, value_addr), "tag")
            .expect(POSITIONED)
            .into_int_value()
    }

    /// The slot that holds payload field `field` of variant `variant` in the enum `id` at
    /// `value_addr`, through the variant's struct laid over the payload storage: a box pointer
    /// for a boxed field.
    fn field_slot(
        &self,
        value_addr: PointerValue<'ctx>,
        id: EnumId,
        variant: usize,
        field: usize,
    ) -> PointerValue<'ctx> {
        let layout = &self.enums[id.0];
        let storage = self
            .builder
            .build_struct_gep(layout.ty, value_addr, 1, "payload")
            .expect("an enum with a payload field should have its payload storage at element 1");
        let index = u32::try_from(field).expect(
            "a variant should have fewer than 2^32 fields, since field indexes are `u32` in LLVM",
        );
        self.builder
            .build_struct_gep(layout.payloads[variant], storage, index, "field")
            .expect("the checker should only emit a payload field index that the variant has")
    }

    /// The box pointer of boxed payload field `field` of variant `variant` in the enum `id` at
    /// `value_addr`: `null` for a `None` of `F?`, and in the zero value that a `Move` of a
    /// flagged local leaves.
    fn load_box(
        &self,
        value_addr: PointerValue<'ctx>,
        id: EnumId,
        variant: usize,
        field: usize,
    ) -> PointerValue<'ctx> {
        self.builder
            .build_load(
                self.context.ptr_type(AddressSpace::default()),
                self.field_slot(value_addr, id, variant, field),
                "box",
            )
            .expect(POSITIONED)
            .into_pointer_value()
    }

    /// The address of the value in `boxed`, a box of the enum `inner`.
    fn box_value(&self, inner: EnumId, boxed: PointerValue<'ctx>) -> PointerValue<'ctx> {
        self.builder
            .build_struct_gep(self.enums[inner.0].boxed, boxed, 1, "boxed")
            .expect("a box's struct should have the value at element 1")
    }

    /// The address of the value of payload field `field` of variant `variant` in the enum `id`
    /// at `value_addr`: the field's slot when it is inline, the value in its box when it is a
    /// boxed `F`. Never for a boxed `F?`, whose box may be `null`: a pattern there is only a
    /// name or `_`, which `payload_value` reads.
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
            "a boxed `F?` field should have no place: the checker allows only a name or `_` there"
        );
        let boxed = self.load_box(value_addr, id, variant, field);
        self.box_value(boxed_enum(payload.ty), boxed)
    }

    /// The value of payload field `field` of variant `variant` in the enum `id` at
    /// `value_addr`, borrowed: not retained. A boxed `F` is the value in its box; a boxed `F?`
    /// is `{ false, zero }` when its box is `null` and `{ true, value }` otherwise, and never
    /// reads through a `null` box.
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
        let (true, TypeKind::Optional(inner)) = (payload.boxed, ty.kind()) else {
            let place = self.payload_place(value_addr, id, variant, field);
            return self
                .builder
                .build_load(llvm_ty, place, "field")
                .expect(POSITIONED);
        };
        let boxed = self.load_box(value_addr, id, variant, field);
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
        let (ty, tag) = (layout.ty, layout.tag);
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

    /// Stores at `place` a new box that holds `value`, of type `F`, or of `F?` when present. With
    /// a `token`, the box's block comes from `@sisu_rc.reuse.<F>`, and a flagged token's slot is
    /// left `null`.
    fn store_boxed(
        &self,
        place: PointerValue<'ctx>,
        value: BasicValueEnum<'ctx>,
        ty: Type,
        token: Option<LocalId>,
    ) {
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
    fn component(&self, id: EnumId) -> Option<Vec<EnumId>> {
        let enums = &self.program.enums;
        let cycle = enums[id.0].cycle?;
        Some(
            (0..enums.len())
                .filter(|&e| enums[e].cycle == Some(cycle))
                .map(EnumId)
                .collect(),
        )
    }

    /// The enum of payload field `payload` of the enum `id`, whose component has boxes, when the
    /// field is an inline link: not boxed, and of an enum in that component, or its optional.
    fn inline_link(&self, id: EnumId, payload: &Payload) -> Option<EnumId> {
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
        let heads = self.entry_alloca(heads_type.into(), "heads");
        self.store(heads, heads_type.const_zero().into());
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

    /// The pending lists of a component of `members` enums: one head per member, in
    /// declaration order, each `null` when its list is empty.
    fn heads_type(&self, members: usize) -> StructType<'ctx> {
        let ptr_type = self.context.ptr_type(AddressSpace::default()).into();
        self.context.struct_type(&vec![ptr_type; members], false)
    }

    /// The address of the head of member `index`'s list in `heads`, of type `heads_type`.
    fn head(
        &self,
        heads_type: StructType<'ctx>,
        heads: PointerValue<'ctx>,
        index: usize,
    ) -> PointerValue<'ctx> {
        let index = u32::try_from(index)
            .expect("a component should have fewer than 2^32 members, as it has as many heads");
        self.builder
            .build_struct_gep(heads_type, heads, index, "head")
            .expect("a component's heads should have one element per member")
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
                if !matches!(payload.ty.kind(), TypeKind::Optional(_)) {
                    self.call(walk, &[place.into(), heads.into()]);
                    return;
                }
                let (some, next) = (block("link"), block("link.next"));
                let link = self.load_link(payload.ty, place);
                self.branch_on(self.has_value(payload.ty, link, "flag"), some, next);
                self.builder.position_at_end(some);
                let inner_value = self.enum_slot(self.optional_value(payload.ty, link, "link"));
                self.call(walk, &[inner_value.into(), heads.into()]);
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
            let index = member_position(&members, boxed_enum(payload.ty));
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
            let &Projection::Payload { variant, field } = projection else {
                unreachable!("a take-apart's places should hold only payload projections")
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
            |variant, field, payload| {
                let pair = self.equal_at(
                    payload.ty,
                    self.payload_place(a, id, variant, field),
                    self.payload_place(b, id, variant, field),
                );
                let next = block("next");
                self.branch_on(pair, next, unequal);
                self.builder.position_at_end(next);
            },
        );
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
    fn load_link(&self, ty: Type, place: PointerValue<'ctx>) -> BasicValueEnum<'ctx> {
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
            let box_a = self.load_box(a, id, variant, field);
            let box_b = self.load_box(b, id, variant, field);
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
        if !matches!(ty.kind(), TypeKind::Optional(_)) {
            return (inner, place_a, place_b);
        }
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
        (
            inner,
            self.optional_value_place(ty, place_a),
            self.optional_value_place(ty, place_b),
        )
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

    /// The `kind` of `member` in a component of `members`: its index, as an `i32`.
    fn member_index(&self, members: &[EnumId], member: EnumId) -> IntValue<'ctx> {
        let index = u64::try_from(member_position(members, member))
            .expect("a member index should fit in a `u64`");
        self.context.i32_type().const_int(index, false)
    }

    /// `@sisu_rc.eq_component.<F0>(kind, a, b) -> i1`: `==` on the values at `a` and `b` of
    /// member `kind` of the component whose first member is `F0`. It switches on `kind`, then
    /// compares as `eq` does. It compares a link, a box or an inline link (`inline_link`) by
    /// the values it holds: see `link_pair`. It neither retains nor releases. It compares depth
    /// first. In each variant it compares the fields in order, but the last link comes after
    /// the fields that follow it. So a field after the last link is compared before a mismatch
    /// in that link, which a recursive `==` would find first. The last link loops: `kind`, `a`
    /// and `b` become that link's enum and its two values. Before a link that is not the last,
    /// it pushes where the value's comparison goes on (`push_pending`). Then it loops into the
    /// link. When a value is done, it pops that place and goes on there. So it compares any
    /// depth and shape of value in one frame. Every address it holds points into the compared
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
        let cases: Vec<_> = members
            .iter()
            .zip(&starts)
            .skip(1)
            .map(|(&m, &start)| (self.member_index(&members, m), start))
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
            // No variant has a link before its last, so nothing is ever pushed.
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
    fn is_link(&self, id: EnumId, payload: &Payload) -> bool {
        payload.boxed || self.inline_link(id, payload).is_some()
    }

    /// Compares the fields of variant `variant` of the enum `id`, in the loop `eq`, from the
    /// builder's block. Before each link that is not the last, it pushes the index of a new
    /// block in `resumes`, which goes on from the next field, and loops into the link. Then the
    /// last link loops, or with no link the value is done and goes to `pop`.
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
            .rposition(|payload| self.is_link(id, payload));
        for (field, payload) in payloads.iter().enumerate() {
            if looped == Some(field) {
                continue;
            }
            if !self.is_link(id, payload) {
                let next = self.context.append_basic_block(function, "next");
                let pair = self.equal_at(
                    payload.ty,
                    self.payload_place(a, id, variant, field),
                    self.payload_place(b, id, variant, field),
                );
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
    fn empty_pending(&self) -> Pending<'ctx> {
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
    fn push_pending(&self, pending: &Pending<'ctx>, pair: [BasicValueEnum<'ctx>; 3]) {
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
    fn pop_pending(
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
    fn free_pending(&self, pending: &Pending<'ctx>) {
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
    members: Vec<EnumId>,
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
struct Pending<'ctx> {
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
            let &Projection::Payload { variant, field } = projection else {
                unreachable!("a take-apart's places should hold only payload projections")
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
