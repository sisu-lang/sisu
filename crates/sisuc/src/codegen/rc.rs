//! Reference counting: retaining, releasing and comparing counted values, and the helpers
//! that do it. Each counted type gets one helper of each kind, declared on first use and
//! defined by `define_helpers` in one worklist pass. A string, an object or an array is a
//! pointer, which `@sisu_rc.retain` and its `release` helper take; an enum is a value, which its
//! `retain`, `release` and `eq` helpers take by address. An enum whose component has boxes also
//! has `walk`, `drain` and `eq_component` helpers (see `codegen/enums.rs`).

use inkwell::basic_block::BasicBlock;
use inkwell::module::Linkage;
use inkwell::values::{BasicValueEnum, FunctionValue, IntValue, PointerValue};
use inkwell::{AddressSpace, FloatPredicate, IntPredicate};
use rustc_hash::FxHashSet;

use super::abi::{RC_FAULT_RELEASE, RC_FAULT_RETAIN};
use super::{Codegen, POSITIONED};
use crate::tir::{Program, Type, TypeKind};

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
    /// `@sisu_rc.drain.<F0>(heads)`: frees the boxes on a component's pending lists.
    Drain,
    /// `@sisu_rc.eq_component.<F0>(kind, a, b)`: `==` on values of a component's member `kind`.
    EqComponent,
}

/// The type whose helpers a pointer-shaped `ty` uses: a `string`, a class or an array, with any
/// `?` removed. `None` for every other type, an enum and its optional included: those are not
/// pointers, and a `None` of theirs is not `null`.
pub(super) fn counted_core(ty: Type) -> Option<Type> {
    match ty.kind() {
        TypeKind::String | TypeKind::Class(_) | TypeKind::Array(_) => Some(ty),
        TypeKind::Optional(payload) => counted_core(payload),
        _ => None,
    }
}

