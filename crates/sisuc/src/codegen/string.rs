//! Strings at run time. A string is a pointer to one block: a header of three `i64`s, the count,
//! the length and the capacity, then `cap` bytes. A literal's block is a writable global that
//! holds a reference of its own, so it is never freed and never reused in place. This module is the
//! only one that reads a string header; `layout.rs` owns the layout. This module owns the string
//! nodes and the bodies of the string helpers, which `define_helpers` reaches through
//! `define_string_helper`.

use inkwell::attributes::AttributeLoc;
use inkwell::module::Linkage;
use inkwell::values::{
    BasicMetadataValueEnum, BasicValueEnum, FunctionValue, IntValue, PointerValue,
};
use inkwell::{AddressSpace, IntPredicate};

use super::abi::{PAD_CENTER, PAD_LEFT, PAD_RIGHT};
use super::layout::{BYTES, CAP, HeapBlock, LEN};
use super::rc::{Helper, param};
use super::{Codegen, POSITIONED, zeroext};
use crate::source::Span;
use crate::tir::{Align, Format, Pad, Stream, Type};

impl<'ctx> Codegen<'ctx, '_> {
    /// The `i64` header field `index` of `string`.
    fn load_string_field(&self, string: PointerValue<'ctx>, index: u32) -> IntValue<'ctx> {
        self.builder
            .build_load(
                self.context.i64_type(),
                self.string_field(string, index),
                "field",
            )
            .expect(POSITIONED)
            .into_int_value()
    }

    /// The length of `string` in bytes.
    pub(super) fn string_len(&self, string: PointerValue<'ctx>) -> IntValue<'ctx> {
        self.load_string_field(string, LEN)
    }

    /// `Str`: `string_global` for this literal node, retained.
    pub(super) fn string_lit(&self, text: &str) -> PointerValue<'ctx> {
        let string = self.string_global(text);
        self.retain(string.into(), Type::STRING);
        string
    }

    /// A private global `{ i64 1, i64 len, i64 len, [len x i8] }` holding `text`, borrowed. The
    /// global holds a reference of its own, so its count never reaches 0.
    pub(super) fn string_global(&self, text: &str) -> PointerValue<'ctx> {
        let layout = self.string_layout();
        let i64_type = self.context.i64_type();
        let len = i64_type.const_int(
            u64::try_from(text.len()).expect("a literal should be shorter than 2^64 bytes"),
            false,
        );
        let block = self.context.const_struct(
            &[
                i64_type.const_int(1, false).into(),
                len.into(),
                len.into(),
                self.context.const_string(text.as_bytes(), false).into(),
            ],
            false,
        );
        let global = self.module.add_global(block.get_type(), None, "str");
        global.set_initializer(&block);
        // Retain and release write the count, so the block is not an LLVM `constant`.
        global.set_constant(false);
        global.set_alignment(layout.align);
        global.set_linkage(Linkage::Private);
        global.as_pointer_value()
    }

    /// The address of byte `offset` of `string`, which is at most its capacity.
    #[expect(
        unsafe_code,
        reason = "inkwell marks GEP unsafe for a wrong index list; this GEP takes one `i64` index"
    )]
    fn string_byte(
        &self,
        string: PointerValue<'ctx>,
        offset: IntValue<'ctx>,
    ) -> PointerValue<'ctx> {
        // SAFETY: the one `i64` index steps over `i8`s, so the index list is valid for its
        // pointee type. `inbounds` is sound because the block holds `cap` bytes after the
        // header and every caller passes an offset of at most `cap`, one past the end at most.
        unsafe {
            self.builder
                .build_in_bounds_gep(
                    self.context.i8_type(),
                    self.string_field(string, BYTES),
                    &[offset],
                    "byte",
                )
                .expect(POSITIONED)
        }
    }

    /// `sisu_realloc(ptr, old_size, align, new_size) -> ptr` from the runtime, declared on
    /// first use, so that a module without `+` stays as it was.
    fn realloc(&self) -> FunctionValue<'ctx> {
        self.runtime("sisu_realloc")
    }

    /// `lhs + rhs`, which takes both owned references and returns an owned one. When
    /// the count of `lhs` is 1, the bytes of `rhs` are appended into its block, which is first
    /// reallocated to `max(needed, 2 * cap)` when its capacity is below the `needed` length.
    /// Otherwise a new block with capacity `needed` receives both byte runs, and `lhs` is
    /// released. Either way `rhs` is released last.
    pub(super) fn concat(
        &self,
        lhs: PointerValue<'ctx>,
        rhs: PointerValue<'ctx>,
    ) -> PointerValue<'ctx> {
        let i64_type = self.context.i64_type();
        let (len, rhs_len) = (self.string_len(lhs), self.string_len(rhs));
        // Both strings exist in memory, which on x86-64 and aarch64 Linux is below 2^57 bytes,
        // so no length, capacity or block size here can overflow.
        let needed = self
            .builder
            .build_int_nuw_add(len, rhs_len, "needed")
            .expect(POSITIONED);
        let function = self.current_function();
        let block = |name| self.context.append_basic_block(function, name);
        let (owned, grow, copy, join) =
            (block("owned"), block("grow"), block("copy"), block("join"));
        let count = self.load_count(lhs, "count");
        let one = self.int_compare(IntPredicate::EQ, count, i64_type.const_int(1, false));
        self.builder
            .build_conditional_branch(one, owned, copy)
            .expect(POSITIONED);

        // Both operands are owned references, so with a count of 1 `lhs` is not `rhs`'s block:
        // reallocating it leaves `rhs` in place.
        self.builder.position_at_end(owned);
        let cap = self.load_string_field(lhs, CAP);
        let full = self.int_compare(IntPredicate::ULT, cap, needed);
        self.builder
            .build_conditional_branch(full, grow, join)
            .expect(POSITIONED);

        self.builder.position_at_end(grow);
        let double = self
            .builder
            .build_int_nuw_mul(cap, i64_type.const_int(2, false), "double")
            .expect(POSITIONED);
        let more = self.int_compare(IntPredicate::UGT, needed, double);
        let new_cap = self
            .builder
            .build_select(more, needed, double, "cap")
            .expect(POSITIONED)
            .into_int_value();
        let (old_size, align) = self.heap_size(&HeapBlock::String { cap });
        let (new_size, _) = self.heap_size(&HeapBlock::String { cap: new_cap });
        let grown = self
            .builder
            .build_call(
                self.realloc(),
                &[lhs.into(), old_size.into(), align.into(), new_size.into()],
                "grown",
            )
            .expect(POSITIONED)
            .try_as_basic_value()
            .basic()
            .expect("`sisu_realloc` should return a pointer, as the runtime declares it")
            .into_pointer_value();
        self.store(self.string_field(grown, CAP), new_cap.into());
        self.branch_to(join);

        self.builder.position_at_end(copy);
        let fresh = self.new_string(needed);
        self.copy_bytes(self.string_field(fresh, BYTES), lhs, len);
        self.release(lhs.into(), Type::STRING);
        let copied = self.current_block();
        self.branch_to(join);

        self.builder.position_at_end(join);
        let result = self
            .builder
            .build_phi(self.context.ptr_type(AddressSpace::default()), "string")
            .expect(POSITIONED);
        result.add_incoming(&[(&lhs, owned), (&grown, grow), (&fresh, copied)]);
        let result = result.as_basic_value().into_pointer_value();
        self.copy_bytes(self.string_byte(result, len), rhs, rhs_len);
        self.store(self.string_field(result, LEN), needed.into());
        self.release(rhs.into(), Type::STRING);
        result
    }

    /// A new string holding a copy of the `len` bytes at `bytes`, which are valid UTF-8.
    pub(super) fn string_from_bytes(
        &self,
        bytes: PointerValue<'ctx>,
        len: IntValue<'ctx>,
    ) -> PointerValue<'ctx> {
        let string = self.new_string(len);
        self.builder
            .build_memcpy(self.string_field(string, BYTES), 1, bytes, 1, len)
            .expect("an alignment of 1 should be a power of two");
        self.store(self.string_field(string, LEN), len.into());
        string
    }

    /// A new block with count 1 and capacity `cap`, whose length is not yet stored.
    fn new_string(&self, cap: IntValue<'ctx>) -> PointerValue<'ctx> {
        let (size, align) = self.heap_size(&HeapBlock::String { cap });
        let string = self
            .builder
            .build_call(self.alloc, &[size.into(), align.into()], "string")
            .expect(POSITIONED)
            .try_as_basic_value()
            .basic()
            .expect("`sisu_alloc` should return a pointer, as the runtime declares it")
            .into_pointer_value();
        self.init_count(string);
        self.store(self.string_field(string, CAP), cap.into());
        string
    }

    /// The string of the evaluated `parts`, each with its type, in one pass: allocates one
    /// block with room for the most text each part can give, writes each part at the running
    /// offset, pads it there when its format has a width, and stores the length written. The
    /// caller releases the `string` parts.
    pub(super) fn interpolate(
        &self,
        parts: &[(BasicValueEnum<'ctx>, Type)],
        formats: &[Format],
    ) -> PointerValue<'ctx> {
        let i64_type = self.context.i64_type();
        let add = |a, b| {
            self.builder
                .build_int_nuw_add(a, b, "len")
                .expect(POSITIONED)
        };
        // The formats are in part order, so one pass over them meets each in turn.
        let mut formats_in_order = formats.iter().peekable();
        let bound = |bytes| i64_type.const_int(bytes, false);
        // The most bytes each part's text takes: see `sisu_write_int`, `_f64`, `_f64_fixed`,
        // `_bool` and `_char`, plus the width when it is padded. A string's length is below
        // 2^57 on x86-64 and aarch64 Linux, so the sum cannot overflow.
        let cap = parts
            .iter()
            .enumerate()
            .fold(i64_type.const_zero(), |cap, (i, &(value, ty))| {
                let format = formats_in_order.next_if(|format| format.part == i);
                let text = match ty {
                    Type::STRING => self.string_len(value.into_pointer_value()),
                    Type::I64 => bound(20),
                    Type::U8 => bound(3),
                    Type::BOOL => bound(5),
                    Type::CHAR => bound(4),
                    Type::F64 => bound(
                        format
                            .and_then(|format| format.precision)
                            .map_or(24, |precision| 311 + u64::from(precision)),
                    ),
                    _ => unreachable!(
                        "the checker should accept only text, numbers, `bool` and `char`"
                    ),
                };
                let cap = add(cap, text);
                match format.and_then(|format| format.pad) {
                    Some(pad) => add(cap, bound(u64::from(pad.width))),
                    None => cap,
                }
            });
        let result = self.new_string(cap);
        let mut formats_in_order = formats.iter().peekable();
        let mut len = i64_type.const_zero();
        for (i, &(value, ty)) in parts.iter().enumerate() {
            let format = formats_in_order.next_if(|format| format.part == i);
            let at = self.string_byte(result, len);
            let written = match ty {
                Type::STRING => {
                    let string = value.into_pointer_value();
                    let part_len = self.string_len(string);
                    self.copy_bytes(at, string, part_len);
                    part_len
                }
                Type::U8 => {
                    let widened = self
                        .builder
                        .build_int_z_extend(value.into_int_value(), i64_type, "widened")
                        .expect(POSITIONED);
                    self.write_text("sisu_write_int", &[widened.into(), at.into()])
                }
                Type::I64 => self.write_text("sisu_write_int", &[value, at.into()]),
                Type::F64 => match format.and_then(|format| format.precision) {
                    Some(precision) => {
                        let precision = self.context.i8_type().const_int(precision.into(), false);
                        let args = [value, precision.into(), at.into()];
                        self.write_text("sisu_write_f64_fixed", &args)
                    }
                    None => self.write_text("sisu_write_f64", &[value, at.into()]),
                },
                Type::BOOL => self.write_text("sisu_write_bool", &[value, at.into()]),
                _ => self.write_text("sisu_write_char", &[value, at.into()]),
            };
            let written = match format.and_then(|format| format.pad) {
                Some(Pad { align, width }) => {
                    let align = match align {
                        Align::Left => PAD_LEFT,
                        Align::Right => PAD_RIGHT,
                        Align::Center => PAD_CENTER,
                    };
                    let args = [
                        at.into(),
                        written.into(),
                        bound(width.into()).into(),
                        self.context.i8_type().const_int(align, false).into(),
                    ];
                    self.write_text("sisu_pad", &args)
                }
                None => written,
            };
            len = add(len, written);
        }
        self.store(self.string_field(result, LEN), len.into());
        result
    }

    /// The `[char]` of `string`.
    pub(super) fn chars(&self, string: PointerValue<'ctx>) -> PointerValue<'ctx> {
        let (bytes, len) = (self.string_field(string, BYTES), self.string_len(string));
        let count = self.utf8_size("sisu_utf8_count", bytes, len);
        let array = self.alloc_array(Type::CHAR, count, None);
        let first = self.element_ptr(Type::CHAR, array, self.context.i64_type().const_zero());
        self.utf8_fill("sisu_utf8_decode", bytes, len, first);
        array
    }

    /// The `[u8]` of `string`.
    pub(super) fn bytes(&self, string: PointerValue<'ctx>) -> PointerValue<'ctx> {
        let len = self.string_len(string);
        let array = self.alloc_array(Type::U8, len, None);
        let first = self.element_ptr(Type::U8, array, self.context.i64_type().const_zero());
        self.copy_bytes(first, string, len);
        array
    }

    /// The number of words in `string`.
    pub(super) fn len_words(&self, string: PointerValue<'ctx>) -> IntValue<'ctx> {
        let (bytes, len) = (self.string_field(string, BYTES), self.string_len(string));
        self.call_word_function("sisu_words_count", &[bytes.into(), len.into()])
    }

    /// The `[string]` of the words of `string`.
    pub(super) fn words(&self, string: PointerValue<'ctx>) -> PointerValue<'ctx> {
        let i64_type = self.context.i64_type();
        let (bytes, len) = (self.string_field(string, BYTES), self.string_len(string));
        let count = self.call_word_function("sisu_words_count", &[bytes.into(), len.into()]);
        let array = self.alloc_array(Type::STRING, count, None);
        // `count_loop` gives its body only the index, so the offset where the last word ended,
        // and the start of the word just found, live in slots.
        let from = self.entry_alloca(i64_type.into(), "from");
        let start = self.entry_alloca(i64_type.into(), "start");
        self.store(from, i64_type.const_zero().into());
        self.count_loop(count, |index| {
            let args = [
                bytes.into(),
                len.into(),
                self.load(i64_type.into(), from).into(),
                start.into(),
            ];
            let end = self.call_word_function("sisu_word_next", &args);
            let first = self.load(i64_type.into(), start).into_int_value();
            let word_len = self
                .builder
                .build_int_nuw_sub(end, first, "word_len")
                .expect(POSITIONED);
            let word = self.string_from_bytes(self.string_byte(string, first), word_len);
            let slot = self.element_ptr(Type::STRING, array, index);
            self.store(slot, word.into());
            self.store(from, end.into());
        });
        array
    }

    /// A call of the runtime's `name`, which returns an `i64`: `sisu_words_count(bytes, len)`
    /// or `sisu_word_next(bytes, len, from, start)`. Declared on first use; the first only reads
    /// the memory behind its arguments, and the second also writes `start`.
    fn call_word_function(
        &self,
        name: &str,
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> IntValue<'ctx> {
        self.builder
            .build_call(self.runtime(name), args, "word")
            .expect(POSITIONED)
            .try_as_basic_value()
            .basic()
            .expect("a `sisu_word*` function should return an `i64`, as the runtime declares it")
            .into_int_value()
    }

    /// The string of the `[char]` `array`.
    pub(super) fn string_from_chars(&self, array: PointerValue<'ctx>) -> PointerValue<'ctx> {
        let zero = self.context.i64_type().const_zero();
        let (chars, count) = (
            self.element_ptr(Type::CHAR, array, zero),
            self.array_len(array),
        );
        let len = self.utf8_size("sisu_utf8_len", chars, count);
        let string = self.new_string(len);
        self.utf8_fill(
            "sisu_utf8_encode",
            chars,
            count,
            self.string_byte(string, zero),
        );
        self.store(self.string_field(string, LEN), len.into());
        string
    }

    /// A call of the runtime's `name(text, n) -> i64`, which sizes the `n` units at `text`;
    /// declared on first use, so that a module without these nodes stays as it was.
    fn utf8_size(&self, name: &str, text: PointerValue<'ctx>, n: IntValue<'ctx>) -> IntValue<'ctx> {
        self.builder
            .build_call(self.runtime(name), &[text.into(), n.into()], "size")
            .expect(POSITIONED)
            .try_as_basic_value()
            .basic()
            .expect("a `sisu_utf8_*` sizing function should return an `i64`, as the runtime declares it")
            .into_int_value()
    }

    /// A call of the runtime's `name(text, n, out)`, which converts the `n` units at `text` into
    /// `out`; declared on first use, as `utf8_size` is.
    fn utf8_fill(
        &self,
        name: &str,
        text: PointerValue<'ctx>,
        n: IntValue<'ctx>,
        out: PointerValue<'ctx>,
    ) {
        self.call(self.runtime(name), &[text.into(), n.into(), out.into()]);
    }

    /// A call of the runtime's `name(args) -> len`, such as `sisu_write_int(value, out)`, which
    /// writes text at `out`; declared on first use, so that a module without interpolation
    /// stays as it was.
    fn write_text(&self, name: &str, args: &[BasicValueEnum<'ctx>]) -> IntValue<'ctx> {
        // The C ABI extends a `bool` or a `u8` argument to 32 bits and the callee may read all
        // of them, so the caller extends it with zeros, as for `sisu_print_bool`.
        let narrow = || {
            (0..).zip(args).filter_map(|(i, arg)| {
                matches!(arg, BasicValueEnum::IntValue(int) if int.get_type().get_bit_width() < 32)
                    .then_some(i)
            })
        };
        let function = self.runtime(name);
        let values: Vec<BasicMetadataValueEnum> = args.iter().map(|&arg| arg.into()).collect();
        let call = self
            .builder
            .build_call(function, &values, "written")
            .expect(POSITIONED);
        for i in narrow() {
            call.add_attribute(AttributeLoc::Param(i), zeroext(self.context));
        }
        call.try_as_basic_value()
            .basic()
            .expect("a `sisu_write_*` or `sisu_pad` function should return an `i64`, as the runtime declares it")
            .into_int_value()
    }

    /// Copies the first `len` bytes of `string` to `to`.
    fn copy_bytes(&self, to: PointerValue<'ctx>, string: PointerValue<'ctx>, len: IntValue<'ctx>) {
        self.builder
            .build_memcpy(to, 1, self.string_field(string, BYTES), 1, len)
            .expect("an alignment of 1 should be a power of two");
    }

    /// `sisu_str_compare` on the bytes of `a` and `b`: -1, 0 or 1.
    pub(super) fn str_compare(
        &self,
        a: PointerValue<'ctx>,
        b: PointerValue<'ctx>,
    ) -> IntValue<'ctx> {
        let args = [
            self.string_field(a, BYTES).into(),
            self.string_len(a).into(),
            self.string_field(b, BYTES).into(),
            self.string_len(b).into(),
        ];
        self.builder
            .build_call(self.str_compare, &args, "order")
            .expect(POSITIONED)
            .try_as_basic_value()
            .basic()
            .expect("`sisu_str_compare` should return an `i64`, as the runtime declares it")
            .into_int_value()
    }

    /// `print` or `eprint` of a string: `sisu_print_str(bytes, len)`, or `sisu_eprint_str`.
    pub(super) fn print_string(&self, stream: Stream, string: PointerValue<'ctx>) {
        self.call(
            self.printers[stream.index()].str,
            &[
                self.string_field(string, BYTES).into(),
                self.string_len(string).into(),
            ],
        );
    }

    /// `panic(message)` at `span`: `sisu_panic_message(at, at_len, bytes, len)`, then
    /// `unreachable`. The message is not released: the process exits.
    pub(super) fn panic_value(&self, string: PointerValue<'ctx>, span: Span) {
        let at = self.location(span);
        let at_ptr = self
            .builder
            .build_global_string_ptr(&at, "panic.at")
            .expect(POSITIONED);
        let at_len = self.context.i64_type().const_int(
            u64::try_from(at.len()).expect("a location should be shorter than 2^64 bytes"),
            false,
        );
        self.call(
            self.panic_message,
            &[
                at_ptr.as_pointer_value().into(),
                at_len.into(),
                self.string_field(string, BYTES).into(),
                self.string_len(string).into(),
            ],
        );
        self.builder.build_unreachable().expect(POSITIONED);
    }

    /// The body of `@sisu_rc.drop.string` or `@sisu_rc.eq.string`.
    pub(super) fn define_string_helper(&self, kind: Helper, function: FunctionValue<'ctx>) {
        match kind {
            Helper::Drop => self.define_string_drop(function),
            Helper::Eq => self.define_string_eq(function),
            Helper::Retain
            | Helper::Release
            | Helper::Unique
            | Helper::Walk
            | Helper::Drain
            | Helper::EqComponent
            | Helper::Unbox
            | Helper::Free
            | Helper::Reuse => {
                unreachable!("only a string's `drop` and `eq` should reach `define_string_helper`")
            }
        }
    }

    /// `@sisu_rc.drop.string`: frees the block, whose size is the header's plus the capacity.
    fn define_string_drop(&self, function: FunctionValue<'ctx>) {
        self.builder
            .position_at_end(self.context.append_basic_block(function, "entry"));
        let string = param(function);
        let cap = self.load_string_field(string, CAP);
        let (size, align) = self.heap_size(&HeapBlock::String { cap });
        self.call(self.free, &[string.into(), size.into(), align.into()]);
        self.builder.build_return(None).expect(POSITIONED);
    }

    /// `@sisu_rc.eq.string(a, b)`: see `eq_entry` for the same pointer and `null`; otherwise
    /// the lengths, then the bytes by `sisu_str_compare`.
    fn define_string_eq(&self, function: FunctionValue<'ctx>) {
        let (a, b, equal, unequal) = self.eq_entry(function, true);
        let same_len = self.int_compare(IntPredicate::EQ, self.string_len(a), self.string_len(b));
        let bytes = self.context.append_basic_block(function, "bytes");
        self.builder
            .build_conditional_branch(same_len, bytes, unequal)
            .expect(POSITIONED);
        self.builder.position_at_end(bytes);
        let order = self.str_compare(a, b);
        let same = self.int_compare(
            IntPredicate::EQ,
            order,
            self.context.i64_type().const_zero(),
        );
        self.builder
            .build_conditional_branch(same, equal, unequal)
            .expect(POSITIONED);
    }
}
