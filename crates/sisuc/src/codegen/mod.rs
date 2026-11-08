//! Builds LLVM IR through inkwell and writes it out as a native object file. Each mir function
//! (`crate::mir`) becomes one LLVM function, and each of its blocks one LLVM block; the
//! emitters in the submodules build the code of each statement, rvalue and terminator.

mod abi;
mod any;
mod arith;
mod array;
mod deep;
mod enums;
mod io;
mod layout;
mod matching;
mod math;
mod rc;
mod show;
mod string;
pub(crate) mod units;

#[cfg(test)]
pub(crate) use rc::CLASS_DRAINS;
#[cfg(test)]
pub(crate) use string::LEN_WORDS;

use rustc_hash::FxHashMap;
use std::cell::{Cell, RefCell};
use std::fmt::Write as _;

use inkwell::attributes::{Attribute, AttributeLoc};
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::intrinsics::Intrinsic;
use inkwell::memory_buffer::MemoryBuffer;
use inkwell::module::{Linkage, Module};
use inkwell::passes::PassBuilderOptions;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetData, TargetMachine,
};
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum};
use inkwell::values::{
    BasicMetadataValueEnum, BasicValue, BasicValueEnum, FunctionValue, GlobalValue, IntValue,
    MetadataValue, PointerValue,
};
use inkwell::{AddressSpace, GlobalVisibility, IntPredicate, OptimizationLevel};

use self::array::element_of;
use self::enums::{Component, EnumLayout};
use self::layout::{ClassLayout, HeapBlock, Niche, tbaa_tag};
use self::rc::Helper;
use self::units::{Placement, Unit};
use crate::conformance::Written;
use crate::mir::{
    self, BlockId, Const, Operand, Place, Projection, Rvalue, Side, Statement, Terminator,
};
use crate::source::{LineIndex, Span};
use crate::tir::{
    ClassId, EnumId, FuncId, InterfaceId, LocalId, Program, Stream, Type, TypeArgs, TypeKind,
    UnaryOp,
};

/// The `expect` message for a builder call: codegen positions the builder before it emits code.
const POSITIONED: &str =
    "the builder should be positioned in a block while a function body is emitted";

#[cfg(test)]
thread_local! {
    /// How many class fields and enum payload fields the reflexivity analysis has read on this
    /// thread.
    static TYPE_READS: Cell<usize> = const { Cell::new(0) };
    /// How many token places and rest places a `take_apart` has visited on this thread.
    static TAKE_APART_PROBES: Cell<usize> = const { Cell::new(0) };
    /// How many calls of the lookups of a component have happened on this thread. The tick counts
    /// the enums that `components` builds the table from. It also counts each call of the one
    /// lookup helper, `member_position`, and not the steps inside it.
    static MEMBER_SCANS: Cell<usize> = const { Cell::new(0) };
}

impl<'ctx, 'src> Codegen<'ctx, 'src> {
    /// An empty module for `program`, with the runtime's declarations and no function defined.
    #[expect(
        clippy::too_many_arguments,
        reason = "a unit needs the program, its analyses, its place, its text and the target"
    )]
    fn new(
        context: &'ctx Context,
        program: &'src Program,
        mir: &'src mir::Program,
        analyses: &'src Analyses,
        unit: &'src Unit,
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
        // index, length)` prints a bounds panic and exits; `sisu_panic_range(at, at_len, kind,
        // start, end, length)` prints a range panic and exits; `sisu_panic_message(at, at_len,
        // msg, msg_len)` prints a `panic` call's panic and exits.
        let panic = abi::declare(&module, abi::row("sisu_panic"));
        let panic_bounds = abi::declare(&module, abi::row("sisu_panic_bounds"));
        let panic_range = abi::declare(&module, abi::row("sisu_panic_range"));
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
            panic_range,
            panic_message,
            alloc,
            free,
            rc_retain,
            rc_fault,
            tbaa_kind: context.get_kind_id("tbaa"),
            header_tag: tbaa_tag(context, tbaa_root, "array header"),
            element_tag: tbaa_tag(context, tbaa_root, "array element"),
            tbaa_root,
            program,
            mir,
            analyses,
            unit,
            any_tables: RefCell::new(FxHashMap::default()),
            pending_tables: RefCell::new(Vec::new()),
            helpers: RefCell::new(FxHashMap::default()),
            pending: RefCell::new(Vec::new()),
            written: RefCell::new(Written::default()),
            class_held: RefCell::new(FxHashMap::default()),
            niches: RefCell::new(FxHashMap::default()),
            optional_types: RefCell::new(FxHashMap::default()),
            deep: RefCell::default(),
            target_data,
            path,
            source,
            classes: Vec::new(),
            enums: Vec::new(),
            functions: Vec::new(),
            slots: Vec::new(),
            outs: Vec::new(),
            last_outs: RefCell::new(Vec::new()),
            memory_values: RefCell::new(FxHashMap::default()),
            flagged: Vec::new(),
            types: Vec::new(),
            caller: Cell::new(None),
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
// `scripts/scaling.py` reads this function's instruction count.
#[inline(never)]
pub(crate) fn compile<'ctx>(
    context: &'ctx Context,
    program: &Program,
    mir: &mir::Program,
    path: &str,
    source: &str,
    machine: &TargetMachine,
    rc_checks: bool,
) -> Module<'ctx> {
    compile_unit(
        context,
        program,
        mir,
        &Analyses::new(program, source),
        &Unit::whole(mir.functions.len()),
        path,
        source,
        machine,
        rc_checks,
    )
}

