//! Reference counting: retaining, releasing and comparing counted values, and the helpers
//! that do it. Each counted type gets one helper of each kind, declared on first use and
//! defined by `define_helpers` in one worklist pass. A string, an object or an array is a
//! pointer, which `@sisu_rc.retain` and its `release` helper take; an enum is a value, which its
//! `retain`, `release` and `eq` helpers take by address. An enum whose component has boxes also
//! has `walk`, `drain` and `eq_component` helpers (see `codegen/enums.rs`).

use inkwell::basic_block::BasicBlock;
use inkwell::module::Linkage;
use inkwell::types::StructType;
use inkwell::values::{BasicValueEnum, FunctionValue, IntValue, PointerValue};
use inkwell::{AddressSpace, FloatPredicate, IntPredicate};

use super::abi::{RC_FAULT_MOVED, RC_FAULT_RELEASE, RC_FAULT_RETAIN};
use super::layout::{HeapBlock, Niche, Storage, niche};
use super::{Codegen, POSITIONED};
use crate::graph::components;
use crate::tir::{ClassId, LocalId, Program, Type, TypeArgs, TypeKind};

/// A kind of helper that a counted type has.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Helper {
    Retain,
    Release,
    Drop,
    Eq,
    Unique,
    /// `@sisu_rc.walk.<F>(value, heads)`: releases a value's counted parts, listing its boxes.
    Walk,
    /// `@sisu_rc.drain.<F0>(heads)`: frees the boxes of an enum component, or the objects of
    /// a cycle of classes, on its pending lists.
    Drain,
    /// `@sisu_rc.eq_component.<F0>(kind, a, b)`: `==` on values of a component's member `kind`.
    EqComponent,
    /// `@sisu_rc.unbox.<F>(slot, out) -> ptr`: moves the value out of the box in `slot`, and
    /// gives the box when nothing else holds it.
    Unbox,
    /// `@sisu_rc.free.<F>(token)`: frees a token's box, if it holds one.
    Free,
    /// `@sisu_rc.reuse.<F>(token) -> ptr`: a token's box, or a new block for a box of `F` when
    /// the token is `null`.
    Reuse,
}

/// The cycles of classes that hold each other, which `define_drop` frees in a loop. A class is
/// in a cycle when a class holds it in a field, `X` or `X?`, along a path that comes back to
/// the holder: its strongly connected component of that graph has two classes, or it holds
/// itself. A field of an array or of an enum is not an edge.
pub(super) struct ClassCycles {
    /// For each class, the index in `members` of its cycle, when it is in one.
    cycle: Vec<Option<usize>>,
    /// For each class, its index among the classes of its cycle.
    position: Vec<usize>,
    /// The classes of each cycle, in declaration order.
    members: Vec<Vec<ClassId>>,
}

impl ClassCycles {
    /// The classes of the cycle of `class`, in declaration order, when it is in one.
    pub(super) fn members(&self, class: ClassId) -> Option<&[ClassId]> {
        self.cycle[class.0].map(|cycle| self.members[cycle].as_slice())
    }

    /// The index of `class` among the classes of its cycle.
    pub(super) fn position(&self, class: ClassId) -> usize {
        self.position[class.0]
    }

    /// The index among the classes of the cycle of `holder` of the class that a field of type
    /// `field` holds, when `holder` is in a cycle and that class is in it too.
    fn link(&self, program: &Program, holder: ClassId, field: Type) -> Option<usize> {
        let TypeKind::Class(held, _) = counted_core(program, field)?.kind() else {
            return None;
        };
        let cycle = self.cycle[holder.0]?;
        (self.cycle[held.0] == Some(cycle)).then(|| self.position[held.0])
    }
}

/// The `ClassCycles` of `program`, by `components` of the graph with an edge from each class to
/// each class it holds in a field. It reads every field once.
pub(super) fn class_cycles(program: &Program) -> ClassCycles {
    let held = |ty| match counted_core(program, ty)?.kind() {
        TypeKind::Class(id, _) => Some(id.0),
        _ => None,
    };
    let edges: Vec<Vec<usize>> = program
        .classes
        .iter()
        .map(|class| class.fields.iter().filter_map(|f| held(f.ty)).collect())
        .collect();
    let mut groups = vec![Vec::new(); edges.len()];
    for (class, component) in components(&edges).into_iter().enumerate() {
        groups[component].push(class);
    }
    let mut cycles = ClassCycles {
        cycle: vec![None; edges.len()],
        position: vec![0; edges.len()],
        members: Vec::new(),
    };
    for group in groups {
        let loops = group.len() > 1 || group.iter().any(|&class| edges[class].contains(&class));
        if !loops {
            continue;
        }
        for (position, &class) in group.iter().enumerate() {
            cycles.cycle[class] = Some(cycles.members.len());
            cycles.position[class] = position;
        }
        cycles
            .members
            .push(group.into_iter().map(ClassId).collect());
    }
    cycles
}

