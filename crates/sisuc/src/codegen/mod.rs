//! Builds LLVM IR through inkwell and writes it out as a native object file. Each mir function
//! (`crate::mir`) becomes one LLVM function, and each of its blocks one LLVM block; the
//! emitters in the submodules build the code of each statement, rvalue and terminator.

mod abi;
mod arith;
mod array;
mod enums;
mod io;
mod layout;
mod matching;
mod math;
mod rc;
mod string;

use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::fmt::Write as _;

use inkwell::attributes::{Attribute, AttributeLoc};
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::memory_buffer::MemoryBuffer;
use inkwell::module::{Linkage, Module};
use inkwell::passes::PassBuilderOptions;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetData, TargetMachine,
};
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum};
use inkwell::values::{
    BasicMetadataValueEnum, BasicValue, BasicValueEnum, FunctionValue, IntValue, MetadataValue,
    PointerValue,
};
use inkwell::{AddressSpace, IntPredicate, OptimizationLevel};

use self::array::element_of;
use self::enums::EnumLayout;
use self::layout::{ClassLayout, HeapBlock, tbaa_tag};
use self::rc::Helper;
use crate::mir::{self, BlockId, Const, Operand, Place, Projection, Rvalue, Statement, Terminator};
use crate::source::{LineIndex, Span};
use crate::tir::{ClassId, EnumId, FuncId, LocalId, Program, Stream, Type, TypeKind, UnaryOp};

/// The `expect` message for a builder call: codegen positions the builder before it emits code.
const POSITIONED: &str =
    "the builder should be positioned in a block while a function body is emitted";

#[cfg(test)]
thread_local! {
    /// How many class fields and enum payload fields the reflexivity analysis has read on this
    /// thread.
    static TYPE_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl<'ctx, 'src> Codegen<'ctx, 'src> {
    /// An empty module for `program`, with the runtime's declarations and no function defined.
    fn new(
        context: &'ctx Context,
        program: &'src Program,
        path: &'src str,
        source: &'src str,
        machine: &TargetMachine,
        rc_checks: bool,
    ) -> Self {
        let module = context.create_module("main");
        // The passes and the object file both read the target from the module.
        let target_data = machine.get_target_data();
        module.set_triple(&machine.get_triple());
        module.set_data_layout(&target_data.get_data_layout());
        let ptr_type = context.ptr_type(AddressSpace::default());
        let void = context.void_type();

        // `sisu_alloc(size, align)` and `sisu_free(ptr, size, align)`, from the runtime.
        let alloc = abi::declare(&module, abi::row("sisu_alloc"));
        let free = abi::declare(&module, abi::row("sisu_free"));
        // `@sisu_rc.retain(object)`; see `define_helpers`.
        let rc_retain = module.add_function(
            "sisu_rc.retain",
            void.fn_type(&[ptr_type.into()], false),
            Some(Linkage::Internal),
        );

        // The runtime's print functions, one set for each `Stream`; `Stream::index` indexes them.
        let printers = [Stream::Stdout, Stream::Stderr]
            .map(|stream| declare_printers(&module, stream.symbol_prefix()));
        let str_compare = abi::declare(&module, abi::row("sisu_str_compare"));
        // `sisu_panic(msg, len)` prints the message and exits; `sisu_panic_bounds(msg, len,
        // index, length)` prints a bounds panic and exits; `sisu_panic_message(at, at_len, msg,
        // msg_len)` prints a `panic` call's panic and exits.
        let panic = abi::declare(&module, abi::row("sisu_panic"));
        let panic_bounds = abi::declare(&module, abi::row("sisu_panic_bounds"));
        let panic_message = abi::declare(&module, abi::row("sisu_panic_message"));
        // Declared only with the checks, so that a plain build's module stays as it was.
        let rc_fault = rc_checks.then(|| abi::declare(&module, abi::row("sisu_rc_fault")));
        let tbaa_root = context.metadata_node(&[context.metadata_string("sisu").into()]);

        Codegen {
            context,
            module,
            builder: context.create_builder(),
            printers,
            str_compare,
            panic,
            panic_bounds,
            panic_message,
            alloc,
            free,
            rc_retain,
            rc_fault,
            tbaa_kind: context.get_kind_id("tbaa"),
            header_tag: tbaa_tag(context, tbaa_root, "count and length"),
            element_tag: tbaa_tag(context, tbaa_root, "array element"),
            tbaa_root,
            program,
            reaches_f64: rc::reaching_f64(program),
            helpers: RefCell::new(FxHashMap::default()),
            pending: RefCell::new(Vec::new()),
            target_data,
            path,
            source,
            lines: LineIndex::new(source),
            classes: Vec::new(),
            enums: Vec::new(),
            functions: Vec::new(),
            slots: Vec::new(),
            flagged: Vec::new(),
            types: Vec::new(),
            blocks: Vec::new(),
        }
    }
}

/// Compiles a program that passed `check`, whose mir is `mir` (`mir::build`), into an LLVM
/// module for `machine`; `program` gives the classes, the enums and the types. With
/// `rc_checks`, a retain or release of an object whose count is below 1, and a read of a `string`,
/// class or array local that was moved, call `sisu_rc_fault`, and `@main` calls
/// `sisu_rc_check_exit` after `sisu.main` returns. `@main` always calls `sisu_flush_stdout`
/// first.
pub(crate) fn compile<'ctx>(
    context: &'ctx Context,
    program: &Program,
    mir: &mir::Program,
    path: &str,
    source: &str,
    machine: &TargetMachine,
    rc_checks: bool,
) -> Module<'ctx> {
    let mut codegen = Codegen::new(context, program, path, source, machine, rc_checks);
    codegen.lay_out_types();
    // Every class gets its release helper, used or not; an `eq` helper is made on first use.
    for id in 0..program.classes.len() {
        codegen.helper(Helper::Release, Type::intern(TypeKind::Class(ClassId(id))));
    }
    // Declare every function first so that calls can refer to functions defined later.
    for f in &mir.functions {
        codegen.declare(f);
    }
    for (i, f) in mir.functions.iter().enumerate() {
        let function = codegen.functions[i];
        codegen.define(f, function);
    }
    codegen.c_main();
    codegen.define_helpers();
    codegen.module
}

