//! Arrays at run time. An array block is one heap block: a header of three `i64`s, the count,
//! the length and the capacity, then the elements. An array value is three words, `{ block,
//! data, len }`: the block, the first element and the number of elements (`layout.rs`,
//! `make_value`). A value that covers its whole block has `data` at the block's first element
//! and `len` equal to the header's `len`; every value has that shape for now. `layout.rs` owns
//! the layout; this module owns the array nodes and the bodies of the array helpers, which
//! `define_helpers` reaches through `define_array_helper`.

use inkwell::intrinsics::Intrinsic;
use inkwell::values::{BasicValueEnum, FunctionValue, IntValue, PointerValue};
use inkwell::{AddressSpace, IntPredicate};

use super::abi::{
    RANGE_END_PAST_LENGTH, RANGE_HAS_END, RANGE_INCLUSIVE, RANGE_INSIDE_CHARACTER,
    RANGE_NEGATIVE_START, RANGE_START_PAST_END, RANGE_STRING, RC_FAULT_RELEASE, RC_FAULT_RETAIN,
};
use super::layout::{BYTES, HeapBlock};
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
    /// A block with count 1, and length and capacity `len`; `array_value` makes the value that
    /// covers it. A literal passes a constant length,
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

    /// The length in the header of the block `array`, marked `!range [0, 2^63)` and `!noundef`.
    /// `alloc_array` and `array_push` are the only writers of a length, in a header or in a value.
    /// Neither writes a negative one. `array_push` stores `len + 1` only after it checks that
    /// this many elements fit. A new write of a length must keep this true.
    pub(super) fn header_len(&self, array: PointerValue<'ctx>) -> IntValue<'ctx> {
        let len = self
            .load_tagged(
                self.context.i64_type().into(),
                self.len_ptr(array),
                "len",
                self.header_tag,
            )
            .into_int_value();
        self.mark_len(len);
        len
    }

    /// The first element of the block `array` of `element`s: where the elements start.
    #[expect(
        unsafe_code,
        reason = "inkwell marks GEP unsafe for a wrong index list; this GEP takes one `i64` index"
    )]
    pub(super) fn array_data(
        &self,
        element: Type,
        array: PointerValue<'ctx>,
    ) -> PointerValue<'ctx> {
        let offset = self
            .context
            .i64_type()
            .const_int(self.array_layout(element).offset, false);
        // SAFETY: the GEP takes one `i64` index, which steps over whole `i8` values and never
        // indexes into an aggregate, so the index list is valid for its pointee type.
        // `inbounds` is sound because the block holds the header and then its elements, and
        // `offset` is where the elements start, which is at most one past the block's end.
        unsafe {
            self.builder
                .build_in_bounds_gep(self.context.i8_type(), array, &[offset], "data")
                .expect(POSITIONED)
        }
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

    /// The address of element `index` of the elements that start at `data`.
    #[expect(
        unsafe_code,
        reason = "inkwell marks GEP unsafe for a wrong index list; this GEP takes one `i64` index"
    )]
    pub(super) fn element_ptr(
        &self,
        element: Type,
        data: PointerValue<'ctx>,
        index: IntValue<'ctx>,
    ) -> PointerValue<'ctx> {
        let layout = self.array_layout(element);
        // SAFETY: the GEP takes one `i64` index, which steps over whole element values and
        // never indexes into an aggregate, so the index list is valid for its pointee type.
        // `inbounds` is sound because every caller passes an index below the length:
        // `check_bounds` proved it, a loop counts up to it, or `array_lit` stores at the
        // constant indexes `0..n` of a block it just sized to `n`. A caller may pass 0, whose
        // address is at most one past the block's end, which `inbounds` allows.
        unsafe {
            self.builder
                .build_in_bounds_gep(layout.element, data, &[index], "element")
                .expect(POSITIONED)
        }
    }

    /// The element at `index` of the elements that start at `data`, without retaining it.
    pub(super) fn load_element(
        &self,
        element: Type,
        data: PointerValue<'ctx>,
        index: IntValue<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        self.load_place(
            element,
            self.element_ptr(element, data, index),
            "element",
            Some(self.element_tag),
        )
    }

    /// Panics at `span` with `sisu_panic_bounds` when `index uge len`, which is
    /// `0 <= index < len` negated: one unsigned compare also catches a negative index.
    pub(super) fn check_bounds(&self, len: IntValue<'ctx>, index: IntValue<'ctx>, span: Span) {
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

    /// `base[start..end]`, or `..=` with `inclusive`, of the array or string `base` of type `ty`:
    /// the value with the same block, `data` moved to the start, and the length of the range. It
    /// takes `base`'s reference, so the block's count does not change, and it allocates nothing.
    /// A missing bound is `None`. With `s` the start (0 when missing) and `e` the end (the length
    /// when missing), the checks run in this order, and the first that fails panics at `span` with
    /// `sisu_panic_range`: `s < 0`; `e > len`, or for `..=` `b >= len`, compared before any
    /// `+ 1` so that `i64::MAX` does not overflow; `s > e`, where `e` is `b + 1` for `..=`; and on
    /// a string, the byte at `s` and then at `e`, each when below `len`: a continuation byte
    /// would cut a character. With `safe`, which gives the optional of `ty`, a check that fails
    /// releases `base` and gives `None`, and the value is `Some`, which holds the value itself.
    #[expect(
        clippy::too_many_lines,
        reason = "the checks of a range, in their order, read best in one place"
    )]
    pub(super) fn slice(
        &self,
        (base, ty): (BasicValueEnum<'ctx>, Type),
        (start, end): (Option<IntValue<'ctx>>, Option<IntValue<'ctx>>),
        (inclusive, safe): (bool, bool),
        span: Span,
    ) -> BasicValueEnum<'ctx> {
        let string = ty == Type::STRING;
        let i64_type = self.context.i64_type();
        let (block, data, len) = (
            self.value_block(base),
            self.value_data(base),
            self.value_len(base),
        );
        let kind = |check: u64| {
            let flags = [
                (inclusive, RANGE_INCLUSIVE),
                (end.is_some(), RANGE_HAS_END),
                (string, RANGE_STRING),
            ];
            let bits = flags
                .into_iter()
                .filter_map(|(on, bit)| on.then_some(bit))
                .fold(check, |kind, bit| kind | bit);
            i64_type.const_int(bits, false)
        };
        let from = start.unwrap_or_else(|| i64_type.const_zero());
        // What the program wrote as the end, for the text of the panic.
        let written = end.unwrap_or(len);
        // The stop for a failed check: a `safe` slice goes to `none`, any other panics at `span`.
        let function = self.current_function();
        let none = safe.then(|| self.context.append_basic_block(function, "slice.none"));
        let fail = |fails: IntValue<'ctx>, check: u64, first: IntValue<'ctx>| {
            if let Some(none) = none {
                let ok = self.context.append_basic_block(function, "slice.ok");
                self.branch_on(fails, none, ok);
                self.builder.position_at_end(ok);
                return;
            }
            self.panic_with(fails, |codegen| {
                let (at, at_len) = codegen.report_site(span);
                (
                    codegen.panic_range,
                    vec![
                        at.into(),
                        at_len.into(),
                        kind(check).into(),
                        first.into(),
                        written.into(),
                        len.into(),
                    ],
                )
            });
        };
        if start.is_some() {
            let negative = self.int_compare(IntPredicate::SLT, from, i64_type.const_zero());
            fail(negative, RANGE_NEGATIVE_START, from);
        }
        if let Some(end) = end {
            let predicate = if inclusive {
                IntPredicate::SGE
            } else {
                IntPredicate::SGT
            };
            let past = self.int_compare(predicate, end, len);
            fail(past, RANGE_END_PAST_LENGTH, from);
        }
        // `b < len` here for `..=`, so `b + 1` cannot overflow.
        let to = match end {
            Some(end) if inclusive => self
                .builder
                .build_int_nsw_add(end, i64_type.const_int(1, false), "end")
                .expect(POSITIONED),
            Some(end) => end,
            None => len,
        };
        let inverted = self.int_compare(IntPredicate::SGT, from, to);
        fail(inverted, RANGE_START_PAST_END, from);
        if string {
            // A missing bound is 0 or `len`, which are on a boundary. A position at `len` is too.
            for position in [start.map(|_| from), end.map(|_| to)].into_iter().flatten() {
                let peek = self.context.append_basic_block(function, "boundary.peek");
                let next = self.context.append_basic_block(function, "boundary.next");
                let below = self.int_compare(IntPredicate::ULT, position, len);
                self.branch_on(below, peek, next);
                self.builder.position_at_end(peek);
                let byte = self
                    .load(
                        self.context.i8_type().into(),
                        self.string_byte(data, position),
                    )
                    .into_int_value();
                // A continuation byte is `0b10xx_xxxx`.
                let top = self
                    .builder
                    .build_and(byte, self.context.i8_type().const_int(0xC0, false), "top")
                    .expect(POSITIONED);
                let cuts = self.int_compare(
                    IntPredicate::EQ,
                    top,
                    self.context.i8_type().const_int(0x80, false),
                );
                fail(cuts, RANGE_INSIDE_CHARACTER, position);
                self.branch_to(next);
                self.builder.position_at_end(next);
            }
        }
        let slice_len = self
            .builder
            .build_int_nuw_sub(to, from, "slice.len")
            .expect(POSITIONED);
        let first = if string {
            self.string_byte(data, from)
        } else {
            self.element_ptr(element_of(ty), data, from)
        };
        let value = self.make_value(block, first, slice_len);
        let Some(none) = none else {
            return value;
        };
        // `Some(value)` holds the value itself; `None` is the empty value of the optional.
        let some = self.current_block();
        let join = self.context.append_basic_block(function, "slice.join");
        self.branch_to(join);
        self.builder.position_at_end(none);
        self.release(base, ty);
        let gone = self.current_block();
        let empty = self.empty(Type::optional(ty));
        self.branch_to(join);
        self.builder.position_at_end(join);
        self.phi(self.value_type(), &[(value, some), (empty, gone)])
    }

    /// `Rvalue::Trim`: `value`, of the array or string type `ty` or one inside `?` at any depth,
    /// as it is stored inside another value. A `None` needs no check, and a layer of `?` is
    /// looked through, so the check runs on the array or string inside (see `trim_words`).
    pub(super) fn trim(&self, ty: Type, value: BasicValueEnum<'ctx>) -> BasicValueEnum<'ctx> {
        let TypeKind::Optional(payload) = ty.kind() else {
            return self.trim_words(ty, value);
        };
        let function = self.current_function();
        let (some, join) = (
            self.context.append_basic_block(function, "trim.some"),
            self.context.append_basic_block(function, "trim.join"),
        );
        let before = self.current_block();
        let present = self.has_value(ty, value, "present");
        self.branch_on(present, some, join);
        self.builder.position_at_end(some);
        let inner = self.optional_value(ty, value, "inner");
        let inner = self.trim(payload, inner);
        let wrapped = self.optional(ty, self.context.bool_type().const_all_ones(), inner);
        let placed = self.current_block();
        self.branch_to(join);
        self.builder.position_at_end(join);
        self.phi(value.get_type(), &[(value, before), (wrapped, placed)])
    }

    /// The trim of the array or string `value` of type `ty`, which has a block. The value is kept
    /// as it is, with no call, when it covers its block: its `data` is the block's first element,
    /// which needs no load, and its `len` is the header's. Otherwise `@sisu_rc.trim.[T]` decides
    /// (see `define_array_trim`) and returns the value or its copy.
    fn trim_words(&self, ty: Type, value: BasicValueEnum<'ctx>) -> BasicValueEnum<'ctx> {
        let (block, data, len) = (
            self.value_block(value),
            self.value_data(value),
            self.value_len(value),
        );
        let first = match ty.kind() {
            TypeKind::Array(element) => self.array_data(element, block),
            _ => self.string_field(block, BYTES),
        };
        let function = self.current_function();
        let named = |name| self.context.append_basic_block(function, name);
        let (whole, call, join) = (named("trim.whole"), named("trim.call"), named("trim.join"));
        let starts = self
            .builder
            .build_int_compare(IntPredicate::EQ, data, first, "at.start")
            .expect(POSITIONED);
        self.branch_on(starts, whole, call);

        self.builder.position_at_end(whole);
        let held = match ty.kind() {
            TypeKind::Array(_) => self.header_len(block),
            _ => self.string_len(block),
        };
        let covers = self.int_compare(IntPredicate::EQ, len, held);
        self.branch_on(covers, join, call);

        self.builder.position_at_end(call);
        let trimmed = self
            .builder
            .build_call(
                self.helper(Helper::Trim, ty),
                &self.split(ty, value),
                "trimmed",
            )
            .expect(POSITIONED)
            .try_as_basic_value()
            .basic()
            .expect("`@sisu_rc.trim.[T]` should return the value, as `helper` declares it");
        self.assume_len(ty, trimmed);
        self.branch_to(join);

        self.builder.position_at_end(join);
        self.phi(self.value_type(), &[(value, whole), (trimmed, call)])
    }

    /// The `keep` test of `@sisu_rc.trim.[T]` and `@sisu_rc.trim.string` for a value of `len`
    /// elements of `size` bytes in a block of capacity `cap`. The value is kept when its bytes
    /// are more than a quarter of the block's capacity in bytes, or that capacity is below 256
    /// bytes. The element size cancels in the first test: `len * 4 > cap`.
    pub(super) fn trim_keeps(
        &self,
        len: IntValue<'ctx>,
        cap: IntValue<'ctx>,
        size: u64,
    ) -> IntValue<'ctx> {
        // A block below this many bytes is kept: it holds too little to be worth a copy.
        const SMALL_BLOCK: u64 = 256;
        let i64_type = self.context.i64_type();
        // `cap * size < 256` is `cap < ceil(256 / size)`, with no multiplication to overflow.
        let few = i64_type.const_int(SMALL_BLOCK.div_ceil(size), false);
        let small = self.int_compare(IntPredicate::ULT, cap, few);
        let quarter = self
            .builder
            .build_right_shift(cap, i64_type.const_int(2, false), false, "quarter")
            .expect(POSITIONED);
        let big = self.int_compare(IntPredicate::UGT, len, quarter);
        self.builder.build_or(small, big, "keep").expect(POSITIONED)
    }

    /// `@sisu_rc.trim.[T](value)`: returns the value as it is when `trim_keeps` says so.
    /// Otherwise it returns the array whose block holds exactly the value's elements, copied with
    /// `llvm.memcpy` and each counted one retained. The call consumes the value: the old block
    /// loses its reference, and is freed with its elements when that was its last.
    fn define_array_trim(&self, element: Type, function: FunctionValue<'ctx>) {
        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);
        let ty = Type::array(element);
        let old = self.joined_params(function, 0, &[ty])[0];
        let (data, len) = (self.value_data(old), self.value_len(old));
        let cap = self.array_cap(self.value_block(old));
        let keep = self.trim_keeps(len, cap, self.array_layout(element).size);
        let (done, copying) = (
            self.context.append_basic_block(function, "keep"),
            self.context.append_basic_block(function, "copy"),
        );
        self.branch_on(keep, done, copying);
        self.builder.position_at_end(done);
        self.builder.build_return(Some(&old)).expect(POSITIONED);

        self.builder.position_at_end(copying);
        let copy = self.alloc_array(element, len, None);
        let copy_data = self.array_data(element, copy);
        self.copy_elements(element, data, copy_data, len);
        let trimmed = self.make_value(copy, copy_data, len);
        self.release(old, ty);
        self.builder.build_return(Some(&trimmed)).expect(POSITIONED);
    }

    /// `slot.push(value)`: appends `value`, which it consumes, to the array of type `ty` that
    /// `slot` holds. The value in `slot` has the block `array`, its `data` and its `len`, and the
    /// block has a header `len` of set elements and a capacity. There are three ways:
    /// 1. The count is 1, the value ends at the block's last set element (`data + len` is where
    ///    the header's `len` ends), and the header's `len` is below the capacity: the element
    ///    goes in place.
    /// 2. The count is 1, the value covers its block, and the block is full: the block is
    ///    reallocated to `max(len + 1, min(2 * cap, largest))` elements.
    /// 3. Otherwise the value is copied into a new block, and the old block is released. The new
    ///    block has `len + 1` elements of room when the value covers its old block, else
    ///    `max(len + 1, min(2 * (len + 1), largest))`, so that a queue that drops its front
    ///    and pushes at its back copies once in about `len` steps.
    ///
    /// Every way, the header's `len` is raised by one and ends equal to the set elements, and the
    /// value in `slot` ends one element longer. A block that has `largest` elements cannot take
    /// another: that panics at `span`. No way writes over an element that another value can see.
    #[expect(
        clippy::too_many_lines,
        reason = "the three paths of a push read best in one place"
    )]
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
        let old = self.load_place(ty, slot, "old", None);
        let (array, data, len) = (
            self.value_block(old),
            self.value_data(old),
            self.value_len(old),
        );
        // A dead block, count below 1, would otherwise pass for a shared one and be copied.
        self.check_count(array, RC_FAULT_RELEASE);
        let held = self.header_len(array);
        let first = self.array_data(element, array);
        // The set elements after the push: the header's `len` and one, or for a copy, which holds
        // only the value's elements, the value's `len` and one.
        let one = i64_type.const_int(1, false);
        let raised = self
            .builder
            .build_int_nuw_add(held, one, "held")
            .expect(POSITIONED);
        let new_len = self
            .builder
            .build_int_nuw_add(len, one, "len")
            .expect(POSITIONED);
        let function = self.current_function();
        let block = |name| self.context.append_basic_block(function, name);
        let (alone, ends, full, resize, copy, join) = (
            block("alone"),
            block("ends"),
            block("full"),
            block("resize"),
            block("copy"),
            block("join"),
        );
        let count = self.load_count(array, "count");
        let one = self.int_compare(IntPredicate::EQ, count, i64_type.const_int(1, false));
        self.builder
            .build_conditional_branch(one, alone, copy)
            .expect(POSITIONED);

        // Path 1 needs the value to end at the block's last set element.
        self.builder.position_at_end(alone);
        let value_end = self.element_ptr(element, data, len);
        let block_end = self.element_ptr(element, first, held);
        let at_end = self
            .builder
            .build_int_compare(IntPredicate::EQ, value_end, block_end, "at.end")
            .expect(POSITIONED);
        self.builder
            .build_conditional_branch(at_end, ends, copy)
            .expect(POSITIONED);

        self.builder.position_at_end(ends);
        let cap = self.array_cap(array);
        let room = self.int_compare(IntPredicate::ULT, held, cap);
        self.builder
            .build_conditional_branch(room, join, full)
            .expect(POSITIONED);

        // Path 2 needs the value to cover its block, which it ends already.
        self.builder.position_at_end(full);
        let starts = self
            .builder
            .build_int_compare(IntPredicate::EQ, data, first, "at.start")
            .expect(POSITIONED);
        self.builder
            .build_conditional_branch(starts, resize, copy)
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
        self.store_block(slot, element, grown);
        let grown_data = self.array_data(element, grown);
        let grew = self.current_block();
        self.branch_to(join);

        self.builder.position_at_end(copy);
        let too_large = self.int_compare(IntPredicate::UGE, len, largest);
        self.panic_if(too_large, span, "array is too large");
        let needed = self
            .builder
            .build_int_nuw_add(len, i64_type.const_int(1, false), "needed")
            .expect(POSITIONED);
        // A value that covers its block gets room for one element, as a copy always did. One that
        // does not gets room to grow in, up to twice its length, so that its pushes take path 1.
        let covers = self
            .builder
            .build_and(
                self.builder
                    .build_int_compare(IntPredicate::EQ, data, first, "at.start")
                    .expect(POSITIONED),
                self.int_compare(IntPredicate::EQ, len, held),
                "covers",
            )
            .expect(POSITIONED);
        // `needed` is at most `largest`, which is below 2^63, so the double fits.
        let double = self
            .builder
            .build_int_nuw_mul(needed, i64_type.const_int(2, false), "double")
            .expect(POSITIONED);
        let doubled = self.unsigned_extreme("llvm.umin", double, largest);
        let roomy = self.unsigned_extreme("llvm.umax", doubled, needed);
        let room = self
            .builder
            .build_select(covers, needed, roomy, "room")
            .expect(POSITIONED)
            .into_int_value();
        let fresh = self.alloc_array(element, room, None);
        let fresh_data = self.array_data(element, fresh);
        self.copy_elements(element, data, fresh_data, len);
        self.store_block(slot, element, fresh);
        // The copy holds its own references to the elements. The old block loses the slot's, and
        // is freed, with its elements, when that was its last.
        self.release(old, ty);
        let copied = self.current_block();
        self.branch_to(join);

        self.builder.position_at_end(join);
        let pointer = self.context.ptr_type(AddressSpace::default());
        let result = self.builder.build_phi(pointer, "array").expect(POSITIONED);
        result.add_incoming(&[(&array, ends), (&grown, grew), (&fresh, copied)]);
        // Where the value's elements start, in the block that now holds them.
        let start = self.builder.build_phi(pointer, "start").expect(POSITIONED);
        start.add_incoming(&[(&data, ends), (&grown_data, grew), (&fresh_data, copied)]);
        let set = self.builder.build_phi(i64_type, "set").expect(POSITIONED);
        set.add_incoming(&[(&raised, ends), (&raised, grew), (&new_len, copied)]);
        // The element goes after the value's last.
        self.store_tagged(
            self.element_ptr(element, start.as_basic_value().into_pointer_value(), len),
            value,
            self.element_tag,
        );
        self.store_tagged(
            self.len_ptr(result.as_basic_value().into_pointer_value()),
            set.as_basic_value(),
            self.header_tag,
        );
        self.store(self.words_ptr(slot, 2), new_len.into());
    }

    /// Stores the block `array` and its first element into the array value at `slot`. The length
    /// word stays: the caller stores it.
    fn store_block(&self, slot: PointerValue<'ctx>, element: Type, array: PointerValue<'ctx>) {
        self.store(self.words_ptr(slot, 0), array.into());
        self.store(
            self.words_ptr(slot, 1),
            self.array_data(element, array).into(),
        );
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

    /// The array literal of type `ty`, which takes each of the evaluated `values`.
    pub(super) fn array_lit(
        &self,
        ty: Type,
        values: &[BasicValueEnum<'ctx>],
    ) -> BasicValueEnum<'ctx> {
        let element = element_of(ty);
        let i64_type = self.context.i64_type();
        let len = u64::try_from(values.len())
            .expect("an array literal should have fewer than 2^64 elements");
        let len = i64_type.const_int(len, false);
        let array = self.alloc_array(element, len, None);
        let data = self.array_data(element, array);
        for (index, value) in (0..).zip(values) {
            let slot = self.element_ptr(element, data, i64_type.const_int(index, false));
            self.store_tagged(slot, *value, self.element_tag);
        }
        self.make_value(array, data, len)
    }

    /// `[filler; count]` of type `ty`; a negative or oversized `count` panics at
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
    ) -> BasicValueEnum<'ctx> {
        let element = element_of(ty);
        let array = self.alloc_array(element, count, Some(span));
        let data = self.array_data(element, array);
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
                self.element_ptr(element, data, index),
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
        self.make_value(array, data, count)
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
        let object = self.counted_pointer(ty, filler);
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
            Helper::Trim => self.define_array_trim(element, function),
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

    /// `@sisu_rc.drop.[T]`: releases each counted element in order, over the header's length from
    /// the block's first element, then frees the block, its size recomputed from the capacity.
    fn define_array_drop(&self, element: Type, function: FunctionValue<'ctx>) {
        self.builder
            .position_at_end(self.context.append_basic_block(function, "entry"));
        let exit = self.begin_drop(function, Type::intern(TypeKind::Array(element)));
        let array = param(function);
        if element.is_counted(&self.program.enums) {
            let data = self.array_data(element, array);
            self.count_loop(self.header_len(array), |index| {
                self.release(self.load_element(element, data, index), element);
            });
        }
        let layout = self.array_layout(element);
        let (size, align) = self.heap_size(&HeapBlock::Array {
            layout: &layout,
            cap: self.array_cap(array),
        });
        self.call(self.free, &[array.into(), size.into(), align.into()]);
        self.end_drop(exit);
    }

    /// `@sisu_rc.eq.[T](a, b)`: see `eq_entry` for the same block and `null`; otherwise the
    /// lengths, then each pair of elements by `T`'s `==`, up to the first difference.
    fn define_array_eq(&self, element: Type, function: FunctionValue<'ctx>) {
        let (a, b, equal, unequal) = self.eq_entry(
            function,
            Type::intern(TypeKind::Array(element)),
            self.reflexive(element),
        );
        let len = self.value_len(a);
        let same_len = self.int_compare(IntPredicate::EQ, len, self.value_len(b));
        let elements = self.context.append_basic_block(function, "elements");
        self.builder
            .build_conditional_branch(same_len, elements, unequal)
            .expect(POSITIONED);
        self.builder.position_at_end(elements);
        let (data_a, data_b) = (self.value_data(a), self.value_data(b));
        self.count_loop(len, |index| {
            let pair_equal = self.equal(
                element,
                self.load_element(element, data_a, index),
                self.load_element(element, data_b, index),
            );
            let next = self.context.append_basic_block(function, "next");
            self.builder
                .build_conditional_branch(pair_equal, next, unequal)
                .expect(POSITIONED);
            self.builder.position_at_end(next);
        });
        self.branch_to(equal);
    }

    /// `@sisu_rc.unique.[T](slot)`: leaves in `slot` an array whose block has count 1. When the
    /// count of the block is 1 already, that is the array. Otherwise the old block is released,
    /// and the value's elements go into a new block with count 1, the elements copied with
    /// `llvm.memcpy` and each counted one retained; the value in `slot` is then the one that
    /// covers the new block. It has no `null` early return: the slot is an `alloca`, a field
    /// address or an element address.
    fn define_array_unique(&self, element: Type, function: FunctionValue<'ctx>) {
        let block = |name| self.context.append_basic_block(function, name);
        let (entry, only, shared) = (block("entry"), block("only"), block("shared"));
        self.builder.position_at_end(entry);
        let slot = param(function);
        let i64_type = self.context.i64_type();
        let array = self.load_pointer(slot);
        // A dead block, count below 1, would otherwise pass for a shared one and be copied.
        self.check_count(array, RC_FAULT_RELEASE);
        let count = self.load_count(array, "count");
        let one = self.int_compare(IntPredicate::EQ, count, i64_type.const_int(1, false));
        self.builder
            .build_conditional_branch(one, only, shared)
            .expect(POSITIONED);
        self.builder.position_at_end(only);
        self.builder.build_return(None).expect(POSITIONED);
        self.builder.position_at_end(shared);
        let ty = Type::intern(TypeKind::Array(element));
        let old = self.load_place(ty, slot, "old", None);
        let (data, len) = (self.value_data(old), self.value_len(old));
        // The slot's reference moves to the copy. Releasing it never frees the old block: its
        // count was above 1. Releasing it before the copy exists, not after, leaves the copy's
        // count, length and capacity as the last stores, so that LLVM can forward them past an
        // inlined `unique` instead of loading them again. With the TBAA tags, that drops the
        // second count and bounds checks of a swap; see
        // `a_barred_partition_should_make_its_array_unique_once`.
        self.add_to_count(array, i64_type.const_int((-1_i64).cast_unsigned(), false));
        // The length fit once, so the new block needs no size check.
        let copy = self.alloc_array(element, len, None);
        let copy_data = self.array_data(element, copy);
        self.copy_elements(element, data, copy_data, len);
        self.store(slot, self.make_value(copy, copy_data, len));
        self.builder.build_return(None).expect(POSITIONED);
    }

    /// Copies the first `len` elements that start at `from` to `to`, which has room for them,
    /// with `llvm.memcpy`, and retains each counted one.
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
        // `from` and `to` each point at an element, so each is aligned as an element is.
        let align = self.target_data.get_abi_alignment(&layout.element);
        self.builder
            .build_memcpy(to, align, from, align, bytes)
            .expect("an element's alignment should be a power of two, as each ABI alignment is");
        if element.is_counted(&self.program.enums) {
            self.count_loop(len, |index| {
                self.retain(self.load_element(element, to, index), element);
            });
        }
    }
}
