//! Standard input and the program's arguments: `read_line`, through the runtime's
//! `sisu_read_line`, and `main`'s `args`, through `sisu_args_len` and `sisu_arg`.

use inkwell::values::{FunctionValue, IntValue, PointerValue};

use super::{Codegen, POSITIONED};
use crate::tir::Type;

impl<'ctx> Codegen<'ctx, '_> {
    /// `sisu_read_line(len) -> ptr` from the runtime, declared on first use. It never unwinds,
    /// and has no `memory` attribute: each call overwrites the line an earlier call returned,
    /// which generated code can reach, so no encoding of its effects is sound.
    fn read_line_function(&self) -> FunctionValue<'ctx> {
        self.runtime("sisu_read_line")
    }

    /// `ReadLine`: the next line of standard input as a new string, copied at once, before
    /// anything can call `sisu_read_line` again; `null`, which is `None`, at the end of input.
    pub(super) fn read_line(&self) -> PointerValue<'ctx> {
        let i64_type = self.context.i64_type();
        let len_slot = self.entry_alloca(i64_type.into(), "len");
        let bytes = self
            .builder
            .build_call(self.read_line_function(), &[len_slot.into()], "line")
            .expect(POSITIONED)
            .try_as_basic_value()
            .basic()
            .expect("`sisu_read_line` should return a pointer, as the runtime declares it")
            .into_pointer_value();
        let function = self.current_function();
        let (copy, join) = (
            self.context.append_basic_block(function, "copy"),
            self.context.append_basic_block(function, "join"),
        );
        let ended = self.current_block();
        let none = self
            .builder
            .build_is_null(bytes, "ended")
            .expect(POSITIONED);
        self.builder
            .build_conditional_branch(none, join, copy)
            .expect(POSITIONED);
        self.builder.position_at_end(copy);
        let len = self
            .builder
            .build_load(i64_type, len_slot, "len")
            .expect(POSITIONED)
            .into_int_value();
        let string = self.string_from_bytes(bytes, len);
        let ty = Type::optional(Type::STRING);
        let some = self.context.bool_type().const_int(1, false);
        let line = self.optional(ty, some, string.into());
        self.branch_to(join);
        self.builder.position_at_end(join);
        let absent = self.empty(ty);
        self.phi(absent.get_type(), &[(absent, ended), (line, copy)])
            .into_pointer_value()
    }

    /// The C `main`'s `argc` and `argv`, passed as `count` and `strings`, as `main`'s `args`: a
    /// new `[string]` of the arguments after the program name. `sisu_args_len(argc)` counts
    /// them, and reads no memory. `sisu_arg(argv, index, len)` gives each, which is copied at
    /// once, before the next call can overwrite it; like `sisu_read_line`, it has no `memory`
    /// attribute. Neither unwinds.
    pub(super) fn args(
        &self,
        count: IntValue<'ctx>,
        strings: PointerValue<'ctx>,
    ) -> PointerValue<'ctx> {
        let i64_type = self.context.i64_type();
        let args_len = self.runtime("sisu_args_len");
        let arg = self.runtime("sisu_arg");
        let len = self
            .builder
            .build_call(args_len, &[count.into()], "args.len")
            .expect(POSITIONED)
            .try_as_basic_value()
            .basic()
            .expect("`sisu_args_len` should return an `i64`, as the runtime declares it")
            .into_int_value();
        let array = self.alloc_array(Type::STRING, len, None);
        let len_slot = self.entry_alloca(i64_type.into(), "len");
        self.count_loop(len, |index| {
            let bytes = self
                .builder
                .build_call(arg, &[strings.into(), index.into(), len_slot.into()], "arg")
                .expect(POSITIONED)
                .try_as_basic_value()
                .basic()
                .expect("`sisu_arg` should return a pointer, as the runtime declares it")
                .into_pointer_value();
            let n = self.load(i64_type.into(), len_slot).into_int_value();
            let string = self.string_from_bytes(bytes, n);
            self.store(self.element_ptr(Type::STRING, array, index), string.into());
        });
        array
    }
}