/// The runtime's print functions for one stream, by operand type.
#[derive(Clone, Copy)]
struct Printers<'ctx> {
    int: FunctionValue<'ctx>,
    f64: FunctionValue<'ctx>,
    bool: FunctionValue<'ctx>,
    char: FunctionValue<'ctx>,
    str: FunctionValue<'ctx>,
}

/// Declares `<prefix>_int`, `_f64`, `_bool`, `_char` and `_str`, each from its row of
/// `abi::RUNTIME`.
fn declare_printers<'ctx>(module: &Module<'ctx>, prefix: &str) -> Printers<'ctx> {
    let declare = |kind: &str| abi::declare(module, abi::row(&format!("{prefix}_{kind}")));
    // `bool` first: `--emit ir-raw` lists the declarations in the order they were added.
    let bool = declare("bool");
    Printers {
        int: declare("int"),
        f64: declare("f64"),
        bool,
        char: declare("char"),
        str: declare("str"),
    }
}

/// The class of a field access's base, whose type is `ty`.
fn class_of(ty: Type) -> ClassId {
    let TypeKind::Class(id) = ty.kind() else {
        unreachable!("the checker should only allow field access on a class-typed value")
    };
    id
}

/// The enum of a payload projection's base, whose type is `ty`.
fn enum_of(ty: Type) -> EnumId {
    let TypeKind::Enum(id) = ty.kind() else {
        unreachable!("mir should project a payload only out of an enum")
    };
    id
}

fn zeroext(context: &Context) -> Attribute {
    context.create_enum_attribute(Attribute::get_named_enum_kind_id("zeroext"), 0)
}

