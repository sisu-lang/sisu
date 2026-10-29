//! Printing a compound value. `print` and `eprint` of an array, a class, an enum or an optional
//! write the value's text into a sink (`crates/runtime/src/show.rs`): standard output's buffer
//! for `print`, a block that becomes a string for `eprint`. Each concrete array, class and enum
//! type gets one `@sisu_show.<type>(sink, value)` helper, declared on first use and defined by
//! `define_helpers` (see `rc::Helper`), that writes the spec's "Printing" table: a `string` or a
//! `char` quoted inside a compound value and bare at the top, an optional transparent by the
//! Some rule, a variant without its enum's name and with its labels, a class as its name with its
//! type arguments, and a written `to_string`'s text as it is.
//!
//! `dbg` writes a value the same way into a buffer sink, behind a pretty sink. A second set of
//! helpers (`Mode::Dbg`, named `@sisu_show.dbg.<type>`) writes a class with no written
//! `to_string` as its name and its fields, each field in `Mode::Print`. A type that holds no
//! such class has one helper for both modes (`Codegen::holds_class`).
//!
//! An enum whose component has boxes is written in one frame, as `==` compares it
//! (`define_eq_component`). `@sisu_show_component.<F0>(sink, kind, value)` writes a value of any
//! member of the component of `F0`. It keeps, for the value it is at, the address of the value
//! and `owed`, the number of `)` that are still to write. A link that is the last field of its
//! variant adds 1 to `owed`, and the loop goes on in the linked value: that value writes the `)`
//! when it is done. Before any other link it pushes `(resume index, address, owed)` on the
//! pending list, sets `owed` to 0 and loops. When a value is done, it writes its `owed` closing
//! parentheses and pops an entry, which goes on at the field after the link. So the frame does
//! not grow with the depth or the shape of the value. The first entry of the pending list is in
//! the frame. A second entry that waits at the same time, as in a left-leaning value, goes to a
//! buffer on the heap, which the helper frees before it returns.

use inkwell::AddressSpace;
use inkwell::IntPredicate;
use inkwell::basic_block::BasicBlock;
use inkwell::types::BasicTypeEnum;
use inkwell::values::{
    BasicMetadataValueEnum, BasicValueEnum, CallSiteValue, FunctionValue, IntValue, PhiValue,
    PointerValue,
};

use inkwell::attributes::AttributeLoc;

use super::abi::{self, Abi, PAD_CENTER, PAD_LEFT, PAD_RIGHT, SINK_CAP, SINK_LEN, SINK_SIZE};
use super::enums::{Component, Pending, boxed_enum, nested};
use super::layout::{BYTES, CAP, LEN};
use super::rc::{Helper, Mode, param};
use super::{Codegen, POSITIONED, zeroext};
use crate::conformance::{calls_written, holds_class, prints_by_helper};
use crate::source::Span;
use crate::tir::{Align, ClassId, EnumId, Format, FuncId, Pad, Stream, Type, TypeArgs, TypeKind};
use rustc_hash::FxHashMap;
use std::rc::Rc;

