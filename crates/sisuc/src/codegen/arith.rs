//! Checked integer operations: arithmetic that panics on overflow or a zero divisor, and
//! comparisons.

use inkwell::intrinsics::Intrinsic;
use inkwell::values::{BasicValueEnum, FloatValue, IntValue};
use inkwell::{FloatPredicate, IntPredicate};

use super::{Codegen, POSITIONED};
use crate::diagnostic::Panic;
use crate::source::Span;
use crate::tir::{BinaryOp, CompareOp, Expr, ExprKind, Type, TypeKind};

impl<'ctx> Codegen<'ctx, '_> {
    /// `span` is the operator expression's: arithmetic panics point at its start.
    pub(super) fn binary(
        &mut self,
        op: BinaryOp,
        lhs: &Expr,
        rhs: &Expr,
        span: Span,
    ) -> IntValue<'ctx> {
        if matches!(op, BinaryOp::And | BinaryOp::Or) {
            return self.short_circuit(op == BinaryOp::And, lhs, rhs);
        }
        let signed = lhs.ty != Type::U8;
        let (l, r) = (self.value(lhs), self.value(rhs));
        let intrinsic = |name| {
            format!(
                "llvm.{}{name}.with.overflow",
                if signed { 's' } else { 'u' }
            )
        };
        match op {
            BinaryOp::Add => self.checked(&intrinsic("add"), l, r, span),
            BinaryOp::AddUnchecked => self
                .builder
                .build_int_nsw_add(l, r, "step")
                .expect(POSITIONED),
            BinaryOp::Sub => self.checked(&intrinsic("sub"), l, r, span),
            BinaryOp::Mul => self.checked(&intrinsic("mul"), l, r, span),
            BinaryOp::Div | BinaryOp::Rem => self.division(op == BinaryOp::Rem, signed, l, r, span),
            BinaryOp::And | BinaryOp::Or => unreachable!(
                "`binary` should send `and` and `or` to `short_circuit` before this match"
            ),
        }
    }

    /// `lhs op rhs` on `f64`s, to IEEE 754: no overflow or zero divisor check, and no fast-math
    /// flag. `%` is `frem`, whose result has the sign of `lhs`, as C's `fmod`.
    pub(super) fn float_binary(
        &mut self,
        op: BinaryOp,
        lhs: &Expr,
        rhs: &Expr,
    ) -> FloatValue<'ctx> {
        let (l, r) = (self.float(lhs), self.float(rhs));
        let b = &self.builder;
        match op {
            BinaryOp::Add => b.build_float_add(l, r, "sum"),
            BinaryOp::Sub => b.build_float_sub(l, r, "difference"),
            BinaryOp::Mul => b.build_float_mul(l, r, "product"),
            BinaryOp::Div => b.build_float_div(l, r, "quotient"),
            BinaryOp::Rem => b.build_float_rem(l, r, "remainder"),
            BinaryOp::AddUnchecked => {
                unreachable!("the checker should step only `i64` counters, never `f64`")
            }
            BinaryOp::And | BinaryOp::Or => {
                unreachable!("the checker should type `and` and `or` `bool`, never `f64`")
            }
        }
        .expect(POSITIONED)
    }

    pub(super) fn float_compare(
        &self,
        predicate: FloatPredicate,
        lhs: FloatValue<'ctx>,
        rhs: FloatValue<'ctx>,
    ) -> IntValue<'ctx> {
        self.builder
            .build_float_compare(predicate, lhs, rhs, "cmp")
            .expect(POSITIONED)
    }

    pub(super) fn int_compare(
        &self,
        predicate: IntPredicate,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
    ) -> IntValue<'ctx> {
        self.builder
            .build_int_compare(predicate, lhs, rhs, "cmp")
            .expect(POSITIONED)
    }

    /// `lhs op rhs` through `intrinsic`, an `llvm.{s,u}*.with.overflow` overloaded on the
    /// operands' width; the overflow flag branches to an "integer overflow" panic.
    pub(super) fn checked(
        &self,
        intrinsic: &str,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        span: Span,
    ) -> IntValue<'ctx> {
        let (result, overflow) = self.overflowing(intrinsic, lhs, rhs);
        self.panic_if(overflow, span, Panic::Overflow.message());
        result
    }

    /// `lhs op rhs` through `intrinsic`, as for `checked`: the wrapped result and the
    /// overflow flag.
    pub(super) fn overflowing(
        &self,
        intrinsic: &str,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
    ) -> (IntValue<'ctx>, IntValue<'ctx>) {
        let function = Intrinsic::find(intrinsic)
            .expect("LLVM should provide the `llvm.*.with.overflow` intrinsics by name")
            .get_declaration(&self.module, &[lhs.get_type().into()])
            .expect("the overflow intrinsics should be overloaded on any integer width");
        let pair = self
            .builder
            .build_call(function, &[lhs.into(), rhs.into()], "checked")
            .expect(POSITIONED)
            .try_as_basic_value()
            .basic()
            .expect("an overflow intrinsic should return a `{ value, overflow }` pair")
            .into_struct_value();
        let field = |index, name| {
            self.builder
                .build_extract_value(pair, index, name)
                .expect("the overflow pair should have a value at index 0 and a flag at index 1")
                .into_int_value()
        };
        (field(0, "result"), field(1, "overflow"))
    }

    /// `lhs / rhs` or `lhs % rhs`: panics on a zero divisor, then, when `signed`, on
    /// `i64::MIN / -1`, whose quotient does not fit (`sdiv` and `srem` are undefined for both).
    /// An unsigned division has no other failure.
    fn division(
        &self,
        rem: bool,
        signed: bool,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        span: Span,
    ) -> IntValue<'ctx> {
        let i64_type = self.context.i64_type();
        let zero = self.int_compare(IntPredicate::EQ, rhs, rhs.get_type().const_zero());
        self.panic_if(zero, span, Panic::DivisionByZero.message());
        if !signed {
            return if rem {
                self.builder.build_int_unsigned_rem(lhs, rhs, "rem")
            } else {
                self.builder.build_int_unsigned_div(lhs, rhs, "div")
            }
            .expect(POSITIONED);
        }
        let min = i64_type.const_int(i64::MIN.cast_unsigned(), false);
        let minus_one = i64_type.const_int((-1_i64).cast_unsigned(), false);
        let overflow = self
            .builder
            .build_and(
                self.int_compare(IntPredicate::EQ, lhs, min),
                self.int_compare(IntPredicate::EQ, rhs, minus_one),
                "overflow",
            )
            .expect(POSITIONED);
        self.panic_if(overflow, span, Panic::Overflow.message());
        if rem {
            self.builder.build_int_signed_rem(lhs, rhs, "rem")
        } else {
            self.builder.build_int_signed_div(lhs, rhs, "div")
        }
        .expect(POSITIONED)
    }

    /// `lhs op rhs` on two integers, `signed` or not, or on two `f64`s, ordered: false beside a
    /// NaN.
    pub(super) fn compare_pair(
        &self,
        op: CompareOp,
        signed: bool,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
    ) -> IntValue<'ctx> {
        if let (BasicValueEnum::FloatValue(lhs), BasicValueEnum::FloatValue(rhs)) = (lhs, rhs) {
            let predicate = match op {
                CompareOp::Lt => FloatPredicate::OLT, // spellchecker:disable-line
                CompareOp::Le => FloatPredicate::OLE,
                CompareOp::Gt => FloatPredicate::OGT,
                CompareOp::Ge => FloatPredicate::OGE,
            };
            return self.float_compare(predicate, lhs, rhs);
        }
        let predicate = match (op, signed) {
            (CompareOp::Lt, true) => IntPredicate::SLT,
            (CompareOp::Le, true) => IntPredicate::SLE,
            (CompareOp::Gt, true) => IntPredicate::SGT,
            (CompareOp::Ge, true) => IntPredicate::SGE,
            (CompareOp::Lt, false) => IntPredicate::ULT,
            (CompareOp::Le, false) => IntPredicate::ULE,
            (CompareOp::Gt, false) => IntPredicate::UGT,
            (CompareOp::Ge, false) => IntPredicate::UGE,
        };
        self.int_compare(predicate, lhs.into_int_value(), rhs.into_int_value())
    }

    /// `i64(u8)`, `i64(char)`, `i64(f64)`, `u8(i64)`, `char(i64)` and `f64(i64)`, the type
    /// `e.ty` saying which. A `u8?` or `char?` is a `{ i1, T }` pair of whether the value fits
    /// and the value truncated, with no branch: `u8` fits `0..=255`, and `char` a scalar value.
    /// One unsigned compare also catches the negatives. An `i64?` comes from an `f64`.
    pub(super) fn convert(&mut self, e: &Expr) -> BasicValueEnum<'ctx> {
        let ExprKind::Convert(operand) = &e.kind else {
            unreachable!("`convert` should be called on a `Convert` node");
        };
        let kind = e.ty.kind();
        if kind == TypeKind::Optional(Type::I64) {
            let x = self.float(operand);
            let (fits, n) = self.float_to_int(x);
            return self.optional(e.ty, fits, n.into());
        }
        let value = self.value(operand);
        let (fits, payload) = match kind {
            TypeKind::F64 => {
                return self
                    .builder
                    .build_signed_int_to_float(value, self.context.f64_type(), "float")
                    .expect(POSITIONED)
                    .into();
            }
            TypeKind::I64 => {
                return self
                    .builder
                    .build_int_z_extend(value, self.context.i64_type(), "widened")
                    .expect(POSITIONED)
                    .into();
            }
            TypeKind::Optional(payload) => match payload {
                Type::CHAR => (self.scalar(value), self.context.i32_type()),
                Type::U8 => {
                    let max = value.get_type().const_int(u64::from(u8::MAX), false);
                    (
                        self.int_compare(IntPredicate::ULE, value, max),
                        self.context.i8_type(),
                    )
                }
                ty => unreachable!("the checker should give no conversion to `{ty}?`"),
            },
            _ => unreachable!("the checker should give no conversion to `{}`", e.ty),
        };
        let narrowed = self
            .builder
            .build_int_truncate(value, payload, "narrowed")
            .expect(POSITIONED);
        self.optional(e.ty, fits, narrowed.into())
    }

    /// Whether `x` is a whole number in `i64`'s range, and that number, with no branch.
    /// `llvm.fptosi.sat` gives `0` for NaN and saturates out of range, where `fptosi` would be
    /// poison; the range compares fail for NaN, and converting back finds a fractional part.
    fn float_to_int(&self, x: FloatValue<'ctx>) -> (IntValue<'ctx>, IntValue<'ctx>) {
        let i64_type = self.context.i64_type();
        let f64_type = self.context.f64_type();
        // -2^63 and 2^63, exact as `f64`s.
        let bound = 9_223_372_036_854_775_808.0;
        let above = self.float_compare(FloatPredicate::OGE, x, f64_type.const_float(-bound));
        let below = self.float_compare(FloatPredicate::OLT, x, f64_type.const_float(bound)); // spellchecker:disable-line
        let saturating = Intrinsic::find("llvm.fptosi.sat")
            .expect("LLVM should provide `llvm.fptosi.sat` by name")
            .get_declaration(&self.module, &[i64_type.into(), f64_type.into()])
            .expect("`llvm.fptosi.sat` should be overloaded on its result and operand types");
        let n = self
            .builder
            .build_call(saturating, &[x.into()], "truncated")
            .expect(POSITIONED)
            .try_as_basic_value()
            .basic()
            .expect("`llvm.fptosi.sat` should return an integer")
            .into_int_value();
        let back = self
            .builder
            .build_signed_int_to_float(n, f64_type, "back")
            .expect(POSITIONED);
        let whole = self.float_compare(FloatPredicate::OEQ, back, x);
        let b = &self.builder;
        let in_range = b.build_and(above, below, "in_range").expect(POSITIONED);
        let fits = b.build_and(in_range, whole, "fits").expect(POSITIONED);
        (fits, n)
    }

    /// Whether the `i64` `value` is a Unicode scalar value: at most `0x10FFFF`, and not a
    /// surrogate.
    fn scalar(&self, value: IntValue<'ctx>) -> IntValue<'ctx> {
        let i64_type = value.get_type();
        let in_range = self.int_compare(
            IntPredicate::ULE,
            value,
            i64_type.const_int(u64::from(char::MAX), false),
        );
        // `value - 0xD800` is below `0x800` exactly for the surrogates.
        let shifted = self
            .builder
            .build_int_sub(value, i64_type.const_int(0xD800, false), "shifted")
            .expect(POSITIONED);
        let not_surrogate =
            self.int_compare(IntPredicate::UGE, shifted, i64_type.const_int(0x800, false));
        self.builder
            .build_and(in_range, not_surrogate, "scalar")
            .expect(POSITIONED)
    }
}
