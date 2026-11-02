//! `any` values: a pair of pointers, the data and the table. The data of an object is the object
//! itself, which holds its count first. The data of any other value is a counted box,
//! `{ i64 count, T value }`, that the conversion allocates and that every copy of the `any`
//! value shares. The table is one constant global for each type and `any` type, named
//! `sisu.<type>:<interface>` (`sisu.Circle:Shape`), a struct of pointers:
//!
//! - slot 0 drops the data: `@sisu_rc.drop.<C>` for an object, `@sisu_any.drop.<T>` for a box,
//!   which a release calls once the count is 0;
//! - slots 1 and 2 show the value, as `print` and as `dbg` do: `(sink, data, quoted)`;
//! - then one entry for each requirement of each interface of the layout of the `any` type
//!   (`tir::AnyLayout`), in its order: `(data, owned arguments...)` returning the result;
//! - then a pointer to the table of each interface of the layout after the first, so that
//!   converting an `any J` to an `any I` that `J` inherits, or to `any Display`, loads the table
//!   of `I` without knowing the type inside.
//!
//! An entry is glue for one concrete method. The `DynCall` that reaches it reads its receiver in
//! place and owns every other argument, so the entry adapts to the method's own flags: it reads
//! the value out of a box, retains the receiver when the method owns `self`, passes on each
//! argument that the method owns, and releases each argument that the method only borrows, after
//! the call, since the caller of a borrowing method keeps its arguments alive for the whole call
//! (ADR 0029).

use inkwell::AddressSpace;
use inkwell::module::Linkage;
use inkwell::types::{BasicMetadataTypeEnum, BasicType, StructType};
use inkwell::values::{
    BasicMetadataValueEnum, BasicValue, BasicValueEnum, FunctionValue, PointerValue,
};

use super::layout::HeapBlock;
use super::rc::{Helper, Mode};
use super::{Codegen, POSITIONED};
use crate::tir::{FuncId, InterfaceId, Type, TypeArgs, TypeKind};

/// Whether the data of an `any` value that holds a `ty` is the object itself. Any other value is
/// in a box.
fn is_object(ty: Type) -> bool {
    matches!(ty.kind(), TypeKind::Class(..))
}

/// The slot of a table that holds the drop function.
const DROP: u32 = 0;
/// The slots that hold the helper that writes the value as `print` does, and as `dbg` does.
const SHOW_PRINT: u32 = 1;
const SHOW_DBG: u32 = 2;
/// The slot of the first entry.
const FIRST_ENTRY: u32 = 3;

