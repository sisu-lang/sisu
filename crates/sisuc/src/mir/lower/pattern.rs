//! `if let` and `match`, after `codegen/matching.rs`. The scrutinee is evaluated once into
//! `temp`. The arms' patterns test it in order, in place; a binding borrows its part of `temp`
//! while the patterns and the guard run, and is not on the scope stack. An `|` is a choice
//! point: a local `tried` holds the alternative being tried, a dispatch block switches on it to
//! that alternative's test, and a retry block switches on it to the retry target of the
//! alternative that matched, so a later failure resumes the innermost choice point first. The
//! first arm whose pattern and guard hold commits: each counted binding copies its value, `temp`
//! is released and leaves the scope stack, and the counted bindings join it. Each edge that
//! leaves a test or a guard before the commit ends the arm's bindings; the edges out of a
//! committed body do not. The last arm's failure is `unreachable`: the checker proves some arm
//! matches.
//!
//! A failure target becomes a block only when an edge reaches it, so every block stays
//! reachable: an arm whose pattern cannot fail and has no guard leaves the arms after it
//! unlowered.

use std::mem;

use super::{Lowered, Lowering, valued, whole};
use crate::mir::{BlockId, Const, Operand, Place, Projection, Rvalue, Statement, Terminator};
use crate::tir::{
    Arm, BinaryOp, Block, CompareOp, Expr, LocalId, Pattern, PatternKind, Type, TypeKind,
};

/// An index into `Lowering::targets`: a failure target that has no block until an edge needs it.
#[derive(Clone, Copy)]
pub(super) struct Target(usize);

/// A failure target, and its block once an edge has reached it.
pub(super) struct Lazy {
    block: Option<BlockId>,
    kind: Kind,
}

enum Kind {
    /// The failure of an arm: the next arm's test, after the `end` of each of the arm's
    /// bindings, which the arm adds once it is lowered.
    Fail,
    /// The failure of the alternative `index - 1`: `tried = index`, then the dispatch.
    Next {
        tried: LocalId,
        index: usize,
        dispatch: BlockId,
    },
    /// The retry target of a choice point: a switch on `tried` to the retry target of the
    /// alternative that matched, or to `fail` when `tried` is past the last.
    Retry {
        tried: LocalId,
        retries: Vec<Target>,
        fail: Target,
    },
}

