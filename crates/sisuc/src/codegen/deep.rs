//! Deep drops (ADR 0038): which drop functions count the drop depth, and the drop weight of
//! each. A drop function that counts adds its weight to the global `@sisu_rc.depth` while it
//! runs. At `DEPTH_LIMIT` it defers its block to the runtime's queue instead. The drop that
//! brings the depth back to 0 empties the queue when `@sisu_rc.deferred` is set. The drop
//! functions that can count are those of a class and an array, and the drop glue of an `any` box.
//!
//! The walks here follow the nesting of the source types: a chain of enums that each hold the
//! next, or nested arrays. They call themselves once for each level, so each recursive step
//! goes through `ensure_stack`, as the other walks over the depth of a type do.
use rustc_hash::{FxHashMap, FxHashSet};
#[cfg(test)]
use std::cell::Cell;

use inkwell::basic_block::BasicBlock;
use inkwell::module::Linkage;
use inkwell::types::BasicTypeEnum;
use inkwell::values::{BasicMetadataValueEnum, FunctionValue, GlobalValue};
use inkwell::{AddressSpace, GlobalVisibility, IntPredicate};

use super::rc::param;
use super::{Codegen, POSITIONED};
use crate::conformance::ensure_stack;
use crate::tir::{ClassId, Inline, Type, TypeKind};

/// The drop depth at which a counting drop function stops recursing and defers its block. The
/// depth counts frames. The value keeps the stack of a drop at the limit, plus the largest
/// drop weight, under a quarter of the 256 KiB stack of the small-stack tests (ADR 0038).
pub(super) const DEPTH_LIMIT: u64 = 256;

#[cfg(test)]
thread_local! {
    /// How many parts the walks of this module have read on this thread. A part is a field of a
    /// class that `cycle_fields` gives. It is also a field that `inline_frames` takes, and
    /// an element that `recursing` takes from an array.
    pub(super) static PART_READS: Cell<usize> = const { Cell::new(0) };
}

/// Counts one part read for `PART_READS`; nothing in a build without tests.
fn count_read() {
    #[cfg(test)]
    PART_READS.set(PART_READS.get() + 1);
}

/// The drop decisions that `Codegen` has made so far. Each is made once for each type, so the
/// walk over the fields of all types is linear in the program.
#[derive(Default)]
pub(super) struct DropMemo {
    /// Whether the drop function of a class or an array counts. A cycle of classes is one entry,
    /// under its first class.
    counts: FxHashMap<Type, bool>,
    /// The drop weight of the drop function of a class or an array, under the same keys.
    weights: FxHashMap<Type, u32>,
    /// The frames that the release of an enum or a struct opens, for each component under its
    /// first member.
    inline_frames: FxHashMap<Inline, u32>,
    /// The keys of the drop weights that are being worked out. A walk that reaches its own key
    /// would never end, so it is an error.
    weighing: FxHashSet<Type>,
    /// The same for the components in `inline_frames`.
    framing: FxHashSet<Inline>,
}

impl Codegen<'_, '_> {
    /// Whether a counted field, element or payload of type `ty` is a recursing part: its release
    /// can call a drop function without a bound that the source fixes. `holder` is the class
    /// whose field this is, and `None` for an array element. Counting is the default: a part is
    /// bounded only when its type, with each `?` removed, is on this list:
    /// - not counted, or a `string`: its drop calls nothing;
    /// - a class in the cycle of `holder`: `define_drop` puts it on a list and frees it in a
    ///   loop, in one frame;
    /// - an enum with a clear reach flag: the source fixes how deep the enums and strings in it
    ///   nest (see `tir::Enum::reaches`);
    /// - an array whose element, with each `?` removed, is bounded as an array element.
    ///   Examples: `[string]`, `[[i64]]`, `[[Kind]]`. The type fixes how many arrays nest, and
    ///   each level is a frame of the drop weight. A class element is never bounded, because no
    ///   array is in a class cycle: `[A]` and `[[A]]` recurse.
    ///
    /// Any other type is a recursing part, so a new kind of counted reference needs no rule.
    pub(super) fn recursing(&self, holder: Option<ClassId>, ty: Type) -> bool {
        let enums = &self.program.enums;
        let ty = ty.without_optionals();
        if !ty.is_counted(enums, &self.program.structs) {
            return false;
        }
        match ty.kind() {
            TypeKind::String => false,
            TypeKind::Class(..) => {
                holder.is_none_or(|class| self.analyses.class_cycles.link(class, ty).is_none())
            }
            // The elements are dropped in the frame of the array, so the array is as deep as one
            // element. That is `counts` of the array, which keeps its answer: each level of
            // nested arrays is decided once.
            TypeKind::Array(_) => {
                count_read();
                ensure_stack(|| self.counts(ty))
            }
            TypeKind::Enum(id, _) => enums[id.0].reaches,
            TypeKind::Struct(id, _) => self.program.structs[id.0].reaches,
            _ => true,
        }
    }