/// The type whose helpers a pointer-shaped `ty` uses: a `string`, a class or an array, with
/// the `?` of a `null` niche removed. `None` for every other type, an enum, its optional and an
/// optional of an optional included: those are not pointers, and a `None` of theirs is not
/// `null`.
pub(super) fn counted_core(program: &Program, ty: Type) -> Option<Type> {
    match ty.kind() {
        TypeKind::String | TypeKind::Class(..) | TypeKind::Array(_) => Some(ty),
        TypeKind::Optional(payload) => match niche(program, payload) {
            Some(Niche {
                storage: Storage::Pointer,
                ..
            }) => Some(payload),
            _ => None,
        },
        _ => None,
    }
}

impl<'ctx> Codegen<'ctx, '_> {
    /// Declares `sisu_rc.<kind>.<name>` on first use and queues its body: `kind` is `retain`,
    /// `release`, `drop`, `eq`, `unique`, `walk`, `drain`, `eq_component`, `unbox`, `free` or
    /// `reuse`, and `name` is a class's or an enum's name, `string`, or `[<element name>]` for an
    /// array (`[Node?]`, `[[i64]]`).
    pub(super) fn helper(&self, kind: Helper, ty: Type) -> FunctionValue<'ctx> {
        let key = (kind, ty);
        if let Some(function) = self.helpers.borrow().get(&key) {
            return *function;
        }
        let name = self.program.type_name(ty);
        let ptr_type = self.context.ptr_type(AddressSpace::default());
        let takes_object = self.context.void_type().fn_type(&[ptr_type.into()], false);
        let (prefix, fn_type) = match kind {
            Helper::Retain => ("retain", takes_object),
            Helper::Release => ("release", takes_object),
            Helper::Drop => ("drop", takes_object),
            Helper::Eq => (
                "eq",
                self.context
                    .bool_type()
                    .fn_type(&[ptr_type.into(), ptr_type.into()], false),
            ),
            Helper::Unique => ("unique", ptr_type.fn_type(&[ptr_type.into()], false)),
            Helper::Walk => (
                "walk",
                self.context
                    .void_type()
                    .fn_type(&[ptr_type.into(), ptr_type.into()], false),
            ),
            Helper::Drain => ("drain", takes_object),
            Helper::Unbox => (
                "unbox",
                ptr_type.fn_type(&[ptr_type.into(), ptr_type.into()], false),
            ),
            Helper::Free => ("free", takes_object),
            Helper::Reuse => ("reuse", ptr_type.fn_type(&[ptr_type.into()], false)),
            Helper::EqComponent => (
                "eq_component",
                self.context.bool_type().fn_type(
                    &[
                        self.context.i32_type().into(),
                        ptr_type.into(),
                        ptr_type.into(),
                    ],
                    false,
                ),
            ),
        };
        let function = self.module.add_function(
            &format!("sisu_rc.{prefix}.{name}"),
            fn_type,
            Some(Linkage::Internal),
        );
        self.helpers.borrow_mut().insert(key, function);
        self.pending.borrow_mut().push(key);
        function
    }

    /// Defines the counting helpers, which take their object without taking ownership:
    /// `@sisu_rc.retain` adds 1 to its count; `@sisu_rc.release.<C>` subtracts 1 and at 0
    /// calls `@sisu_rc.drop.<C>`, which releases each counted field, then frees the object.
    /// Both return at once on `null`. Drop does not recurse on a field of a class in its own
    /// cycle: see `define_drop`.
    /// `@sisu_rc.eq.<C>` is `==` on two objects; see `define_eq`. An array's `drop`, `eq` and
    /// `unique` come from `define_array_helper`, a string's `drop` and `eq` from
    /// `define_string_helper`, an enum's `retain`, `release` and `eq` from `define_enum_helper`.
    /// Defining a body can declare more helpers, so this runs until none is left to define.
    pub(super) fn define_helpers(&self) {
        let (object, done) = self.helper_entry(self.rc_retain);
        self.check_count(object, RC_FAULT_RETAIN);
        self.add_to_count(object, self.context.i64_type().const_int(1, false));
        self.branch_to(done);
        loop {
            let next = self.pending.borrow_mut().pop();
            let Some((kind, ty)) = next else {
                break;
            };
            let function = self.helpers.borrow()[&(kind, ty)];
            match (kind, ty.kind()) {
                (_, TypeKind::Enum(id, _)) => self.define_enum_helper(kind, id, function),
                (Helper::Release, _) => self.define_release(function, ty),
                (Helper::Retain, _) => {
                    unreachable!("only an enum type should be queued for a `retain` helper")
                }
                (_, TypeKind::Array(element)) => self.define_array_helper(kind, element, function),
                (_, TypeKind::String) => self.define_string_helper(kind, function),
                (Helper::Drop, _) => self.define_drop(function, ty),
                (Helper::Drain, _) => self.define_class_drain(function, ty),
                (Helper::Eq, _) => self.define_eq(function, ty),
                (Helper::Unique, _) => {
                    unreachable!("only an array type should be queued for a `unique` helper")
                }
                (Helper::Walk | Helper::EqComponent, _) => {
                    unreachable!("only an enum type should be queued for a component helper")
                }
                (Helper::Unbox | Helper::Free | Helper::Reuse, _) => {
                    unreachable!("only an enum type should be queued for a box's helper")
                }
            }
        }
    }

    /// `@sisu_rc.release.<C>`: subtracts 1 from the count, and at 0 drops the object or array.
    fn define_release(&self, function: FunctionValue<'ctx>, ty: Type) {
        let (object, done) = self.helper_entry(function);
        let drop_block = self.context.append_basic_block(function, "drop");
        self.decrement(object, drop_block, done);
        self.builder.position_at_end(drop_block);
        self.call(self.helper(Helper::Drop, ty), &[object.into()]);
        self.branch_to(done);
    }

    /// `@sisu_rc.drop.<C>`: frees the object, and in the same frame each object that it holds the
    /// last reference to, at any depth, in the cycle of classes of `C` (see `class_cycles`).
    /// A field of a class in the cycle that is not `null` loses 1 from its object's count. At 0,
    /// that object goes onto the pending list of its class, linked to the old head through its
    /// count word, which is dead by then. Every other counted field goes through `release_at`.
    /// A loop reads every field of an object, frees it, then takes the next object off a list.
    /// No other holder reaches a listed object, so no RC check reads that word. A class in no
    /// cycle has no loop. A class that is alone in its cycle loops in its own frame. Else the
    /// object is already dead, its count 0 from `release`, so it only goes onto the list of
    /// its class, with no decrement, and `@sisu_rc.drain.<C0>` of the first class `C0` of the
    /// cycle runs the loop for the cycle. So the code for a cycle is as large as the cycle.
    fn define_drop(&self, drop: FunctionValue<'ctx>, ty: Type) {
        let TypeKind::Class(id, _) = ty.kind() else {
            unreachable!("only a class type should reach the class helper bodies")
        };
        let ptr_type = self.context.ptr_type(AddressSpace::default());
        let entry = self.context.append_basic_block(drop, "entry");
        self.builder.position_at_end(entry);
        let Some(members) = self.class_cycles.members(id) else {
            self.release_class_fields(id, param(drop), None);
            self.free_class(id, param(drop));
            self.builder.build_return(None).expect(POSITIONED);
            return;
        };
        let heads_type = self.heads_type(members.len());
        let heads = self.entry_alloca(heads_type.into(), "heads");
        self.store(heads, heads_type.const_zero().into());
        if members.len() > 1 {
            let head = self.head(heads_type, heads, self.class_cycles.position(id));
            self.store(head, param(drop).into());
            // The object is the whole list, so the link in its count word ends it.
            self.store(param(drop), ptr_type.const_null().into());
            let first = Type::intern(TypeKind::Class(members[0], TypeArgs::NONE));
            self.call(self.helper(Helper::Drain, first), &[heads.into()]);
            self.builder.build_return(None).expect(POSITIONED);
            return;
        }
        let head = self.head(heads_type, heads, 0);
        let (body, done) = (
            self.context.append_basic_block(drop, "loop"),
            self.context.append_basic_block(drop, "done"),
        );
        self.branch_to(body);
        self.builder.position_at_end(body);
        let phi = self
            .builder
            .build_phi(ptr_type, "object")
            .expect(POSITIONED);
        phi.add_incoming(&[(&param(drop), entry)]);
        let object = phi.as_basic_value().into_pointer_value();
        self.release_class_fields(id, object, Some((heads_type, heads)));
        self.free_class(id, object);
        let take = self.context.append_basic_block(drop, "take");
        let taken = self
            .builder
            .build_load(ptr_type, head, "taken")
            .expect(POSITIONED)
            .into_pointer_value();
        let null = self.builder.build_is_null(taken, "null").expect(POSITIONED);
        self.branch_on(null, done, take);
        self.builder.position_at_end(take);
        let following = self
            .builder
            .build_load(ptr_type, taken, "following")
            .expect(POSITIONED);
        self.store(head, following);
        phi.add_incoming(&[(&taken, take)]);
        self.branch_to(body);
        self.builder.position_at_end(done);
        self.builder.build_return(None).expect(POSITIONED);
    }

    /// `@sisu_rc.drain.<C0>(heads)`, for the cycle of classes whose first class is `C0`: while
    /// some list in `heads` is not empty, takes the object at the head of the first such list,
    /// makes the next object its head, releases the fields of the object
    /// (`release_class_fields`) and frees it. It runs in one frame, however many objects it
    /// frees. It scans the heads from the first for each object, so it costs the number of
    /// objects times the number of classes in the cycle, as the drain of an enum component does.
    fn define_class_drain(&self, function: FunctionValue<'ctx>, ty: Type) {
        let TypeKind::Class(first, _) = ty.kind() else {
            unreachable!("only a class type should reach the class helper bodies")
        };
        let members = self.class_cycles.members(first).expect(
            "only the first class of a cycle should get a `drain` helper, and it has a cycle",
        );
        let heads = param(function);
        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);
        let heads_type = self.heads_type(members.len());
        let ptr_type = self.context.ptr_type(AddressSpace::default());
        let block = |name| self.context.append_basic_block(function, name);
        let next = block("drain");
        self.branch_to(next);
        self.builder.position_at_end(next);
        for (index, &member) in members.iter().enumerate() {
            let head = self.head(heads_type, heads, index);
            let object = self
                .builder
                .build_load(ptr_type, head, "object")
                .expect(POSITIONED)
                .into_pointer_value();
            let (take, empty) = (block("take"), block("empty"));
            let null = self
                .builder
                .build_is_null(object, "null")
                .expect(POSITIONED);
            self.branch_on(null, empty, take);
            self.builder.position_at_end(take);
            let following = self
                .builder
                .build_load(ptr_type, object, "following")
                .expect(POSITIONED);
            self.store(head, following);
            self.release_class_fields(member, object, Some((heads_type, heads)));
            self.free_class(member, object);
            self.branch_to(next);
            self.builder.position_at_end(empty);
        }
        self.builder.build_return(None).expect(POSITIONED);
    }

    /// Releases each counted field of the object of class `id` at `object`. A `null` field of a
    /// class in the cycle of `id`, with its `heads`, is skipped. Another such field loses 1
    /// from its object's count, and at 0 the object becomes the head of its class's list, linked
    /// to the old head through its count word. Every other counted field goes through
    /// `release_at`.
    fn release_class_fields(
        &self,
        id: ClassId,
        object: PointerValue<'ctx>,
        heads: Option<(StructType<'ctx>, PointerValue<'ctx>)>,
    ) {
        let ptr_type = self.context.ptr_type(AddressSpace::default());
        let block = |name| {
            self.context
                .append_basic_block(self.current_function(), name)
        };
        for (index, field) in self.program.classes[id.0].fields.iter().enumerate() {
            let place = self.field_ptr(id, object, index);
            let Some(((heads_type, heads), position)) =
                heads.zip(self.class_cycles.link(self.program, id, field.ty))
            else {
                if field.ty.is_counted(&self.program.enums) {
                    self.release_at(place, field.ty);
                }
                continue;
            };
            let (live, last, next) = (block("link"), block("link.last"), block("link.next"));
            let child = self.load_value(field.ty, place).into_pointer_value();
            let null = self.builder.build_is_null(child, "null").expect(POSITIONED);
            self.branch_on(null, next, live);
            self.builder.position_at_end(live);
            self.decrement(child, last, next);
            self.builder.position_at_end(last);
            let head = self.head(heads_type, heads, position);
            let old = self
                .builder
                .build_load(ptr_type, head, "old")
                .expect(POSITIONED);
            self.store(child, old);
            self.store(head, child.into());
            self.branch_to(next);
            self.builder.position_at_end(next);
        }
    }

    /// Frees the object of class `id` at `object`, with the size and alignment it has.
    fn free_class(&self, id: ClassId, object: PointerValue<'ctx>) {
        let (size, align) = self.heap_size(&HeapBlock::Class(id));
        self.call(self.free, &[object.into(), size.into(), align.into()]);
    }

    /// `@sisu_rc.eq.<C>(a, b)`: see `eq_entry` for the same pointer and `null`; otherwise
    /// whether each pair of fields is equal, comparing the chain field last and stopping at the
    /// first difference. It neither retains nor releases. It does not recurse on the chain
    /// field: it compares the other fields of the pair, then steps both objects to the objects
    /// their chain fields hold. Then it repeats the checks of `eq_entry`, in one frame.
    fn define_eq(&self, function: FunctionValue<'ctx>, ty: Type) {
        let TypeKind::Class(id, _) = ty.kind() else {
            unreachable!("only a class type should reach the class helper bodies")
        };
        let ptr_type = self.context.ptr_type(AddressSpace::default());
        let block = |name| self.context.append_basic_block(function, name);
        let reflexive = self.reflexive(ty);
        let (a, b, equal, unequal) = self.eq_entry(function, reflexive);
        let branch = |cond, then, other| {
            self.builder
                .build_conditional_branch(cond, then, other)
                .expect(POSITIONED);
        };
        let chain = self.program.chain_field(id);
        // With a chain field, the body is a loop over the pairs of the two chains.
        let looped = chain.map(|_| {
            let (entered, body) = (self.current_block(), block("loop"));
            self.branch_to(body);
            self.builder.position_at_end(body);
            let pair = |name| self.builder.build_phi(ptr_type, name).expect(POSITIONED);
            let (pa, pb) = (pair("a"), pair("b"));
            pa.add_incoming(&[(&a, entered)]);
            pb.add_incoming(&[(&b, entered)]);
            (body, pa, pb)
        });
        let (a, b) = looped.map_or((a, b), |(_, pa, pb)| {
            (
                pa.as_basic_value().into_pointer_value(),
                pb.as_basic_value().into_pointer_value(),
            )
        });
        for (index, field) in self.program.classes[id.0].fields.iter().enumerate() {
            if chain == Some(index) {
                continue;
            }
            let place = |object| self.field_ptr(id, object, index);
            let next = block("next");
            branch(self.equal_at(field.ty, place(a), place(b)), next, unequal);
            self.builder.position_at_end(next);
        }
        let (Some(index), Some((body, pa, pb))) = (chain, looped) else {
            self.branch_to(equal);
            return;
        };
        let load = |object| {
            self.builder
                .build_load(ptr_type, self.field_ptr(id, object, index), "chain")
                .expect(POSITIONED)
                .into_pointer_value()
        };
        let (next_a, next_b) = (load(a), load(b));
        self.compare_at_once(reflexive, true, next_a, next_b, equal, unequal);
        let step = self.current_block();
        pa.add_incoming(&[(&next_a, step)]);
        pb.add_incoming(&[(&next_b, step)]);
        self.branch_to(body);
    }

    /// Starts `eq` helper `function` for a type that is `reflexive` or not: returns its two
    /// parameters and the blocks that return `true` and `false`. It returns `true` at once for
    /// what `equal_at_once` finds equal and `false` when one is `null`; the builder is left
    /// where both are real, and distinct when `reflexive`.
    pub(super) fn eq_entry(
        &self,
        function: FunctionValue<'ctx>,
        reflexive: bool,
    ) -> (
        PointerValue<'ctx>,
        PointerValue<'ctx>,
        BasicBlock<'ctx>,
        BasicBlock<'ctx>,
    ) {
        let entry = self.context.append_basic_block(function, "entry");
        let (equal, unequal) = self.eq_returns(function);
        let nth = |n| {
            function
                .get_nth_param(n)
                .expect("the generated `eq` helper should be declared with two parameters")
                .into_pointer_value()
        };
        let (a, b) = (nth(0), nth(1));
        self.builder.position_at_end(entry);
        self.compare_at_once(reflexive, true, a, b, equal, unequal);
        (a, b, equal, unequal)
    }

    /// The blocks of `eq` helper `function` that return `true` and `false`.
    pub(super) fn eq_returns(
        &self,
        function: FunctionValue<'ctx>,
    ) -> (BasicBlock<'ctx>, BasicBlock<'ctx>) {
        let bool_type = self.context.bool_type();
        let [equal, unequal] = [(1, "equal"), (0, "unequal")].map(|(value, name)| {
            let at = self.context.append_basic_block(function, name);
            self.builder.position_at_end(at);
            self.builder
                .build_return(Some(&bool_type.const_int(value, false)))
                .expect(POSITIONED);
            at
        });
        (equal, unequal)
    }

    /// Compares the pointers `a` and `b` of a counted type that is `reflexive` or not, with no
    /// look inside: branches to `equal` when `equal_at_once` finds them equal, and, when either
    /// may be `null` (`nullable`), to `unequal` when only one is. Leaves the builder where they
    /// are two pointers to compare by what they point at.
    pub(super) fn compare_at_once(
        &self,
        reflexive: bool,
        nullable: bool,
        a: PointerValue<'ctx>,
        b: PointerValue<'ctx>,
        equal: BasicBlock<'ctx>,
        unequal: BasicBlock<'ctx>,
    ) {
        let function = self.current_function();
        let differ = self.context.append_basic_block(function, "differ");
        self.branch_on(self.equal_at_once(reflexive, a, b), equal, differ);
        self.builder.position_at_end(differ);
        if nullable {
            let distinct = self.context.append_basic_block(function, "distinct");
            let null = |pointer| {
                self.builder
                    .build_is_null(pointer, "null")
                    .expect(POSITIONED)
            };
            let either_null = self
                .builder
                .build_or(null(a), null(b), "either_null")
                .expect(POSITIONED);
            self.branch_on(either_null, unequal, distinct);
            self.builder.position_at_end(distinct);
        }
    }

    /// Whether two pointers of a counted type are equal with no look inside: the same pointer
    /// when the type is `reflexive`, else both `null`. One block or object that holds a `nan`
    /// is not equal to itself, so only a reflexive type may stop at the same pointer.
    fn equal_at_once(
        &self,
        reflexive: bool,
        a: PointerValue<'ctx>,
        b: PointerValue<'ctx>,
    ) -> IntValue<'ctx> {
        if reflexive {
            return self.same_object(a, b);
        }
        let null = |object| {
            self.builder
                .build_is_null(object, "null")
                .expect(POSITIONED)
        };
        self.builder
            .build_and(null(a), null(b), "both_null")
            .expect(POSITIONED)
    }

    /// Whether every value of `ty` is `==` to itself: no `f64` is reachable from it, directly,
    /// as an optional's payload, an array's element, or a field of a class or a payload field
    /// of an enum it reaches, boxed or inline. `reaches_f64` answers it for a class or an enum.
    pub(super) fn reflexive(&self, ty: Type) -> bool {
        match unwrapped(ty).kind() {
            TypeKind::F64 => false,
            TypeKind::Class(id, _) => !self.reaches_f64[id.0],
            TypeKind::Enum(id, _) => !self.reaches_f64[self.program.classes.len() + id.0],
            _ => true,
        }
    }

    /// Whether `lhs` and `rhs`, of type `ty`, are equal: `==` without releasing them.
    pub(super) fn equal(
        &self,
        ty: Type,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
    ) -> IntValue<'ctx> {
        if let Some(core) = counted_core(self.program, ty) {
            return self.call_eq(core, lhs, rhs);
        }
        let payload = match ty.kind() {
            TypeKind::Enum(..) => {
                return self.equal_at(ty, self.enum_slot(lhs), self.enum_slot(rhs));
            }
            TypeKind::Optional(payload) => payload,
            _ => return self.scalar_equal(lhs, rhs),
        };
        let value = |optional| self.optional_value(ty, optional, "part");
        let (fa, fb) = (
            self.has_value(ty, lhs, "part"),
            self.has_value(ty, rhs, "part"),
        );
        let flags = self.int_compare(IntPredicate::EQ, fa, fb);
        if let TypeKind::Enum(..) | TypeKind::Optional(_) = payload.kind() {
            // Equal flags decide, unless both are set: a `None`'s payload is never read.
            let function = self.current_function();
            let both = self.builder.build_and(fa, fb, "both").expect(POSITIONED);
            let (decided, compare, done) = (
                self.current_block(),
                self.context.append_basic_block(function, "some.both"),
                self.context.append_basic_block(function, "some.equal"),
            );
            self.builder
                .build_conditional_branch(both, compare, done)
                .expect(POSITIONED);
            self.builder.position_at_end(compare);
            let values = self.equal(payload, value(lhs), value(rhs));
            let compared = self.current_block();
            self.branch_to(done);
            self.builder.position_at_end(done);
            return self
                .phi(
                    self.context.bool_type(),
                    &[(flags, decided), (values, compared)],
                )
                .into_int_value();
        }
        // `fa == fb && (!fa || va == vb)` on flags `f` and values `v`, with no branch.
        let values = self.scalar_equal(value(lhs), value(rhs));
        let none = self.builder.build_not(fa, "none").expect(POSITIONED);
        let values = self
            .builder
            .build_or(none, values, "values")
            .expect(POSITIONED);
        self.builder
            .build_and(flags, values, "equal")
            .expect(POSITIONED)
    }

    /// Whether the values of type `ty` at `a` and `b` are equal, as `equal` says. An enum goes
    /// to its helper by address, with no copy; any other type is loaded.
    pub(super) fn equal_at(
        &self,
        ty: Type,
        a: PointerValue<'ctx>,
        b: PointerValue<'ctx>,
    ) -> IntValue<'ctx> {
        if let TypeKind::Enum(..) = ty.kind() {
            return self.call_eq(ty, a.into(), b.into());
        }
        self.equal(ty, self.load_value(ty, a), self.load_value(ty, b))
    }

    /// The value of type `ty` at `place`.
    fn load_value(&self, ty: Type, place: PointerValue<'ctx>) -> BasicValueEnum<'ctx> {
        let llvm_ty = self
            .llvm_type(ty)
            .expect("the checker should give every field a value type, never `unit` or `never`");
        self.builder
            .build_load(llvm_ty, place, "field")
            .expect(POSITIONED)
    }

    /// A call of the `eq` helper of `ty` on `a` and `b`.
    fn call_eq(
        &self,
        ty: Type,
        a: BasicValueEnum<'ctx>,
        b: BasicValueEnum<'ctx>,
    ) -> IntValue<'ctx> {
        self.builder
            .build_call(self.helper(Helper::Eq, ty), &[a.into(), b.into()], "eq")
            .expect(POSITIONED)
            .try_as_basic_value()
            .basic()
            .expect("the generated `eq` helper should return a `bool`")
            .into_int_value()
    }

    /// Whether two scalars are equal: an `f64` to IEEE 754, so `nan` equals nothing and `-0.0`
    /// equals `0.0`.
    fn scalar_equal(&self, lhs: BasicValueEnum<'ctx>, rhs: BasicValueEnum<'ctx>) -> IntValue<'ctx> {
        match (lhs, rhs) {
            (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) => {
                self.float_compare(FloatPredicate::OEQ, l, r)
            }
            _ => self.int_compare(IntPredicate::EQ, lhs.into_int_value(), rhs.into_int_value()),
        }
    }

    /// Subtracts 1 from the count of `object`, then branches to `at_zero` if it is 0, else to
    /// `otherwise`.
    pub(super) fn decrement(
        &self,
        object: PointerValue<'ctx>,
        at_zero: BasicBlock<'ctx>,
        otherwise: BasicBlock<'ctx>,
    ) {
        let minus_one = self
            .context
            .i64_type()
            .const_int((-1_i64).cast_unsigned(), false);
        self.check_count(object, RC_FAULT_RELEASE);
        let count = self.add_to_count(object, minus_one);
        let zero = self.int_compare(
            IntPredicate::EQ,
            count,
            self.context.i64_type().const_zero(),
        );
        self.builder
            .build_conditional_branch(zero, at_zero, otherwise)
            .expect(POSITIONED);
    }

    /// With the RC checks, calls `sisu_rc_fault(kind)`, which never returns, when the count of
    /// `object` is below 1: `RC_FAULT_RETAIN` for a retain or a fill's sharing,
    /// `RC_FAULT_RELEASE` for a release or `unique`. Continues in a fresh block.
    pub(super) fn check_count(&self, object: PointerValue<'ctx>, kind: u64) {
        if self.rc_fault.is_none() {
            return;
        }
        let count = self.load_count(object, "old");
        let dead = self.int_compare(
            IntPredicate::SLT,
            count,
            self.context.i64_type().const_int(1, false),
        );
        self.fault_if(dead, kind);
    }

    /// With the RC checks, calls `sisu_rc_fault(RC_FAULT_MOVED)` when `local` is a whole
    /// `string`, class or array local whose slot is `null`: a read of a local after it was
    /// moved. A scope release reads such a slot by design and does not come here. Enums and
    /// optionals are not checked: an enum is not a pointer, and `None` is `null`.
    pub(super) fn check_not_moved(&self, local: LocalId) {
        let ty = self.types[local.0];
        if self.rc_fault.is_none() || counted_core(self.program, ty) != Some(ty) {
            return;
        }
        let slot = self.load_local(local).into_pointer_value();
        let null = self.builder.build_is_null(slot, "moved").expect(POSITIONED);
        self.fault_if(null, RC_FAULT_MOVED);
    }

    /// Branches on `dead` to a block that calls `sisu_rc_fault(kind)`, which never returns, and
    /// continues in a fresh block. Does nothing without the RC checks.
    fn fault_if(&self, dead: IntValue<'ctx>, kind: u64) {
        let Some(fault) = self.rc_fault else {
            return;
        };
        let function = self.current_function();
        let fault_block = self.context.append_basic_block(function, "rc.fault");
        let ok = self.context.append_basic_block(function, "rc.ok");
        self.builder
            .build_conditional_branch(dead, fault_block, ok)
            .expect(POSITIONED);
        self.builder.position_at_end(fault_block);
        let kind = self.context.i32_type().const_int(kind, false);
        self.call(fault, &[kind.into()]);
        self.builder.build_unreachable().expect(POSITIONED);
        self.builder.position_at_end(ok);
    }

    /// Starts helper `function`, which returns at once when its object is `null`. Leaves the
    /// builder in the block for a real object; returns the object and the returning block.
    pub(super) fn helper_entry(
        &self,
        function: FunctionValue<'ctx>,
    ) -> (PointerValue<'ctx>, BasicBlock<'ctx>) {
        let entry = self.context.append_basic_block(function, "entry");
        let counted = self.context.append_basic_block(function, "object");
        let done = self.context.append_basic_block(function, "done");
        let object = param(function);
        self.builder.position_at_end(entry);
        let null = self
            .builder
            .build_is_null(object, "null")
            .expect(POSITIONED);
        self.builder
            .build_conditional_branch(null, done, counted)
            .expect(POSITIONED);
        self.builder.position_at_end(done);
        self.builder.build_return(None).expect(POSITIONED);
        self.builder.position_at_end(counted);
        (object, done)
    }

    /// Adds `delta` to the count of `object`, element 0; returns the new count.
    pub(super) fn add_to_count(
        &self,
        object: PointerValue<'ctx>,
        delta: IntValue<'ctx>,
    ) -> IntValue<'ctx> {
        let count = self.load_count(object, "count");
        let count = self
            .builder
            .build_int_add(count, delta, "count")
            .expect(POSITIONED);
        self.store_count(object, count);
        count
    }

    /// Makes the array in `slot`, of array type `ty`, the only holder of its block:
    /// `@sisu_rc.unique.[T]` copies the block into `slot` when another holder shares it.
    /// Returns the block `slot` then holds.
    pub(super) fn unique(&self, slot: PointerValue<'ctx>, ty: Type) -> PointerValue<'ctx> {
        self.builder
            .build_call(self.helper(Helper::Unique, ty), &[slot.into()], "unique")
            .expect(POSITIONED)
            .try_as_basic_value()
            .basic()
            .expect("the generated `unique` helper should return a pointer")
            .into_pointer_value()
    }

    /// Adds 1 to the count of each reference `value` holds when `ty` is counted.
    pub(super) fn retain(&self, value: BasicValueEnum<'ctx>, ty: Type) {
        self.retain_or_release(Helper::Retain, value, ty);
    }

    /// Releases each reference `value` holds when `ty` is counted.
    pub(super) fn release(&self, value: BasicValueEnum<'ctx>, ty: Type) {
        self.retain_or_release(Helper::Release, value, ty);
    }

    /// `retain` of the value of type `ty` at `place`; `ty` must be a counted type.
    pub(super) fn retain_at(&self, place: PointerValue<'ctx>, ty: Type) {
        self.retain_or_release_at(Helper::Retain, place, ty);
    }

    /// `release` of the value of type `ty` at `place`; `ty` must be a counted type.
    pub(super) fn release_at(&self, place: PointerValue<'ctx>, ty: Type) {
        self.retain_or_release_at(Helper::Release, place, ty);
    }

    /// `retain_or_release` of the value of type `ty` at `place`. An enum goes to its helper by
    /// address, with no copy; any other type is loaded. `ty` must be a counted
    /// type: unlike `retain_or_release`, this does not check.
    fn retain_or_release_at(&self, kind: Helper, place: PointerValue<'ctx>, ty: Type) {
        if let TypeKind::Enum(..) = ty.kind() {
            self.call(self.helper(kind, ty), &[place.into()]);
        } else {
            self.retain_or_release(kind, self.load_value(ty, place), ty);
        }
    }

    /// `retain` or `release`, as `kind` says. A string, object or array is the pointer itself,
    /// `null` for `None`; an enum goes to its helper by address; the payload of any other
    /// optional is counted only when the optional holds a value, one layer at a time.
    fn retain_or_release(&self, kind: Helper, value: BasicValueEnum<'ctx>, ty: Type) {
        if let Some(core) = counted_core(self.program, ty) {
            let function = match kind {
                Helper::Retain => self.rc_retain,
                _ => self.helper(Helper::Release, core),
            };
            self.call(function, &[value.into()]);
            return;
        }
        if !ty.is_counted(&self.program.enums) {
            return;
        }
        match ty.kind() {
            TypeKind::Enum(..) => self.retain_or_release_at(kind, self.enum_slot(value), ty),
            TypeKind::Optional(payload) => {
                let function = self.current_function();
                let some = self.context.append_basic_block(function, "some");
                let done = self.context.append_basic_block(function, "some.done");
                self.builder
                    .build_conditional_branch(self.has_value(ty, value, "flag"), some, done)
                    .expect(POSITIONED);
                self.builder.position_at_end(some);
                let value = self.optional_value(ty, value, "payload");
                self.retain_or_release(kind, value, payload);
                self.branch_to(done);
                self.builder.position_at_end(done);
            }
            _ => unreachable!(
                "a counted type that is not a pointer should be an enum or its optional"
            ),
        }
    }
}

