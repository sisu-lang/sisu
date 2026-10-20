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
//! the box. So the stack stays the same for any depth and shape of value.

use std::num::NonZeroU32;

use inkwell::AddressSpace;
use inkwell::IntPredicate;
use inkwell::basic_block::BasicBlock;
use inkwell::types::{BasicTypeEnum, IntType, StructType};
use inkwell::values::{
    BasicMetadataValueEnum, BasicValueEnum, FunctionValue, IntValue, PointerValue,
};

use super::layout::HeapBlock;
use super::rc::{Helper, param};
use super::{Codegen, POSITIONED};
use crate::tir::{Enum, EnumId, Payload, Type, TypeKind};

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
    /// holder: post-order over the inline fields, with an explicit stack. The checker boxes
    /// every field that closes a cycle, so the inline fields form no cycle and the walk ends.
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
    /// box with count 1.
    pub(super) fn variant(
        &self,
        id: EnumId,
        variant: usize,
        values: &[BasicValueEnum<'ctx>],
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
        for (field, (&value, payload)) in values.iter().zip(fields).enumerate() {
            let place = self.field_slot(slot, id, variant, field);
            if payload.boxed {
                self.store_boxed(place, value, payload.ty);
            } else {
                self.store(place, value);
            }
        }
        self.builder
            .build_load(ty, slot, "variant")
            .expect(POSITIONED)
    }

    /// Stores at `place` a new box that holds `value`, of type `F`, or of `F?` when present.
    fn store_boxed(&self, place: PointerValue<'ctx>, value: BasicValueEnum<'ctx>, ty: Type) {
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
        let (size, align) = self.heap_size(&HeapBlock::Box(inner));
        let boxed = self
            .builder
            .build_call(self.alloc, &[size.into(), align.into()], "box")
            .expect(POSITIONED)
            .try_as_basic_value()
            .basic()
            .expect("`sisu_alloc` should return a pointer, as the runtime declares it")
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
    /// other field through `retain`.
    fn define_enum_retain(&self, id: EnumId, function: FunctionValue<'ctx>) {
        let (value, done) = self.enum_entry(function);
        self.for_each_counted_field(id, value, done, |variant, field, payload| {
            if payload.boxed {
                let boxed = self.load_box(value, id, variant, field);
                self.call(self.rc_retain, &[boxed.into()]);
            } else {
                self.retain(self.payload_value(value, id, variant, field), payload.ty);
            }
        });
    }

    /// `@sisu_rc.release.<E>(ptr)`: releases each counted part of the value at `ptr`. Without
    /// boxes, it switches on the tag and releases each counted field of the variant through
    /// `release`. With boxes, it walks the value with one empty pending list per member of
    /// `E`'s component, held in its own frame, then drains them. Releasing the zero value, which
    /// a `Move` of a flagged local leaves in a slot, releases nothing: it is variant 0 with
    /// every field zero, and `release` and `walk` skip a `null` string, object, array or box,
    /// an optional whose flag is clear, and a zero enum in turn.
    fn define_enum_release(&self, id: EnumId, function: FunctionValue<'ctx>) {
        let (value, done) = self.enum_entry(function);
        let Some(members) = self.component(id) else {
            self.for_each_counted_field(id, value, done, |variant, field, payload| {
                self.release(self.payload_value(value, id, variant, field), payload.ty);
            });
            return;
        };
        let heads_type = self.heads_type(members.len());
        let heads = self.entry_alloca(heads_type.into(), "heads");
        self.store(heads, heads_type.const_zero().into());
        let walk = self.helper(Helper::Walk, Type::intern(TypeKind::Enum(id)));
        self.call(walk, &[value.into(), heads.into()]);
        let drain = self.helper(Helper::Drain, Type::intern(TypeKind::Enum(members[0])));
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
    /// Nothing else reaches a listed box, so no RC check reads that word. Every other counted
    /// field goes through `release`, which may drain another component's lists in a frame of
    /// its own.
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
            if !payload.boxed {
                self.release(self.payload_value(value, id, variant, field), payload.ty);
                return;
            }
            let block = |name| self.context.append_basic_block(function, name);
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
            let walk = self.helper(Helper::Walk, Type::intern(TypeKind::Enum(member)));
            self.call(walk, &[self.box_value(member, boxed).into(), heads.into()]);
            let (size, align) = self.heap_size(&HeapBlock::Box(member));
            self.call(self.free, &[boxed.into(), size.into(), align.into()]);
            self.branch_to(next);
            self.builder.position_at_end(empty);
        }
        self.builder.build_return(None).expect(POSITIONED);
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
                Type::intern(TypeKind::Enum(members[0])),
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
                let pair = self.equal(
                    payload.ty,
                    self.payload_value(a, id, variant, field),
                    self.payload_value(b, id, variant, field),
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
    /// compares as `eq` does, with two boxes compared by their values: see `boxes_differ`. It
    /// neither retains nor releases. Each variant compares its last boxed field last, by
    /// looping: `kind`, `a` and `b` become that field's enum and its two boxes' values, so a
    /// right-deep value compares in one frame, also when the field's enum is another member.
    /// shortcut: other boxed fields recurse, so a left-deep tree can overflow the stack in ==;
    /// loop on the deeper side when that bites
    fn define_eq_component(&self, first: EnumId, function: FunctionValue<'ctx>) {
        let members = self.component(first).expect(
            "only the first member of a component with boxes should get an `eq_component` helper",
        );
        let block = |name| self.context.append_basic_block(function, name);
        let (entry, top) = (block("entry"), block("loop"));
        let (equal, unequal) = self.eq_returns(function);
        let nth = |n| {
            function.get_nth_param(n).expect(
                "the generated `eq_component` helper should be declared with three parameters",
            )
        };
        self.builder.position_at_end(entry);
        self.branch_to(top);
        self.builder.position_at_end(top);
        let ptr_type = self.context.ptr_type(AddressSpace::default());
        let phi = |ty: BasicTypeEnum<'ctx>, name, n| {
            let phi = self.builder.build_phi(ty, name).expect(POSITIONED);
            phi.add_incoming(&[(&nth(n), entry)]);
            phi
        };
        let kind = phi(self.context.i32_type().into(), "kind", 0);
        let (pa, pb) = (phi(ptr_type.into(), "a", 1), phi(ptr_type.into(), "b", 2));
        let (a, b) = (
            pa.as_basic_value().into_pointer_value(),
            pb.as_basic_value().into_pointer_value(),
        );
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
        self.builder
            .build_switch(kind.as_basic_value().into_int_value(), starts[0], &cases)
            .expect(POSITIONED);
        for (&id, &start) in members.iter().zip(&starts) {
            self.builder.position_at_end(start);
            let tag = self.load_tag(id, a);
            let same = self.int_compare(IntPredicate::EQ, tag, self.load_tag(id, b));
            let fields = block("fields");
            self.branch_on(same, fields, unequal);
            self.builder.position_at_end(fields);
            for (variant, at) in self.switch_variants(id, tag, equal, |_| true) {
                self.builder.position_at_end(at);
                let payloads = &self.program.enums[id.0].variants[variant].fields;
                let looped = payloads.iter().rposition(|payload| payload.boxed);
                for (field, payload) in payloads.iter().enumerate() {
                    if looped == Some(field) {
                        continue;
                    }
                    let next = block("next");
                    let pair = if payload.boxed {
                        let inner = boxed_enum(payload.ty);
                        let box_a = self.load_box(a, id, variant, field);
                        let box_b = self.load_box(b, id, variant, field);
                        self.boxes_differ(payload.ty, box_a, box_b, next, unequal);
                        let args = [
                            self.member_index(&members, inner).into(),
                            self.box_value(inner, box_a).into(),
                            self.box_value(inner, box_b).into(),
                        ];
                        self.call_component(function, &args)
                    } else {
                        self.equal(
                            payload.ty,
                            self.payload_value(a, id, variant, field),
                            self.payload_value(b, id, variant, field),
                        )
                    };
                    self.branch_on(pair, next, unequal);
                    self.builder.position_at_end(next);
                }
                let Some(field) = looped else {
                    self.branch_to(equal);
                    continue;
                };
                let inner = boxed_enum(payloads[field].ty);
                let box_a = self.load_box(a, id, variant, field);
                let box_b = self.load_box(b, id, variant, field);
                self.boxes_differ(payloads[field].ty, box_a, box_b, equal, unequal);
                let step = self.current_block();
                kind.add_incoming(&[(&self.member_index(&members, inner), step)]);
                pa.add_incoming(&[(&self.box_value(inner, box_a), step)]);
                pb.add_incoming(&[(&self.box_value(inner, box_b), step)]);
                self.branch_to(top);
            }
        }
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