impl<'ctx> Codegen<'ctx, '_> {
    /// The type of an `any` value: the data pointer, then the table pointer.
    pub(super) fn any_type(&self) -> StructType<'ctx> {
        let ptr = self.context.ptr_type(AddressSpace::default()).into();
        self.context.struct_type(&[ptr, ptr], false)
    }

    /// `{ i64 count, T value }`: the box that holds a value of type `ty` in an `any` value.
    pub(super) fn value_box(&self, ty: Type) -> StructType<'ctx> {
        let value = self
            .llvm_type(ty)
            .expect("a boxed value should have a value type");
        self.context
            .struct_type(&[self.context.i64_type().into(), value], false)
    }

    /// The value that `data`, the data of an `any` value that holds a `ty`, stands for: the
    /// object itself, or the value that its box holds. The box keeps its reference.
    fn data_value(&self, ty: Type, data: PointerValue<'ctx>) -> BasicValueEnum<'ctx> {
        if is_object(ty) {
            return data.into();
        }
        let value = self
            .builder
            .build_struct_gep(self.value_box(ty), data, 1, "boxed")
            .expect("a box should hold its value after its count");
        let value_type = self
            .llvm_type(ty)
            .expect("a boxed value should have a value type");
        self.builder
            .build_load(value_type, value, "value")
            .expect(POSITIONED)
    }

    /// A new box with count 1 that holds `value`, of type `ty`, which the box takes over from
    /// its owner: no retain.
    fn new_box(&self, ty: Type, value: BasicValueEnum<'ctx>) -> PointerValue<'ctx> {
        let (size, align) = self.heap_size(&HeapBlock::Value(ty));
        let block = self
            .builder
            .build_call(self.alloc, &[size.into(), align.into()], "box")
            .expect(POSITIONED)
            .try_as_basic_value()
            .basic()
            .expect("`sisu_alloc` should return a pointer, as the runtime declares it")
            .into_pointer_value();
        self.init_count(block);
        let slot = self
            .builder
            .build_struct_gep(self.value_box(ty), block, 1, "boxed")
            .expect("a box should hold its value after its count");
        self.builder.build_store(slot, value).expect(POSITIONED);
        block
    }

    /// `@sisu_any.drop.<T>(box)`: the drop of a box that holds a `ty`, for slot 0 of a table:
    /// releases the value, and frees the box. An object's drop is its class helper.
    fn drop_glue(&self, ty: Type) -> FunctionValue<'ctx> {
        if is_object(ty) {
            return self.helper(Helper::Drop, ty);
        }
        let name = format!("sisu_any.drop.{}", self.program.type_name(ty));
        if let Some(glue) = self.module.get_function(&name) {
            return glue;
        }
        let ptr = self.context.ptr_type(AddressSpace::default());
        let glue = self.module.add_function(
            &name,
            self.context.void_type().fn_type(&[ptr.into()], false),
            Some(Linkage::Internal),
        );
        let entry = self.context.append_basic_block(glue, "entry");
        self.builder.position_at_end(entry);
        let data = glue
            .get_nth_param(0)
            .expect("a drop glue should take its box")
            .into_pointer_value();
        self.release(self.data_value(ty, data), ty);
        let (size, align) = self.heap_size(&HeapBlock::Value(ty));
        self.call(self.free, &[data.into(), size.into(), align.into()]);
        self.builder.build_return(None).expect(POSITIONED);
        glue
    }

    /// The interfaces of the table of the `any` type `any`, as `mono` laid them out.
    fn any_layout(&self, any: Type) -> &[(InterfaceId, TypeArgs)] {
        self.any_layouts
            .get(&any)
            .expect("mono should give every `any` type that the program names a layout")
    }

    /// How many requirements the interfaces of the layout of `any` have, before the interface at
    /// place `before` in it: the place of an entry among the entries.
    fn entries_before(&self, any: Type, before: usize) -> u32 {
        let count: usize = (self.any_layout(any).iter().take(before))
            .map(|&(interface, _)| self.program.interfaces[interface.0].methods.len())
            .sum();
        u32::try_from(count).expect("the requirements of a table should fit in 32 bits")
    }

    /// The struct type of the table of the `any` type `any`: a pointer for each slot.
    fn table_type(&self, any: Type) -> StructType<'ctx> {
        let layout = self.any_layout(any);
        let slots = FIRST_ENTRY
            + self.entries_before(any, layout.len())
            + u32::try_from(layout.len() - 1).expect("a layout should have few interfaces");
        let ptr = self.context.ptr_type(AddressSpace::default());
        self.context
            .struct_type(&vec![ptr.into(); slots as usize], false)
    }

    /// The name of the table of the object type `ty` for the `any` type `any`.
    fn table_name(&self, ty: Type, any: Type) -> String {
        let TypeKind::Any(interface, args) = any.kind() else {
            unreachable!("a table should be for an `any` type");
        };
        format!(
            "sisu.{}:{}{}",
            self.program.type_name(ty),
            self.program.interfaces[interface.0].name,
            self.program.args_name(args.get())
        )
    }

    /// Declares the global of each table that `mono` made, so that a conversion can name it
    /// before `define_any_tables` fills it.
    pub(super) fn declare_any_tables(&self) {
        for table in &self.program.any_tables {
            let global = self.module.add_global(
                self.table_type(table.any),
                None,
                &self.table_name(table.ty, table.any),
            );
            global.set_linkage(Linkage::Internal);
            global.set_constant(true);
            self.any_tables
                .borrow_mut()
                .insert((table.ty, table.any), global);
        }
    }

    /// The pointer to the table of the object type `ty` for the `any` type `any`.
    fn table_global(&self, ty: Type, any: Type) -> PointerValue<'ctx> {
        self.any_tables
            .borrow()
            .get(&(ty, any))
            .expect("mono should make the table of each object type that converts to `any`")
            .as_pointer_value()
    }

    /// Gives each declared table its slots, and defines the entries and the show helpers that
    /// they name.
    pub(super) fn define_any_tables(&self) {
        for table in &self.program.any_tables {
            let layout = self.any_layout(table.any);
            let as_pointer =
                |function: FunctionValue<'ctx>| function.as_global_value().as_pointer_value();
            let mut slots: Vec<PointerValue<'ctx>> = vec![
                as_pointer(self.drop_glue(table.ty)),
                as_pointer(self.show_glue(table.ty, Mode::Print)),
                as_pointer(self.show_glue(table.ty, Mode::Dbg)),
            ];
            for methods in &table.methods {
                for &method in methods {
                    slots.push(as_pointer(self.entry_glue(table.ty, method)));
                }
            }
            for &(interface, args) in &layout[1..] {
                let inner = Type::intern(TypeKind::Any(interface, args));
                slots.push(self.table_global(table.ty, inner));
            }
            let global = self.any_tables.borrow()[&(table.ty, table.any)];
            global.set_initializer(&self.context.const_struct(
                &slots.iter().map(|&slot| slot.into()).collect::<Vec<_>>(),
                false,
            ));
            debug_assert_eq!(
                slots.len(),
                self.table_type(table.any).count_fields() as usize,
                "a table should have a slot for each part of its layout"
            );
        }
    }

    /// `@sisu_any.show.<T>` or `@sisu_any.show.dbg.<T>(sink, data, quoted)`: the show helper of
    /// the type `ty` for a table: `show_value` of the value that the data stands for.
    fn show_glue(&self, ty: Type, mode: Mode) -> FunctionValue<'ctx> {
        let name = match mode {
            Mode::Print => format!("sisu_any.show.{}", self.program.type_name(ty)),
            Mode::Dbg => format!("sisu_any.show.dbg.{}", self.program.type_name(ty)),
        };
        if let Some(glue) = self.module.get_function(&name) {
            return glue;
        }
        let ptr = self.context.ptr_type(AddressSpace::default());
        let fn_type = self.context.void_type().fn_type(
            &[ptr.into(), ptr.into(), self.context.bool_type().into()],
            false,
        );
        let glue = self
            .module
            .add_function(&name, fn_type, Some(Linkage::Internal));
        let entry = self.context.append_basic_block(glue, "entry");
        self.builder.position_at_end(entry);
        let (sink, data) = (glue.get_nth_param(0), glue.get_nth_param(1));
        let (Some(sink), Some(data)) = (sink, data) else {
            unreachable!("a show glue should have its parameters");
        };
        let value = self.data_value(ty, data.into_pointer_value());
        let sink = sink.into_pointer_value();
        if is_object(ty) {
            // An object's text does not depend on `quoted`.
            self.show_value(sink, ty, value, false, mode);
            self.builder.build_return(None).expect(POSITIONED);
            return glue;
        }
        let quoted = glue
            .get_nth_param(2)
            .expect("a show glue should have its `quoted`")
            .into_int_value();
        // `show_value` takes `quoted` as a constant, so the glue has a branch for each.
        let (plain, marked) = (
            self.context.append_basic_block(glue, "plain"),
            self.context.append_basic_block(glue, "quoted"),
        );
        let done = self.context.append_basic_block(glue, "done");
        self.branch_on(quoted, marked, plain);
        for (block, flag) in [(plain, false), (marked, true)] {
            self.builder.position_at_end(block);
            self.show_value(sink, ty, value, flag, mode);
            self.branch_to(done);
        }
        self.builder.position_at_end(done);
        self.builder.build_return(None).expect(POSITIONED);
        glue
    }

    /// The entry of a table for `method`, a method of the type `ty`, or for the
    /// `to_string` that the compiler writes when `method` is `None`.
    fn entry_glue(&self, ty: Type, method: Option<FuncId>) -> FunctionValue<'ctx> {
        let Some(method) = method else {
            return self.generated_to_string_glue(ty);
        };
        let function = &self.mir.functions[method.0];
        let name = format!("sisu_any.entry.{}", function.name);
        if let Some(glue) = self.module.get_function(&name) {
            return glue;
        }
        assert!(
            !function.prelude,
            "no table entry should call a method of the prelude"
        );
        // The receiver is the data, a pointer, whatever the method's `self` is.
        let ptr = self.context.ptr_type(AddressSpace::default());
        let params: Vec<BasicMetadataTypeEnum<'ctx>> = function.locals[..function.params]
            .iter()
            .enumerate()
            .map(|(i, param)| {
                if i == 0 {
                    return ptr.into();
                }
                self.llvm_type(param.ty)
                    .expect("a parameter should have a value type")
                    .into()
            })
            .collect();
        let fn_type = match self.llvm_type(function.ret) {
            Some(ret) => ret.fn_type(&params, false),
            None => self.context.void_type().fn_type(&params, false),
        };
        let glue = self
            .module
            .add_function(&name, fn_type, Some(Linkage::Internal));
        let entry = self.context.append_basic_block(glue, "entry");
        self.builder.position_at_end(entry);
        let mut args: Vec<BasicValueEnum<'ctx>> = glue.get_param_iter().collect();
        // The receiver is the object, or the value in the box. A method that owns `self` gets a
        // reference of its own, as the caller of a borrowing method keeps its.
        args[0] = self.data_value(ty, args[0].into_pointer_value());
        if !function.borrowed[0] {
            self.retain(args[0], ty);
        }
        let call_args: Vec<BasicMetadataValueEnum<'ctx>> =
            args.iter().map(|&arg| arg.into()).collect();
        let result = self.call_value(method, &call_args);
        // An argument that the method borrows is the entry's to release, after the call.
        for (i, local) in function.locals[..function.params]
            .iter()
            .enumerate()
            .skip(1)
        {
            if function.borrowed[i] && local.ty.is_counted(&self.program.enums) {
                self.release(args[i], local.ty);
            }
        }
        match result {
            Some(value) => self.builder.build_return(Some(&value)),
            None => self.builder.build_return(None),
        }
        .expect(POSITIONED);
        glue
    }

    /// `@sisu_any.to_string.<T>(data) -> string`: the text that `print` writes of the type
    /// `ty` when it has no written `to_string`, which every table holds beside the entries
    /// of its interfaces.
    fn generated_to_string_glue(&self, ty: Type) -> FunctionValue<'ctx> {
        let name = format!("sisu_any.to_string.{}", self.program.type_name(ty));
        if let Some(glue) = self.module.get_function(&name) {
            return glue;
        }
        let ptr = self.context.ptr_type(AddressSpace::default());
        let glue = self.module.add_function(
            &name,
            ptr.fn_type(&[ptr.into()], false),
            Some(Linkage::Internal),
        );
        let entry = self.context.append_basic_block(glue, "entry");
        self.builder.position_at_end(entry);
        let data = glue
            .get_nth_param(0)
            .expect("a `to_string` glue should have its receiver");
        let sink = self.buffer_sink();
        let value = self.data_value(ty, data.into_pointer_value());
        self.show_value(sink, ty, value, false, Mode::Print);
        let text = self.finish_buffer_sink(sink);
        self.builder.build_return(Some(&text)).expect(POSITIONED);
        glue
    }

    /// `ToAny`: `value`, of type `source`, as the `any` type `target`. An object keeps its pointer
    /// and takes the table that `mono` made for it. Any other value, which the conversion
    /// consumes, goes into a new box that takes the table. An `any` value keeps its data and
    /// loads the table of the interface that `target` names from its own.
    pub(super) fn to_any(
        &self,
        source: Type,
        value: BasicValueEnum<'ctx>,
        target: Type,
    ) -> BasicValueEnum<'ctx> {
        let (data, table): (BasicValueEnum<'ctx>, BasicValueEnum<'ctx>) = match source.kind() {
            TypeKind::Class(..) => (value, self.table_global(source, target).into()),
            TypeKind::Any(..) => {
                let TypeKind::Any(interface, args) = target.kind() else {
                    unreachable!("a conversion to `any` should have an `any` target");
                };
                let at = self
                    .any_layout(source)
                    .iter()
                    .position(|&entry| entry == (interface, args))
                    .expect("an `any` value should convert to the interfaces of its layout");
                let slot = FIRST_ENTRY
                    + self.entries_before(source, self.any_layout(source).len())
                    + u32::try_from(at - 1).expect("a layout should have few interfaces");
                let (data, table) = self.any_parts(value);
                (data.into(), self.table_slot(source, table, slot).into())
            }
            _ => (
                self.new_box(source, value).into(),
                self.table_global(source, target).into(),
            ),
        };
        let pair = self.any_type().get_undef();
        let pair = self
            .builder
            .build_insert_value(pair, data, 0, "data")
            .expect(POSITIONED);
        self.builder
            .build_insert_value(pair, table, 1, "any")
            .expect(POSITIONED)
            .as_basic_value_enum()
    }

    /// The data pointer and the table pointer of the `any` value `value`.
    fn any_parts(&self, value: BasicValueEnum<'ctx>) -> (PointerValue<'ctx>, PointerValue<'ctx>) {
        let part = |index, name| {
            self.builder
                .build_extract_value(value.into_struct_value(), index, name)
                .expect("an `any` value should be a pair of pointers")
                .into_pointer_value()
        };
        (part(0, "data"), part(1, "table"))
    }

    /// The pointer that slot `slot` of `table`, a table for the `any` type `any`, holds.
    fn table_slot(&self, any: Type, table: PointerValue<'ctx>, slot: u32) -> PointerValue<'ctx> {
        let address = self
            .builder
            .build_struct_gep(self.table_type(any), table, slot, "slot")
            .expect("a slot should lie in its table");
        self.load_pointer(address)
    }

    /// The data pointer of an `any` value or its optional: what `is` compares.
    pub(super) fn any_data(&self, value: BasicValueEnum<'ctx>) -> PointerValue<'ctx> {
        self.any_parts(value).0
    }

    /// `DynCall`: the requirement `method` of `interface`, with the arguments `iargs` after
    /// `Self`, called on `receiver`, an `any` value of type `any`, with the owned `args`.
    pub(super) fn dyn_call(
        &self,
        any: Type,
        (interface, iargs, method): (InterfaceId, TypeArgs, u32),
        receiver: BasicValueEnum<'ctx>,
        args: &[BasicValueEnum<'ctx>],
        ret: Option<Type>,
    ) -> Option<BasicValueEnum<'ctx>> {
        let at = self
            .any_layout(any)
            .iter()
            .position(|&entry| entry == (interface, iargs))
            .expect("the interface of a call through a table should be in its layout");
        let slot = FIRST_ENTRY + self.entries_before(any, at) + method;
        let (data, table) = self.any_parts(receiver);
        let entry = self.table_slot(any, table, slot);
        let mut params: Vec<BasicMetadataTypeEnum<'ctx>> =
            vec![self.context.ptr_type(AddressSpace::default()).into()];
        params.extend(
            args.iter()
                .map(|arg| BasicMetadataTypeEnum::from(arg.get_type())),
        );
        let fn_type = match ret.and_then(|ret| self.llvm_type(ret)) {
            Some(ret) => ret.fn_type(&params, false),
            None => self.context.void_type().fn_type(&params, false),
        };
        let mut call_args: Vec<BasicMetadataValueEnum<'ctx>> = vec![data.into()];
        call_args.extend(args.iter().map(|&arg| BasicMetadataValueEnum::from(arg)));
        self.builder
            .build_indirect_call(fn_type, entry, &call_args, "dyn")
            .expect(POSITIONED)
            .try_as_basic_value()
            .basic()
    }

    /// Writes the text of the `any` value `value`, of type `any`, through the show helper of its
    /// table: what `show_value` does for the type inside, quoted or not.
    pub(super) fn show_any(
        &self,
        sink: PointerValue<'ctx>,
        any: Type,
        value: BasicValueEnum<'ctx>,
        quoted: bool,
        mode: Mode,
    ) {
        let (data, table) = self.any_parts(value);
        let slot = match mode {
            Mode::Print => SHOW_PRINT,
            Mode::Dbg => SHOW_DBG,
        };
        let helper = self.table_slot(any, table, slot);
        let ptr = self.context.ptr_type(AddressSpace::default());
        let fn_type = self.context.void_type().fn_type(
            &[ptr.into(), ptr.into(), self.context.bool_type().into()],
            false,
        );
        let quoted = self.context.bool_type().const_int(u64::from(quoted), false);
        self.builder
            .build_indirect_call(
                fn_type,
                helper,
                &[sink.into(), data.into(), quoted.into()],
                "",
            )
            .expect(POSITIONED);
    }

    /// Adds 1 to the count of the data of the `any` value `value`.
    pub(super) fn retain_any(&self, value: BasicValueEnum<'ctx>) {
        let (data, _) = self.any_parts(value);
        self.call(self.rc_retain, &[data.into()]);
    }

    /// Subtracts 1 from the count of the data of the `any` value `value`, and at 0 runs the drop
    /// function of its table. A zero value, which a move leaves, releases nothing.
    pub(super) fn release_any(&self, value: BasicValueEnum<'ctx>) {
        self.call(self.any_release(), &[value.into()]);
    }

    /// `@sisu_any.release(any)`: the release of every `any` value, as its table is its own.
    fn any_release(&self) -> FunctionValue<'ctx> {
        const NAME: &str = "sisu_any.release";
        if let Some(function) = self.module.get_function(NAME) {
            return function;
        }
        let function = self.module.add_function(
            NAME,
            self.context
                .void_type()
                .fn_type(&[self.any_type().into()], false),
            Some(Linkage::Internal),
        );
        let current = self.builder.get_insert_block();
        let entry = self.context.append_basic_block(function, "entry");
        let counted = self.context.append_basic_block(function, "object");
        let drop_block = self.context.append_basic_block(function, "drop");
        let done = self.context.append_basic_block(function, "done");
        self.builder.position_at_end(entry);
        let value = function
            .get_nth_param(0)
            .expect("the release of an `any` should take the value");
        let (data, table) = self.any_parts(value);
        let null = self.builder.build_is_null(data, "null").expect(POSITIONED);
        self.branch_on(null, done, counted);
        self.builder.position_at_end(counted);
        self.decrement(data, drop_block, done);
        self.builder.position_at_end(drop_block);
        // Every table keeps the drop function in slot 0, whatever its interface.
        let ptr = self.context.ptr_type(AddressSpace::default());
        let drop_slot = self
            .builder
            .build_struct_gep(
                self.context.struct_type(&[ptr.into()], false),
                table,
                DROP,
                "drop",
            )
            .expect("a table should hold its drop function first");
        let drop = self.load_pointer(drop_slot);
        self.builder
            .build_indirect_call(
                self.context.void_type().fn_type(&[ptr.into()], false),
                drop,
                &[data.into()],
                "",
            )
            .expect(POSITIONED);
        self.branch_to(done);
        self.builder.position_at_end(done);
        self.builder.build_return(None).expect(POSITIONED);
        if let Some(block) = current {
            self.builder.position_at_end(block);
        }
        function
    }
}