/// What a whole program gives every codegen unit: the analyses of its types and the index of its
/// `any` tables. It also gives the line starts of its source. A unit reads them and never changes
/// them.
pub(crate) struct Analyses {
    /// Whether each class, then each enum, reaches an `f64` or a written `eq` through its fields;
    /// see `rc::reaching_unreflexive`.
    unreflexive: Vec<bool>,
    /// The classes that hold each other in a loop; see `rc::class_cycles`.
    class_cycles: rc::ClassCycles,
    /// For each enum, indexed by `EnumId`, its component when the component has boxes; see
    /// `enums::components`.
    components: Vec<Option<Component>>,
    /// The interfaces of the table of each `any` type, as `mono` laid them out.
    any_layouts: FxHashMap<Type, Vec<(InterfaceId, TypeArgs)>>,
    /// The place of each `(type, any type)` pair in `program.any_tables`.
    any_table_index: FxHashMap<(Type, Type), usize>,
    /// The line starts of the source, so that a panic site costs a binary search.
    lines: LineIndex,
}

// Every unit reads the same analyses, program and mir from its own thread.
const _: () = {
    const fn assert_sync<T: Sync>() {}
    assert_sync::<Analyses>();
    assert_sync::<Program>();
    assert_sync::<mir::Program>();
};

impl Analyses {
    /// The analyses of `program`, whose text is `source`.
    pub(crate) fn new(program: &Program, source: &str) -> Analyses {
        Analyses {
            unreflexive: rc::reaching_unreflexive(program),
            class_cycles: rc::class_cycles(program),
            components: enums::components(&program.enums),
            any_layouts: (program.any_layouts.iter())
                .map(|layout| (layout.any, layout.interfaces.clone()))
                .collect(),
            any_table_index: (program.any_tables.iter().enumerate())
                .map(|(index, table)| ((table.ty, table.any), index))
                .collect(),
            lines: LineIndex::new(source),
        }
    }
}

/// The LLVM module of codegen unit `unit` of a program; see `compile`. With one unit, every
/// function is internal and the module is the whole program. In a split build, a function that
/// another unit refers to (see `units::exported`) has external linkage and hidden visibility. The
/// units link by name. Every other function is internal. A unit defines the functions that it
/// owns. It defines an `available_externally` copy of each small function of another unit that it
/// calls (see `units::copies`). It declares the other functions. Unit 0 holds the C
/// `main` and the drop-depth state (`sisu_rc.depth`, `sisu_rc.deferred`), which the other units
/// declare. A unit makes the release helpers and the `any` tables that its functions use, and no
/// other.
#[inline(never)]
#[expect(
    clippy::too_many_arguments,
    reason = "a unit needs the program, its analyses, its place, its text and the target"
)]
pub(crate) fn compile_unit<'ctx>(
    context: &'ctx Context,
    program: &Program,
    mir: &mir::Program,
    analyses: &Analyses,
    unit: &Unit,
    path: &str,
    source: &str,
    machine: &TargetMachine,
    rc_checks: bool,
) -> Module<'ctx> {
    let mut codegen = Codegen::new(
        context, program, mir, analyses, unit, path, source, machine, rc_checks,
    );
    codegen.lay_out_types();
    if unit.is_split() {
        codegen.declare_drop_globals();
    } else {
        codegen.declare_any_tables();
        // Every class gets its release helper, used or not; an `eq` helper is made on first use.
        for id in 0..program.classes.len() {
            codegen.helper(
                Helper::Release,
                Type::intern(TypeKind::Class(ClassId(id), TypeArgs::NONE)),
            );
        }
    }
    // Declare every function first so that calls can refer to functions defined later.
    for (i, f) in mir.functions.iter().enumerate() {
        codegen.declare(i, f);
    }
    for (i, f) in mir.functions.iter().enumerate() {
        if unit.placement[i] != Placement::Other {
            let function = codegen.functions[i];
            codegen.define(f, function);
        }
    }
    codegen.define_any_tables();
    if unit.index == 0 {
        codegen.c_main();
    }
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
    let TypeKind::Class(id, _) = ty.kind() else {
        unreachable!("the checker should only allow field access on a class-typed value")
    };
    id
}