impl<'ctx> Codegen<'ctx, '_> {
    /// Declares `sisu_rc.<retain|release|drop|eq|unique|walk|drain|eq_component>.<name>` on first
    /// use and queues its body; `name` is a class's or an enum's name, `string`, or
    /// `[<element name>]` for an array (`[Node?]`, `[[i64]]`).
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
    /// Both return at once on `null`. Drop does not release the chain field: it loads it,
    /// frees the object, subtracts 1 from the next object's count and at 0 drops it in a loop.
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
                (_, TypeKind::Enum(id)) => self.define_enum_helper(kind, id, function),
                (Helper::Release, _) => self.define_release(function, ty),
                (Helper::Retain, _) => {
                    unreachable!("only an enum type should be queued for a `retain` helper")
                }
                (_, TypeKind::Array(element)) => self.define_array_helper(kind, element, function),
                (_, TypeKind::String) => self.define_string_helper(kind, function),
                (Helper::Drop, _) => self.define_drop(function, ty),
                (Helper::Eq, _) => self.define_eq(function, ty),
                (Helper::Unique, _) => {
                    unreachable!("only an array type should be queued for a `unique` helper")
                }
                (Helper::Walk | Helper::Drain | Helper::EqComponent, _) => {
                    unreachable!("only an enum type should be queued for a component helper")
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

    /// `@sisu_rc.drop.<C>`: releases each counted field but the chain field, frees the
    /// object, then drops the chain's next object in a loop when its count reaches 0.
    fn define_drop(&self, drop: FunctionValue<'ctx>, ty: Type) {
        let TypeKind::Class(id) = ty.kind() else {
            unreachable!("only a class type should reach the class helper bodies")
        };
        let ptr_type = self.context.ptr_type(AddressSpace::default());
        let entry = self.context.append_basic_block(drop, "entry");
        self.builder.position_at_end(entry);
        let chain = self.program.chain_field(id);
        // With a chain field, the body is a loop over the chain's objects, in one frame.
        let looped = chain.map(|_| {
            let body = self.context.append_basic_block(drop, "loop");
            self.branch_to(body);
            self.builder.position_at_end(body);
            let object = self
                .builder
                .build_phi(ptr_type, "object")
                .expect(POSITIONED);
            object.add_incoming(&[(&param(drop), entry)]);
            (body, object)
        });
        let object = looped.map_or_else(
            || param(drop),
            |(_, phi)| phi.as_basic_value().into_pointer_value(),
        );
        let fields = &self.program.classes[id.0].fields;
        let load_field = |index: usize| {
            let llvm_ty = self.llvm_type(fields[index].ty).expect(
                "the checker should give every class field a value type, never `unit` or `never`",
            );
            self.builder
                .build_load(llvm_ty, self.field_ptr(id, object, index), "field")
                .expect(POSITIONED)
        };
        for (index, field) in fields.iter().enumerate() {
            if field.ty.is_counted(&self.program.enums) && chain != Some(index) {
                self.release(load_field(index), field.ty);
            }
        }
        // Loaded before the free, which ends the object.
        let next = chain.map(|index| load_field(index).into_pointer_value());
        let (size, align) = self.class_layout(id);
        let i64_type = self.context.i64_type();
        self.call(
            self.free,
            &[
                object.into(),
                i64_type.const_int(size, false).into(),
                i64_type.const_int(u64::from(align), false).into(),
            ],
        );
        if let (Some(next), Some((body, phi))) = (next, looped) {
            let done = self.context.append_basic_block(drop, "done");
            let counted = self.context.append_basic_block(drop, "next");
            let null = self.builder.build_is_null(next, "null").expect(POSITIONED);
            self.builder
                .build_conditional_branch(null, done, counted)
                .expect(POSITIONED);
            self.builder.position_at_end(counted);
            self.decrement(next, body, done);
            // The RC checks put the branch in a block after `counted`.
            phi.add_incoming(&[(&next, self.current_block())]);
            self.builder.position_at_end(done);
        }
        self.builder.build_return(None).expect(POSITIONED);
    }

    /// `@sisu_rc.eq.<C>(a, b)`: see `eq_entry` for the same pointer and `null`; otherwise
    /// whether each pair of fields is equal, comparing the chain field last and stopping at the
    /// first difference. It neither retains nor releases. Like `define_drop`, it does not
    /// recurse on the chain field: it compares the other fields of the pair, then steps both
    /// objects to the objects their chain fields hold and repeats the checks of `eq_entry`, in
    /// one frame.
    fn define_eq(&self, function: FunctionValue<'ctx>, ty: Type) {
        let TypeKind::Class(id) = ty.kind() else {
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
            let llvm_ty = self.llvm_type(field.ty).expect(
                "the checker should give every class field a value type, never `unit` or `never`",
            );
            let load = |object| {
                self.builder
                    .build_load(llvm_ty, self.field_ptr(id, object, index), "field")
                    .expect(POSITIONED)
            };
            let next = block("next");
            branch(self.equal(field.ty, load(a), load(b)), next, unequal);
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
    /// of an enum it reaches, boxed or inline. Fields close cycles through classes and boxes,
    /// so a class or enum met again counts as reflexive until a path from it finds an `f64`.
    pub(super) fn reflexive(&self, ty: Type) -> bool {
        fn reaches_no_f64(program: &Program, ty: Type, seen: &mut FxHashSet<Type>) -> bool {
            match ty.kind() {
                TypeKind::F64 => false,
                TypeKind::Optional(inner) | TypeKind::Array(inner) => {
                    reaches_no_f64(program, inner, seen)
                }
                // A class or enum already seen is being checked further up, or was found
                // reflexive.
                TypeKind::Class(_) | TypeKind::Enum(_) if !seen.insert(ty) => true,
                TypeKind::Class(id) => program.classes[id.0]
                    .fields
                    .iter()
                    .all(|field| reaches_no_f64(program, field.ty, seen)),
                TypeKind::Enum(id) => program.enums[id.0]
                    .variants
                    .iter()
                    .flat_map(|variant| &variant.fields)
                    .all(|field| reaches_no_f64(program, field.ty, seen)),
                _ => true,
            }
        }
        reaches_no_f64(self.program, ty, &mut FxHashSet::default())
    }

    /// Whether `lhs` and `rhs`, of type `ty`, are equal: `==` without releasing them.
    pub(super) fn equal(
        &self,
        ty: Type,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
    ) -> IntValue<'ctx> {
        if let Some(core) = counted_core(ty) {
            return self.call_eq(core, lhs, rhs);
        }
        let payload = match ty.kind() {
            TypeKind::Enum(_) => {
                let (a, b) = (self.enum_slot(lhs), self.enum_slot(rhs));
                return self.call_eq(ty, a.into(), b.into());
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
        if let TypeKind::Enum(_) = payload.kind() {
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
        let Some(fault) = self.rc_fault else {
            return;
        };
        let i64_type = self.context.i64_type();
        let count = self.load_count(object, "old");
        let dead = self.int_compare(IntPredicate::SLT, count, i64_type.const_int(1, false));
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
    fn helper_entry(
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

    /// `retain` or `release`, as `kind` says. A string, object or array is the pointer itself,
    /// `null` for `None`; an enum goes to its helper by address; an enum optional's payload is
    /// counted only when its flag is set.
    fn retain_or_release(&self, kind: Helper, value: BasicValueEnum<'ctx>, ty: Type) {
        if let Some(core) = counted_core(ty) {
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
            TypeKind::Enum(_) => self.call(self.helper(kind, ty), &[self.enum_slot(value).into()]),
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