/// The blocks and phis of a `show_component` helper's loop that each variant uses.
struct ShowLoop<'ctx> {
    sink: PointerValue<'ctx>,
    mode: Mode,
    members: Rc<[EnumId]>,
    /// The `kind` of each member, so that a link finds the one it enters at once.
    kinds: FxHashMap<EnumId, u64>,
    /// The loop's head, and its `kind`, address and `owed` phis.
    top: BasicBlock<'ctx>,
    current: [PhiValue<'ctx>; 3],
    /// Where a value whose text is written goes, and the `owed` it carries there.
    pop: BasicBlock<'ctx>,
    pop_owed: PhiValue<'ctx>,
    /// The resume index, address and `owed` (as a pointer) of a popped entry.
    popped: [PhiValue<'ctx>; 3],
}

impl<'ctx> Codegen<'ctx, '_> {
    /// `print` or `eprint` of an evaluated operand of type `ty`, a compound value, which it
    /// borrows.
    pub(super) fn print_compound(&self, stream: Stream, ty: Type, value: BasicValueEnum<'ctx>) {
        match stream {
            Stream::Stdout => {
                let sink = self.sink_slot();
                self.show_call("sisu_show_stdout", &[sink.into()]);
                self.show_value(sink, ty, value, false, Mode::Print);
                self.show_call("sisu_show_line_end", &[sink.into()]);
            }
            Stream::Stderr => {
                let sink = self.buffer_sink();
                self.show_value(sink, ty, value, false, Mode::Print);
                let block = self.finish_buffer_sink(sink);
                self.print_string(Stream::Stderr, block);
                self.release(block.into(), Type::STRING);
            }
        }
    }

    /// The string of the evaluated `parts`, each with its type, when one of them is a compound
    /// value: every part is written in turn through one buffer sink, which becomes the result
    /// string with no copy. A width pads the text of its part in place, from the mark taken
    /// before the part, a precision is for an `f64`, and a `#` part goes through a pretty sink
    /// over the buffer sink. The parts are evaluated already, so a part that returns, breaks or
    /// continues has left no buffer behind. The caller releases the parts.
    pub(super) fn interpolate_compound(
        &self,
        parts: &[(BasicValueEnum<'ctx>, Type)],
        formats: &[Format],
    ) -> PointerValue<'ctx> {
        let sink = self.buffer_sink();
        // The formats are in part order, so one pass over them meets each in turn.
        let mut formats = formats.iter().peekable();
        for (i, &(value, ty)) in parts.iter().enumerate() {
            let format = formats.next_if(|format| format.part == i);
            let mark = format.and_then(|format| format.pad).map(|_| {
                (self.show_call("sisu_show_mark", &[sink.into()]))
                    .try_as_basic_value()
                    .basic()
                    .expect("`sisu_show_mark` should return the mark, as the runtime declares it")
            });
            match (ty, format.and_then(|format| format.precision)) {
                (Type::F64, Some(precision)) => {
                    let precision = self.context.i8_type().const_int(precision.into(), false);
                    let args = [sink.into(), value.into(), precision.into()];
                    self.show_call("sisu_show_f64_fixed", &args);
                }
                _ if format.is_some_and(|format| format.pretty) => {
                    // The layout starts at column 0, wherever the text of the parts before it
                    // ended, and goes into the buffer sink when the value is done.
                    let pretty = self.sink_slot();
                    self.show_call("sisu_show_pretty", &[pretty.into(), sink.into()]);
                    self.show_value(pretty, ty, value, false, Mode::Print);
                    self.show_call("sisu_show_pretty_finish", &[pretty.into()]);
                }
                _ => self.show_value(sink, ty, value, false, Mode::Print),
            }
            if let (Some(Pad { align, width, .. }), Some(mark)) =
                (format.and_then(|format| format.pad), mark)
            {
                let align = match align {
                    Align::Left => PAD_LEFT,
                    Align::Right => PAD_RIGHT,
                    Align::Center => PAD_CENTER,
                };
                let i64_type = self.context.i64_type();
                let args = [
                    sink.into(),
                    mark.into(),
                    i64_type.const_int(width.into(), false).into(),
                    self.context.i8_type().const_int(align, false).into(),
                ];
                self.show_call("sisu_show_pad", &args);
            }
        }
        self.finish_buffer_sink(sink)
    }

    /// `dbg` of the evaluated operand of type `ty`, which it borrows: one line on standard error,
    /// `<path>:<line>:<col>: <text> = <value>`. The value is laid out as `{x:#}` lays it out,
    /// from column 0 after the prefix, and a class shows its fields (`Mode::Dbg`). The line
    /// goes into a buffer sink first, so it reaches standard error in one write.
    pub(super) fn dbg(&self, at: Span, text: &str, ty: Type, value: BasicValueEnum<'ctx>) {
        let sink = self.buffer_sink();
        self.show_text(
            "sisu_show_str",
            sink,
            &format!("{}: {text} = ", self.location(at)),
        );
        if prints_by_helper(ty) {
            let pretty = self.sink_slot();
            self.show_call("sisu_show_pretty", &[pretty.into(), sink.into()]);
            self.show_value(pretty, ty, value, false, Mode::Dbg);
            self.show_call("sisu_show_pretty_finish", &[pretty.into()]);
        } else {
            self.show_value(sink, ty, value, false, Mode::Dbg);
        }
        let block = self.finish_buffer_sink(sink);
        self.print_string(Stream::Stderr, block);
        self.release(block.into(), Type::STRING);
    }

    /// Whether a value of `ty` holds a class with no written `to_string`: `ty` is one, or an
    /// array, an optional or an enum payload, at any depth, is. Only such a type is written
    /// differently by `dbg`.
    pub(super) fn holds_class(&self, ty: Type) -> bool {
        holds_class(
            (&self.program.conformances, self.program.lang.display),
            self.program.enums.as_slice(),
            ty,
            &mut self.class_held.borrow_mut(),
        )
    }

    /// Whether the `print` helper for `ty` may call a written `to_string`.
    fn calls_display(&self, ty: Type) -> bool {
        prints_by_helper(ty)
            && calls_written(
                &self.program.conformances,
                (
                    self.program.enums.as_slice(),
                    self.program.classes.as_slice(),
                ),
                ty,
                self.program.lang.display,
                false,
                &mut self.written.borrow_mut(),
            )
    }

    /// A slot in the frame for a sink.
    fn sink_slot(&self) -> PointerValue<'ctx> {
        self.entry_alloca(self.sink_type().into(), "sink")
    }

    fn sink_type(&self) -> inkwell::types::ArrayType<'ctx> {
        let words = u32::try_from(SINK_SIZE / 8).expect("the sink should be a few words");
        self.context.i64_type().array_type(words)
    }

    /// Starts a sink in a new slot that builds a block with a string's header, and gives the slot.
    fn buffer_sink(&self) -> PointerValue<'ctx> {
        let sink = self.sink_slot();
        let header = self
            .context
            .i64_type()
            .const_int(self.string_layout().size, false);
        self.show_call("sisu_show_buffer", &[sink.into(), header.into()]);
        sink
    }

    /// Ends the buffer sink `sink`: its block is a string once its header holds the count, the
    /// length and the capacity that the sink ended with.
    fn finish_buffer_sink(&self, sink: PointerValue<'ctx>) -> PointerValue<'ctx> {
        let block = self
            .show_call("sisu_show_finish", &[sink.into()])
            .try_as_basic_value()
            .basic()
            .expect("`sisu_show_finish` should return the block, as the runtime declares it")
            .into_pointer_value();
        self.init_count(block);
        let sink_type = self.sink_type();
        self.store(
            self.string_field(block, LEN),
            self.sink_word(sink_type, sink, SINK_LEN).into(),
        );
        self.store(
            self.string_field(block, CAP),
            self.sink_word(sink_type, sink, SINK_CAP).into(),
        );
        block
    }

    /// The word of the sink `sink`, of type `sink_type`, at byte `offset`.
    fn sink_word(
        &self,
        sink_type: inkwell::types::ArrayType<'ctx>,
        sink: PointerValue<'ctx>,
        offset: u64,
    ) -> IntValue<'ctx> {
        let word = u32::try_from(offset / 8).expect("a sink word should be a few words in");
        let at = self.indexed_word(sink_type, sink, word);
        self.builder
            .build_load(self.context.i64_type(), at, "sink.word")
            .expect(POSITIONED)
            .into_int_value()
    }

    #[expect(
        unsafe_code,
        reason = "inkwell marks GEP unsafe for a wrong index list; this GEP takes the constant index of a word of the sink"
    )]
    fn indexed_word(
        &self,
        sink_type: inkwell::types::ArrayType<'ctx>,
        sink: PointerValue<'ctx>,
        word: u32,
    ) -> PointerValue<'ctx> {
        let i32_type = self.context.i32_type();
        // SAFETY: `word` is below the sink's `SINK_SIZE / 8` words, so the GEP stays inside the
        // sink's array.
        unsafe {
            self.builder
                .build_in_bounds_gep(
                    sink_type,
                    sink,
                    &[
                        i32_type.const_zero(),
                        i32_type.const_int(u64::from(word), false),
                    ],
                    "sink.at",
                )
                .expect(POSITIONED)
        }
    }

    /// A call of the runtime function `name`, a row of `abi::RUNTIME`. An argument narrower than
    /// 32 bits is passed `zeroext` at the call too, as `print` does.
    fn show_call(&self, name: &str, args: &[BasicMetadataValueEnum<'ctx>]) -> CallSiteValue<'ctx> {
        let call = self
            .builder
            .build_call(self.runtime(name), args, "")
            .expect(POSITIONED);
        for (i, abi) in (0..).zip(abi::row(name).params) {
            if matches!(abi, Abi::Bool | Abi::U8) {
                call.add_attribute(AttributeLoc::Param(i), zeroext(self.context));
            }
        }
        call
    }

    /// Calls `name` (`sisu_show_str`, `_open` or `_close`) with the bytes of `text`.
    fn show_text(&self, name: &str, sink: PointerValue<'ctx>, text: &str) {
        let bytes = self
            .builder
            .build_global_string_ptr(text, "show")
            .expect(POSITIONED)
            .as_pointer_value();
        let len = self.context.i64_type().const_int(
            u64::try_from(text.len()).expect("a name should be shorter than 2^64 bytes"),
            false,
        );
        self.show_call(name, &[sink.into(), bytes.into(), len.into()]);
    }

    fn show_sep(&self, sink: PointerValue<'ctx>) {
        self.show_call("sisu_show_sep", &[sink.into()]);
    }

    /// Writes `None` inside `depth` `Some(` … `)`: the text of an optional of `depth + 1`
    /// layers that holds a value in the first `depth` and `None` in the next.
    fn show_none(&self, sink: PointerValue<'ctx>, depth: usize) {
        self.show_repeated("sisu_show_open", sink, "Some(", depth);
        self.show_text("sisu_show_str", sink, "None");
        self.show_repeated("sisu_show_close", sink, ")", depth);
    }

    /// Calls `name` with `text` `times` times, in a loop, so that the code does not grow with
    /// `times`: the `Some(` and `)` of every layer of a deep optional would otherwise take a
    /// number of calls that grows with the square of the layers.
    fn show_repeated(&self, name: &str, sink: PointerValue<'ctx>, text: &str, times: usize) {
        if times == 0 {
            return;
        }
        let times = u64::try_from(times).expect("the layers of a type should fit in a `u64`");
        let len = self.context.i64_type().const_int(times, false);
        self.count_loop(len, |_| self.show_text(name, sink, text));
    }

    /// Writes the text of `value`, of type `ty`, which it borrows. A `string` or a `char` is
    /// `quoted` inside a compound value. In `Mode::Dbg`, a class with no written `to_string` in
    /// it shows its fields.
    pub(super) fn show_value(
        &self,
        sink: PointerValue<'ctx>,
        ty: Type,
        value: BasicValueEnum<'ctx>,
        quoted: bool,
        mode: Mode,
    ) {
        let args = |value: BasicValueEnum<'ctx>| [sink.into(), value.into()];
        match ty.kind() {
            TypeKind::I64 => {
                self.show_call("sisu_show_int", &args(value));
            }
            TypeKind::U8 => {
                let widened = self
                    .builder
                    .build_int_z_extend(value.into_int_value(), self.context.i64_type(), "widened")
                    .expect(POSITIONED);
                self.show_call("sisu_show_int", &args(widened.into()));
            }
            TypeKind::F64 => {
                self.show_call("sisu_show_f64", &args(value));
            }
            TypeKind::Bool => {
                self.show_call("sisu_show_bool", &args(value));
            }
            TypeKind::Char => {
                let flag = self.context.bool_type().const_int(u64::from(quoted), false);
                self.show_call("sisu_show_char", &[sink.into(), value.into(), flag.into()]);
            }
            TypeKind::String => {
                let string = value.into_pointer_value();
                let call = if quoted {
                    "sisu_show_quoted"
                } else {
                    "sisu_show_str"
                };
                self.show_call(
                    call,
                    &[
                        sink.into(),
                        self.string_field(string, BYTES).into(),
                        self.string_len(string).into(),
                    ],
                );
            }
            TypeKind::Optional(_) => self.show_optional(sink, ty, value, (quoted, mode)),
            TypeKind::Enum(..) => {
                self.call(
                    self.helper(Helper::Show(mode), ty),
                    &[sink.into(), self.enum_slot(value).into()],
                );
            }
            TypeKind::Array(_) | TypeKind::Class(..) => {
                self.call(self.helper(Helper::Show(mode), ty), &args(value));
            }
            _ => {
                unreachable!("only a type with a value should be written, never `unit` or `never`")
            }
        }
    }

    /// Writes an optional by the Some rule: it unwraps every `Some`; at a value it writes the
    /// value's text, and at a `None` under `k` `Some`s it writes `Some(` `k` times, `None` and
    /// `)` `k` times. So every state of `i64??` writes differently, and `Some(` wraps only `None`.
    fn show_optional(
        &self,
        sink: PointerValue<'ctx>,
        mut ty: Type,
        mut value: BasicValueEnum<'ctx>,
        (quoted, mode): (bool, Mode),
    ) {
        let function = self.current_function();
        let done = self.context.append_basic_block(function, "show.done");
        let mut depth = 0;
        while let TypeKind::Optional(inner) = ty.kind() {
            let present = self.has_value(ty, value, "present");
            let some = self.context.append_basic_block(function, "show.some");
            let none = self.context.append_basic_block(function, "show.none");
            self.branch_on(present, some, none);
            self.builder.position_at_end(none);
            self.show_none(sink, depth);
            self.branch_to(done);
            self.builder.position_at_end(some);
            value = self.optional_value(ty, value, "inner");
            (ty, depth) = (inner, depth + 1);
        }
        self.show_value(sink, ty, value, quoted, mode);
        self.branch_to(done);
        self.builder.position_at_end(done);
    }

    /// `@sisu_show.<T>(sink, value)`: writes a value of an array, a class or an enum type, as
    /// `mode` says. The value is the pointer of an array or an object, or the address of an enum.
    pub(super) fn define_show(&self, function: FunctionValue<'ctx>, ty: Type, mode: Mode) {
        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);
        let sink = param(function);
        let value = function
            .get_nth_param(1)
            .expect("a `show` helper should be declared with two parameters")
            .into_pointer_value();
        match ty.kind() {
            TypeKind::Array(element) => self.show_array(sink, element, value, mode),
            TypeKind::Class(id, _) => match self.written_display(ty) {
                Some(method) => self.show_written(sink, method, ty, value.into()),
                None if mode == Mode::Dbg => self.show_fields(sink, (id, ty), value),
                None => self.show_text("sisu_show_str", sink, &self.program.type_name(ty)),
            },
            TypeKind::Enum(id, _) => match self.written_display(ty) {
                Some(method) => {
                    self.show_written(sink, method, ty, self.load_value(ty, value));
                }
                None => match &self.components[id.0] {
                    Some(Component { members, index }) => {
                        let first = Type::intern(TypeKind::Enum(members[0], TypeArgs::NONE));
                        let kind = self.kind(*index);
                        self.call(
                            self.helper(Helper::ShowComponent(mode), first),
                            &[sink.into(), kind.into(), value.into()],
                        );
                    }
                    None => self.show_variants(sink, id, value, mode),
                },
            },
            _ => unreachable!("only an array, a class or an enum should have a `show` helper"),
        }
        self.builder.build_return(None).expect(POSITIONED);
    }

    /// Writes the object `object` of the class `id`, of type `ty`, for `dbg`. It writes the
    /// name with its type arguments, then the fields in declaration order, each labeled and
    /// written by the `print` helper. So a class in a field shows its name or its own
    /// `to_string`, never its fields. A class with no fields writes `Name()`, as its constructor
    /// reads. The `dbg` that calls the helper borrows the object. A field with no `mut` is stable
    /// while it is read, and so is one whose helper runs no written method. Such a field is read
    /// in place.
    /// A counted `mut` field whose helper may run a written `to_string` is retained before the
    /// helper runs and released after. So a `to_string` that clears the field cannot free the
    /// value being shown. The snapshot holds what is below the field too.
    fn show_fields(
        &self,
        sink: PointerValue<'ctx>,
        (id, ty): (ClassId, Type),
        object: PointerValue<'ctx>,
    ) {
        self.show_text(
            "sisu_show_open",
            sink,
            &format!("{}(", self.program.type_name(ty)),
        );
        for (index, field) in self.program.classes[id.0].fields.iter().enumerate() {
            if index > 0 {
                self.show_sep(sink);
            }
            self.show_text("sisu_show_str", sink, &format!("{}: ", field.name));
            let value = self.load_value(field.ty, self.field_ptr(id, object, index));
            let snapshot = field.mutable
                && field.ty.is_counted(&self.program.enums)
                && self.calls_display(field.ty);
            if snapshot {
                self.retain(value, field.ty);
            }
            self.show_value(sink, field.ty, value, true, Mode::Print);
            if snapshot {
                self.release(value, field.ty);
            }
        }
        self.show_text("sisu_show_close", sink, ")");
    }

    /// Writes the text of `receiver`, of type `ty`, which the written `to_string` `method`
    /// returns: as it is. The helper retains the receiver first when the method owns it, as the
    /// method's final `borrowed` flags say, and releases the text once it has written it.
    fn show_written(
        &self,
        sink: PointerValue<'ctx>,
        method: FuncId,
        ty: Type,
        receiver: BasicValueEnum<'ctx>,
    ) {
        let owned = !self.mir.functions[method.0].borrowed[0];
        if owned {
            self.retain(receiver, ty);
        }
        let text = self
            .call_value(method, &[receiver.into()])
            .expect("a written `to_string` should return a `string`")
            .into_pointer_value();
        self.show_call(
            "sisu_show_str",
            &[
                sink.into(),
                self.string_field(text, BYTES).into(),
                self.string_len(text).into(),
            ],
        );
        self.release(text.into(), Type::STRING);
    }

    /// Writes the array at `array`: `[`, each element quoted if it is a `string` or a `char`,
    /// with `sep` between them, and `]`.
    fn show_array(
        &self,
        sink: PointerValue<'ctx>,
        element: Type,
        array: PointerValue<'ctx>,
        mode: Mode,
    ) {
        self.show_text("sisu_show_open", sink, "[");
        let len = self.array_len(array);
        let function = self.current_function();
        self.count_loop(len, |index| {
            let (sep, item) = (
                self.context.append_basic_block(function, "show.sep"),
                self.context.append_basic_block(function, "show.item"),
            );
            let first = self.int_compare(
                IntPredicate::EQ,
                index,
                self.context.i64_type().const_zero(),
            );
            self.branch_on(first, item, sep);
            self.builder.position_at_end(sep);
            self.show_sep(sink);
            self.branch_to(item);
            self.builder.position_at_end(item);
            self.show_value(
                sink,
                element,
                self.load_element(element, array, index),
                true,
                mode,
            );
        });
        self.show_text("sisu_show_close", sink, "]");
    }

    /// Writes the value of the enum `id` at `value`, an enum whose component has no box: its
    /// variant, and the fields of that variant in order.
    fn show_variants(
        &self,
        sink: PointerValue<'ctx>,
        id: EnumId,
        value: PointerValue<'ctx>,
        mode: Mode,
    ) {
        let function = self.current_function();
        let done = self
            .context
            .append_basic_block(function, "show.variant.done");
        let tag = self.load_tag(id, value);
        let variants = &self.program.enums[id.0].variants;
        let blocks: Vec<_> = variants
            .iter()
            .map(|variant| self.context.append_basic_block(function, &variant.name))
            .collect();
        let cases: Vec<_> = (0_u64..)
            .zip(&blocks)
            .skip(1)
            .map(|(index, &block)| (self.enum_layout(id).tag.const_int(index, false), block))
            .collect();
        // The tag is always a variant's index, so the first variant can be the default.
        self.builder
            .build_switch(tag, blocks[0], &cases)
            .expect(POSITIONED);
        for (variant, (&block, v)) in blocks.iter().zip(variants).enumerate() {
            self.builder.position_at_end(block);
            if v.fields.is_empty() {
                self.show_text("sisu_show_str", sink, &v.name);
            } else {
                self.show_text("sisu_show_open", sink, &format!("{}(", v.name));
                for (field, payload) in v.fields.iter().enumerate() {
                    if field > 0 {
                        self.show_sep(sink);
                    }
                    if let Some(label) = &payload.label {
                        self.show_text("sisu_show_str", sink, &format!("{label}: "));
                    }
                    self.show_field(sink, (id, variant, field), value, mode);
                }
                self.show_text("sisu_show_close", sink, ")");
            }
            self.branch_to(done);
        }
        self.builder.position_at_end(done);
    }

    /// Writes payload field `field` of variant `variant` in the enum `id` at `value`, which is
    /// not a link: an enum by its helper, in place, and any other type by its value.
    fn show_field(
        &self,
        sink: PointerValue<'ctx>,
        (id, variant, field): (EnumId, usize, usize),
        value: PointerValue<'ctx>,
        mode: Mode,
    ) {
        let ty = self.program.enums[id.0].variants[variant].fields[field].ty;
        if let TypeKind::Enum(..) = ty.kind() {
            let place = self.payload_place(value, id, variant, field);
            self.call(
                self.helper(Helper::Show(mode), ty),
                &[sink.into(), place.into()],
            );
        } else {
            self.show_value(
                sink,
                ty,
                self.payload_value(value, id, variant, field),
                true,
                mode,
            );
        }
    }

    /// `@sisu_show_component.<F0>(sink, kind, value)`: see the module doc.
    pub(super) fn define_show_component(
        &self,
        first: EnumId,
        function: FunctionValue<'ctx>,
        mode: Mode,
    ) {
        let members = self.component(first).expect(
            "only the first member of a component with boxes should get a `show_component` helper",
        );
        let block = |name| self.context.append_basic_block(function, name);
        let (entry, top, pop, dispatch, done) = (
            block("entry"),
            block("loop"),
            block("pop"),
            block("dispatch"),
            block("done"),
        );
        let nth = |n| {
            function.get_nth_param(n).expect(
                "the generated `show_component` helper should be declared with three parameters",
            )
        };
        self.builder.position_at_end(entry);
        let pending = self.empty_pending();
        self.builder.position_at_end(done);
        self.free_pending(&pending);
        self.builder.build_return(None).expect(POSITIONED);
        self.builder.position_at_end(entry);
        self.branch_to(top);
        let i64_type = self.context.i64_type();
        let ptr_type = self.context.ptr_type(AddressSpace::default());
        let types: [BasicTypeEnum<'ctx>; 3] = [
            self.context.i32_type().into(),
            ptr_type.into(),
            i64_type.into(),
        ];
        self.builder.position_at_end(top);
        let current = types.map(|ty| self.builder.build_phi(ty, "current").expect(POSITIONED));
        current[0].add_incoming(&[(&nth(1), entry)]);
        current[1].add_incoming(&[(&nth(2), entry)]);
        current[2].add_incoming(&[(&i64_type.const_zero(), entry)]);
        self.builder.position_at_end(dispatch);
        let popped_types: [BasicTypeEnum<'ctx>; 3] = [
            self.context.i32_type().into(),
            ptr_type.into(),
            ptr_type.into(),
        ];
        let popped = popped_types.map(|ty| self.builder.build_phi(ty, "popped").expect(POSITIONED));
        self.builder.position_at_end(pop);
        let pop_owed = self.builder.build_phi(i64_type, "owed").expect(POSITIONED);
        self.builder.position_at_end(top);
        let starts: Vec<_> = members
            .iter()
            .map(|m| block(&self.program.enums[m.0].name))
            .collect();
        let cases: Vec<_> = (0_u64..)
            .zip(&starts)
            .skip(1)
            .map(|(index, &start)| (self.context.i32_type().const_int(index, false), start))
            .collect();
        // `kind` is always a member's index, so the first member can be the default.
        self.builder
            .build_switch(
                current[0].as_basic_value().into_int_value(),
                starts[0],
                &cases,
            )
            .expect(POSITIONED);
        let kinds = (0_u64..)
            .zip(members.iter())
            .map(|(kind, &m)| (m, kind))
            .collect();
        let show = ShowLoop {
            sink: nth(0).into_pointer_value(),
            mode,
            members,
            kinds,
            top,
            current,
            pop,
            pop_owed,
            popped,
        };
        let mut resumes = Vec::new();
        for (&id, &start) in show.members.iter().zip(&starts) {
            self.builder.position_at_end(start);
            self.show_member(&show, &pending, &mut resumes, id);
        }
        self.builder.position_at_end(pop);
        self.show_owed(&show);
        self.pop_pending(&pending, done, dispatch, popped);
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
        self.builder
            .build_switch(popped[0].as_basic_value().into_int_value(), default, &cases)
            .expect(POSITIONED);
    }

    /// At `pop`: writes the `)` that the value owes, one for each count in `owed`, then goes on
    /// where the builder is left, after the loop.
    fn show_owed(&self, show: &ShowLoop<'ctx>) {
        let function = self.current_function();
        let i64_type = self.context.i64_type();
        let before = self.current_block();
        let (head, body, after) = (
            self.context.append_basic_block(function, "owed.head"),
            self.context.append_basic_block(function, "owed.write"),
            self.context.append_basic_block(function, "owed.done"),
        );
        self.branch_to(head);
        self.builder.position_at_end(head);
        let count = self.builder.build_phi(i64_type, "owed").expect(POSITIONED);
        count.add_incoming(&[(&show.pop_owed.as_basic_value(), before)]);
        let more = self.int_compare(
            IntPredicate::NE,
            count.as_basic_value().into_int_value(),
            i64_type.const_zero(),
        );
        self.branch_on(more, body, after);
        self.builder.position_at_end(body);
        self.show_text("sisu_show_close", show.sink, ")");
        let less = self
            .builder
            .build_int_nuw_sub(
                count.as_basic_value().into_int_value(),
                i64_type.const_int(1, false),
                "owed",
            )
            .expect(POSITIONED);
        count.add_incoming(&[(&less, self.current_block())]);
        self.branch_to(head);
        self.builder.position_at_end(after);
    }

    /// Branches to `pop`, with `owed` closing parentheses to write there.
    fn show_pop(&self, show: &ShowLoop<'ctx>, owed: IntValue<'ctx>) {
        show.pop_owed.add_incoming(&[(&owed, self.current_block())]);
        self.branch_to(show.pop);
    }

    /// Writes member `id` of the component at the builder's block: a written `to_string`, or
    /// its variants.
    fn show_member(
        &self,
        show: &ShowLoop<'ctx>,
        pending: &Pending<'ctx>,
        resumes: &mut Vec<BasicBlock<'ctx>>,
        id: EnumId,
    ) {
        let function = self.current_function();
        let address = show.current[1].as_basic_value().into_pointer_value();
        let owed = show.current[2].as_basic_value().into_int_value();
        let ty = Type::intern(TypeKind::Enum(id, TypeArgs::NONE));
        // A member whose `Display` is written is its `to_string`'s text, wherever a link meets it.
        if let Some(method) = self.written_display(ty) {
            self.show_written(show.sink, method, ty, self.load_value(ty, address));
            self.show_pop(show, owed);
            return;
        }
        let variants = &self.program.enums[id.0].variants;
        let blocks: Vec<_> = variants
            .iter()
            .map(|variant| self.context.append_basic_block(function, &variant.name))
            .collect();
        let cases: Vec<_> = (0_u64..)
            .zip(&blocks)
            .skip(1)
            .map(|(index, &block)| (self.enum_layout(id).tag.const_int(index, false), block))
            .collect();
        let tag = self.load_tag(id, address);
        self.builder
            .build_switch(tag, blocks[0], &cases)
            .expect(POSITIONED);
        for (variant, &block) in blocks.iter().enumerate() {
            self.builder.position_at_end(block);
            self.show_variant(show, pending, resumes, (id, variant));
        }
    }

    /// Writes variant `variant` of the enum `id` in the loop `show`, from the builder's block: a
    /// variant with no field is its name alone. Else `Name(`, then the fields in order with
    /// `sep` between them and a label first when a field has one. Before each link that is not
    /// the last field, it pushes the index of a new block in `resumes`, which goes on from the
    /// next field, and loops into the link. A last field that is a link adds 1 to `owed` and
    /// loops with no push. Else the value writes `)` and goes to `pop`.
    fn show_variant(
        &self,
        show: &ShowLoop<'ctx>,
        pending: &Pending<'ctx>,
        resumes: &mut Vec<BasicBlock<'ctx>>,
        (id, variant): (EnumId, usize),
    ) {
        let function = self.current_function();
        let i64_type = self.context.i64_type();
        let mut address = show.current[1].as_basic_value().into_pointer_value();
        let mut owed = show.current[2].as_basic_value().into_int_value();
        let v = &self.program.enums[id.0].variants[variant];
        if v.fields.is_empty() {
            self.show_text("sisu_show_str", show.sink, &v.name);
            self.show_pop(show, owed);
            return;
        }
        self.show_text("sisu_show_open", show.sink, &format!("{}(", v.name));
        let looped = (v.fields.iter().enumerate())
            .next_back()
            .filter(|(_, payload)| self.is_link(id, payload))
            .map(|(field, _)| field);
        for (field, payload) in v.fields.iter().enumerate() {
            if field > 0 {
                self.show_sep(show.sink);
            }
            if let Some(label) = &payload.label {
                self.show_text("sisu_show_str", show.sink, &format!("{label}: "));
            }
            if looped == Some(field) {
                continue;
            }
            if !self.is_link(id, payload) {
                self.show_field(show.sink, (id, variant, field), address, show.mode);
                continue;
            }
            let resume = self.context.append_basic_block(function, "resume");
            let index = u64::try_from(resumes.len()).expect("a resume index should fit in a `u64`");
            resumes.push(resume);
            let ptr_type = self.context.ptr_type(AddressSpace::default());
            let waiting = self
                .builder
                .build_int_to_ptr(owed, ptr_type, "owed")
                .expect(POSITIONED);
            let index = self.context.i32_type().const_int(index, false).into();
            self.push_pending(pending, [index, address.into(), waiting.into()]);
            self.show_enter_link(show, (id, variant, field), address, i64_type.const_zero());
            self.builder.position_at_end(resume);
            address = show.popped[1].as_basic_value().into_pointer_value();
            owed = self
                .builder
                .build_ptr_to_int(
                    show.popped[2].as_basic_value().into_pointer_value(),
                    i64_type,
                    "owed",
                )
                .expect(POSITIONED);
        }
        if let Some(field) = looped {
            let more = self
                .builder
                .build_int_nuw_add(owed, i64_type.const_int(1, false), "owed")
                .expect(POSITIONED);
            self.show_enter_link(show, (id, variant, field), address, more);
        } else {
            self.show_text("sisu_show_close", show.sink, ")");
            self.show_pop(show, owed);
        }
    }

    /// Loops into link `field` of variant `variant` in the value of the enum `id` at `address`,
    /// with `owed` for the linked value. A link that holds none (an absent `G?`, a null box, a
    /// clear flag of a boxed `G??`) writes `None` by the Some rule and goes to `pop`.
    fn show_enter_link(
        &self,
        show: &ShowLoop<'ctx>,
        (id, variant, field): (EnumId, usize, usize),
        address: PointerValue<'ctx>,
        owed: IntValue<'ctx>,
    ) {
        let payload = &self.program.enums[id.0].variants[variant].fields[field];
        let ty = payload.ty;
        let inner = ty
            .inline_enum()
            .expect("a link should be of an enum type or its optional");
        let linked = if payload.boxed {
            // A boxed `F` with k `?`s is k - 1 flags around a box pointer; a clear flag at layer
            // j is a `None` under j `Some`s, and a null box one under k - 1.
            let mut slot = self.field_slot(address, id, variant, field);
            let mut layer = ty;
            let mut depth = 0;
            while let Some(contents) = nested(layer) {
                let flag = self.load_layer_flag(layer, slot);
                self.show_unless(show, flag, depth, owed);
                slot = self.layer_place(layer, slot, 1);
                (layer, depth) = (contents, depth + 1);
            }
            let boxed = self
                .builder
                .build_load(self.context.ptr_type(AddressSpace::default()), slot, "box")
                .expect(POSITIONED)
                .into_pointer_value();
            if matches!(layer.kind(), TypeKind::Optional(_)) {
                let present = self
                    .builder
                    .build_is_not_null(boxed, "present")
                    .expect(POSITIONED);
                self.show_unless(show, present, depth, owed);
            }
            self.box_value(boxed_enum(ty), boxed)
        } else {
            // Each `?` of the link adds a `Some(` to the `None` that an absent layer writes.
            let mut place = self.payload_place(address, id, variant, field);
            let (mut layer, mut depth) = (ty, 0);
            while let TypeKind::Optional(contents) = layer.kind() {
                let link = self.load_link(layer, place);
                let present = self.has_value(layer, link, "present");
                self.show_unless(show, present, depth, owed);
                place = self.optional_value_place(layer, place);
                (layer, depth) = (contents, depth + 1);
            }
            place
        };
        let from = self.current_block();
        let kind = show
            .kinds
            .get(&inner)
            .expect("a link should enter a member of the component");
        let kind = self.context.i32_type().const_int(*kind, false);
        show.current[0].add_incoming(&[(&kind, from)]);
        show.current[1].add_incoming(&[(&linked, from)]);
        show.current[2].add_incoming(&[(&owed, from)]);
        self.branch_to(show.top);
    }

    /// Goes on when `present` holds. Otherwise writes `None` under `depth` `Some`s and goes to
    /// `pop` with `owed`.
    fn show_unless(
        &self,
        show: &ShowLoop<'ctx>,
        present: IntValue<'ctx>,
        depth: usize,
        owed: IntValue<'ctx>,
    ) {
        let function = self.current_function();
        let (none, some) = (
            self.context.append_basic_block(function, "link.none"),
            self.context.append_basic_block(function, "link.some"),
        );
        self.branch_on(present, some, none);
        self.builder.position_at_end(none);
        self.show_none(show.sink, depth);
        self.show_pop(show, owed);
        self.builder.position_at_end(some);
    }
}
