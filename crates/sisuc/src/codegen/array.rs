//! Arrays at run time. An array is one heap block: a header of two `i64`s, the count and
//! the length, then the elements. `layout.rs` owns the layout; this module owns the array nodes
//! and the bodies of the array helpers, which `define_helpers` reaches through
//! `define_array_helper`.

use inkwell::values::{BasicValueEnum, FunctionValue, IntValue, PointerValue};
use inkwell::{AddressSpace, IntPredicate};

use super::abi::{RC_FAULT_RELEASE, RC_FAULT_RETAIN};
use super::layout::HeapBlock;
use super::rc::{Helper, counted_core, param};
use super::{Codegen, POSITIONED, Slot, class_of};
use crate::source::Span;
use crate::tir::{Expr, ExprKind, LocalId, Place, Type, TypeKind};

/// A field or element place with its own operands evaluated.
pub(super) struct PlaceOperands<'p, 'ctx> {
    /// The root's slot: a local's `alloca`, or the field's address in a field root's base.
    pub(super) slot: PointerValue<'ctx>,
    /// The type of the value the root's slot holds.
    pub(super) ty: Type,
    /// A field root's base and its type, which the write releases last.
    object: Option<(PointerValue<'ctx>, Type)>,
    /// Each index with its value, from the root outwards.
    pub(super) indexes: Vec<(&'p Expr, IntValue<'ctx>)>,
    /// Whether the root is a local that an enclosing loop made unique before it started, so
    /// that a write skips `unique` at the root.
    pub(super) unique_root: bool,
}

impl<'ctx> PlaceOperands<'_, 'ctx> {
    /// Releases a field root's base, which kept the object alive through the write.
    pub(super) fn release_object(self, codegen: &Codegen<'ctx, '_>) {
        if let Some((object, ty)) = self.object {
            codegen.release(object.into(), ty);
        }
    }
}

/// The element type of `ty`, an array type.
fn element_of(ty: Type) -> Type {
    let TypeKind::Array(element) = ty.kind() else {
        unreachable!("the checker should give an array node an array type")
    };
    element
}

/// Whether evaluating `expr` leaves every local as it was, so a local read before it still
/// holds the same block after it. A whitelist of node kinds that call nothing, bind nothing and
/// move nothing: a node kind added later is excluded until it is listed here.
pub(super) fn leaves_locals(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Int(_) | ExprKind::Local(_) => true,
        ExprKind::Unary { operand, .. }
        | ExprKind::Convert(operand)
        | ExprKind::Len(operand)
        | ExprKind::Field { base: operand, .. } => leaves_locals(operand),
        ExprKind::Binary { lhs, rhs, .. } => leaves_locals(lhs) && leaves_locals(rhs),
        ExprKind::Compare { operands, .. } => operands.iter().all(leaves_locals),
        _ => false,
    }
}

/// The local that a read of `base` loads without retaining it, as `borrow_or_expr` does: the
/// local keeps the block alive, so codegen skips its retain and release.
pub(super) fn borrowed_local(base: &Expr) -> Option<LocalId> {
    match base.kind {
        ExprKind::Local(local) => Some(local),
        _ => None,
    }
}

/// The local that `Codegen::index` loads without retaining it: a local `base`, while the
/// parameter `index` passes `leaves_locals`.
pub(super) fn borrowed_index_base(base: &Expr, index: &Expr) -> Option<LocalId> {
    borrowed_local(base).filter(|_| leaves_locals(index))
}