/// The enum of a payload projection's base, whose type is `ty`.
fn enum_of(ty: Type) -> EnumId {
    let TypeKind::Enum(id, _) = ty.kind() else {
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
    panic_range: FunctionValue<'ctx>,
    panic_message: FunctionValue<'ctx>,
    alloc: FunctionValue<'ctx>,
    free: FunctionValue<'ctx>,
    /// `@sisu_rc.retain`, which every counted type shares.
    rc_retain: FunctionValue<'ctx>,
    /// `sisu_rc_fault(kind)`, declared only with the RC checks: `Some` turns them on.
    rc_fault: Option<FunctionValue<'ctx>>,
    /// The kind id of `!tbaa` metadata.
    tbaa_kind: u32,
    /// The TBAA tag of an array's header, which is its length and capacity, and of every count
    /// that `load_count` and `store_count` read and write. An array element's loads and stores
    /// carry `element_tag`, so that LLVM knows a store to an element leaves those counts and
    /// lengths as they were. An enum element is loaded and stored whole with `element_tag` too. A
    /// tag describes only the bytes the access itself touches, and an element's bytes never hold a
    /// count or a length: the strings, objects and boxes an enum element points at keep their
    /// counts in their own blocks, as the strings of a `[string]` do. Other accesses, such as a
    /// string's length or a new object's count, carry no tag and may alias anything.
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
    /// The mir, whose functions' `borrowed` flags say how a helper passes a part to a written
    /// method.
    mir: &'src mir::Program,
    /// The analyses of the whole program, shared by every unit.
    analyses: &'src Analyses,
    /// The unit this module is for.
    unit: &'src Unit,
    /// The global of the table of each object type for each `any` type. One unit declares every
    /// table first; a split build declares a table on its first use (`table_global`).
    any_tables: RefCell<FxHashMap<(Type, Type), GlobalValue<'ctx>>>,
    /// The places in `program.any_tables` of the tables of a split build that are declared and not
    /// yet defined; `define_any_tables` empties it.
    pending_tables: RefCell<Vec<usize>>,
    /// The helpers declared so far, by kind and counted type; see `rc::helper`.
    helpers: RefCell<FxHashMap<(Helper, Type), FunctionValue<'ctx>>>,
    /// The helpers declared but not yet defined; `define_helpers` empties it.
    pending: RefCell<Vec<(Helper, Type)>>,
    /// The types whose `Display` helper reaches a written `to_string`, settled so far.
    written: RefCell<Written>,
    /// The types that hold a class, settled so far; see `show::holds_class`.
    class_held: RefCell<FxHashMap<Type, bool>>,
    /// The niche of an optional of each payload type, settled so far; see `niche`.
    niches: RefCell<FxHashMap<Type, Option<Niche>>>,
    /// The LLVM type of each optional type, settled so far; see `llvm_type`.
    optional_types: RefCell<FxHashMap<Type, BasicTypeEnum<'ctx>>>,
    /// Which drop functions count the drop depth, and their weights; see `deep`.
    deep: RefCell<deep::DropMemo>,
    /// The target's layout: the sizes and alignments of classes.
    target_data: TargetData,
    /// The input path as given, and its text: panic messages name a position in it.
    path: &'src str,
    source: &'src str,
    /// `%sisu.<Class>` and its field elements, indexed by `ClassId`.
    classes: Vec<ClassLayout<'ctx>>,
    /// `%sisu.<Enum>` and its payload structs, indexed by `EnumId`.
    enums: Vec<EnumLayout<'ctx>>,
    /// Indexed by `FuncId`.
    functions: Vec<FunctionValue<'ctx>>,
    /// The current function's locals, indexed by `LocalId`: each one's `alloca` and its LLVM
    /// type, `None` for `unit`.
    slots: Vec<Option<(PointerValue<'ctx>, BasicTypeEnum<'ctx>)>>,
    /// The out pointers of the current function, indexed by parameter: `Some` for a `mut`
    /// parameter, whose final value `SetOut` stores there.
    outs: Vec<Option<PointerValue<'ctx>>>,
    /// The out slots of the last call that passed any, indexed by the callee's parameter: `Some`
    /// for a `mut` parameter. `Out` loads from them. Each slot is an `alloca` in the entry block.
    last_outs: RefCell<Vec<Option<PointerValue<'ctx>>>>,
    /// The LLVM type of each value that stays in memory, by its pointer (`by_memory`).
    memory_values: RefCell<FxHashMap<PointerValue<'ctx>, BasicTypeEnum<'ctx>>>,
    /// The current function's flagged locals, indexed by `LocalId`: a `Move` of one leaves the
    /// zero value in its slot (every local is flagged under the RC checks, which test it).
    flagged: Vec<bool>,
    /// The current function's local types, indexed by `LocalId`.
    types: Vec<Type>,
    /// The location of the caller that the current function got, when the prelude writes it:
    /// its hidden last parameters, a pointer and a length. A panic in the function reports
    /// them, so it names the caller's line and not a line of the prelude.
    caller: Cell<Option<(PointerValue<'ctx>, IntValue<'ctx>)>>,
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
        // A statement uses its operands before it writes any local.
        self.value_at(ty, ptr, &name)
    }

    /// `Move`: the value of `local`, whose reference leaves its slot without a retain. A
    /// flagged local's slot then holds the zero value, which releasing skips; no other local's
    /// slot is read again.
    fn move_local(&self, local: LocalId) -> BasicValueEnum<'ctx> {
        self.check_not_moved(local);
        let (ptr, ty) = self.slot(local);
        if self.flagged[local.0] {
            let value = self.copied_at(ty, ptr, "moved");
            self.store_zero(ptr, ty);
            value
        } else {
            self.value_at(ty, ptr, "moved")
        }
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

    /// The out pointers of `function`, indexed by parameter: the parameters after its first
    /// `values` ones (`out_param_types`) go to the `mut` parameters of `muts`.
    fn out_pointers(
        function: FunctionValue<'ctx>,
        values: usize,
        muts: &[bool],
    ) -> Vec<Option<PointerValue<'ctx>>> {
        let mut pointers = function.get_param_iter().skip(values);
        (muts.iter())
            .map(|&m| {
                m.then(|| {
                    pointers
                        .next()
                        .expect("a `mut` parameter should have an out pointer")
                        .into_pointer_value()
                })
            })
            .collect()
    }

    /// The out slots of a call whose parameters `muts` flags: one `alloca` in the entry block for
    /// each `mut` parameter, indexed by parameter, of the type `ty` gives. Their pointers go onto
    /// `args` after the value arguments and before the location of a prelude callee
    /// (`out_param_types`).
    fn push_out_slots(
        &self,
        muts: &[bool],
        ty: impl Fn(usize) -> Type,
        args: &mut Vec<BasicMetadataValueEnum<'ctx>>,
    ) -> Vec<Option<PointerValue<'ctx>>> {
        let slots: Vec<_> = (muts.iter().enumerate())
            .map(|(index, &m)| {
                m.then(|| {
                    let ty = (self.llvm_type(ty(index)))
                        .expect("a `mut` parameter should have a value type");
                    self.entry_alloca(ty, "out")
                })
            })
            .collect();
        args.extend(
            slots
                .iter()
                .flatten()
                .map(|&slot| BasicMetadataValueEnum::from(slot)),
        );
        slots
    }

    /// `Call`: `func` applied to the evaluated `args`; `None` for a `unit` result. The caller
    /// adds the two location arguments of a prelude function. The helpers of `to_string` and `eq`
    /// call `call_written` instead. A result that stays in memory goes to a scratch slot, whose
    /// pointer this adds as the last argument. The count of `args` is checked.
    fn call_value(
        &self,
        func: FuncId,
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> Option<BasicValueEnum<'ctx>> {
        let ret = self.mir.functions[func.0].ret;
        let result_slot = self.result_slot(ret);
        let with_slot;
        let args = if let Some((slot, _)) = result_slot {
            with_slot = [args, &[slot.into()]].concat();
            &with_slot
        } else {
            args
        };
        debug_assert_eq!(
            args.len(),
            self.functions[func.0].count_params() as usize,
            "a call should pass every parameter; a prelude function takes its caller's location too"
        );
        debug_assert!(
            self.refers_only_to_exported(func),
            "`{}` is not defined in unit {} and is not exported, so the link would fail",
            self.mir.functions[func.0].name,
            self.unit.index
        );
        let call = self
            .builder
            .build_call(self.functions[func.0], args, "call")
            .expect(POSITIONED);
        if let Some((slot, ty)) = result_slot {
            return Some(self.in_memory(slot, ty));
        }
        let result = call.try_as_basic_value().basic()?;
        self.assume_len(ret, result);
        Some(result)
    }

    /// A scratch slot for the result of a call that returns a `ret`, and its LLVM type, when the
    /// result stays in memory.
    fn result_slot(&self, ret: Type) -> Option<(PointerValue<'ctx>, BasicTypeEnum<'ctx>)> {
        let ty = self.llvm_type(ret).filter(|&ty| self.by_memory(ty))?;
        Some((self.entry_alloca(ty, "result"), ty))
    }

    /// Returns `value` from the current function: a value that stays in memory goes to the place
    /// that the last parameter points to.
    fn return_value(&self, value: Option<BasicValueEnum<'ctx>>) {
        if let Some(value) = value.filter(|&value| self.memory_type(value).is_some()) {
            let place = self
                .current_function()
                .get_last_param()
                .expect("a function that returns a value in memory should take its place")
                .into_pointer_value();
            self.store(place, value);
            self.builder.build_return(None).expect(POSITIONED);
            return;
        }
        self.builder
            .build_return(value.as_ref().map(|v| -> &dyn BasicValue<'_> { v }))
            .expect(POSITIONED);
    }

    /// Whether this unit may refer to function `func`: it defines it, or the unit that does
    /// defines it with external linkage. Every path of codegen that names a user function checks
    /// this, so a split test catches a missing export.
    fn refers_only_to_exported(&self, func: FuncId) -> bool {
        !self.unit.is_split()
            || self.unit.placement[func.0] == Placement::Own
            || self.unit.exported[func.0]
    }

    /// States to LLVM that the length of `value`, a value returned by a call, is not negative:
    /// a callee's `range` attribute holds for a parameter and not for a returned struct, so the
    /// caller asserts it. Does nothing unless `ty` is an array, a string, or the optional of one.
    fn assume_len(&self, ty: Type, value: BasicValueEnum<'ctx>) {
        if !layout::three_words(ty) {
            return;
        }
        let len = self.value_len(value);
        let not_negative =
            self.int_compare(IntPredicate::SGE, len, self.context.i64_type().const_zero());
        let assume = Intrinsic::find("llvm.assume")
            .and_then(|assume| assume.get_declaration(&self.module, &[]))
            .expect("LLVM should provide `llvm.assume`, which takes no type parameters");
        self.call(assume, &[not_negative.into()]);
    }

    /// `call_value` for a helper of `to_string` or `eq`, which calls a written method with its
    /// own parameters. A prelude method also takes the two location arguments, and a helper has
    /// no caller to name, so it passes `<prelude>`.
    // shortcut: a panic in a prelude `to_string` or `eq` names `<prelude>`, not the caller; give
    // the helpers a location parameter when the prelude writes such a method that can panic.
    fn call_written(
        &self,
        func: FuncId,
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> Option<BasicValueEnum<'ctx>> {
        let mut args = args.to_vec();
        if self.mir.functions[func.0].prelude {
            let (at, at_len) = self.text("<prelude>");
            args.push(at.into());
            args.push(at_len.into());
        }
        self.call_value(func, &args)
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

    /// Declares function `index`, `f`; a `Copy` of the unit gets a body for the optimizer only.
    fn declare(&mut self, index: usize, f: &mir::Function) {
        let ptr_type = self.context.ptr_type(AddressSpace::default());
        let mut params: Vec<BasicMetadataTypeEnum<'_>> = f.locals[..f.params]
            .iter()
            .flat_map(|p| self.param_types(p.ty))
            .collect();
        params.extend(self.out_param_types(&f.muts));
        // A prelude function takes its caller's location: a pointer and a length.
        if f.prelude {
            params.push(ptr_type.into());
            params.push(self.context.i64_type().into());
        }
        let fn_type = self.function_type(Some(f.ret), params);
        // `mono` names a method of a conformance block with its interface (`C.Label.name`), so
        // two blocks that each hold `name` get two symbols.
        let placement = self.unit.placement[index];
        // Only a function that another unit refers to is external; the others stay internal, so
        // that LLVM can inline, specialize and delete them as in a build of one unit.
        let exported = self.unit.is_split() && self.unit.exported[index];
        let linkage = match placement {
            Placement::Own if !exported => Linkage::Internal,
            // A body for the optimizer only: LLVM drops it, and the owner's unit has the symbol.
            Placement::Copy => Linkage::AvailableExternally,
            Placement::Own | Placement::Other => Linkage::External,
        };
        let function =
            self.module
                .add_function(&format!("sisu.{}", f.name), fn_type, Some(linkage));
        // A hidden declaration is a symbol that the object file lists as undefined, even when
        // nothing uses it, so only an exported function gets one: its owner defines it.
        if self.unit.is_split() && (placement == Placement::Copy || exported) {
            function
                .as_global_value()
                .set_visibility(GlobalVisibility::Hidden);
        }
        let tys: Vec<Type> = f.locals[..f.params].iter().map(|p| p.ty).collect();
        self.mark_len_params(function, 0, &tys);
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
        self.last_outs.get_mut().clear();
        self.flagged = if self.rc_fault.is_some() {
            vec![true; f.locals.len()]
        } else {
            mir::flagged(f)
        };
        self.slots = f
            .locals
            .iter()
            .map(|local| {
                let ty = if local.token {
                    self.context.ptr_type(AddressSpace::default()).into()
                } else {
                    self.llvm_type(local.ty)?
                };
                let name = local.name.as_deref().unwrap_or("");
                Some((self.entry_alloca(ty, name), ty))
            })
            .collect();
        let tys: Vec<Type> = f.locals[..f.params].iter().map(|p| p.ty).collect();
        for (local, param) in (0..f.params)
            .map(LocalId)
            .zip(self.joined_params(function, 0, &tys))
        {
            self.store(self.slot(local).0, param);
        }
        let values: usize = tys.iter().map(|&ty| self.param_types(ty).len()).sum();
        self.outs = Self::out_pointers(function, values, &f.muts);
        self.caller.set(f.prelude.then(|| {
            // The location follows the Sisu parameters, so it is the last two before a pointer
            // to the result, when the result stays in memory.
            let after = u32::from(self.llvm_type(f.ret).is_some_and(|ty| self.by_memory(ty)));
            let hidden =
                |from_end| function.get_nth_param(function.count_params() - after - from_end);
            (
                hidden(2)
                    .expect("a prelude function should have its location pointer")
                    .into_pointer_value(),
                hidden(1)
                    .expect("a prelude function should have its location length")
                    .into_int_value(),
            )
        }));
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
            if let Some(exit) = tail_return(f, data) {
                for stmt in &exit.stmts {
                    self.statement(stmt);
                }
                self.terminator(&exit.term);
            } else {
                self.terminator(&data.term);
            }
        }
        self.caller.set(None);
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
            Statement::Push { place, value, at } => {
                let (value, _) = self.operand(value);
                let (slot, ty, _) = self.address(place.local, &place.projections, false);
                self.array_push(slot, ty, value, *at);
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
                let (address, _, tag) = self.address(place.local, array, false);
                let len = self.load_len(address, tag);
                let (index, _) = self.operand(index);
                self.check_bounds(len, index.into_int_value(), *span);
            }
            Statement::TakeApart {
                scrutinee,
                tokens,
                bindings,
                rest,
            } => self.take_apart(*scrutinee, tokens, bindings, rest),
            Statement::Free(token) => {
                let free = self.helper(Helper::Free, self.types[token.0]);
                self.call(free, &[self.load_local(*token).into()]);
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
            Terminator::TestPresent { place, some, none } => {
                let (address, ty, _, boxed) = self.located(place.local, &place.projections, true);
                let present = self.present_at(address, ty, boxed);
                self.branch_on(present, self.block(*some), self.block(*none));
            }
            Terminator::SwitchValue {
                operand,
                arms,
                otherwise,
            } => self.switch_value(operand, arms, *otherwise),
            Terminator::Return(value) => {
                self.return_value(value.as_ref().map(|value| self.operand(value).0));
            }
            Terminator::Panic { message, span } => {
                self.panic_value(self.operand(message).0, *span);
            }
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
        let dest_ty = ty;
        let ty =
            || ty.expect("lowering should give this rvalue a destination, whose type it needs");
        let value = match rvalue {
            Rvalue::Use(op) => self.operand(op).0,
            Rvalue::Copy(place) => {
                let (value, ty) = self.read(place, true);
                self.retain(value, ty);
                value
            }
            Rvalue::CopyStr(text) => self.string_lit(text),
            Rvalue::Borrow(place) => self.read(place, true).0,
            Rvalue::Unwrap(op) => {
                let (value, ty) = self.operand(op);
                self.optional_value(ty, value, "value")
            }
            Rvalue::Call {
                func, args, span, ..
            } => {
                let mut args: Vec<BasicMetadataValueEnum<'_>> = (args.iter())
                    .flat_map(|arg| {
                        let (value, ty) = self.operand(arg);
                        self.split(ty, value)
                    })
                    .collect();
                let callee = &self.mir.functions[func.0];
                if callee.muts.contains(&true) {
                    *self.last_outs.borrow_mut() =
                        self.push_out_slots(&callee.muts, |i| callee.locals[i].ty, &mut args);
                }
                if callee.prelude {
                    // The caller's own location when the caller is the prelude's too, and the
                    // call's when it is the user's.
                    let (at, at_len) = self.report_site(*span);
                    args.push(at.into());
                    args.push(at_len.into());
                }
                return self.call_value(*func, &args);
            }
            Rvalue::New { class, args } => self.new_object(*class, &self.values(args)).into(),
            Rvalue::Variant {
                id,
                variant,
                args,
                reuse,
            } => self.variant(*id, *variant, &self.values(args), reuse),
            Rvalue::ArrayLit(elements) => self.array_lit(ty(), &self.values(elements)),
            Rvalue::ArrayFill { value, len, span } => {
                let (filler, _) = self.operand(value);
                let (count, _) = self.operand(len);
                self.array_fill(filler, count.into_int_value(), ty(), *span)
            }
            Rvalue::Slice {
                base,
                start,
                end,
                inclusive,
                safe,
                span,
            } => {
                let (value, base_ty) = self.operand(base);
                let bound = |op: &Option<Operand>| {
                    op.as_ref().map(|op| self.operand(op).0.into_int_value())
                };
                let (start, end) = (bound(start), bound(end));
                self.slice((value, base_ty), (start, end), (*inclusive, *safe), *span)
            }
            Rvalue::Wrap(op) => self.wrap(ty(), self.operand(op).0),
            Rvalue::ToAny(op) => {
                let (value, source) = self.operand(op);
                self.to_any(source, value, ty())
            }
            Rvalue::Trim(op) => {
                let (value, ty) = self.operand(op);
                self.trim(ty, value)
            }
            Rvalue::DynCall {
                interface,
                iargs,
                method,
                args,
                muts,
            } => {
                let (receiver, any) = self.operand(&args[0]);
                let owned: Vec<_> = args[1..].iter().map(|arg| self.operand(arg)).collect();
                let rest = TypeArgs::intern(&iargs.get()[1..]);
                return self.dyn_call(
                    any,
                    (*interface, rest, *method),
                    receiver,
                    (&owned, muts),
                    dest_ty,
                );
            }
            Rvalue::Concat { lhs, rhs } => {
                let lhs = self.operand(lhs).0;
                self.concat(lhs, self.operand(rhs).0)
            }
            Rvalue::Interpolate { parts, formats, .. } => {
                let parts: Vec<_> = parts.iter().map(|part| self.operand(part)).collect();
                // Text, numbers, `bool` and `char` go in one block of known size; a compound part
                // goes through a sink.
                let text = if parts.iter().all(|&(_, ty)| text_part(ty)) {
                    self.interpolate(&parts, formats)
                } else {
                    self.interpolate_compound(&parts, formats)
                };
                for (value, ty) in parts {
                    self.release(value, ty);
                }
                text
            }
            Rvalue::ReadLine => self.read_line(),
            Rvalue::Out { index } => {
                let slot = self.last_outs.borrow()[*index]
                    .expect("`out` should follow a call that has that `mut` parameter");
                let ty = self
                    .llvm_type(ty())
                    .expect("a `mut` parameter should have a value type");
                self.load(ty, slot)
            }
            Rvalue::SetOut { index, value } => {
                let (value, _) = self.operand(value);
                let slot = self.outs[*index].expect("`set_out` should name a `mut` parameter");
                self.store(slot, value);
                return None;
            }
            Rvalue::Chars(op) => self.chars(self.operand(op).0),
            Rvalue::Bytes(op) => self.bytes(self.operand(op).0),
            Rvalue::Words(op) => self.words(self.operand(op).0),
            Rvalue::FromChars(op) => self.string_from_chars(self.operand(op).0),
            Rvalue::Len(op) => self.value_len(self.operand(op).0).into(),
            Rvalue::LenWords(op) => self.len_words(self.operand(op).0).into(),
            Rvalue::Convert(op) => self.convert(ty(), self.operand(op).0),
            Rvalue::Math { op, operands } => {
                let values: Vec<_> = operands.iter().map(|o| self.operand(o).0).collect();
                self.math(*op, &values)
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
            Rvalue::Equal {
                negated,
                lhs,
                rhs,
                none,
                ..
            } => {
                let (lhs, ty) = self.operand(lhs);
                let (rhs, other) = self.operand(rhs);
                // `x == None` on a counted optional tests whether `x` holds a value; it compares
                // no payload, so it calls no `eq` helper, which a class has none of.
                if let Some(side) = none {
                    let (value, held) = match side {
                        Side::Lhs => (rhs, other),
                        Side::Rhs => (lhs, ty),
                    };
                    if held.is_counted(&self.program.enums) {
                        let present = self.has_value(held, value, "present");
                        return Some(if *negated {
                            present.into()
                        } else {
                            self.builder
                                .build_not(present, "none")
                                .expect(POSITIONED)
                                .into()
                        });
                    }
                }
                self.equal_value(*negated, ty, lhs, rhs).into()
            }
            Rvalue::Is { lhs, rhs } => {
                let lhs = self.object(lhs);
                self.same_object(lhs, self.object(rhs)).into()
            }
            Rvalue::Print {
                stream, operand, ..
            } => {
                let (value, ty) = self.operand(operand);
                self.print(*stream, ty, value);
                return None;
            }
            Rvalue::Dbg {
                at, text, operand, ..
            } => {
                let (value, ty) = self.operand(operand);
                self.dbg(*at, text, ty, value);
                return None;
            }
        };
        Some(value)
    }

    /// The values of `operands`, left to right.
    fn values(&self, operands: &[Operand]) -> Vec<BasicValueEnum<'ctx>> {
        operands.iter().map(|op| self.operand(op).0).collect()
    }

    /// The object that an operand of `is` holds: an object, or the data of an `any` value, which
    /// is `null` for a `None`.
    fn object(&self, op: &Operand) -> PointerValue<'ctx> {
        let (value, ty) = self.operand(op);
        match ty.kind() {
            TypeKind::Any(..) => self.any_data(value),
            TypeKind::Optional(payload) if matches!(payload.kind(), TypeKind::Any(..)) => {
                self.any_data(value)
            }
            _ => value.into_pointer_value(),
        }
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
            Const::None { ty, .. } => (self.empty(*ty), *ty),
            Const::Str(text) => (self.string_const(text), Type::STRING),
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
                let tag = tag.expect("a field or an element should have a TBAA tag");
                (self.load_place(ty, address, "load", Some(tag)), ty)
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
        let (address, ty, tag, boxed) = self.located(local, projections, checked);
        assert!(
            !boxed,
            "mir should only test the presence of a boxed `F?`, which has no address of a value"
        );
        (address, ty, tag)
    }

    /// `address`, and whether the address is the storage of a boxed field of an optional type,
    /// whose box may be `null`, in place of the address of a value.
    fn located(
        &self,
        local: LocalId,
        projections: &[Projection],
        checked: bool,
    ) -> (PointerValue<'ctx>, Type, Option<MetadataValue<'ctx>>, bool) {
        self.check_not_moved(local);
        let (mut address, _) = self.slot(local);
        let mut ty = self.types[local.0];
        let mut tag = None;
        let mut boxed = false;
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
                    let place;
                    (place, boxed) = self.field_place(address, id, *variant, *field);
                    (
                        place,
                        self.program.enums[id.0].variants[*variant].fields[*field].ty,
                        None,
                    )
                }
                Projection::Index { index, span } => {
                    // `tag` is the tag of the array's own place, the step before this one.
                    let data = self.load_data(address, tag);
                    let index = self.operand(index).0.into_int_value();
                    if checked {
                        self.check_bounds(self.load_len(address, tag), index, *span);
                    }
                    let element = element_of(ty);
                    (
                        self.element_ptr(element, data, index),
                        element,
                        Some(self.element_tag),
                    )
                }
                Projection::Some => {
                    let place;
                    (place, ty, boxed) = self.some_place(address, ty, boxed);
                    (place, ty, None)
                }
            };
        }
        (address, ty, tag, boxed)
    }

    /// The first element of the array at `address`, tagged `tag` when its place has one.
    fn load_data(
        &self,
        address: PointerValue<'ctx>,
        tag: Option<MetadataValue<'ctx>>,
    ) -> PointerValue<'ctx> {
        self.load_word(address, 1, "data", tag).into_pointer_value()
    }

    /// The length of the array at `address`, tagged `tag` when its place has one, and marked as
    /// `mark_len` does.
    fn load_len(
        &self,
        address: PointerValue<'ctx>,
        tag: Option<MetadataValue<'ctx>>,
    ) -> IntValue<'ctx> {
        self.load_word(address, 2, "len", tag).into_int_value()
    }

    /// The pointer at `address`: an object, or the block of an array or a string.
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
        debug_assert!(
            self.mir
                .main
                .is_none_or(|main| self.refers_only_to_exported(FuncId(main))),
            "`main` is not defined in unit 0 and is not exported, so the link would fail"
        );
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
        let args_type = Type::intern(TypeKind::Array(Type::STRING));
        let mut passed = if sisu_main.count_params() == 0 {
            Vec::new()
        } else {
            let args = self.args(count.into_int_value(), strings.into_pointer_value());
            self.split(args_type, args)
        };
        // A `mut` parameter of `main` has an out slot too. Nothing reads the final value, so it
        // is released after the call.
        let entry = (self.functions.iter().position(|&f| f == sisu_main))
            .map(|id| &self.mir.functions[id])
            .expect("`sisu.main` should be one of the program's functions");
        let slots = self.push_out_slots(&entry.muts, |i| entry.locals[i].ty, &mut passed);
        self.builder
            .build_call(sisu_main, &passed, "")
            .expect(POSITIONED);
        for slot in slots.into_iter().flatten() {
            let ty = (self.llvm_type(args_type))
                .expect("the arguments of `main` should have a value type");
            self.release(self.load(ty, slot), args_type);
        }
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
        self.copied_at(ty, ptr, "old")
    }

    fn store(&self, ptr: PointerValue<'ctx>, value: BasicValueEnum<'ctx>) {
        if let Some(ty) = self.memory_type(value) {
            self.copy_memory(ptr, value.into_pointer_value(), ty);
            return;
        }
        self.builder.build_store(ptr, value).expect(POSITIONED);
    }

    /// `print` or `eprint` of an evaluated operand of type `ty`, which it borrows.
    fn print(&self, stream: Stream, ty: Type, value: BasicValueEnum<'ctx>) {
        if !matches!(
            ty.kind(),
            TypeKind::I64
                | TypeKind::U8
                | TypeKind::F64
                | TypeKind::Bool
                | TypeKind::Char
                | TypeKind::String
        ) {
            self.print_compound(stream, ty, value);
            return;
        }
        if ty == Type::STRING {
            self.print_string(stream, value);
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
    /// holds; see `panic_with`. In a prelude function the location is its caller's.
    fn panic_if(&self, fails: IntValue<'ctx>, span: Span, message: &str) {
        self.panic_with(fails, |codegen| {
            if let Some((at, at_len)) = codegen.caller.get() {
                let (msg, len) = codegen.text(message);
                (
                    codegen.panic_message,
                    vec![at.into(), at_len.into(), msg.into(), len.into()],
                )
            } else {
                let (msg, len) = codegen.text(&format!("{}: {message}", codegen.location(span)));
                (codegen.panic, vec![msg.into(), len.into()])
            }
        });
    }

    /// `<path>:<line>:<col>` for the start of `span`, which is in the user's file: a panic in
    /// the prelude reports the location of its caller.
    fn location(&self, span: Span) -> String {
        assert!(
            !span.in_prelude(),
            "a prelude span should not reach a baked location: the function should report its caller's"
        );
        let (line, col) = self.analyses.lines.line_col(self.source, span.start());
        format!("{}:{line}:{col}", self.path)
    }

    /// A pointer to the bytes of `text`, in a global, and its length: the message of a panic, or
    /// the position that a panic or a `dbg` reports. The builder is positioned.
    fn text(&self, text: &str) -> (PointerValue<'ctx>, IntValue<'ctx>) {
        let ptr = self
            .builder
            .build_global_string_ptr(text, "report.text")
            .expect(POSITIONED);
        let len = self.context.i64_type().const_int(
            u64::try_from(text.len()).expect("a text should be shorter than 2^64 bytes"),
            false,
        );
        (ptr.as_pointer_value(), len)
    }

    /// The position that a panic or a `dbg` at `span` reports: the text and its length. A prelude
    /// function takes it from its caller.
    fn report_site(&self, span: Span) -> (PointerValue<'ctx>, IntValue<'ctx>) {
        self.caller
            .get()
            .unwrap_or_else(|| self.text(&self.location(span)))
    }

    /// Branches to a fresh panic block when `fails` holds, and continues in a fresh block
    /// otherwise. The panic block calls the function and arguments that `report` gives, which
    /// never returns.
    fn panic_with(
        &self,
        fails: IntValue<'ctx>,
        report: impl FnOnce(&Self) -> (FunctionValue<'ctx>, Vec<BasicMetadataValueEnum<'ctx>>),
    ) {
        let current = self.current_function();
        let panic_block = self.context.append_basic_block(current, "panic");
        let ok = self.context.append_basic_block(current, "ok");
        self.builder
            .build_conditional_branch(fails, panic_block, ok)
            .expect(POSITIONED);
        self.builder.position_at_end(panic_block);
        let (function, args) = report(self);
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
        let stored = incoming
            .first()
            .and_then(|(value, _)| self.memory_type(value.as_basic_value_enum()));
        match stored {
            Some(stored) => self.in_memory(phi.as_basic_value().into_pointer_value(), stored),
            None => phi.as_basic_value(),
        }
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

/// Initializes LLVM's native target. The driver calls it once on the calling thread, before the
/// threads of a split build make their target machines with `create_target_machine`.
pub(crate) fn initialize_native_target() -> Result<(), String> {
    Target::initialize_native(&InitializationConfig::default())
}

/// A target machine for the host `sisu` runs on. It initializes the native target first, so it
/// suits one thread.
pub(crate) fn target_machine(level: OptLevel) -> Result<TargetMachine, String> {
    initialize_native_target()?;
    create_target_machine(level)
}

/// A target machine for the host `sisu` runs on, after `initialize_native_target`. Several
/// threads can each make their own. inkwell takes a read lock for the lookup of the target, and
/// the initialization takes the write lock.
pub(crate) fn create_target_machine(level: OptLevel) -> Result<TargetMachine, String> {
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

/// The return block that `data` reaches by `goto`, through empty blocks. It is `None` unless
/// `data` ends in a call. The return block must only release, and then return the call's result.
///
/// `define` emits that block's code in place of the `goto`. The call and its `ret` are then in
/// one LLVM block. The `-O2` pass `TailCallElim` makes a loop of a tail call only when the
/// call's block ends in `ret %call`. A result with several LLVM fields, such as an array, can
/// join the branches of an `if` in one return block. SROA then makes one `phi` for each field.
/// It builds the value again with `insertvalue` in the return block. The call is then not a tail
/// call, and a deep recursion overflows the stack. The original return block stays for the other
/// paths.
fn tail_return<'f>(f: &'f mir::Function, data: &mir::Block) -> Option<&'f mir::Block> {
    let Some(Statement::Assign {
        dest: Some(dest),
        value: Rvalue::Call { .. },
    }) = data.stmts.last()
    else {
        return None;
    };
    let mut term = &data.term;
    // A chain of empty blocks is at most as long as the function; the bound stops a cycle.
    for _ in 0..f.blocks.len() {
        let Terminator::Goto(to) = term else {
            return None;
        };
        let next = &f.blocks[to.0];
        if next.stmts.is_empty() && matches!(next.term, Terminator::Goto(_)) {
            term = &next.term;
            continue;
        }
        // Lowering writes an uncounted result, such as an `i64?`, as a read of the local.
        let returns_result = match &next.term {
            Terminator::Return(Some(Operand::Move(local))) => local == dest,
            Terminator::Return(Some(Operand::Read(place))) => {
                place.projections.is_empty() && place.local == *dest
            }
            _ => false,
        };
        let only_releases = next.stmts.iter().all(|stmt| {
            matches!(
                stmt,
                Statement::Release(_) | Statement::ScopeRelease(_) | Statement::End(_)
            )
        });
        return (returns_result && only_releases).then_some(next);
    }
    None
}

/// Whether an interpolation writes a part of type `ty` itself, in the one block of its result: a
/// number, a `bool`, a `char` or a `string`.
fn text_part(ty: Type) -> bool {
    matches!(
        ty.kind(),
        TypeKind::I64
            | TypeKind::U8
            | TypeKind::F64
            | TypeKind::Bool
            | TypeKind::Char
            | TypeKind::String
    )
}

/// Verifies `module` in a debug build, then runs the pipeline. A release build does not verify,
/// because the verifier takes about 180 ms of a 143k-line build. The tests that run `sisu` run a
/// debug build, so it verifies each module that they build. No verifier runs after the pipeline: a
/// pass that broke the IR reaches instruction selection, which would be an LLVM bug.
pub(crate) fn optimize(
    module: &Module<'_>,
    machine: &TargetMachine,
    level: OptLevel,
) -> Result<(), String> {
    if cfg!(debug_assertions) {
        module.verify().map_err(|e| e.to_string())?;
    }
    module
        .run_passes(pipeline(level), machine, PassBuilderOptions::create())
        .map_err(|e| e.to_string())
}

/// Compiles `module`, which `optimize` ran, to object code for `machine`, in memory.
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
