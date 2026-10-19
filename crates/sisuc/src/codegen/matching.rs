//! The terminators that `match` lowers to besides a branch (`mir/lower/pattern.rs`): a switch
//! on an enum's tag, a switch on a scalar or a string, and `unreachable`, the failure of the
//! last arm. The checker proves that some arm matches, so that block traps: a checker bug must
//! not become undefined behaviour.

use inkwell::basic_block::BasicBlock;
use inkwell::intrinsics::Intrinsic;

use super::{Codegen, POSITIONED, enum_of};
use crate::mir::{BlockId, Const, Operand, Place};
use crate::tir::Type;

impl<'ctx> Codegen<'ctx, '_> {
    /// A switch on the tag of the enum at `place`: to each arm's block for its variant, and to
    /// `otherwise` for every other tag.
    pub(super) fn switch_tag(
        &self,
        place: &Place,
        arms: &[(usize, BlockId)],
        otherwise: Option<BlockId>,
    ) {
        let (address, ty, _) = self.address(place.local, &place.projections, true);
        let id = enum_of(ty);
        let tag = self.load_tag(id, address);
        let tag_type = self.enum_layout(id).tag;
        let cases: Vec<_> = arms
            .iter()
            .map(|&(variant, to)| {
                let variant =
                    u64::try_from(variant).expect("a variant index should fit in a `u64`");
                (tag_type.const_int(variant, false), self.block(to))
            })
            .collect();
        self.builder
            .build_switch(tag, self.otherwise(otherwise), &cases)
            .expect(POSITIONED);
    }

    /// A switch on the value of `operand`: to each arm's block for its literal, and to
    /// `otherwise` for every other value. A string is compared with each literal's global in
    /// place, in arm order, so the test retains and releases nothing.
    pub(super) fn switch_value(
        &self,
        operand: &Operand,
        arms: &[(Const, BlockId)],
        otherwise: Option<BlockId>,
    ) {
        let (value, ty) = self.operand(operand);
        let otherwise = self.otherwise(otherwise);
        if ty == Type::STRING {
            let function = self.current_function();
            for (literal, to) in arms {
                let equal = self.equal(Type::STRING, value, self.constant(literal).0);
                let next = self.context.append_basic_block(function, "literal.next");
                self.branch_on(equal, self.block(*to), next);
                self.builder.position_at_end(next);
            }
            self.branch_to(otherwise);
            return;
        }
        let cases: Vec<_> = arms
            .iter()
            .map(|(literal, to)| (self.constant(literal).0.into_int_value(), self.block(*to)))
            .collect();
        self.builder
            .build_switch(value.into_int_value(), otherwise, &cases)
            .expect(POSITIONED);
    }

    /// The block a switch goes to when no arm's value matches.
    fn otherwise(&self, otherwise: Option<BlockId>) -> BasicBlock<'ctx> {
        self.block(otherwise.expect("lowering should give every switch an `otherwise` block"))
    }

    /// `Unreachable`: `llvm.trap`, then LLVM's `unreachable`.
    pub(super) fn trap(&self) {
        let trap = Intrinsic::find("llvm.trap")
            .and_then(|trap| trap.get_declaration(&self.module, &[]))
            .expect("LLVM should provide `llvm.trap`, which takes no type parameters");
        self.call(trap, &[]);
        self.builder.build_unreachable().expect(POSITIONED);
    }
}