struct Codegen<'ctx, 'src> {
    context: &'ctx Context,
    module: Module<'ctx>,
    builder: Builder<'ctx>,
    /// The runtime's print functions, indexed by `Stream`.
    printers: [Printers<'ctx>; 2],
    str_compare: FunctionValue<'ctx>,
    panic: FunctionValue<'ctx>,
    panic_bounds: FunctionValue<'ctx>,
    panic_message: FunctionValue<'ctx>,
    alloc: FunctionValue<'ctx>,
    free: FunctionValue<'ctx>,
    /// `@sisu_rc.retain`, which every counted type shares.
    rc_retain: FunctionValue<'ctx>,
    /// `sisu_rc_fault(kind)`, declared only with the RC checks: `Some` turns them on.
    rc_fault: Option<FunctionValue<'ctx>>,
    /// The kind id of `!tbaa` metadata.
    tbaa_kind: u32,
    /// The TBAA tag of an array's length, and of every count that `load_count` and `store_count`
    /// read and write. An array element's loads and stores carry `element_tag`, so
    /// that LLVM knows a store to an element leaves those counts and lengths as they were. An
    /// enum element is loaded and stored whole with `element_tag` too. A tag describes only the
    /// bytes the access itself touches, and an element's bytes never hold a count or a length:
    /// the strings, objects and boxes an enum element points at keep their counts in their own
    /// blocks, as the strings of a `[string]` do. Other accesses, such as a string's length or a
    /// new object's count, carry no tag and may alias anything.
    header_tag: MetadataValue<'ctx>,
    element_tag: MetadataValue<'ctx>,
    /// The root of every TBAA type. Each class field has its own type, `ClassLayout::field_tag`:
    /// a field read and a field assignment tag their load and store with it, so that LLVM knows
    /// a store to one field leaves the other fields, and every count, length and element, as
    /// they were. That holds because nothing touches a field's bytes with another tag: other
    /// accesses to a field, such as a new object's stores, a drop's loads or an element write
    /// through a field, carry no tag.
    tbaa_root: MetadataValue<'ctx>,
    program: &'src Program,
    /// Whether each class, then each enum, reaches an `f64` through its fields; see
    /// `rc::reaching_f64`.
    reaches_f64: Vec<bool>,
    /// The helpers declared so far, by kind and counted type; see `rc::helper`.
    helpers: RefCell<FxHashMap<(Helper, Type), FunctionValue<'ctx>>>,
    /// The helpers declared but not yet defined; `define_helpers` empties it.
    pending: RefCell<Vec<(Helper, Type)>>,
    /// The target's layout: the sizes and alignments of classes.
    target_data: TargetData,
    /// The input path as given, and its text: panic messages name a position in it.
    path: &'src str,
    source: &'src str,
    /// The line starts of `source`, so that a panic site costs a binary search.
    lines: LineIndex,
    /// `%sisu.<Class>` and its field elements, indexed by `ClassId`.
    classes: Vec<ClassLayout<'ctx>>,
    /// `%sisu.<Enum>` and its payload structs, indexed by `EnumId`.
    enums: Vec<EnumLayout<'ctx>>,
    /// Indexed by `FuncId`.
    functions: Vec<FunctionValue<'ctx>>,
    /// The current function's locals, indexed by `LocalId`: each one's `alloca` and its LLVM
    /// type, `None` for `unit`.
    slots: Vec<Option<(PointerValue<'ctx>, BasicTypeEnum<'ctx>)>>,
    /// The current function's flagged locals, indexed by `LocalId`: a `Move` of one leaves the
    /// zero value in its slot (every local is flagged under the RC checks, which test it).
    flagged: Vec<bool>,
    /// The current function's local types, indexed by `LocalId`.
    types: Vec<Type>,
    /// The current function's blocks, indexed by `BlockId`.
    blocks: Vec<BasicBlock<'ctx>>,
}

impl<'ctx> Codegen<'ctx, '_> {
    /// A pointer to field `index` of `object`, an object of class `id`.
    fn field_ptr(
        &self,
        id: ClassId,
        object: PointerValue<'ctx>,
        index: usize,
    ) -> PointerValue<'ctx> {
        let layout = &self.classes[id.0];
        self.builder
            .build_struct_gep(layout.ty, object, layout.element[index], "field")
            .expect("the checker should only emit a field index that the class has")
    }

    /// The TBAA tag of field `index` of class `id`.
    fn field_tag(&self, id: ClassId, index: usize) -> MetadataValue<'ctx> {
        self.classes[id.0].field_tag[index]
    }

    /// `a is b`: whether two class or class-optional values are the same pointer.
    fn same_object(&self, a: PointerValue<'ctx>, b: PointerValue<'ctx>) -> IntValue<'ctx> {
        self.builder
            .build_int_compare(IntPredicate::EQ, a, b, "same")
            .expect(POSITIONED)
    }

    /// The runtime function `name`: the module's declaration if it has one, and otherwise the
    /// declaration `abi::row(name)` describes, added now. `compile` still declares some
    /// functions up front, whether or not the program calls them.
    pub(super) fn runtime(&self, name: &str) -> FunctionValue<'ctx> {
        self.module
            .get_function(name)
            .unwrap_or_else(|| abi::declare(&self.module, abi::row(name)))
    }

    fn call(&self, function: FunctionValue<'ctx>, args: &[BasicMetadataValueEnum<'ctx>]) {
        self.builder
            .build_call(function, args, "")
            .expect(POSITIONED);
    }

    /// The `alloca` of `local` and its LLVM type.
    fn slot(&self, local: LocalId) -> (PointerValue<'ctx>, BasicTypeEnum<'ctx>) {
        self.slots[local.0].expect("mir should read and write only locals that have a value type")
    }

    /// The value of `local`, without retaining it.
    fn load_local(&self, local: LocalId) -> BasicValueEnum<'ctx> {
        let (ptr, ty) = self.slot(local);
        // Name the load after the local (the alloca carries its name).
        let name = ptr.get_name().to_string_lossy();
        self.builder.build_load(ty, ptr, &name).expect(POSITIONED)
    }

    /// `Move`: the value of `local`, whose reference leaves its slot without a retain. A
    /// flagged local's slot then holds the zero value, which releasing skips; no other local's
    /// slot is read again.
    fn move_local(&self, local: LocalId) -> BasicValueEnum<'ctx> {
        self.check_not_moved(local);
        let (ptr, ty) = self.slot(local);
        let value = self.builder.build_load(ty, ptr, "moved").expect(POSITIONED);
        if self.flagged[local.0] {
            self.store(ptr, ty.const_zero());
        }
        value
    }

    /// A new object of `class` with count 1, whose fields are the evaluated `args`.
    fn new_object(&self, class: ClassId, args: &[BasicValueEnum<'ctx>]) -> PointerValue<'ctx> {
        let (size, align) = self.heap_size(&HeapBlock::Class(class));
        let object = self
            .builder
            .build_call(self.alloc, &[size.into(), align.into()], "object")
            .expect(POSITIONED)
            .try_as_basic_value()
            .basic()
            .expect("`sisu_alloc` should return a pointer, as the runtime declares it")
            .into_pointer_value();
        self.init_count(object);
        for (index, &arg) in args.iter().enumerate() {
            let field = self.field_ptr(class, object, index);
            self.store(field, arg);
        }
        object
    }

    /// `Call`: `func` applied to the evaluated `args`; `None` for a `unit` result.
    fn call_value(
        &self,
        func: FuncId,
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> Option<BasicValueEnum<'ctx>> {
        self.builder
            .build_call(self.functions[func.0], args, "call")
            .expect(POSITIONED)
            .try_as_basic_value()
            .basic()
    }

    /// `-x` or `!x` on an evaluated operand of type `ty`; negating an `i64` panics at `span` on
    /// overflow.
    fn unary(
        &self,
        op: UnaryOp,
        ty: Type,
        operand: BasicValueEnum<'ctx>,
        span: Span,
    ) -> BasicValueEnum<'ctx> {
        if ty == Type::F64 {
            let negated = self
                .builder
                .build_float_neg(operand.into_float_value(), "neg");
            return negated.expect(POSITIONED).into();
        }
        let operand = operand.into_int_value();
        match op {
            UnaryOp::Neg => {
                let zero = self.context.i64_type().const_zero();
                self.checked("llvm.ssub.with.overflow", zero, operand, span)
            }
            UnaryOp::Not => self.builder.build_not(operand, "not").expect(POSITIONED),
        }
        .into()
    }

    /// `Wrap`: `value` as a present optional of type `ty`.
    fn wrap(&self, ty: Type, value: BasicValueEnum<'ctx>) -> BasicValueEnum<'ctx> {
        let some = self.context.bool_type().const_int(1, false);
        self.optional(ty, some, value)
    }

    fn declare(&mut self, f: &mir::Function) {
        let params: Vec<BasicMetadataTypeEnum> = f.locals[..f.params]
            .iter()
            .map(|p| {
                self.llvm_type(p.ty)
                    .expect("the checker should reject a `unit` or `never` parameter, so each parameter has a value type")
                    .into()
            })
            .collect();
        let fn_type = match self.llvm_type(f.ret) {
            Some(ret) => ret.fn_type(&params, false),
            None => self.context.void_type().fn_type(&params, false),
        };
        let function = self.module.add_function(
            &format!("sisu.{}", f.name),
            fn_type,
            Some(Linkage::Internal),
        );
        self.functions.push(function);
    }

    /// The body of `function` from `f`: one LLVM block per mir block, in mir's order and each
    /// followed by the blocks its code adds, named `bbN`, or `loop.body` for a loop's header and
    /// `loop.pre` for its pre-header. The entry
    /// block starts with an `alloca` for every local that has a value type (see
    /// `entry_alloca`), and stores each parameter into its own.
    fn define(&mut self, f: &mir::Function, function: FunctionValue<'ctx>) {
        // Each block is named as it is appended, in mir's order, so LLVM numbers repeated names
        // in that order. The sort is stable: a header that also enters an inner loop keeps the
        // header's name, which comes last.
        let mut roles: Vec<(usize, &str)> = (f.loops.iter())
            .map(|l| (l.pre_header.0, "loop.pre"))
            .chain(f.loops.iter().map(|l| (l.header.0, "loop.body")))
            .collect();
        roles.sort_by_key(|&(block, _)| block);
        let mut roles = roles.into_iter().peekable();
        let mut name = String::new();
        self.blocks = (0..f.blocks.len())
            .map(|b| {
                name.clear();
                while let Some((_, role)) = roles.next_if(|&(block, _)| block == b) {
                    role.clone_into(&mut name);
                }
                if name.is_empty() {
                    write!(name, "bb{b}").expect("writing to a `String` should not fail");
                }
                self.context.append_basic_block(function, &name)
            })
            .collect();
        self.builder.position_at_end(self.blocks[0]);
        self.types = f.locals.iter().map(|local| local.ty).collect();
        self.flagged = if self.rc_fault.is_some() {
            vec![true; f.locals.len()]
        } else {
            mir::flagged(f)
        };
        self.slots = f
            .locals
            .iter()
            .map(|local| {
                let ty = self.llvm_type(local.ty)?;
                let name = local.name.as_deref().unwrap_or("");
                Some((self.entry_alloca(ty, name), ty))
            })
            .collect();
        for (local, param) in (0..).map(LocalId).zip(function.get_param_iter()) {
            self.store(self.slot(local).0, param);
        }
        for (&block, data) in self.blocks.iter().zip(&f.blocks) {
            // The blocks an emitter adds go to the end of the function: moving each mir block
            // there before its code keeps the blocks in mir's order, each followed by its own.
            // The entry block stays first.
            let last = function
                .get_last_basic_block()
                .expect("the function should have its mir blocks");
            if block != self.blocks[0] && block != last {
                block
                    .move_after(last)
                    .expect("both blocks should belong to the function");
            }
            self.builder.position_at_end(block);
            for stmt in &data.stmts {
                self.statement(stmt);
            }
            self.terminator(&data.term);
        }
    }

    /// The LLVM block of mir block `id`.
    fn block(&self, id: BlockId) -> BasicBlock<'ctx> {
        self.blocks[id.0]
    }

    /// One statement; the count work of each is the spec's table (`mir/mod.rs`).
    fn statement(&self, stmt: &Statement) {
        match stmt {
            Statement::Assign { dest, value } => {
                let value = self.rvalue(value, dest.map(|dest| self.types[dest.0]));
                if let Some(dest) = dest {
                    let value = value.expect(
                        "lowering should give a destination only to an rvalue with a value",
                    );
                    self.store(self.slot(*dest).0, value);
                }
            }
            Statement::Take { dest, place } => {
                // The `check` before it bounds-checked the place.
                let (value, _) = self.read(place, false);
                self.store(self.slot(*dest).0, value);
            }
            Statement::Store { place, value } => {
                let (value, _) = self.operand(value);
                let (address, _, tag) = self.address(place.local, &place.projections, false);
                // Lowering stores only into a field or an element; a local is assigned.
                let tag = tag.expect("a store's place should end in a field or an element");
                self.store_tagged(address, value, tag);
            }
            Statement::Release(local) => {
                self.check_not_moved(*local);
                self.release(self.load_local(*local), self.types[local.0]);
            }
            // The slot may hold the zero value that a `Move` of a flagged local leaves, and
            // releasing it releases nothing, so there is no moved check.
            Statement::ScopeRelease(local) => {
                self.release(self.load_local(*local), self.types[local.0]);
            }
            Statement::MakeUnique(place) => {
                let (address, ty, _) = self.address(place.local, &place.projections, false);
                self.unique(address, ty);
            }
            Statement::Check(place) => {
                let Some((Projection::Index { index, span }, array)) =
                    place.projections.split_last()
                else {
                    unreachable!("lowering should check only an element place")
                };
                let (address, _, _) = self.address(place.local, array, false);
                let array = self.load_pointer(address);
                let (index, _) = self.operand(index);
                self.check_bounds(array, index.into_int_value(), *span);
            }
            Statement::End(_) => {}
        }
    }

    /// Ends the current block with `term`.
    fn terminator(&self, term: &Terminator) {
        match term {
            Terminator::Goto(to) => self.branch_to(self.block(*to)),
            Terminator::Branch { cond, then, else_ } => {
                let (cond, _) = self.operand(cond);
                self.branch_on(cond.into_int_value(), self.block(*then), self.block(*else_));
            }
            Terminator::IfPresent {
                optional,
                some,
                none,
            } => {
                let value = self.load_local(*optional);
                let present = self.has_value(self.types[optional.0], value, "some");
                self.branch_on(present, self.block(*some), self.block(*none));
            }
            Terminator::SwitchTag {
                place,
                arms,
                otherwise,
            } => self.switch_tag(place, arms, *otherwise),
            Terminator::SwitchValue {
                operand,
                arms,
                otherwise,
            } => self.switch_value(operand, arms, *otherwise),
            Terminator::Return(value) => {
                let value = value.as_ref().map(|value| self.operand(value).0);
                self.builder
                    .build_return(value.as_ref().map(|v| v as &dyn BasicValue))
                    .expect(POSITIONED);
            }
            Terminator::Panic { message, span } => self.panic_value(self.pointer(message), *span),
            Terminator::Unreachable => self.trap(),
        }
    }

    /// The value of `rvalue`, `None` for `unit`; `ty` is the type of its destination, when it
    /// has one. Each rvalue consumes or borrows its operands as the spec's table says: the
    /// releases of the consumed ones that the emitters leave to their caller are here.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per rvalue kind, so the emission reads in one place"
    )]
    fn rvalue(&self, rvalue: &Rvalue, ty: Option<Type>) -> Option<BasicValueEnum<'ctx>> {
        let ty =
            || ty.expect("lowering should give this rvalue a destination, whose type it needs");
        let value = match rvalue {
            Rvalue::Use(op) => self.operand(op).0,
            Rvalue::Copy(place) => {
                let (value, ty) = self.read(place, true);
                self.retain(value, ty);
                value
            }
            Rvalue::CopyStr(text) => self.string_lit(text).into(),
            Rvalue::Borrow(place) => self.read(place, true).0,
            Rvalue::Unwrap(op) => {
                let (value, ty) = self.operand(op);
                self.optional_value(ty, value, "value")
            }
            Rvalue::Call { func, args, .. } => {
                let args: Vec<BasicMetadataValueEnum> =
                    args.iter().map(|arg| self.operand(arg).0.into()).collect();
                return self.call_value(*func, &args);
            }
            Rvalue::New { class, args } => self.new_object(*class, &self.values(args)).into(),
            Rvalue::Variant { id, variant, args } => {
                self.variant(*id, *variant, &self.values(args))
            }
            Rvalue::ArrayLit(elements) => self.array_lit(ty(), &self.values(elements)).into(),
            Rvalue::ArrayFill { value, len, span } => {
                let (filler, _) = self.operand(value);
                let (count, _) = self.operand(len);
                self.array_fill(filler, count.into_int_value(), ty(), *span)
                    .into()
            }
            Rvalue::Wrap(op) => self.wrap(ty(), self.operand(op).0),
            Rvalue::Concat { lhs, rhs } => {
                let lhs = self.pointer(lhs);
                self.concat(lhs, self.pointer(rhs)).into()
            }
            Rvalue::Interpolate { parts, formats } => {
                let parts: Vec<_> = parts.iter().map(|part| self.operand(part)).collect();
                let text = self.interpolate(&parts, formats);
                for (value, ty) in parts {
                    self.release(value, ty);
                }
                text.into()
            }
            Rvalue::ReadLine => self.read_line().into(),
            Rvalue::Chars(op) => self.chars(self.pointer(op)).into(),
            Rvalue::Bytes(op) => self.bytes(self.pointer(op)).into(),
            Rvalue::Words(op) => self.words(self.pointer(op)).into(),
            Rvalue::FromChars(op) => self.string_from_chars(self.pointer(op)).into(),
            Rvalue::Len(op) => {
                let (base, ty) = self.operand(op);
                self.len(base.into_pointer_value(), ty).into()
            }
            Rvalue::LenWords(op) => self.len_words(self.pointer(op)).into(),
            Rvalue::Sum { base, span } => {
                let (array, base_ty) = self.operand(base);
                let sum = self.sum(ty(), array.into_pointer_value(), *span);
                self.release(array, base_ty);
                sum
            }
            Rvalue::Contains { base, value } => {
                let (array, base_ty) = self.operand(base);
                let (needle, needle_ty) = self.operand(value);
                let hit = self.contains(element_of(base_ty), array.into_pointer_value(), needle);
                self.release(array, base_ty);
                self.release(needle, needle_ty);
                hit.into()
            }
            Rvalue::Convert(op) => self.convert(ty(), self.operand(op).0),
            Rvalue::Math { op, operands } => {
                // The receiver's type picks the instructions.
                let mut receiver = None;
                let values: Vec<_> = operands
                    .iter()
                    .map(|operand| {
                        let (value, ty) = self.operand(operand);
                        receiver.get_or_insert(ty);
                        value
                    })
                    .collect();
                let ty = receiver.expect("a method call should have its receiver as an operand");
                self.math(*op, ty, &values)
            }
            Rvalue::Unary { op, operand, span } => {
                let (value, ty) = self.operand(operand);
                self.unary(*op, ty, value, *span)
            }
            Rvalue::Binary { op, lhs, rhs, span } => {
                let (lhs, ty) = self.operand(lhs);
                self.binary(*op, ty, lhs, self.operand(rhs).0, *span)
            }
            Rvalue::Compare { op, lhs, rhs } => {
                let (lhs, ty) = self.operand(lhs);
                self.compare(*op, ty, lhs, self.operand(rhs).0).into()
            }
            Rvalue::Equal { negated, lhs, rhs } => {
                let (lhs, ty) = self.operand(lhs);
                self.equal_value(*negated, ty, lhs, self.operand(rhs).0)
                    .into()
            }
            Rvalue::Is { lhs, rhs } => {
                let lhs = self.pointer(lhs);
                self.same_object(lhs, self.pointer(rhs)).into()
            }
            Rvalue::Print { stream, operand } => {
                let (value, ty) = self.operand(operand);
                self.print(*stream, ty, value);
                return None;
            }
        };
        Some(value)
    }

    /// The values of `operands`, left to right.
    fn values(&self, operands: &[Operand]) -> Vec<BasicValueEnum<'ctx>> {
        operands.iter().map(|op| self.operand(op).0).collect()
    }

    /// An operand the checker gives a pointer: a string, an array or an object.
    fn pointer(&self, op: &Operand) -> PointerValue<'ctx> {
        self.operand(op).0.into_pointer_value()
    }

    /// The value of `op` and its type. A read takes no reference; a move leaves its local
    /// empty.
    fn operand(&self, op: &Operand) -> (BasicValueEnum<'ctx>, Type) {
        match op {
            Operand::Const(c) => self.constant(c),
            Operand::Read(place) => self.read(place, true),
            Operand::Move(local) => (self.move_local(*local), self.types[local.0]),
            Operand::Use(_) | Operand::Guarded(_) => {
                unreachable!("the ownership pass should leave no `use` for codegen")
            }
        }
    }

    /// The value of `c` and its type. A string is its literal's global, borrowed.
    fn constant(&self, c: &Const) -> (BasicValueEnum<'ctx>, Type) {
        match c {
            Const::Int(n, ty) => {
                let int = self
                    .llvm_type(*ty)
                    .expect("lowering should give an integer constant an integer type, which has an LLVM type")
                    .into_int_type();
                (int.const_int(n.cast_unsigned(), false).into(), *ty)
            }
            Const::Float(x) => (self.context.f64_type().const_float(*x).into(), Type::F64),
            Const::Bool(b) => (
                self.context
                    .bool_type()
                    .const_int(u64::from(*b), false)
                    .into(),
                Type::BOOL,
            ),
            Const::Char(c) => (
                self.context
                    .i32_type()
                    .const_int(u64::from(*c), false)
                    .into(),
                Type::CHAR,
            ),
            Const::None(ty) => (self.empty(*ty), *ty),
            Const::Str(text) => (self.string_global(text).into(), Type::STRING),
        }
    }

    /// The value at `place`, not retained, and its type. With `checked`, each index is checked
    /// against its array's length first, as a read does; a `take` follows its place's `check`.
    fn read(&self, place: &Place, checked: bool) -> (BasicValueEnum<'ctx>, Type) {
        self.read_at(place.local, &place.projections, checked)
    }

    /// `read` of `local` through `projections`.
    fn read_at(
        &self,
        local: LocalId,
        projections: &[Projection],
        checked: bool,
    ) -> (BasicValueEnum<'ctx>, Type) {
        match projections.split_last() {
            None => {
                self.check_not_moved(local);
                (self.load_local(local), self.types[local.0])
            }
            // `payload_value` reads a boxed `F?`, which has no address.
            Some((&Projection::Payload { variant, field }, outer)) => {
                let (address, ty, _) = self.address(local, outer, checked);
                let id = enum_of(ty);
                let ty = self.program.enums[id.0].variants[variant].fields[field].ty;
                (self.payload_value(address, id, variant, field), ty)
            }
            Some((Projection::Some, outer)) => {
                let (optional, ty) = self.read_at(local, outer, checked);
                let TypeKind::Optional(payload) = ty.kind() else {
                    unreachable!("mir should project `some` only out of an optional")
                };
                (self.optional_value(ty, optional, "some"), payload)
            }
            Some((Projection::Field { .. } | Projection::Index { .. }, _)) => {
                let (address, ty, tag) = self.address(local, projections, checked);
                let llvm_ty = self.llvm_type(ty).expect(
                    "the checker should give every field and element a value type, never `unit` or `never`",
                );
                let tag = tag.expect("a field or an element should have a TBAA tag");
                (self.load_tagged(llvm_ty, address, "load", tag), ty)
            }
        }
    }

    /// The address of `local` through `projections`, the type of the value there, and the TBAA
    /// tag of the last step when it is a field or an element. A field or index step loads the
    /// pointer that the step before it reached. With `checked`, each index is checked against
    /// its array's length. Every root read, a store's and a `switch`'s included, comes through
    /// here, so this is where a root slot is checked for a move (`check_not_moved`).
    fn address(
        &self,
        local: LocalId,
        projections: &[Projection],
        checked: bool,
    ) -> (PointerValue<'ctx>, Type, Option<MetadataValue<'ctx>>) {
        self.check_not_moved(local);
        let (mut address, _) = self.slot(local);
        let mut ty = self.types[local.0];
        let mut tag = None;
        for projection in projections {
            (address, ty, tag) = match projection {
                Projection::Field { index, .. } => {
                    let class = class_of(ty);
                    let object = self.load_pointer(address);
                    (
                        self.field_ptr(class, object, *index),
                        self.program.classes[class.0].fields[*index].ty,
                        Some(self.field_tag(class, *index)),
                    )
                }
                Projection::Payload { variant, field } => {
                    let id = enum_of(ty);
                    (
                        self.payload_place(address, id, *variant, *field),
                        self.program.enums[id.0].variants[*variant].fields[*field].ty,
                        None,
                    )
                }
                Projection::Index { index, span } => {
                    let array = self.load_pointer(address);
                    let index = self.operand(index).0.into_int_value();
                    if checked {
                        self.check_bounds(array, index, *span);
                    }
                    let element = element_of(ty);
                    (
                        self.element_ptr(element, array, index),
                        element,
                        Some(self.element_tag),
                    )
                }
                Projection::Some => {
                    panic!("`some` should only ever be the last projection, which `read` reads")
                }
            };
        }
        (address, ty, tag)
    }

    /// The pointer at `address`: an object, an array or a string.
    fn load_pointer(&self, address: PointerValue<'ctx>) -> PointerValue<'ctx> {
        self.builder
            .build_load(
                self.context.ptr_type(AddressSpace::default()),
                address,
                "pointer",
            )
            .expect(POSITIONED)
            .into_pointer_value()
    }

    /// `define i32 @main(i32 %argc, ptr %argv)`, which the C startup code calls: runs
    /// `sisu.main`, passing it `args` when it takes them, flushes standard output, returns 0.
    /// `sisu.main` owns `args` and releases it, as any counted parameter.
    fn c_main(&self) {
        let i32_type = self.context.i32_type();
        let ptr_type = self.context.ptr_type(AddressSpace::default());
        let main = self.module.add_function(
            "main",
            i32_type.fn_type(&[i32_type.into(), ptr_type.into()], false),
            None,
        );
        self.builder
            .position_at_end(self.context.append_basic_block(main, "entry"));
        let sisu_main = self
            .module
            .get_function("sisu.main")
            .expect("the checker should reject a program without a `main` function");
        let [count, strings] = [(0, "argc"), (1, "argv")].map(|(n, name)| {
            let param = main
                .get_nth_param(n)
                .expect("the C `main` should take `argc` and `argv`, as declared above");
            param.set_name(name);
            param
        });
        let passed = if sisu_main.count_params() == 0 {
            Vec::new()
        } else {
            vec![
                self.args(count.into_int_value(), strings.into_pointer_value())
                    .into(),
            ]
        };
        self.builder
            .build_call(sisu_main, &passed, "")
            .expect(POSITIONED);
        // `print` buffers; this delivers what is left, or panics if standard output fails.
        self.call(self.runtime("sisu_flush_stdout"), &[]);
        if self.rc_fault.is_some() {
            self.call(self.runtime("sisu_rc_check_exit"), &[]);
        }
        self.builder
            .build_return(Some(&i32_type.const_zero()))
            .expect(POSITIONED);
    }

    fn current_block(&self) -> BasicBlock<'ctx> {
        self.builder.get_insert_block().expect(POSITIONED)
    }

    fn current_function(&self) -> FunctionValue<'ctx> {
        self.current_block()
            .get_parent()
            .expect("the block the builder is positioned in should belong to a function")
    }

    /// `lhs == rhs`, or `!=` when `negated`, on two evaluated operands of type `ty`.
    fn equal_value(
        &self,
        negated: bool,
        ty: Type,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
    ) -> IntValue<'ctx> {
        match ty {
            Type::I64 | Type::U8 | Type::BOOL | Type::CHAR if negated => {
                self.int_compare(IntPredicate::NE, lhs.into_int_value(), rhs.into_int_value())
            }
            _ if negated => {
                let equal = self.equal(ty, lhs, rhs);
                self.builder.build_not(equal, "not").expect(POSITIONED)
            }
            _ => self.equal(ty, lhs, rhs),
        }
    }

    /// An `alloca` at the start of the function's entry block, wherever the variable sits:
    /// `mem2reg` promotes only entry-block allocas, and one in a loop body would grow the
    /// stack on every iteration.
    fn entry_alloca(&self, ty: BasicTypeEnum<'ctx>, name: &str) -> PointerValue<'ctx> {
        let entry = self.current_function().get_first_basic_block().expect(
            "a function with a body should have an entry block before any alloca is emitted",
        );
        let builder = self.context.create_builder();
        match entry.get_first_instruction() {
            Some(first) => builder.position_before(&first),
            None => builder.position_at_end(entry),
        }
        builder.build_alloca(ty, name).expect(POSITIONED)
    }

    /// The value a place held before an assignment.
    fn load(&self, ty: BasicTypeEnum<'ctx>, ptr: PointerValue<'ctx>) -> BasicValueEnum<'ctx> {
        self.builder.build_load(ty, ptr, "old").expect(POSITIONED)
    }

    fn store(&self, ptr: PointerValue<'ctx>, value: BasicValueEnum<'ctx>) {
        self.builder.build_store(ptr, value).expect(POSITIONED);
    }

    /// `print` or `eprint` of an evaluated operand of type `ty`, which it borrows.
    fn print(&self, stream: Stream, ty: Type, value: BasicValueEnum<'ctx>) {
        if ty == Type::STRING {
            self.print_string(stream, value.into_pointer_value());
            return;
        }
        let printers = self.printers[stream.index()];
        if ty == Type::F64 {
            self.builder
                .build_call(printers.f64, &[value.into_float_value().into()], "")
                .expect(POSITIONED);
            return;
        }
        let mut arg = value.into_int_value();
        if ty == Type::U8 {
            arg = self
                .builder
                .build_int_z_extend(arg, self.context.i64_type(), "widened")
                .expect(POSITIONED);
        }
        let is_bool = ty == Type::BOOL;
        let print = match ty {
            Type::BOOL => printers.bool,
            Type::CHAR => printers.char,
            _ => printers.int,
        };
        let call = self
            .builder
            .build_call(print, &[arg.into()], "")
            .expect(POSITIONED);
        if is_bool {
            call.add_attribute(AttributeLoc::Param(0), zeroext(self.context));
        }
    }

    /// Panics with `<path>:<line>:<col>: <message>`, for the start of `span`, when `fails`
    /// holds; see `panic_with`.
    fn panic_if(&self, fails: IntValue<'ctx>, span: Span, message: &str) {
        let text = format!("{}: {message}", self.location(span));
        self.panic_with(fails, &text, self.panic, &[]);
    }

    /// `<path>:<line>:<col>` for the start of `span`.
    fn location(&self, span: Span) -> String {
        let (line, col) = self.lines.line_col(self.source, span.start());
        format!("{}:{line}:{col}", self.path)
    }

    /// Branches to a fresh panic block when `fails` holds, and continues in a fresh block
    /// otherwise. The panic block calls `function`, which never returns, with `text` and its
    /// length, then `extra`.
    fn panic_with(
        &self,
        fails: IntValue<'ctx>,
        text: &str,
        function: FunctionValue<'ctx>,
        extra: &[BasicMetadataValueEnum<'ctx>],
    ) {
        let current = self.current_function();
        let panic_block = self.context.append_basic_block(current, "panic");
        let ok = self.context.append_basic_block(current, "ok");
        self.builder
            .build_conditional_branch(fails, panic_block, ok)
            .expect(POSITIONED);
        self.builder.position_at_end(panic_block);
        let msg = self
            .builder
            .build_global_string_ptr(text, "panic.msg")
            .expect(POSITIONED);
        let len = self.context.i64_type().const_int(
            u64::try_from(text.len()).expect("a panic message should be shorter than 2^64 bytes"),
            false,
        );
        let args: Vec<BasicMetadataValueEnum> = [msg.as_pointer_value().into(), len.into()]
            .into_iter()
            .chain(extra.iter().copied())
            .collect();
        self.builder
            .build_call(function, &args, "")
            .expect(POSITIONED);
        self.builder.build_unreachable().expect(POSITIONED);
        self.builder.position_at_end(ok);
    }

    fn branch_to(&self, block: BasicBlock<'ctx>) {
        self.builder
            .build_unconditional_branch(block)
            .expect(POSITIONED);
    }

    fn branch_on(
        &self,
        condition: IntValue<'ctx>,
        then: BasicBlock<'ctx>,
        other: BasicBlock<'ctx>,
    ) {
        self.builder
            .build_conditional_branch(condition, then, other)
            .expect(POSITIONED);
    }

    fn phi<V: BasicValue<'ctx>>(
        &self,
        ty: impl BasicType<'ctx>,
        incoming: &[(V, BasicBlock<'ctx>)],
    ) -> BasicValueEnum<'ctx> {
        let phi = self.builder.build_phi(ty, "phi").expect(POSITIONED);
        for (value, block) in incoming {
            phi.add_incoming(&[(value, *block)]);
        }
        phi.as_basic_value()
    }
}