    /// Whether the drop function of the class or array `ty` counts. An array counts when its
    /// element is a recursing part. A class counts when a field of it, or of a class in its
    /// cycle, is one: the drop of a cycle frees every object of it in one drain, so a chain can
    /// leave the cycle at a field of any member and come back through a class that has no such
    /// field.
    pub(super) fn counts(&self, ty: Type) -> bool {
        let key = self.drop_key(ty);
        if let Some(&counts) = self.deep.borrow().counts.get(&key) {
            return counts;
        }
        let counts = match key.kind() {
            TypeKind::Class(id, _) => self
                .cycle_fields(id)
                .any(|(class, field)| self.recursing(Some(class), field)),
            TypeKind::Array(element) => self.recursing(None, element),
            _ => unreachable!("only a class or an array should have a drop function"),
        };
        self.deep.borrow_mut().counts.insert(key, counts);
        counts
    }

    /// The drop weight of the drop function of the class or array `ty`: 1 for the function,
    /// plus the largest number of frames that a part can open before the next counting drop
    /// function starts. The depth counts frames, not objects, so the depth limit bounds the
    /// stack. The weight is the same for each class of a cycle, because one drain frees them.
    /// A link of the cycle opens no frame: `define_drop` lists it. A drain adds one frame.
    pub(super) fn drop_weight(&self, ty: Type) -> u32 {
        let key = self.drop_key(ty);
        if let Some(&weight) = self.deep.borrow().weights.get(&key) {
            return weight;
        }
        assert!(
            self.deep.borrow_mut().weighing.insert(key),
            "drop_weight should not reach the drop function it works out"
        );
        let parts = match key.kind() {
            TypeKind::Class(id, _) => {
                let drain = self
                    .analyses
                    .class_cycles
                    .members(id)
                    .map_or(0, |m| u32::from(m.len() > 1));
                let fields = self.cycle_fields(id);
                let linked = |class, field| self.analyses.class_cycles.link(class, field).is_some();
                let fields = fields.filter(|&(class, field)| !linked(class, field));
                drain
                    + fields
                        .map(|(_, field)| self.part_frames(field))
                        .max()
                        .unwrap_or(0)
            }
            TypeKind::Array(element) => self.part_frames(element),
            _ => unreachable!("only a class or an array should have a drop function"),
        };
        let weight = 1 + parts;
        let mut memo = self.deep.borrow_mut();
        memo.weighing.remove(&key);
        memo.weights.insert(key, weight);
        weight
    }

    /// The drop weight of the drop glue of an `any` box that holds a `value`, or `None` when the
    /// glue does not count. It counts when `value` is a recursing part, as an array element is.
    /// The memos have no key for a box: the box of `[T]` frees one block more than the array
    /// `[T]`, and `recursing` and `part_frames` keep the answers that this reads.
    pub(super) fn box_drop_weight(&self, value: Type) -> Option<u32> {
        self.recursing(None, value)
            .then(|| 1 + self.part_frames(value))
    }

    /// The type that the memos use for the drop function of `ty`, a class or an array: a class
    /// of a cycle is the first class of the cycle.
    fn drop_key(&self, ty: Type) -> Type {
        let TypeKind::Class(id, args) = ty.kind() else {
            return ty;
        };
        let first = self
            .analyses
            .class_cycles
            .members(id)
            .map_or(id, |members| members[0]);
        Type::intern(TypeKind::Class(first, args))
    }