impl<'ctx> Codegen<'ctx, '_> {
    /// A block with count 1 and length `len`. A literal passes a constant length, and `unique`
    /// a run-time length that already passed the checks; `[v; n]` gets them at `span`. A length
    /// taken from an existing string, as `chars()` and `bytes()` do, needs no check.
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
                len,
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
        // Tagged like every array count and length (see `header_tag`).
        self.store_count(array, self.context.i64_type().const_int(1, false));
        self.store_tagged(self.len_ptr(array), len.into(), self.header_tag);
        array
    }

    pub(super) fn array_len(&self, array: PointerValue<'ctx>) -> IntValue<'ctx> {
        self.load_tagged(
            self.context.i64_type().into(),
            self.len_ptr(array),
            "len",
            self.header_tag,
        )
        .into_int_value()
    }

    #[expect(
        unsafe_code,
        reason = "inkwell marks GEP unsafe for a wrong index list; each GEP here takes one `i64` index"
    )]
    pub(super) fn element_ptr(
        &self,
        element: Type,
        array: PointerValue<'ctx>,
        index: IntValue<'ctx>,
    ) -> PointerValue<'ctx> {
        let layout = self.array_layout(element);
        let i64_type = self.context.i64_type();
        let offset = i64_type.const_int(layout.offset, false);
        // SAFETY: each GEP takes one `i64` index, which steps over whole `i8` or element
        // values and never indexes into an aggregate, so the index list is valid for its
        // pointee type. `inbounds` is sound because the block holds the header and then its
        // length's elements, `offset` is where the elements start, and every caller passes
        // an index below the length: `check_bounds` proved it, a loop counts up to it, or
        // `array_lit` stores at the constant indexes `0..n` of a block it just sized to `n`.
        // `unique` passes 0, whose address is at most one past the block's end, which
        // `inbounds` allows.
        unsafe {
            let elements = self
                .builder
                .build_in_bounds_gep(self.context.i8_type(), array, &[offset], "elements")
                .expect(POSITIONED);
            self.builder
                .build_in_bounds_gep(layout.element, elements, &[index], "element")
                .expect(POSITIONED)
        }
    }

    /// The element at `index`, without retaining it.
    fn load_element(
        &self,
        element: Type,
        array: PointerValue<'ctx>,
        index: IntValue<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let layout = self.array_layout(element);
        self.load_tagged(
            layout.element,
            self.element_ptr(element, array, index),
            "element",
            self.element_tag,
        )
    }

    /// Panics at `span` with `sisu_panic_bounds` when `index uge len`, which is
    /// `0 <= index < len` negated: one unsigned compare also catches a negative index.
    pub(super) fn check_bounds(
        &self,
        array: PointerValue<'ctx>,
        index: IntValue<'ctx>,
        span: Span,
    ) {
        let len = self.array_len(array);
        let outside = self.int_compare(IntPredicate::UGE, index, len);
        self.panic_with(
            outside,
            &self.location(span),
            self.panic_bounds,
            &[index.into(), len.into()],
        );
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

    /// `ArrayLit`: every element first, left to right, then the block, which takes each
    /// element's reference. Allocating last means an element that exits leaves no block.
    pub(super) fn array_lit(&mut self, ty: Type, elements: &[Expr]) -> PointerValue<'ctx> {
        let values: Vec<_> = elements
            .iter()
            .map(|e| {
                self.expr(e).expect(
                    "the checker should give every array element a value type, never `unit` or `never`",
                )
            })
            .collect();
        self.array_lit_value(ty, &values)
    }

    /// The block of an array literal of type `ty`, which takes each of the evaluated `values`.
    pub(super) fn array_lit_value(
        &self,
        ty: Type,
        values: &[BasicValueEnum<'ctx>],
    ) -> PointerValue<'ctx> {
        let element = element_of(ty);
        let i64_type = self.context.i64_type();
        let len = u64::try_from(values.len())
            .expect("an array literal should have fewer than 2^64 elements");
        let array = self.alloc_array(element, i64_type.const_int(len, false), None);
        for (index, value) in (0..).zip(values) {
            let slot = self.element_ptr(element, array, i64_type.const_int(index, false));
            self.store_tagged(slot, *value, self.element_tag);
        }
        array
    }

    /// `ArrayFill`: `value`, then `len`, then the block, with `value` in every slot.
    pub(super) fn array_fill(&mut self, ty: Type, value: &Expr, len: &Expr) -> PointerValue<'ctx> {
        let filler = self.expr(value).expect(
            "the checker should give the value of `[v; n]` a value type, never `unit` or `never`",
        );
        let count = self.value(len);
        self.array_fill_value(filler, count, ty, len.span)
    }

    /// The block of `[filler; count]` of type `ty`; a negative or oversized `count` panics at
    /// `span`. A counted pointer `filler` gains `count - 1` references in one step, or loses its
    /// one when `count` is 0. A counted enum `filler` is retained once per slot, then released.
    pub(super) fn array_fill_value(
        &self,
        filler: BasicValueEnum<'ctx>,
        count: IntValue<'ctx>,
        ty: Type,
        span: Span,
    ) -> PointerValue<'ctx> {
        let element = element_of(ty);
        let array = self.alloc_array(element, count, Some(span));
        self.count_loop(count, |index| {
            self.store_tagged(
                self.element_ptr(element, array, index),
                filler,
                self.element_tag,
            );
        });
        if counted_core(element).is_some() {
            self.share_filler(filler, element, count);
        } else if element.is_counted(&self.program.enums) {
            self.count_loop(count, |_| self.retain(filler, element));
            self.release(filler, element);
        }
        array
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
        let object = filler.into_pointer_value();
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

    /// `Index`: `base`, then `index`, the bounds check, then the element, retained. The base
    /// is released after the load, unless it is a local read whose index leaves every local
    /// as it was: the local then keeps the block alive, so codegen skips its retain and
    /// release.
    pub(super) fn index(&mut self, ty: Type, base: &Expr, index: &Expr) -> BasicValueEnum<'ctx> {
        let local = borrowed_index_base(base, index);
        let array = match local {
            Some(local) => self.load_local(local),
            None => self.expr(base),
        }
        .expect("the checker should only index an array, which lowers to a pointer")
        .into_pointer_value();
        let at = self.value(index);
        let value = self.index_value(array, at, ty, index.span);
        if local.is_none() {
            self.release(array.into(), base.ty);
        }
        value
    }

    /// The element of `array` at `index`, retained; an index outside the array panics at `span`.
    /// The caller releases the array if it owns a reference.
    pub(super) fn index_value(
        &self,
        array: PointerValue<'ctx>,
        index: IntValue<'ctx>,
        ty: Type,
        span: Span,
    ) -> BasicValueEnum<'ctx> {
        self.check_bounds(array, index, span);
        let value = self.load_element(ty, array, index);
        self.retain(value, ty);
        value
    }

    /// `Len`: the length of `base`, an array or a string, which is then released, unless it is
    /// a local: the local keeps the block alive, so codegen skips its retain and release.
    /// `s.words().len()` counts the words without building them (see `word_count`).
    pub(super) fn len(&mut self, base: &Expr) -> IntValue<'ctx> {
        if let ExprKind::Words(string) = &base.kind {
            return self.word_count(string);
        }
        let (array, borrowed) = self.borrow_or_expr(base);
        let array = array
            .expect("the checker should only call `len` on an array or a string, which lower to a pointer")
            .into_pointer_value();
        let len = self.len_value(array, base.ty);
        if !borrowed {
            self.release(array.into(), base.ty);
        }
        len
    }

    /// The length of `array`, of type `ty`, an array or a string.
    pub(super) fn len_value(&self, array: PointerValue<'ctx>, ty: Type) -> IntValue<'ctx> {
        if ty == Type::STRING {
            self.string_len(array)
        } else {
            self.array_len(array)
        }
    }

    /// `Sum`: the elements of `base`, an `[i64]` or an `[f64]`, then the release of the base.
    pub(super) fn sum(&mut self, ty: Type, base: &Expr, span: Span) -> BasicValueEnum<'ctx> {
        let array = self
            .expr(base)
            .expect("the checker should only call `sum` on an array, which lowers to a pointer")
            .into_pointer_value();
        let sum = self.sum_value(ty, array, span);
        self.release(array.into(), base.ty);
        sum
    }

    /// The elements of `array`, an `[i64]` or an `[f64]` as `ty` says, added in order from
    /// zero. An `i64` adds with the checked add of `+`, which panics at `span`, and an `f64`
    /// with `fadd` from `+0.0`, so an empty `[f64]` sums to `0.0` and the order of the additions
    /// is the order of the elements.
    pub(super) fn sum_value(
        &self,
        ty: Type,
        array: PointerValue<'ctx>,
        span: Span,
    ) -> BasicValueEnum<'ctx> {
        let llvm_type = self.array_layout(ty).element;
        let total = self.entry_alloca(llvm_type, "total");
        self.store(total, llvm_type.const_zero());
        self.count_loop(self.array_len(array), |index| {
            let element = self.load_element(ty, array, index);
            let so_far = self.load(llvm_type, total);
            let added = if ty == Type::F64 {
                self.builder
                    .build_float_add(so_far.into_float_value(), element.into_float_value(), "sum")
                    .expect(POSITIONED)
                    .into()
            } else {
                self.checked(
                    "llvm.sadd.with.overflow",
                    so_far.into_int_value(),
                    element.into_int_value(),
                    span,
                )
                .into()
            };
            self.store(total, added);
        });
        self.load(llvm_type, total)
    }

    /// `Contains`: `base`, then `value`; whether some element of `base` is `==` to `value`,
    /// stopping at the first that is. Then both are released.
    pub(super) fn contains(&mut self, base: &Expr, value: &Expr) -> IntValue<'ctx> {
        let array = self
            .expr(base)
            .expect(
                "the checker should only call `contains` on an array, which lowers to a pointer",
            )
            .into_pointer_value();
        let needle = self.operand(value);
        let hit = self.contains_value(element_of(base.ty), array, needle);
        self.release(array.into(), base.ty);
        self.release(needle, value.ty);
        hit
    }

    /// Whether some element of `array`, whose element type is `element`, is `==` to `needle`,
    /// stopping at the first that is.
    pub(super) fn contains_value(
        &self,
        element: Type,
        array: PointerValue<'ctx>,
        needle: BasicValueEnum<'ctx>,
    ) -> IntValue<'ctx> {
        let function = self.current_function();
        let found = self.context.append_basic_block(function, "contains.found");
        let end = self.context.append_basic_block(function, "contains.end");
        self.count_loop(self.array_len(array), |index| {
            let same = self.equal(element, self.load_element(element, array, index), needle);
            let next = self.context.append_basic_block(function, "contains.next");
            self.builder
                .build_conditional_branch(same, found, next)
                .expect(POSITIONED);
            self.builder.position_at_end(next);
        });
        let missed = self.current_block();
        self.branch_to(end);
        self.builder.position_at_end(found);
        self.branch_to(end);
        self.builder.position_at_end(end);
        let bool_type = self.context.bool_type();
        self.phi(
            bool_type,
            &[
                (bool_type.const_zero(), missed),
                (bool_type.const_int(1, false), found),
            ],
        )
        .into_int_value()
    }

    /// The operands of a field or element `place`, evaluated left to right: a field root's
    /// base, then each index from the root outwards. `None` when an index exits, which ends
    /// the statement: `spill` bound every counted operand before it to a local, so nothing is
    /// left to release.
    pub(super) fn place_operands<'p>(
        &mut self,
        place: &'p Place,
    ) -> Option<PlaceOperands<'p, 'ctx>> {
        let mut indexes = Vec::new();
        let mut root = place;
        while let Place::Index { base, index } = root {
            indexes.push(index);
            root = base;
        }
        indexes.reverse();
        let object = match root {
            Place::Field { base, .. } => Some((
                self.expr(base)
                    .expect("the checker should only make a field place on a class-typed base, which lowers to a pointer")
                    .into_pointer_value(),
                base.ty,
            )),
            _ => None,
        };
        let mut at = Vec::with_capacity(indexes.len());
        for index in indexes {
            at.push((index, self.expr(index)?.into_int_value()));
        }
        let (slot, ty) = match root {
            Place::Local(local) => (self.variable_slot(*local), self.types[local.0]),
            Place::Field { base, index } => {
                let class = class_of(base.ty);
                let (object, _) =
                    object.expect("a field root's base should have been evaluated above");
                (
                    self.field_ptr(class, object, *index),
                    self.program.classes[class.0].fields[*index].ty,
                )
            }
            Place::Index { .. } => unreachable!("the loop above should stop at the root"),
        };
        Some(PlaceOperands {
            slot,
            ty,
            object,
            indexes: at,
            unique_root: matches!(root, Place::Local(local) if self.unique_locals[local.0]),
        })
    }

    /// The `alloca` of the variable `local`.
    pub(super) fn variable_slot(&self, local: LocalId) -> PointerValue<'ctx> {
        let Some(Slot::Alloca(ptr, _)) = self.locals[local.0] else {
            unreachable!(
                "the checker should only let a variable be written, and every variable has an `alloca`"
            )
        };
        ptr
    }

    /// Walks the indexes of `operands` from its root slot (bounds check, then `unique`, per
    /// level) and returns the final slot and its type. A root that an enclosing loop made
    /// unique skips `unique` at the root.
    pub(super) fn element_slot(
        &mut self,
        operands: &PlaceOperands<'_, 'ctx>,
    ) -> (PointerValue<'ctx>, Type) {
        let unique_from = usize::from(operands.unique_root);
        self.walk(operands.slot, operands.ty, &operands.indexes, unique_from)
    }

    /// Walks `place`'s indexes as `element_slot` does, but makes nothing unique: the slot that
    /// a read of the place loads.
    pub(super) fn read_slot(
        &self,
        root_slot: PointerValue<'ctx>,
        root_ty: Type,
        indexes: &[(&Expr, IntValue<'ctx>)],
    ) -> PointerValue<'ctx> {
        self.walk(root_slot, root_ty, indexes, indexes.len()).0
    }

    /// The walk from `slot`, of type `ty`: at each level, load the array, check the index
    /// against its length, make the array unique if the level is at or past `unique_from`, and
    /// take the element's address as the next slot. Checking first means a level whose index
    /// is out of bounds copies nothing; an outer level may already have copied. In a hoisted
    /// write, the root level was made unique by the enclosing loop, so a level whose index is
    /// out of bounds may also follow that copy. A read passes
    /// `indexes.len()`, so it makes no level unique, even of a shared array.
    fn walk(
        &self,
        mut slot: PointerValue<'ctx>,
        mut ty: Type,
        indexes: &[(&Expr, IntValue<'ctx>)],
        unique_from: usize,
    ) -> (PointerValue<'ctx>, Type) {
        let ptr_type = self.context.ptr_type(AddressSpace::default());
        for (level, &(index, at)) in indexes.iter().enumerate() {
            let mut array = self
                .builder
                .build_load(ptr_type, slot, "array")
                .expect(POSITIONED)
                .into_pointer_value();
            self.check_bounds(array, at, index.span);
            if level >= unique_from {
                array = self.unique(slot, ty);
            }
            ty = element_of(ty);
            slot = self.element_ptr(ty, array, at);
        }
        (slot, ty)
    }

    /// `Assign { place: Index { .. }, value }`. The place's own operands come first, from
    /// `place_operands`. Then `value`, so that a value which stores the array elsewhere makes
    /// `element_slot`'s walk copy it. Last, the store, the release of the old element, and the
    /// release of a field root's base.
    pub(super) fn assign_index(&mut self, place: &Place, value: &Expr) {
        let Some(operands) = self.place_operands(place) else {
            return;
        };
        let Some(value) = self.expr(value) else {
            return;
        };
        let (slot, ty) = self.element_slot(&operands);
        let old = ty
            .is_counted(&self.program.enums)
            .then(|| self.load_tagged(value.get_type(), slot, "old", self.element_tag));
        self.store_tagged(slot, value, self.element_tag);
        self.discard(old, ty);
        operands.release_object(self);
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
            Helper::Retain
            | Helper::Release
            | Helper::Walk
            | Helper::Drain
            | Helper::EqComponent => {
                unreachable!(
                    "`define_helpers` should define every `release` itself, and queue `retain` and the component helpers only for an enum"
                )
            }
        }
    }

    /// `@sisu_rc.drop.[T]`: releases each counted element in order, then frees the block, its
    /// size recomputed from the length.
    fn define_array_drop(&self, element: Type, function: FunctionValue<'ctx>) {
        self.builder
            .position_at_end(self.context.append_basic_block(function, "entry"));
        let array = param(function);
        let len = self.array_len(array);
        if element.is_counted(&self.program.enums) {
            self.count_loop(len, |index| {
                self.release(self.load_element(element, array, index), element);
            });
        }
        let layout = self.array_layout(element);
        let (size, align) = self.heap_size(&HeapBlock::Array {
            layout: &layout,
            len,
        });
        self.call(self.free, &[array.into(), size.into(), align.into()]);
        self.builder.build_return(None).expect(POSITIONED);
    }

    /// `@sisu_rc.eq.[T](a, b)`: see `eq_entry` for the same pointer and `null`; otherwise the
    /// lengths, then each pair of elements by `T`'s `==`, up to the first difference.
    fn define_array_eq(&self, element: Type, function: FunctionValue<'ctx>) {
        let (a, b, equal, unequal) = self.eq_entry(function, self.reflexive(element));
        let len = self.array_len(a);
        let same_len = self.int_compare(IntPredicate::EQ, len, self.array_len(b));
        let elements = self.context.append_basic_block(function, "elements");
        self.builder
            .build_conditional_branch(same_len, elements, unequal)
            .expect(POSITIONED);
        self.builder.position_at_end(elements);
        self.count_loop(len, |index| {
            let pair_equal = self.equal(
                element,
                self.load_element(element, a, index),
                self.load_element(element, b, index),
            );
            let next = self.context.append_basic_block(function, "next");
            self.builder
                .build_conditional_branch(pair_equal, next, unequal)
                .expect(POSITIONED);
            self.builder.position_at_end(next);
        });
        self.branch_to(equal);
    }

    /// `@sisu_rc.unique.[T](slot) -> ptr`: the array in `slot` when its count is 1. Otherwise
    /// the old block is released, and a new block of the same length with count 1, the
    /// elements copied with `llvm.memcpy` and each counted one retained, is stored into
    /// `slot`. It has no `null` early return: the slot is an `alloca`, a field address or an
    /// element address.
    fn define_array_unique(&self, element: Type, function: FunctionValue<'ctx>) {
        let block = |name| self.context.append_basic_block(function, name);
        let (entry, only, shared) = (block("entry"), block("only"), block("shared"));
        self.builder.position_at_end(entry);
        let slot = param(function);
        let i64_type = self.context.i64_type();
        let array = self
            .builder
            .build_load(
                self.context.ptr_type(AddressSpace::default()),
                slot,
                "array",
            )
            .expect(POSITIONED)
            .into_pointer_value();
        // A dead block, count below 1, would otherwise pass for a shared one and be copied.
        self.check_count(array, RC_FAULT_RELEASE);
        let count = self.load_count(array, "count");
        let one = self.int_compare(IntPredicate::EQ, count, i64_type.const_int(1, false));
        self.builder
            .build_conditional_branch(one, only, shared)
            .expect(POSITIONED);
        self.builder.position_at_end(only);
        self.builder.build_return(Some(&array)).expect(POSITIONED);
        self.builder.position_at_end(shared);
        let len = self.array_len(array);
        // The slot's reference moves to the copy. Releasing it never frees the old block: its
        // count was above 1. Releasing it before the copy exists, not after, leaves the copy's
        // count and length as the last stores, so that LLVM can forward them past an inlined
        // `unique` instead of loading them again. With the TBAA tags, that drops the second
        // count and bounds checks of a swap; see `a_barred_partition_makes_its_array_unique_once`.
        self.add_to_count(array, i64_type.const_int((-1_i64).cast_unsigned(), false));
        // The length fit once, so the new block needs no size check.
        let copy = self.alloc_array(element, len, None);
        let layout = self.array_layout(element);
        let bytes = self
            .builder
            .build_int_nuw_mul(len, i64_type.const_int(layout.size, false), "bytes")
            .expect(POSITIONED);
        // The elements start at an offset that is a multiple of the block's alignment.
        let align = u32::try_from(layout.align)
            .expect("the block's alignment should fit in a `u32`, as each ABI alignment does");
        let zero = i64_type.const_zero();
        self.builder
            .build_memcpy(
                self.element_ptr(element, copy, zero),
                align,
                self.element_ptr(element, array, zero),
                align,
                bytes,
            )
            .expect("the block's alignment should be a power of two, as each ABI alignment is");
        if element.is_counted(&self.program.enums) {
            self.count_loop(len, |index| {
                self.retain(self.load_element(element, copy, index), element);
            });
        }
        self.store(slot, copy.into());
        self.builder.build_return(Some(&copy)).expect(POSITIONED);
    }
}