/// How hard LLVM optimizes: not at all (`mem2reg` only), `-O2` or `-O3`.
#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum OptLevel {
    /// No `-O` flag names it.
    #[value(skip)]
    O0,
    #[value(name = "2")]
    O2,
    #[value(name = "3")]
    O3,
}

/// A target machine for the host `sisu` runs on.
pub(crate) fn target_machine(level: OptLevel) -> Result<TargetMachine, String> {
    Target::initialize_native(&InitializationConfig::default())?;
    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple).map_err(|e| e.to_string())?;
    // PIC because Ubuntu's `cc` links position-independent executables by default.
    target
        .create_target_machine(
            &triple,
            "generic",
            "",
            match level {
                OptLevel::O0 => OptimizationLevel::None,
                OptLevel::O2 => OptimizationLevel::Default,
                OptLevel::O3 => OptimizationLevel::Aggressive,
            },
            RelocMode::PIC,
            CodeModel::Default,
        )
        .ok_or_else(|| "LLVM cannot create a target machine for this host".to_string())
}

/// The pass pipeline: `default<O2>` or `default<O3>`, else `mem2reg`, which turns each
/// variable's `alloca` into SSA values.
pub(crate) fn pipeline(level: OptLevel) -> &'static str {
    match level {
        OptLevel::O0 => "mem2reg",
        OptLevel::O2 => "default<O2>",
        OptLevel::O3 => "default<O3>",
    }
}

