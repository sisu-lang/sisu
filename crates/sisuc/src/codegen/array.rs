//! Arrays at run time. An array is one heap block: a header of three `i64`s, the count, the
//! length and the capacity, then the elements. `layout.rs` owns the layout; this module owns the
//! array nodes and the bodies of the array helpers, which `define_helpers` reaches through
//! `define_array_helper`.

use inkwell::intrinsics::Intrinsic;
use inkwell::values::{BasicValue, BasicValueEnum, FunctionValue, IntValue, PointerValue};
use inkwell::{AddressSpace, IntPredicate};

use super::abi::{RC_FAULT_RELEASE, RC_FAULT_RETAIN};
use super::layout::HeapBlock;
use super::rc::{Helper, counted_core, param};
use super::{Codegen, POSITIONED};
use crate::source::Span;
use crate::tir::{Type, TypeKind};

/// The element type of `ty`, an array type.
pub(super) fn element_of(ty: Type) -> Type {
    let TypeKind::Array(element) = ty.kind() else {
        unreachable!("the checker should give an array node an array type")
    };
    element
}

impl<'ctx> Codegen<'ctx, '_> {
    /// A block with count 1, and length and capacity `len`. A literal passes a constant length,
    /// and `unique` a run-time length that already passed the checks; `[v; n]` gets them at
    /// `span`. A length taken from an existing string, as `chars()` and `bytes()` do, needs no
    /// check.
    pub(super) fn alloc_array(
        &self,
        element: Type,
        len: IntValue<'ctx>,
        span: Option<Span>,
    ) -> PointerValue<'ctx> {
        let layout = self.array_layout(element);
        let (size, align) = match span {
            Some(span) => self.checked_array_size(&layout, len, span),
            None => self.heap_size(&HeapBlock::Array {
                layout: &layout,
                cap: len,
            }),
        };
        let array = self
            .builder
            .build_call(self.alloc, &[size.into(), align.into()], "array")
            .expect(POSITIONED)
            .try_as_basic_value()
            .basic()
            .expect("`sisu_alloc` should return a pointer, as the runtime declares it")
            .into_pointer_value();
        // Tagged like every array count, length and capacity (see `header_tag`).
        self.store_count(array, self.context.i64_type().const_int(1, false));
        self.store_tagged(self.len_ptr(array), len.into(), self.header_tag);
        self.store_tagged(self.cap_ptr(array), len.into(), self.header_tag);
        array
    }

    /// The length of `array`, marked `!range [0, 2^63)` and `!noundef`: `alloc_array` and
    /// `array_push` are the only writers of a length, and no length either gets is negative:
    /// `array_push` stores `len + 1` only after it checked that this many elements fit. A new way
    /// to make an array or to change its length must keep this true.
    pub(super) fn array_len(&self, array: PointerValue<'ctx>) -> IntValue<'ctx> {
        let i64_type = self.context.i64_type();
        let len = self
            .load_tagged(i64_type.into(), self.len_ptr(array), "len", self.header_tag)
            .into_int_value();
        let load = len
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
        len
    }

    /// The capacity of `array`: the number of elements its block has room for, which is at
    /// least the length.
    fn array_cap(&self, array: PointerValue<'ctx>) -> IntValue<'ctx> {
        self.load_tagged(
            self.context.i64_type().into(),
            self.cap_ptr(array),
            "cap",
            self.header_tag,
        )
        .into_int_value()
    }

    #[expect(
        unsafe_code,
        reason = "inkwell marks GEP unsafe for a wrong index list; each GEP here takes one `i64` index"
    )]
    pub(super) fn element_ptr(
        &self,
        element: Type,
        array: PointerValue<'ctx>,
        index: IntValue<'ctx>,
    ) -> PointerValue<'ctx> {
        let layout = self.array_layout(element);
        let i64_type = self.context.i64_type();
        let offset = i64_type.const_int(layout.offset, false);
        // SAFETY: the GEP takes one `i64` index, which steps over whole `i8` values and never
        // indexes into an aggregate, so the index list is valid for its pointee type.
        // `inbounds` is sound because the block holds the header and then its elements, and
        // `offset` is where the elements start, which is at most one past the block's end.
        let elements = unsafe {
            self.builder
                .build_in_bounds_gep(self.context.i8_type(), array, &[offset], "elements")
                .expect(POSITIONED)
        };
        // SAFETY: the GEP takes one `i64` index, which steps over whole element values and
        // never indexes into an aggregate, so the index list is valid for its pointee type.
        // `inbounds` is sound because every caller passes an index below the length:
        // `check_bounds` proved it, a loop counts up to it, or `array_lit` stores at the
        // constant indexes `0..n` of a block it just sized to `n`. `unique` passes 0, whose
        // address is at most one past the block's end, which `inbounds` allows.
        unsafe {
            self.builder
                .build_in_bounds_gep(layout.element, elements, &[index], "element")
                .expect(POSITIONED)
        }
    }

    /// The element at `index`, without retaining it.
    pub(super) fn load_element(
        &self,
        element: Type,
        array: PointerValue<'ctx>,
        index: IntValue<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let layout = self.array_layout(element);
        self.load_tagged(
            layout.element,
            self.element_ptr(element, array, index),
            "element",
            self.element_tag,
        )
    }

    /// Panics at `span` with `sisu_panic_bounds` when `index uge len`, which is
    /// `0 <= index < len` negated: one unsigned compare also catches a negative index.
    pub(super) fn check_bounds(
        &self,
        array: PointerValue<'ctx>,
        index: IntValue<'ctx>,
        span: Span,
    ) {
        let len = self.array_len(array);
        let outside = self.int_compare(IntPredicate::UGE, index, len);
        self.panic_with(outside, |codegen| {
            let (at, at_len) = codegen.report_site(span);
            (
                codegen.panic_bounds,
                vec![at.into(), at_len.into(), index.into(), len.into()],
            )
        });
    }

    /// Runs `body` for each index from 0 up to `len`, which is not negative. `body` may leave
    /// the builder in another block; the loop goes on from there.
    pub(super) fn count_loop(&self, len: IntValue<'ctx>, mut body: impl FnMut(IntValue<'ctx>)) {
        let function = self.current_function();
        let before = self.current_block();
        let head = self.context.append_basic_block(function, "array.head");
        let step = self.context.append_basic_block(function, "array.body");
        let done = self.context.append_basic_block(function, "array.done");
        self.branch_to(head);
        self.builder.position_at_end(head);
        let i64_type = self.context.i64_type();
        let phi = self.builder.build_phi(i64_type, "i").expect(POSITIONED);
        phi.add_incoming(&[(&i64_type.const_zero(), before)]);
        let index = phi.as_basic_value().into_int_value();
        let more = self.int_compare(IntPredicate::ULT, index, len);
        self.builder
            .build_conditional_branch(more, step, done)
            .expect(POSITIONED);
        self.builder.position_at_end(step);
        body(index);
        let next = self
            .builder
            .build_int_nuw_add(index, i64_type.const_int(1, false), "next")
            .expect(POSITIONED);
        phi.add_incoming(&[(&next, self.current_block())]);
        self.branch_to(head);
        self.builder.position_at_end(done);
    }

    /// `slot.push(value)`: appends `value`, which it consumes, to the array of type `ty` that
    /// `slot` holds. A block with count 1 and room for the element takes it in place. A full
    /// one is reallocated to `max(len + 1, min(2 * cap, largest))` elements, as a string grows.
    /// A shared one is copied into a block of `len + 1` elements, and the slot's reference to
    /// the old block is released. A block that grew or was copied is stored back into `slot`. A
    /// block that has `largest` elements cannot take another: that panics at `span`.
    pub(super) fn array_push(
        &self,
        slot: PointerValue<'ctx>,
        ty: Type,
        value: BasicValueEnum<'ctx>,
        span: Span,
    ) {
        let element = element_of(ty);
        let i64_type = self.context.i64_type();
        let layout = self.array_layout(element);
        let largest = i64_type.const_int(layout.largest(), false);
        let array = self.load_pointer(slot);
        // A dead block, count below 1, would otherwise pass for a shared one and be copied.
        self.check_count(array, RC_FAULT_RELEASE);
        let len = self.array_len(array);
        let function = self.current_function();
        let block = |name| self.context.append_basic_block(function, name);
        let (owned, resize, copy, join) = (
            block("owned"),
            block("resize"),
            block("copy"),
            block("join"),
        );
        let count = self.load_count(array, "count");
        let one = self.int_compare(IntPredicate::EQ, count, i64_type.const_int(1, false));
        self.builder
            .build_conditional_branch(one, owned, copy)
            .expect(POSITIONED);

        self.builder.position_at_end(owned);
        let cap = self.array_cap(array);
        let full = self.int_compare(IntPredicate::UGE, len, cap);
        self.builder
            .build_conditional_branch(full, resize, join)
            .expect(POSITIONED);

        self.builder.position_at_end(resize);
        let too_large = self.int_compare(IntPredicate::UGE, len, largest);
        self.panic_if(too_large, span, "array is too large");
        let needed = self
            .builder
            .build_int_nuw_add(len, i64_type.const_int(1, false), "needed")
            .expect(POSITIONED);
        // `cap` is at most `largest`, which is below 2^63, so the double fits.
        let double = self
            .builder
            .build_int_nuw_mul(cap, i64_type.const_int(2, false), "double")
            .expect(POSITIONED);
        let doubled = self.unsigned_extreme("llvm.umin", double, largest);
        let new_cap = self.unsigned_extreme("llvm.umax", doubled, needed);
        let (old_size, align) = self.heap_size(&HeapBlock::Array {
            layout: &layout,
            cap,
        });
        let (new_size, _) = self.heap_size(&HeapBlock::Array {
            layout: &layout,
            cap: new_cap,
        });
        let grown = self
            .builder
            .build_call(
                self.realloc(),
                &[array.into(), old_size.into(), align.into(), new_size.into()],
                "grown",
            )
            .expect(POSITIONED)
            .try_as_basic_value()
            .basic()
            .expect("`sisu_realloc` should return a pointer, as the runtime declares it")
            .into_pointer_value();
        self.store_tagged(self.cap_ptr(grown), new_cap.into(), self.header_tag);
        self.store(slot, grown.into());
        let grew = self.current_block();
        self.branch_to(join);

        self.builder.position_at_end(copy);
        let too_large = self.int_compare(IntPredicate::UGE, len, largest);
        self.panic_if(too_large, span, "array is too large");
        let needed = self
            .builder
            .build_int_nuw_add(len, i64_type.const_int(1, false), "needed")
            .expect(POSITIONED);
        // Releasing the slot's reference never frees the block: its count was above 1.
        self.add_to_count(array, i64_type.const_int((-1_i64).cast_unsigned(), false));
        let fresh = self.alloc_array(element, needed, None);
        self.copy_elements(element, array, fresh, len);
        self.store(slot, fresh.into());
        let copied = self.current_block();
        self.branch_to(join);

        self.builder.position_at_end(join);
        let result = self
            .builder
            .build_phi(self.context.ptr_type(AddressSpace::default()), "array")
            .expect(POSITIONED);
        result.add_incoming(&[(&array, owned), (&grown, grew), (&fresh, copied)]);
        let result = result.as_basic_value().into_pointer_value();
        self.store_tagged(
            self.element_ptr(element, result, len),
            value,
            self.element_tag,
        );
        let new_len = self
            .builder
            .build_int_nuw_add(len, i64_type.const_int(1, false), "len")
            .expect(POSITIONED);
        self.store_tagged(self.len_ptr(result), new_len.into(), self.header_tag);
    }

    /// `intrinsic` (`llvm.umin` or `llvm.umax`) of the `i64`s `a` and `b`.
    fn unsigned_extreme(
        &self,
        intrinsic: &str,
        a: IntValue<'ctx>,
        b: IntValue<'ctx>,
    ) -> IntValue<'ctx> {
        let function = Intrinsic::find(intrinsic)
            .expect("LLVM should provide the `llvm.umin` and `llvm.umax` intrinsics by name")
            .get_declaration(&self.module, &[a.get_type().into()])
            .expect("the `umin` and `umax` intrinsics should be overloaded on any integer width");
        self.builder
            .build_call(function, &[a.into(), b.into()], "extreme")
            .expect(POSITIONED)
            .try_as_basic_value()
            .basic()
            .expect("`umin` and `umax` should return an integer")
            .into_int_value()
    }

    /// The block of an array literal of type `ty`, which takes each of the evaluated `values`.
    pub(super) fn array_lit(
        &self,
        ty: Type,
        values: &[BasicValueEnum<'ctx>],
    ) -> PointerValue<'ctx> {
        let element = element_of(ty);
        let i64_type = self.context.i64_type();
        let len = u64::try_from(values.len())
            .expect("an array literal should have fewer than 2^64 elements");
        let array = self.alloc_array(element, i64_type.const_int(len, false), None);
        for (index, value) in (0..).zip(values) {
            let slot = self.element_ptr(element, array, i64_type.const_int(index, false));
            self.store_tagged(slot, *value, self.element_tag);
        }
        array
    }

    /// The block of `[filler; count]` of type `ty`; a negative or oversized `count` panics at
    /// `span`. A counted pointer `filler` gains `count - 1` references in one step, or loses its
    /// one when `count` is 0. A counted enum `filler` is retained once per slot, then released.
    /// A `bool` filler is stored as a byte, so that `-O2` makes the loop a `memset`: LLVM's
    /// loop-idiom pass does not do that for an `i1` store.
    pub(super) fn array_fill(
        &self,
        filler: BasicValueEnum<'ctx>,
        count: IntValue<'ctx>,
        ty: Type,
        span: Span,
    ) -> PointerValue<'ctx> {
        let element = element_of(ty);
        let array = self.alloc_array(element, count, Some(span));
        let stored = if element == Type::BOOL {
            self.builder
                .build_int_z_extend(filler.into_int_value(), self.context.i8_type(), "fill.byte")
                .expect(POSITIONED)
                .into()
        } else {
            filler
        };
        self.count_loop(count, |index| {
            self.store_tagged(
                self.element_ptr(element, array, index),
                stored,
                self.element_tag,
            );
        });
        if counted_core(element).is_some() {
            self.share_filler(filler, element, count);
        } else if element.is_counted(&self.program.enums) {
            self.count_loop(count, |_| self.retain(filler, element));
            self.release(filler, element);
        }
        array
    }

    /// Gives `filler`, stored into `count` slots, one reference per slot: adds `count - 1`
    /// when it is an object or array and `count > 0`, or releases it when `count` is 0.
    fn share_filler(&self, filler: BasicValueEnum<'ctx>, ty: Type, count: IntValue<'ctx>) {
        let i64_type = self.context.i64_type();
        let function = self.current_function();
        let block = |name| self.context.append_basic_block(function, name);
        let (empty, nonzero, shared, done) = (
            block("fill.empty"),
            block("fill.nonzero"),
            block("fill.share"),
            block("fill.done"),
        );
        let zero = self.int_compare(IntPredicate::EQ, count, i64_type.const_zero());
        self.builder
            .build_conditional_branch(zero, empty, nonzero)
            .expect(POSITIONED);
        self.builder.position_at_end(empty);
        self.release(filler, ty);
        self.branch_to(done);
        self.builder.position_at_end(nonzero);
        let object = filler.into_pointer_value();
        let null = self
            .builder
            .build_is_null(object, "null")
            .expect(POSITIONED);
        self.builder
            .build_conditional_branch(null, done, shared)
            .expect(POSITIONED);
        self.builder.position_at_end(shared);
        self.check_count(object, RC_FAULT_RETAIN);
        let extra = self
            .builder
            .build_int_sub(count, i64_type.const_int(1, false), "extra")
            .expect(POSITIONED);
        self.add_to_count(object, extra);
        self.branch_to(done);
        self.builder.position_at_end(done);
    }

    /// The length of `array`, of type `ty`, an array or a string.
    pub(super) fn len(&self, array: PointerValue<'ctx>, ty: Type) -> IntValue<'ctx> {
        if ty == Type::STRING {
            self.string_len(array)
        } else {
            self.array_len(array)
        }
    }

    /// The body of `@sisu_rc.drop.[T]`, `@sisu_rc.eq.[T]` or `@sisu_rc.unique.[T]`, `T`
    /// being `element`.
    pub(super) fn define_array_helper(
        &self,
        kind: Helper,
        element: Type,
        function: FunctionValue<'ctx>,
    ) {
        match kind {
            Helper::Drop => self.define_array_drop(element, function),
            Helper::Eq => self.define_array_eq(element, function),
            Helper::Unique => self.define_array_unique(element, function),
            Helper::Retain
            | Helper::Release
            | Helper::Walk
            | Helper::Drain
            | Helper::EqComponent
            | Helper::Show(_)
            | Helper::ShowComponent(_)
            | Helper::Unbox
            | Helper::Free
            | Helper::Reuse => {
                unreachable!(
                    "`define_helpers` should define every `release` itself, and queue `retain`, the component helpers and a box's only for an enum"
                )
            }
        }
    }

    /// `@sisu_rc.drop.[T]`: releases each counted element in order, then frees the block, its
    /// size recomputed from the capacity.
    fn define_array_drop(&self, element: Type, function: FunctionValue<'ctx>) {
        self.builder
            .position_at_end(self.context.append_basic_block(function, "entry"));
        let array = param(function);
        if element.is_counted(&self.program.enums) {
            self.count_loop(self.array_len(array), |index| {
                self.release(self.load_element(element, array, index), element);
            });
        }
        let layout = self.array_layout(element);
        let (size, align) = self.heap_size(&HeapBlock::Array {
            layout: &layout,
            cap: self.array_cap(array),
        });
        self.call(self.free, &[array.into(), size.into(), align.into()]);
        self.builder.build_return(None).expect(POSITIONED);
    }

    /// `@sisu_rc.eq.[T](a, b)`: see `eq_entry` for the same pointer and `null`; otherwise the
    /// lengths, then each pair of elements by `T`'s `==`, up to the first difference.
    fn define_array_eq(&self, element: Type, function: FunctionValue<'ctx>) {
        let (a, b, equal, unequal) = self.eq_entry(function, self.reflexive(element));
        let len = self.array_len(a);
        let same_len = self.int_compare(IntPredicate::EQ, len, self.array_len(b));
        let elements = self.context.append_basic_block(function, "elements");
        self.builder
            .build_conditional_branch(same_len, elements, unequal)
            .expect(POSITIONED);
        self.builder.position_at_end(elements);
        self.count_loop(len, |index| {
            let pair_equal = self.equal(
                element,
                self.load_element(element, a, index),
                self.load_element(element, b, index),
            );
            let next = self.context.append_basic_block(function, "next");
            self.builder
                .build_conditional_branch(pair_equal, next, unequal)
                .expect(POSITIONED);
            self.builder.position_at_end(next);
        });
        self.branch_to(equal);
    }

    /// `@sisu_rc.unique.[T](slot) -> ptr`: the array in `slot` when its count is 1. Otherwise
    /// the old block is released, and a new block of the same length with count 1, the
    /// elements copied with `llvm.memcpy` and each counted one retained, is stored into
    /// `slot`. It has no `null` early return: the slot is an `alloca`, a field address or an
    /// element address.
    fn define_array_unique(&self, element: Type, function: FunctionValue<'ctx>) {
        let block = |name| self.context.append_basic_block(function, name);
        let (entry, only, shared) = (block("entry"), block("only"), block("shared"));
        self.builder.position_at_end(entry);
        let slot = param(function);
        let i64_type = self.context.i64_type();
        let array = self
            .builder
            .build_load(
                self.context.ptr_type(AddressSpace::default()),
                slot,
                "array",
            )
            .expect(POSITIONED)
            .into_pointer_value();
        // A dead block, count below 1, would otherwise pass for a shared one and be copied.
        self.check_count(array, RC_FAULT_RELEASE);
        let count = self.load_count(array, "count");
        let one = self.int_compare(IntPredicate::EQ, count, i64_type.const_int(1, false));
        self.builder
            .build_conditional_branch(one, only, shared)
            .expect(POSITIONED);
        self.builder.position_at_end(only);
        self.builder.build_return(Some(&array)).expect(POSITIONED);
        self.builder.position_at_end(shared);
        let len = self.array_len(array);
        // The slot's reference moves to the copy. Releasing it never frees the old block: its
        // count was above 1. Releasing it before the copy exists, not after, leaves the copy's
        // count, length and capacity as the last stores, so that LLVM can forward them past an
        // inlined `unique` instead of loading them again. With the TBAA tags, that drops the
        // second count and bounds checks of a swap; see
        // `a_barred_partition_makes_its_array_unique_once`.
        self.add_to_count(array, i64_type.const_int((-1_i64).cast_unsigned(), false));
        // The length fit once, so the new block needs no size check.
        let copy = self.alloc_array(element, len, None);
        self.copy_elements(element, array, copy, len);
        self.store(slot, copy.into());
        self.builder.build_return(Some(&copy)).expect(POSITIONED);
    }

    /// Copies the first `len` elements of the block `from` into the block `to`, which has room
    /// for them, with `llvm.memcpy`, and retains each counted one.
    fn copy_elements(
        &self,
        element: Type,
        from: PointerValue<'ctx>,
        to: PointerValue<'ctx>,
        len: IntValue<'ctx>,
    ) {
        let i64_type = self.context.i64_type();
        let layout = self.array_layout(element);
        let bytes = self
            .builder
            .build_int_nuw_mul(len, i64_type.const_int(layout.size, false), "bytes")
            .expect(POSITIONED);
        // The elements start at an offset that is a multiple of the block's alignment.
        let align = u32::try_from(layout.align)
            .expect("the block's alignment should fit in a `u32`, as each ABI alignment does");
        let zero = i64_type.const_zero();
        self.builder
            .build_memcpy(
                self.element_ptr(element, to, zero),
                align,
                self.element_ptr(element, from, zero),
                align,
                bytes,
            )
            .expect("the block's alignment should be a power of two, as each ABI alignment is");
        if element.is_counted(&self.program.enums) {
            self.count_loop(len, |index| {
                self.retain(self.load_element(element, to, index), element);
            });
        }
    }
}