impl Lowering<'_> {
    fn target(&mut self, kind: Kind) -> Target {
        self.targets.push(Lazy { block: None, kind });
        Target(self.targets.len() - 1)
    }

    /// The block of `target`, made with its statements and terminator on the first call.
    fn realize(&mut self, target: Target) -> BlockId {
        if let Some(block) = self.targets[target.0].block {
            return block;
        }
        let block = self.new_block();
        self.targets[target.0].block = Some(block);
        match &mut self.targets[target.0].kind {
            Kind::Fail => {}
            Kind::Next {
                tried,
                index,
                dispatch,
            } => {
                let (tried, index, dispatch) = (*tried, *index, *dispatch);
                self.blocks[block.0].stmts.push(Statement::Assign {
                    dest: Some(tried),
                    value: Rvalue::Use(Operand::Const(index_const(index))),
                });
                self.blocks[block.0].term = Terminator::Goto(dispatch);
            }
            Kind::Retry {
                tried,
                retries,
                fail,
            } => {
                let (tried, retries, fail) = (*tried, mem::take(retries), *fail);
                let arms = (0..)
                    .zip(retries)
                    .map(|(i, retry)| (index_const(i), self.realize(retry)))
                    .collect();
                let otherwise = Some(self.realize(fail));
                self.blocks[block.0].term = Terminator::SwitchValue {
                    operand: Operand::Read(whole(tried)),
                    arms,
                    otherwise,
                };
            }
        }
        block
    }

    /// `if let bind = scrutinee { then } else { else_ }`: the optional into a temporary, then
    /// `if_present`. The `Some` edge moves the payload out into `bind`.
    pub(super) fn if_some(
        &mut self,
        bind: LocalId,
        scrutinee: &Expr,
        then_block: &Block,
        else_block: Option<&Block>,
        ty: Type,
        dest: Option<LocalId>,
    ) -> Lowered {
        let optional = self.temp_of(scrutinee)?;
        self.branches(
            |some, none| Terminator::IfPresent {
                optional,
                some,
                none,
            },
            Some((bind, optional)),
            then_block,
            else_block,
            ty,
            dest,
        )
    }

    /// `match scrutinee { arms }`, with the scrutinee in `temp`.
    pub(super) fn match_expr(
        &mut self,
        scrutinee: &Expr,
        temp: LocalId,
        arms: &[Arm],
        ty: Type,
        dest: Option<LocalId>,
    ) -> Lowered {
        self.locals[temp.0].scope = self.here();
        self.into(scrutinee, Some(temp))?;
        let dest = valued(ty).then(|| dest.unwrap_or_else(|| self.temp(ty)));
        let counted = self.locals[temp.0].counted;
        for arm in arms {
            for &binding in &arm.bindings {
                self.owners.insert(binding, temp);
            }
        }
        let (outer, guards) = (self.scope.len(), self.guard.len());
        let mut join = None;
        for arm in arms {
            if counted {
                self.scope.push(temp);
            }
            let start = self.here();
            for &binding in &arm.bindings {
                self.locals[binding.0].scope = start;
            }
            self.guard.extend(&arm.bindings);
            let fail = self.target(Kind::Fail);
            let retry = self.test(&arm.pattern, &whole(temp), scrutinee.ty, fail);
            let passed = match &arm.guard {
                None => true,
                Some(guard) => self.guard_holds(guard, retry),
            };
            if passed {
                self.commit(temp, counted, guards, &arm.bindings);
                self.arm(outer, &arm.body, dest, &mut join);
            }
            self.scope.truncate(outer);
            self.guard.truncate(guards);
            let Some(fail) = self.targets[fail.0].block else {
                self.current = None;
                break;
            };
            self.blocks[fail.0]
                .stmts
                .extend(arm.bindings.iter().map(|&binding| Statement::End(binding)));
            self.current = Some(fail);
        }
        self.current = join;
        join.map(|_| dest)
    }

    /// Lowers `guard` and branches on it: on to the arm's commit when it holds, to `retry` when
    /// it does not. False when the guard cannot fall through, so the arm cannot commit.
    fn guard_holds(&mut self, guard: &Expr, retry: Target) -> bool {
        let Some(cond) = self.operand(guard) else {
            return false;
        };
        let then = self.new_block();
        let else_ = self.realize(retry);
        self.terminate(Terminator::Branch { cond, then, else_ });
        self.current = Some(then);
        true
    }

    /// The commit of an arm whose pattern and guard held: each counted binding copies its
    /// borrowed value, then `temp`, when `counted`, is released and leaves the scope stack, and
    /// the counted bindings join it. The bindings are no longer borrowed.
    fn commit(&mut self, temp: LocalId, counted: bool, guards: usize, bindings: &[LocalId]) {
        self.guard.truncate(guards);
        for &binding in bindings {
            if self.locals[binding.0].counted {
                self.push(Statement::Assign {
                    dest: Some(binding),
                    value: Rvalue::Copy(whole(binding)),
                });
            }
        }
        if counted {
            self.push(Statement::ScopeRelease(temp));
            let popped = self.scope.pop();
            debug_assert_eq!(popped, Some(temp), "`temp` should top the scope stack");
        }
        for &binding in bindings {
            if self.locals[binding.0].counted {
                self.scope.push(binding);
            }
        }
    }

    /// Tests `pattern` against the value of type `ty` at `place`: jumps to `fail` when it does
    /// not match, and leaves `current` where it does, with each binding borrowing its part.
    /// Returns the target that a test after this one fails to, its retry target: `fail` for a
    /// pattern with no `|`, the retry target of its last sub-test for a variant, and for an `|`
    /// a switch that resumes the alternative that matched. Each statement that reads `place`
    /// keeps its own copy of it, and the tests after it still need it.
    fn test(&mut self, pattern: &Pattern, place: &Place, ty: Type, fail: Target) -> Target {
        match &pattern.kind {
            PatternKind::Wild => fail,
            PatternKind::Bind(binding) => {
                self.push(Statement::Assign {
                    dest: Some(*binding),
                    value: Rvalue::Borrow(place.clone()),
                });
                fail
            }
            PatternKind::Variant { variant, fields } => {
                let TypeKind::Enum(id) = ty.kind() else {
                    unreachable!("the checker should allow a variant pattern only at an enum")
                };
                let next = self.new_block();
                let otherwise = Some(self.realize(fail));
                self.terminate(Terminator::SwitchTag {
                    place: place.clone(),
                    arms: vec![(*variant, next)],
                    otherwise,
                });
                self.current = Some(next);
                let program = self.program;
                let payloads = &program.enums[id.0].variants[*variant].fields;
                let mut retry = fail;
                for (field, (sub, payload)) in fields.iter().zip(payloads).enumerate() {
                    if matches!(sub.kind, PatternKind::Wild) {
                        continue;
                    }
                    // Each field's place extends the variant's path.
                    let mut at = place.clone();
                    at.projections.push(Projection::Payload {
                        variant: *variant,
                        field,
                    });
                    retry = self.test(sub, &at, payload.ty, retry);
                }
                retry
            }
            PatternKind::Bool(value) => {
                let (pass, other) = (self.new_block(), self.realize(fail));
                let (then, else_) = if *value { (pass, other) } else { (other, pass) };
                self.terminate(Terminator::Branch {
                    cond: Operand::Read(place.clone()),
                    then,
                    else_,
                });
                self.current = Some(pass);
                fail
            }
            PatternKind::Range { lo, hi } if lo == hi => {
                self.switch_value(place, bound(*lo, ty), fail);
                fail
            }
            PatternKind::Range { lo, hi } => {
                let mut compare = |op, n| {
                    let value = Rvalue::Compare {
                        op,
                        lhs: Operand::Read(place.clone()),
                        rhs: Operand::Const(bound(n, ty)),
                    };
                    let local = self.temp(Type::BOOL);
                    self.push(Statement::Assign {
                        dest: Some(local),
                        value,
                    });
                    Operand::Read(whole(local))
                };
                let (lhs, rhs) = (compare(CompareOp::Ge, *lo), compare(CompareOp::Le, *hi));
                let inside = self.temp(Type::BOOL);
                self.push(Statement::Assign {
                    dest: Some(inside),
                    value: Rvalue::Binary {
                        op: BinaryOp::And,
                        lhs,
                        rhs,
                        span: pattern.span,
                    },
                });
                let (then, else_) = (self.new_block(), self.realize(fail));
                self.terminate(Terminator::Branch {
                    cond: Operand::Read(whole(inside)),
                    then,
                    else_,
                });
                self.current = Some(then);
                fail
            }
            PatternKind::Str(text) => {
                self.switch_value(place, Const::Str(text.clone()), fail);
                fail
            }
            PatternKind::Or(alternatives) => self.alternatives(alternatives, place, ty, fail),
        }
    }

    /// `switch place [literal: next] otherwise fail`, leaving `current` at `next`.
    fn switch_value(&mut self, place: &Place, literal: Const, fail: Target) {
        let (next, otherwise) = (self.new_block(), self.realize(fail));
        self.terminate(Terminator::SwitchValue {
            operand: Operand::Read(place.clone()),
            arms: vec![(literal, next)],
            otherwise: Some(otherwise),
        });
        self.current = Some(next);
    }

    /// `a0 | … | am`: `tried = 0`, then the dispatch, a switch on `tried` to each
    /// alternative's test. An alternative's failure sets `tried` to the next index and goes
    /// back to the dispatch, which goes to `fail` past the last. Every alternative's success
    /// joins in one block, where `current` is left. Returns the retry target.
    fn alternatives(
        &mut self,
        alternatives: &[Pattern],
        place: &Place,
        ty: Type,
        fail: Target,
    ) -> Target {
        let tried = self.temp(Type::I64);
        self.locals[tried.0].name = Some("tried".to_owned());
        self.push(Statement::Assign {
            dest: Some(tried),
            value: Rvalue::Use(Operand::Const(index_const(0))),
        });
        let (dispatch, join) = (self.new_block(), self.new_block());
        self.goto(dispatch);
        let mut starts = Vec::with_capacity(alternatives.len());
        let mut retries = Vec::with_capacity(alternatives.len());
        for (i, alternative) in alternatives.iter().enumerate() {
            let start = self.new_block();
            let next = self.target(Kind::Next {
                tried,
                index: i + 1,
                dispatch,
            });
            self.current = Some(start);
            retries.push(self.test(alternative, place, ty, next));
            self.goto(join);
            starts.push((index_const(i), start));
        }
        let otherwise = Some(self.realize(fail));
        self.blocks[dispatch.0].term = Terminator::SwitchValue {
            operand: Operand::Read(whole(tried)),
            arms: starts,
            otherwise,
        };
        self.current = Some(join);
        self.target(Kind::Retry {
            tried,
            retries,
            fail,
        })
    }
}

/// The value of `tried` that names the alternative `index`.
fn index_const(index: usize) -> Const {
    let index = i64::try_from(index).expect("an alternative's index should fit in an `i64`");
    Const::Int(index, Type::I64)
}

/// The bound `n` of a range pattern on `ty`: a code point for `char`.
fn bound(n: i64, ty: Type) -> Const {
    if ty == Type::CHAR {
        let c = u32::try_from(n)
            .ok()
            .and_then(char::from_u32)
            .expect("the checker should give a `char` range code points");
        Const::Char(c)
    } else {
        Const::Int(n, ty)
    }
}