/// Marks each Sisu function in `names`, comma separated, `noinline` so that the optimizer keeps
/// it. The `o2-calls` check uses this to read a caller's own calls; a name that is not a
/// function is skipped.
pub(crate) fn keep_functions(module: &Module<'_>, names: &str) {
    let noinline = module
        .get_context()
        .create_enum_attribute(Attribute::get_named_enum_kind_id("noinline"), 0);
    for name in names.split(',') {
        if let Some(function) = module.get_function(&format!("sisu.{name}")) {
            function.add_attribute(AttributeLoc::Function, noinline);
        }
    }
}

/// Verifies `module`, then runs the pipeline. This is the build's only verifier run: a pass
/// pipeline that broke the IR reaches instruction selection instead of a clean error, which would
/// be an LLVM bug.
pub(crate) fn optimize(
    module: &Module<'_>,
    machine: &TargetMachine,
    level: OptLevel,
) -> Result<(), String> {
    module.verify().map_err(|e| e.to_string())?;
    module
        .run_passes(pipeline(level), machine, PassBuilderOptions::create())
        .map_err(|e| e.to_string())
}

/// Compiles `module`, which `optimize` verified, to object code for `machine`, in memory.
pub(crate) fn object_code(
    module: &Module<'_>,
    machine: &TargetMachine,
) -> Result<MemoryBuffer<'static>, String> {
    machine
        .write_to_memory_buffer(module, FileType::Object)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