    /// Each field type of the class `id`, or of each class of its cycle, with the class that
    /// holds it.
    fn cycle_fields(&self, id: ClassId) -> impl Iterator<Item = (ClassId, Type)> + '_ {
        let cycle = self.analyses.class_cycles.members(id);
        let classes = cycle.into_iter().flatten().copied();
        let classes = classes.chain(cycle.is_none().then_some(id));
        classes.flat_map(|class| {
            let fields = &self.program.classes[class.0].fields;
            fields.iter().map(move |field| {
                count_read();
                (class, field.ty)
            })
        })
    }

    /// The number of frames that the release of a value of type `ty` opens before the next
    /// counting drop function starts, or the whole drop when that function does not count.
    /// A `?` adds none: the caller tests the flag in its own frame. A frame we are not sure of
    /// counts as one. Too many frames only make a drop defer sooner.
    fn part_frames(&self, ty: Type) -> u32 {
        let ty = ty.without_optionals();
        if !ty.is_counted(&self.program.enums, &self.program.structs) {
            return 0;
        }
        // Each walk of a chain of types passes here, so this is the place for the guard.
        ensure_stack(|| match ty.kind() {
            // `release`, then `drop`: the drop of a string has no part.
            TypeKind::String => 2,
            // `release` calls the drop function, which counts the next frames if it counts.
            // A drop function that does not count has bounded parts (see `recursing`), so its
            // weight is a constant that the source fixes.
            TypeKind::Class(..) | TypeKind::Array(_) => {
                1 + if self.counts(ty) {
                    0
                } else {
                    self.drop_weight(ty)
                }
            }
            TypeKind::Enum(id, _) => self.inline_frames(Inline::Enum(id)),
            TypeKind::Struct(id, _) => self.inline_frames(Inline::Struct(id)),
            // `release` calls a drop function through the table. It counts the next frames if
            // it counts. If it does not, what it holds is bounded and ends the chain: those
            // frames come once, at the end, and the margin of `DEPTH_LIMIT` holds them.
            _ => 1,
        })
    }

    /// The frames that `@sisu_rc.release.<E>`, of an enum or a struct `E`, opens before the next
    /// counting drop function starts. It opens `release`. When the component of `E` has boxes,
    /// `release` calls `drain`, which calls `walk`. `walk` calls itself in place for an inline
    /// link, one frame for each level of inline links. The number of members bounds those levels. A
    /// part in the component adds none: `walk` lists a box and `drain` frees it in the same frame.
    /// Every other field of a member adds its own frames. The release of a part in an earlier
    /// component is an enum or a struct release again. The holds-inline graph has no cycle between
    /// components, so the count ends.
    fn inline_frames(&self, id: Inline) -> u32 {
        let members = self.component(id);
        let key = members.as_ref().map_or(id, |members| members[0]);
        if let Some(&frames) = self.deep.borrow().inline_frames.get(&key) {
            return frames;
        }
        assert!(
            self.deep.borrow_mut().framing.insert(key),
            "inline_frames should not reach the component it works out"
        );
        let boxes = members.as_ref().map_or(0, |members| {
            let levels = u32::try_from(members.len())
                .expect("a component should have fewer than 2^32 members");
            1 + levels
        });
        let group = members
            .as_deref()
            .unwrap_or_else(|| std::slice::from_ref(&id));
        // A field that `walk` follows in place adds no frames of its own (see `inline_link`).
        let parts = (group.iter())
            .flat_map(|&member| {
                self.variants_of(member)
                    .flat_map(|(_, fields)| fields)
                    .map(move |field| (member, field))
            })
            .filter(|&(member, field)| {
                !field.boxed && (members.is_none() || self.inline_link(member, field).is_none())
            })
            .map(|(_, field)| {
                count_read();
                self.part_frames(field.ty)
            })
            .max()
            .unwrap_or(0);
        let frames = 1 + boxes + parts;
        let mut memo = self.deep.borrow_mut();
        memo.framing.remove(&key);
        memo.inline_frames.insert(key, frames);
        frames
    }
}

