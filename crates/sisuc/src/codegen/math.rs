//! The methods on `f64` and `i64`: an LLVM intrinsic each, except `hypot`, which is the one
//! `libm` call.

use inkwell::intrinsics::Intrinsic;
use inkwell::values::{BasicValueEnum, FloatValue, FunctionValue, IntValue};
use inkwell::{FloatPredicate, IntPredicate};

use super::{Codegen, POSITIONED};
use crate::tir::{MathOp, Type};

impl<'ctx> Codegen<'ctx, '_> {
    /// `operands[0].op(operands[1..])`, the operands evaluated left to right. `min`, `max` and
    /// `clamp` on `f64` are `minimumnum` and `maximumnum`, never `minnum` or `maxnum`: those
    /// leave the sign of a zero result unspecified, and LLVM picks it differently at `-O0` and
    /// `-O2`. With `lo` above `hi`, `clamp` gives `hi`, as `min(max(x, lo), hi)` does. `ty` is
    /// the operands' type, `i64` or `f64`.
    pub(super) fn math(
        &self,
        op: MathOp,
        ty: Type,
        operands: &[BasicValueEnum<'ctx>],
    ) -> BasicValueEnum<'ctx> {
        if ty == Type::I64 {
            return self.integer_math(op, operands).into();
        }
        let x = operands[0].into_float_value();
        let f64_type = self.context.f64_type();
        let infinity = f64_type.const_float(f64::INFINITY);
        match op {
            MathOp::Hypot => self.call_float(self.hypot(), operands).into(),
            MathOp::Clamp => {
                let floor = self.intrinsic("maximumnum", &[x.into(), operands[1]]);
                self.intrinsic("minimumnum", &[floor.into(), operands[2]])
                    .into()
            }
            MathOp::IsNan => self.float_compare(FloatPredicate::UNO, x, x).into(),
            MathOp::IsFinite => {
                let magnitude = self.intrinsic("fabs", &[x.into()]);
                self.float_compare(FloatPredicate::OLT, magnitude, infinity) // spellchecker:disable-line
                    .into()
            }
            MathOp::IsInfinite => {
                let magnitude = self.intrinsic("fabs", &[x.into()]);
                self.float_compare(FloatPredicate::OEQ, magnitude, infinity)
                    .into()
            }
            _ => {
                let name = match op {
                    MathOp::Abs => "fabs",
                    MathOp::Ln => "log",
                    MathOp::Min => "minimumnum",
                    MathOp::Max => "maximumnum",
                    op => op.name(),
                };
                self.intrinsic(name, operands).into()
            }
        }
    }

    /// `min`, `max` and `clamp` on `i64`: `select`s over signed compares.
    fn integer_math(&self, op: MathOp, operands: &[BasicValueEnum<'ctx>]) -> IntValue<'ctx> {
        let pick = |predicate, a: IntValue<'ctx>, b: IntValue<'ctx>| {
            let better = self.int_compare(predicate, a, b);
            self.builder
                .build_select(better, a, b, "pick")
                .expect(POSITIONED)
                .into_int_value()
        };
        let min = |a, b| pick(IntPredicate::SLT, a, b);
        let max = |a, b| pick(IntPredicate::SGT, a, b);
        let operand = |i: usize| operands[i].into_int_value();
        match op {
            MathOp::Min => min(operand(0), operand(1)),
            MathOp::Max => max(operand(0), operand(1)),
            MathOp::Clamp => min(max(operand(0), operand(1)), operand(2)),
            op => unreachable!(
                "the checker should give an `i64` only `min`, `max` and `clamp`, not `{}`",
                op.name()
            ),
        }
    }

    /// The call of `llvm.<name>` on `f64`s.
    fn intrinsic(&self, name: &str, args: &[BasicValueEnum<'ctx>]) -> FloatValue<'ctx> {
        let function = Intrinsic::find(&format!("llvm.{name}"))
            .expect("LLVM should provide an intrinsic for each `f64` method by name")
            .get_declaration(&self.module, &[self.context.f64_type().into()])
            .expect("the math intrinsics should be overloaded on a float type");
        self.call_float(function, args)
    }

    fn call_float(
        &self,
        function: FunctionValue<'ctx>,
        args: &[BasicValueEnum<'ctx>],
    ) -> FloatValue<'ctx> {
        // The `f64` methods take at most two arguments, so the call needs no `Vec`.
        let mut buffer = [args[0].into(); 2];
        for (slot, &arg) in buffer.iter_mut().zip(args) {
            *slot = arg.into();
        }
        self.builder
            .build_call(function, &buffer[..args.len()], "math")
            .expect(POSITIONED)
            .try_as_basic_value()
            .basic()
            .expect("a math function should return an `f64`")
            .into_float_value()
    }

    /// `double hypot(double, double)` from `libm`, declared on first use.
    fn hypot(&self) -> FunctionValue<'ctx> {
        self.module.get_function("hypot").unwrap_or_else(|| {
            let f64_type = self.context.f64_type();
            let fn_type = f64_type.fn_type(&[f64_type.into(), f64_type.into()], false);
            self.module.add_function("hypot", fn_type, None)
        })
    }
}
