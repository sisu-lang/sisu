//! Arrays at run time. An array is one heap block: a header of two `i64`s, the count and
//! the length, then the elements. `layout.rs` owns the layout; this module owns the array nodes
//! and the bodies of the array helpers, which `define_helpers` reaches through
//! `define_array_helper`.

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
    /// A block with count 1 and length `len`. A literal passes a constant length, and `unique`
    /// a run-time length that already passed the checks; `[v; n]` gets them at `span`. A length
    /// taken from an existing string, as `chars()` and `bytes()` do, needs no check.
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
                len,
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
        // Tagged like every array count and length (see `header_tag`).
        self.store_count(array, self.context.i64_type().const_int(1, false));
        self.store_tagged(self.len_ptr(array), len.into(), self.header_tag);
        array
    }

    /// The length of `array`, marked `!range [0, 2^63)` and `!noundef`: `alloc_array` is the only
    /// writer of a length, and no length it gets is negative. A new way to make an array must
    /// keep this true.
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

    /// Whether some element of `array`, whose element type is `element`, is `==` to `needle`,
    /// stopping at the first that is.
    pub(super) fn contains(
        &self,
        element: Type,
        array: PointerValue<'ctx>,
        needle: BasicValueEnum<'ctx>,
    ) -> IntValue<'ctx> {
        let function = self.current_function();
        let found = self.context.append_basic_block(function, "contains.found");
        let end = self.context.append_basic_block(function, "contains.end");
        self.count_loop(self.array_len(array), |index| {
            let same = self.equal(element, self.load_element(element, array, index), needle);
            let next = self.context.append_basic_block(function, "contains.next");
            self.builder
                .build_conditional_branch(same, found, next)
                .expect(POSITIONED);
            self.builder.position_at_end(next);
        });
        let missed = self.current_block();
        self.branch_to(end);
        self.builder.position_at_end(found);
        self.branch_to(end);
        self.builder.position_at_end(end);
        let bool_type = self.context.bool_type();
        self.phi(
            bool_type,
            &[
                (bool_type.const_zero(), missed),
                (bool_type.const_int(1, false), found),
            ],
        )
        .into_int_value()
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
    /// size recomputed from the length.
    fn define_array_drop(&self, element: Type, function: FunctionValue<'ctx>) {
        self.builder
            .position_at_end(self.context.append_basic_block(function, "entry"));
        let array = param(function);
        let len = self.array_len(array);
        if element.is_counted(&self.program.enums) {
            self.count_loop(len, |index| {
                self.release(self.load_element(element, array, index), element);
            });
        }
        let layout = self.array_layout(element);
        let (size, align) = self.heap_size(&HeapBlock::Array {
            layout: &layout,
            len,
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
        // count and length as the last stores, so that LLVM can forward them past an inlined
        // `unique` instead of loading them again. With the TBAA tags, that drops the second
        // count and bounds checks of a swap; see `a_barred_partition_makes_its_array_unique_once`.
        self.add_to_count(array, i64_type.const_int((-1_i64).cast_unsigned(), false));
        // The length fit once, so the new block needs no size check.
        let copy = self.alloc_array(element, len, None);
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
                self.element_ptr(element, copy, zero),
                align,
                self.element_ptr(element, array, zero),
                align,
                bytes,
            )
            .expect("the block's alignment should be a power of two, as each ABI alignment is");
        if element.is_counted(&self.program.enums) {
            self.count_loop(len, |index| {
                self.retain(self.load_element(element, copy, index), element);
            });
        }
        self.store(slot, copy.into());
        self.builder.build_return(Some(&copy)).expect(POSITIONED);
    }
}