impl<'ctx> Codegen<'ctx, '_> {
    /// Starts the body of the drop function `drop` of `ty`, a class or an array, when it counts:
    /// the builder is in its entry block. If the depth is at or above `DEPTH_LIMIT`, the function
    /// defers its block (`sisu_drop_defer`), sets the flag `@sisu_rc.deferred` and returns.
    /// Otherwise it adds `drop_weight(ty)` to `@sisu_rc.depth`, and the builder is in the block
    /// for the body.
    /// The result is the block where the body ends (see `end_drop`). For a drop function that
    /// does not count, it builds nothing and gives `None`. A program with no counting drop
    /// function names neither global nor runtime function.
    pub(super) fn begin_drop(
        &self,
        drop: FunctionValue<'ctx>,
        ty: Type,
    ) -> Option<BasicBlock<'ctx>> {
        self.counts(ty)
            .then(|| self.begin_counting_drop(drop, self.drop_weight(ty)))
    }

    /// `begin_drop` for `drop`, the drop glue of an `any` box that holds a `value`.
    pub(super) fn begin_box_drop(
        &self,
        drop: FunctionValue<'ctx>,
        value: Type,
    ) -> Option<BasicBlock<'ctx>> {
        self.box_drop_weight(value)
            .map(|weight| self.begin_counting_drop(drop, weight))
    }

    /// The code of `begin_drop` for a drop function that counts and has the drop weight `weight`.
    fn begin_counting_drop(&self, drop: FunctionValue<'ctx>, weight: u32) -> BasicBlock<'ctx> {
        let (i64_type, bool_type) = (self.context.i64_type(), self.context.bool_type());
        let depth = self
            .drop_global("sisu_rc.depth", i64_type.into())
            .as_pointer_value();
        let deferred = self
            .drop_global("sisu_rc.deferred", bool_type.into())
            .as_pointer_value();
        let weight = i64_type.const_int(u64::from(weight), false);
        let block = |name| self.context.append_basic_block(drop, name);
        let (defer, run) = (block("defer"), block("run"));
        let current = self
            .builder
            .build_load(i64_type, depth, "depth")
            .expect(POSITIONED);
        let full = self.int_compare(
            IntPredicate::UGE,
            current.into_int_value(),
            i64_type.const_int(DEPTH_LIMIT, false),
        );
        self.branch_on(full, defer, run);
        self.builder.position_at_end(defer);
        let this = drop.as_global_value().as_pointer_value();
        self.call(
            self.runtime("sisu_drop_defer"),
            &[param(drop).into(), this.into()],
        );
        self.store(deferred, bool_type.const_all_ones().into());
        self.builder.build_return(None).expect(POSITIONED);
        // The end of the function. The body gives the depth back as it was after the add, and a
        // defer returns before the add, so the depth at entry is the depth to store. Then test
        // for depth 0. The queue empties after the store, at depth 0.
        let (exit, flagged, drain, done) = (
            block("exit"),
            block("flagged"),
            block("drain"),
            block("done"),
        );
        self.builder.position_at_end(exit);
        self.store(depth, current);
        let zero = self.int_compare(
            IntPredicate::EQ,
            current.into_int_value(),
            i64_type.const_zero(),
        );
        self.branch_on(zero, flagged, done);
        self.builder.position_at_end(flagged);
        let set = self
            .builder
            .build_load(bool_type, deferred, "deferred")
            .expect(POSITIONED);
        self.branch_on(set.into_int_value(), drain, done);
        self.builder.position_at_end(drain);
        self.call(self.runtime("sisu_drop_drain"), &[]);
        self.store(deferred, bool_type.const_zero().into());
        self.branch_to(done);
        self.builder.position_at_end(done);
        self.builder.build_return(None).expect(POSITIONED);
        self.builder.position_at_end(run);
        let raised = self
            .builder
            .build_int_add(current.into_int_value(), weight, "depth.new")
            .expect(POSITIONED);
        self.store(depth, raised.into());
        exit
    }

    /// Ends the body of a drop function that `begin_drop` started: a return when `exit` is
    /// `None`, and otherwise a branch to the block that takes the weight off.
    pub(super) fn end_drop(&self, exit: Option<BasicBlock<'ctx>>) {
        match exit {
            Some(exit) => self.branch_to(exit),
            None => {
                self.builder.build_return(None).expect(POSITIONED);
            }
        }
    }

    /// Whether a show or `==` helper of an array of `element` must grow the stack (ADR 0047). The
    /// show glue of a box that holds an `element` uses the same rule. It is so when a value of
    /// `element` can lead back to the helper through an array or a box. Those are the recursing
    /// parts of a drop. A class is one of them, because a written `eq` can recurse through an
    /// array of its class.
    pub(super) fn grows_stack(&self, element: Type) -> bool {
        self.recursing(None, element)
    }

    /// Gives `function`, which has no body yet, a body that runs a new function on a stack with
    /// room, through `sisu_stack_grow`. The result is that new function. It takes the same
    /// parameters and gives the same result, so its caller defines it as it would `function`.
    /// `sisu_stack_grow` takes one pointer, so a stack struct holds the parameters and the result,
    /// and a thunk reads them.
    pub(super) fn on_stack_with_room(&self, function: FunctionValue<'ctx>) -> FunctionValue<'ctx> {
        let name = function.get_name().to_string_lossy().into_owned();
        let add = |suffix: &str, ty| {
            let name = format!("{name}.{suffix}");
            self.module.add_function(&name, ty, Some(Linkage::Internal))
        };
        let body = add("body", function.get_type());
        let ptr = self.context.ptr_type(AddressSpace::default());
        let thunk = add(
            "thunk",
            self.context.void_type().fn_type(&[ptr.into()], false),
        );
        let result = function.get_type().get_return_type();
        let fields: Vec<BasicTypeEnum<'ctx>> = function
            .get_param_iter()
            .map(|parameter| parameter.get_type())
            .chain(result)
            .collect();
        let env_type = self.context.struct_type(&fields, false);
        let field = |env, index: usize| {
            let index = u32::try_from(index).expect("a helper should take few parameters");
            self.builder
                .build_struct_gep(env_type, env, index, "env.field")
                .expect("the stack struct should have a field for each part")
        };
        let parameters = function.count_params() as usize;

        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);
        let env = self
            .builder
            .build_alloca(env_type, "env")
            .expect(POSITIONED);
        for (index, parameter) in function.get_param_iter().enumerate() {
            self.store(field(env, index), parameter);
        }
        let thunk_pointer = thunk.as_global_value().as_pointer_value();
        self.call(
            self.runtime("sisu_stack_grow"),
            &[thunk_pointer.into(), env.into()],
        );
        match result {
            Some(result) => {
                let value = self
                    .builder
                    .build_load(result, field(env, parameters), "result")
                    .expect(POSITIONED);
                self.builder.build_return(Some(&value)).expect(POSITIONED);
            }
            None => {
                self.builder.build_return(None).expect(POSITIONED);
            }
        }

        let entry = self.context.append_basic_block(thunk, "entry");
        self.builder.position_at_end(entry);
        let env = param(thunk);
        let arguments: Vec<BasicMetadataValueEnum<'ctx>> = (0..parameters)
            .map(|index| {
                self.builder
                    .build_load(fields[index], field(env, index), "argument")
                    .expect(POSITIONED)
                    .into()
            })
            .collect();
        let call = self
            .builder
            .build_call(body, &arguments, "")
            .expect(POSITIONED);
        if let Some(value) = call.try_as_basic_value().basic() {
            self.store(field(env, parameters), value);
        }
        self.builder.build_return(None).expect(POSITIONED);
        body
    }

    /// Makes `sisu_rc.depth` and `sisu_rc.deferred` for a split build, before any function: unit 0
    /// defines them, zero at the start, and each other unit declares them. All units share them
    /// by name, so there is one drop depth in the program (ADR 0038).
    pub(super) fn declare_drop_globals(&self) {
        let (i64_type, bool_type) = (self.context.i64_type(), self.context.bool_type());
        let globals: [(&str, BasicTypeEnum<'ctx>); 2] = [
            ("sisu_rc.depth", i64_type.into()),
            ("sisu_rc.deferred", bool_type.into()),
        ];
        for (name, ty) in globals {
            let global = self.module.add_global(ty, None, name);
            global.set_linkage(Linkage::External);
            global.set_visibility(GlobalVisibility::Hidden);
            if self.unit.index == 0 {
                global.set_initializer(&ty.const_zero());
            }
        }
    }

    /// The global `name` of type `ty`: the one that `declare_drop_globals` made in a split build.
    /// With one unit it is internal, zero at the start. It is made at the first call
    /// for it, so a program with no counting drop function does not have it.
    fn drop_global(&self, name: &str, ty: BasicTypeEnum<'ctx>) -> GlobalValue<'ctx> {
        self.module.get_global(name).unwrap_or_else(|| {
            let global = self.module.add_global(ty, None, name);
            global.set_linkage(Linkage::Internal);
            global.set_initializer(&ty.const_zero());
            global
        })
    }
}