/// The single parameter of a counting helper: the object or array, or `unique`'s slot.
pub(super) fn param(function: FunctionValue<'_>) -> PointerValue<'_> {
    function
        .get_nth_param(0)
        .expect("a counting helper should be declared with one pointer parameter")
        .into_pointer_value()
}

/// `ty` with any `?` and `[ ]` around it removed.
fn unwrapped(mut ty: Type) -> Type {
    while let TypeKind::Optional(inner) | TypeKind::Array(inner) = ty.kind() {
        ty = inner;
    }
    ty
}

/// Which classes, then which enums, of `program` reach an `f64` through their fields (see
/// `Codegen::reflexive`): the types with a field of `f64` seed it, and each class or enum with
/// a field of a type already in it joins, by a worklist over the reversed field graph. Every
/// field is read once.
pub(super) fn reaching_f64(program: &Program) -> Vec<bool> {
    let classes = program.classes.len();
    let node_count = classes + program.enums.len();
    let mut reaches = vec![false; node_count];
    let mut holders: Vec<Vec<usize>> = vec![Vec::new(); node_count];
    let class_fields = program
        .classes
        .iter()
        .enumerate()
        .flat_map(|(node, class)| class.fields.iter().map(move |field| (node, field.ty)));
    let enum_fields = program.enums.iter().enumerate().flat_map(|(id, decl)| {
        let fields = decl.variants.iter().flat_map(|variant| &variant.fields);
        fields.map(move |field| (classes + id, field.ty))
    });
    for (node, ty) in class_fields.chain(enum_fields) {
        #[cfg(test)]
        super::TYPE_READS.set(super::TYPE_READS.get() + 1);
        match unwrapped(ty).kind() {
            TypeKind::F64 => reaches[node] = true,
            TypeKind::Class(id, _) => holders[id.0].push(node),
            TypeKind::Enum(id, _) => holders[classes + id.0].push(node),
            _ => {}
        }
    }
    // Each seed is queued once, however many `f64` fields it has.
    let mut work: Vec<usize> = (0..node_count).filter(|&node| reaches[node]).collect();
    while let Some(done) = work.pop() {
        for &holder in &holders[done] {
            if !reaches[holder] {
                reaches[holder] = true;
                work.push(holder);
            }
        }
    }
    reaches
}
