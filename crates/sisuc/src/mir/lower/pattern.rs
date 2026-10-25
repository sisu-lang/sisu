//! `if let` and `match`. The scrutinee is evaluated once into `temp`. The arms' patterns test it in
//! order, in place; a binding borrows its part of `temp` while the patterns and the guard run, and
//! is not on the scope stack. An `|` is a choice point: a local `tried` holds the alternative being
//! tried, a dispatch block switches on it to that alternative's test, and a retry block switches on
//! it to the retry target of the alternative that matched, so a later failure resumes the innermost
//! choice point first. `Some(p)` and `None` test an optional with `test_present`, which reads it in
//! place, and `p` tests the value inside through `Projection::Some`. The first arm whose pattern
//! and guard hold commits. When `temp` is a counted enum and the arm binds a counted name or tests
//! a box, outside any `|` that would, the commit takes `temp` apart: each binding moves its part
//! out (one of a boxed `F?`, whose box may be `null`, copies it), each box the pattern tests
//! becomes a reuse token, which the scope stack frees, and the parts nothing moved are released.
//! Otherwise each counted binding copies its value and `temp` is released. Either way `temp` leaves
//! the scope stack, and the counted bindings, then the tokens, join it. Each edge that leaves a
//! test or a guard before the commit ends the arm's bindings; the edges out of a committed body do
//! not. The last arm's failure is `unreachable`: the checker proves some arm matches.
//!
//! A scrutinee that is a borrowed place, a borrowed parameter or a field without `mut` of one,
//! is not copied. A `match` tests the place itself; its commit copies and releases nothing, and
//! the counted bindings join the scope stack borrowed, so each edge that leaves the arm ends
//! them. An `if let` borrows the optional into a temporary, ended after the join and on each
//! edge that leaves the branches, and its binding borrows the payload. The bindings stay
//! borrowed through the arm.
//!
//! A failure target becomes a block only when an edge reaches it, so every block stays
//! reachable: an arm whose pattern cannot fail and has no guard leaves the arms after it
//! unlowered.

use std::mem;

#[cfg(test)]
use super::{DEDUPED, SEEDED};
use super::{Lowered, Lowering, valued, whole};
use crate::mir::{BlockId, Const, Operand, Place, Projection, Rvalue, Statement, Terminator};
use crate::tir::{
    Arm, BinaryOp, Block, CompareOp, EnumId, Expr, ExprKind, LocalId, Pattern, PatternKind, Type,
    TypeKind,
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

    /// The place `e` names when it is a borrowed place of a counted value: a borrowed parameter,
    /// a binding that a `match` or an `if let` on a borrowed place borrows, inside its arm, or a
    /// field without `mut` of a borrowed place. Marks the parameter at its root consulted.
    fn borrowed_place(&mut self, e: &Expr) -> Option<Place> {
        if !self.counted(e.ty) {
            return None;
        }
        match &e.kind {
            ExprKind::Local(local) if self.borrowed.get(local.0) == Some(&true) => {
                self.consulted[local.0] = true;
                Some(whole(*local))
            }
            // A guarded binding outside the guard stack is one that its arm's body reads, and it
            // is guarded there only when the arm borrows. The outer `match` marked the root.
            ExprKind::Local(local) if self.guarded[local.0] && !self.guard.contains(local) => {
                Some(whole(*local))
            }
            ExprKind::Field { base, index } => {
                let projection = self.field(base.ty, *index);
                if !projection.stable() {
                    return None;
                }
                let mut place = self.borrowed_place(base)?;
                place.projections.push(projection);
                Some(place)
            }
            _ => None,
        }
    }

    /// `if let bind = scrutinee { then } else { else_ }`: the optional into a temporary, then
    /// `if_present`. The `Some` edge moves the payload out into `bind`. A borrowed place is
    /// borrowed into the temporary instead, which is ended where the branches join and on each
    /// edge that leaves them, and `bind` borrows the payload.
    pub(super) fn if_some(
        &mut self,
        bind: LocalId,
        scrutinee: &Expr,
        then_block: &Block,
        else_block: Option<&Block>,
        ty: Type,
        dest: Option<LocalId>,
    ) -> Lowered {
        let Some(place) = self.borrowed_place(scrutinee) else {
            let optional = self.temp_of(scrutinee)?;
            return self.branches(
                |some, none| Terminator::IfPresent {
                    optional,
                    some,
                    none,
                },
                Some((bind, optional, false)),
                then_block,
                else_block,
                ty,
                dest,
            );
        };
        let optional = self.temp(scrutinee.ty);
        self.owners.insert(optional, place.local);
        self.push(Statement::Assign {
            dest: Some(optional),
            value: Rvalue::Borrow(place),
        });
        self.scope.push((optional, true));
        let value = self.branches(
            |some, none| Terminator::IfPresent {
                optional,
                some,
                none,
            },
            Some((bind, optional, true)),
            then_block,
            else_block,
            ty,
            dest,
        );
        let popped = self.scope.pop();
        debug_assert_eq!(
            popped,
            Some((optional, true)),
            "the optional should top the scope stack"
        );
        if value.is_some() {
            self.push(Statement::End(optional));
        }
        value
    }

    /// `match scrutinee { arms }`, with the scrutinee in `temp`, or tested in place when it is a
    /// borrowed place. A `match` on a borrowed parameter of an enum type marks it in `reuses` when
    /// the scrutinee holds the parameter's last mention, no loop encloses the `match`, and an arm
    /// would take a box of an enum `F` apart in the owned form and its body builds a
    /// `Variant` with a boxed `F` field: the arm notes `F`'s count in `built` before its body, and
    /// the count differs after.
    pub(super) fn match_expr(
        &mut self,
        scrutinee: &Expr,
        temp: LocalId,
        arms: &[Arm],
        ty: Type,
        dest: Option<LocalId>,
    ) -> Lowered {
        let borrowed = self.borrowed_place(scrutinee);
        let borrows = borrowed.is_some();
        let seed = (borrowed.as_ref())
            .filter(|place| {
                place.projections.is_empty()
                    && matches!(scrutinee.ty.kind(), TypeKind::Enum(..))
                    && self.borrowed.get(place.local.0) == Some(&true)
            })
            .map(|place| place.local)
            // A later read, or a loop, makes the scrutinee a copy that shares the box.
            .filter(|&local| self.frames.is_empty() && self.mentions.last_in(local, scrutinee));
        // `counted`: whether the scrutinee is in a temporary that owns a reference.
        let (place, counted) = if let Some(place) = borrowed {
            (place, false)
        } else {
            self.locals[temp.0].scope = self.here();
            self.into(scrutinee, Some(temp))?;
            (whole(temp), self.locals[temp.0].counted)
        };
        let dest = valued(ty).then(|| dest.unwrap_or_else(|| self.temp(ty)));
        for arm in arms {
            for &binding in &arm.bindings {
                self.owners.insert(binding, place.local);
            }
        }
        let (outer, guards) = (self.scope.len(), self.guard.len());
        let mut join = None;
        for arm in arms {
            if counted {
                self.scope.push((temp, false));
            }
            let start = self.here();
            for &binding in &arm.bindings {
                self.locals[binding.0].scope = start;
            }
            self.open_guards(&arm.bindings);
            let fail = self.target(Kind::Fail);
            let retry = self.test(&arm.pattern, &place, scrutinee.ty, fail);
            let passed = match &arm.guard {
                None => true,
                Some(guard) => self.guard_holds(guard, retry),
            };
            if passed {
                let parts = if counted && matches!(scrutinee.ty.kind(), TypeKind::Enum(..)) {
                    self.parts(&arm.pattern, &place, scrutinee.ty, &arm.bindings)
                } else {
                    None
                };
                self.commit(temp, counted, borrows, guards, &arm.bindings, parts);
                let noted = match seed {
                    Some(_) => self.noted(&arm.pattern, &place, scrutinee.ty, &arm.bindings),
                    None => Vec::new(),
                };
                self.arm(outer, &arm.body, dest, &mut join);
                if let Some(param) = seed
                    && noted.iter().any(|&(f, count)| {
                        #[cfg(test)]
                        SEEDED.set(SEEDED.get() + 1);
                        self.built_of(f) != count
                    })
                {
                    self.reuses[param.0] = true;
                }
                if borrows {
                    for &binding in &arm.bindings {
                        self.guarded[binding.0] = false;
                    }
                }
            }
            self.scope.truncate(outer);
            self.close_guards(guards);
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

    /// The commit of an arm whose pattern and guard held. With `parts`, it takes `temp` apart:
    /// each boxed `F?` binding copies its value, each other counted binding's borrow ends, and
    /// `take_apart` moves those bindings out, makes each tested box a token and releases each
    /// rest; `temp` leaves the scope stack with no release, and the counted bindings, then the
    /// tokens, join it. Otherwise each counted binding copies its borrowed value, then `temp`,
    /// when `counted`, is released and leaves the scope stack, and the counted bindings join it.
    /// The bindings are no longer borrowed. When the match `borrows` its scrutinee, the bindings
    /// copy nothing: they stay borrowed through the body, and join the scope stack as borrowed,
    /// so each edge that leaves the arm ends them.
    fn commit(
        &mut self,
        temp: LocalId,
        counted: bool,
        borrows: bool,
        guards: usize,
        bindings: &[LocalId],
        parts: Option<Parts>,
    ) {
        self.close_guards(guards);
        if borrows {
            for &binding in bindings {
                self.guarded[binding.0] = true;
                if self.locals[binding.0].counted {
                    self.scope.push((binding, true));
                }
            }
            return;
        }
        if let Some(parts) = parts {
            self.take_apart(temp, bindings, parts);
            return;
        }
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
            debug_assert_eq!(
                popped,
                Some((temp, false)),
                "`temp` should top the scope stack"
            );
        }
        for &binding in bindings {
            if self.locals[binding.0].counted {
                self.scope.push((binding, false));
            }
        }
    }

    /// Each enum a box of which the owned form's commit of `pattern` takes apart, with its count
    /// in `built`. It builds the whole `Parts` to read its tokens, as the owned form's commit does.
    fn noted(
        &mut self,
        pattern: &Pattern,
        place: &Place,
        ty: Type,
        bindings: &[LocalId],
    ) -> Vec<(EnumId, usize)> {
        self.stamp += 1;
        let mut noted: Vec<(EnumId, usize)> = Vec::new();
        let tokens = self
            .parts(pattern, place, ty, bindings)
            .map(|parts| parts.tokens);
        for (token, _) in tokens.into_iter().flatten() {
            let TypeKind::Enum(f, _) = token.kind() else {
                unreachable!("a tested box should hold an enum")
            };
            #[cfg(test)]
            DEDUPED.set(DEDUPED.get() + 1);
            let (count, stamp) = self.built.entry(f).or_default();
            if *stamp != self.stamp {
                *stamp = self.stamp;
                #[cfg(test)]
                SEEDED.set(SEEDED.get() + 1);
                noted.push((f, *count));
            }
        }
        noted
    }

    /// The take-apart commit of `parts` on `temp`, whose arm binds `bindings`.
    fn take_apart(&mut self, temp: LocalId, bindings: &[LocalId], parts: Parts) {
        for &binding in &parts.copied {
            self.push(Statement::Assign {
                dest: Some(binding),
                value: Rvalue::Copy(whole(binding)),
            });
        }
        // The certifier applies `temp`'s kill before the statement's own effects, so a binding
        // still borrowing from it there would break rule 3.
        for &(binding, _) in &parts.moved {
            self.push(Statement::End(binding));
        }
        let tokens: Vec<(LocalId, Place)> = (parts.tokens.into_iter())
            .map(|(ty, place)| (self.token(ty), place))
            .collect();
        let rest = (parts.rest.into_iter().zip(parts.kept))
            .filter_map(|(place, kept)| kept.then_some(place))
            .collect();
        let new_tokens: Vec<LocalId> = tokens.iter().map(|&(token, _)| token).collect();
        self.push(Statement::TakeApart {
            scrutinee: temp,
            tokens,
            bindings: parts.moved,
            rest,
        });
        let popped = self.scope.pop();
        debug_assert_eq!(
            popped,
            Some((temp, false)),
            "`temp` should top the scope stack"
        );
        for &binding in bindings {
            if self.locals[binding.0].counted {
                self.scope.push((binding, false));
            }
        }
        self.scope
            .extend(new_tokens.into_iter().map(|token| (token, false)));
    }

    /// What an arm's commit takes apart of a scrutinee at `place`, of the enum type `ty`, or
    /// `None` when the arm keeps the copying commit: when an `|` has an alternative that binds a
    /// counted name or tests a box, whose places differ per alternative, or when there is
    /// nothing to take apart, none of `bindings` counted and no tested box.
    fn parts(
        &self,
        pattern: &Pattern,
        place: &Place,
        ty: Type,
        bindings: &[LocalId],
    ) -> Option<Parts> {
        let mut parts = Parts {
            tokens: Vec::new(),
            moved: Vec::new(),
            copied: Vec::new(),
            rest: vec![place.clone()],
            kept: vec![false],
        };
        self.walk_parts(pattern, place, ty, false, 0, &mut parts)?;
        let binds = bindings
            .iter()
            .any(|binding| self.locals[binding.0].counted);
        (binds || !parts.tokens.is_empty()).then_some(parts)
    }

    /// Walks `pattern` as `test` does, at `place`, a part of value `value` of `parts.rest`, of
    /// type `ty`, which is a boxed payload field when `boxed`. `None` for an `|` that the
    /// take-apart cannot follow.
    fn walk_parts(
        &self,
        pattern: &Pattern,
        place: &Place,
        ty: Type,
        boxed: bool,
        value: usize,
        parts: &mut Parts,
    ) -> Option<()> {
        match &pattern.kind {
            PatternKind::Bind(binding) if self.locals[binding.0].counted => {
                if boxed && matches!(ty.kind(), TypeKind::Optional(_)) {
                    // A boxed `F?` is never tested: its box may be `null`.
                    parts.copied.push(*binding);
                    parts.kept[value] = true;
                } else {
                    if boxed {
                        parts.token(ty, place);
                    }
                    parts.moved.push((*binding, place.clone()));
                }
            }
            PatternKind::Variant { variant, fields } => {
                let TypeKind::Enum(id, _) = ty.kind() else {
                    unreachable!("the checker should allow a variant pattern only at an enum")
                };
                let value = if boxed { parts.token(ty, place) } else { value };
                let payloads = &self.program.enums[id.0].variants[*variant].fields;
                for (field, (sub, payload)) in fields.iter().zip(payloads).enumerate() {
                    let mut at = place.clone();
                    at.projections.push(Projection::Payload {
                        variant: *variant,
                        field,
                    });
                    self.walk_parts(sub, &at, payload.ty, payload.boxed, value, parts)?;
                }
            }
            PatternKind::Or(alternatives) => {
                if (alternatives.iter()).any(|alternative| self.binds_or_tests(alternative, ty)) {
                    return None;
                }
                parts.kept[value] |= self.counted(ty);
            }
            // The box of a boxed `F?` stays in its value, never a token: each counted binding
            // under it copies its part.
            PatternKind::Some(_) if boxed => {
                self.copied_under(pattern, &mut parts.copied);
                parts.kept[value] = true;
            }
            // An inline optional's value is a step of the path, as a payload is.
            PatternKind::Some(inner) => {
                let TypeKind::Optional(payload) = ty.kind() else {
                    unreachable!("the checker should allow a `Some` pattern only at an optional")
                };
                let mut at = place.clone();
                at.projections.push(Projection::Some);
                self.walk_parts(inner, &at, payload, false, value, parts)?;
            }
            // A part nothing moves stays in its value: `_`, `None`, a string literal, an
            // uncounted binding or test, which leave nothing counted.
            PatternKind::Wild
            | PatternKind::Bind(_)
            | PatternKind::None
            | PatternKind::Bool(_)
            | PatternKind::Range { .. }
            | PatternKind::Str(_) => parts.kept[value] |= self.counted(ty),
        }
        Some(())
    }

    /// Adds each counted binding in `pattern` to `copied`, once.
    fn copied_under(&self, pattern: &Pattern, copied: &mut Vec<LocalId>) {
        match &pattern.kind {
            PatternKind::Bind(binding) => {
                if self.locals[binding.0].counted && !copied.contains(binding) {
                    copied.push(*binding);
                }
            }
            PatternKind::Variant { fields: subs, .. } | PatternKind::Or(subs) => {
                for sub in subs {
                    self.copied_under(sub, copied);
                }
            }
            PatternKind::Some(sub) => self.copied_under(sub, copied),
            PatternKind::Wild
            | PatternKind::None
            | PatternKind::Bool(_)
            | PatternKind::Range { .. }
            | PatternKind::Str(_) => {}
        }
    }

    /// Whether `pattern`, of type `ty`, an alternative of an `|` or a part of one, binds a
    /// counted name or tests a box inside it.
    fn binds_or_tests(&self, pattern: &Pattern, ty: Type) -> bool {
        match &pattern.kind {
            PatternKind::Bind(binding) => self.locals[binding.0].counted,
            PatternKind::Variant { variant, fields } => {
                let TypeKind::Enum(id, _) = ty.kind() else {
                    unreachable!("the checker should allow a variant pattern only at an enum")
                };
                let payloads = &self.program.enums[id.0].variants[*variant].fields;
                fields.iter().zip(payloads).any(|(sub, payload)| {
                    payload.boxed
                        && matches!(sub.kind, PatternKind::Bind(_) | PatternKind::Variant { .. })
                        || self.binds_or_tests(sub, payload.ty)
                })
            }
            PatternKind::Or(alternatives) => alternatives
                .iter()
                .any(|alternative| self.binds_or_tests(alternative, ty)),
            PatternKind::Some(sub) => {
                let TypeKind::Optional(payload) = ty.kind() else {
                    unreachable!("the checker should allow a `Some` pattern only at an optional")
                };
                self.binds_or_tests(sub, payload)
            }
            PatternKind::Wild
            | PatternKind::None
            | PatternKind::Bool(_)
            | PatternKind::Range { .. }
            | PatternKind::Str(_) => false,
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
                let TypeKind::Enum(id, _) = ty.kind() else {
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
            PatternKind::Some(_) | PatternKind::None => {
                self.test_optional(pattern, place, ty, fail)
            }
        }
    }

    /// `test` of a `Some(p)` or `None` pattern: `test_present`, then `p` against the value
    /// inside, which the `some` projection reaches.
    fn test_optional(
        &mut self,
        pattern: &Pattern,
        place: &Place,
        ty: Type,
        fail: Target,
    ) -> Target {
        let TypeKind::Optional(payload) = ty.kind() else {
            unreachable!("the checker should allow a `Some` or `None` pattern only at an optional")
        };
        let (next, other) = (self.new_block(), self.realize(fail));
        let PatternKind::Some(inner) = &pattern.kind else {
            self.terminate(Terminator::TestPresent {
                place: place.clone(),
                some: other,
                none: next,
            });
            self.current = Some(next);
            return fail;
        };
        self.terminate(Terminator::TestPresent {
            place: place.clone(),
            some: next,
            none: other,
        });
        self.current = Some(next);
        if matches!(inner.kind, PatternKind::Wild) {
            return fail;
        }
        let mut at = place.clone();
        at.projections.push(Projection::Some);
        self.test(inner, &at, payload, fail)
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

/// What a take-apart commit moves, copies and frees; see `Lowering::parts`.
struct Parts {
    /// Each tested box, outermost first: the enum it holds and its field's place.
    tokens: Vec<(Type, Place)>,
    /// Each counted binding that moves out of its place, with the place.
    moved: Vec<(LocalId, Place)>,
    /// Each counted binding of a boxed `F?`, which the commit copies.
    copied: Vec<LocalId>,
    /// The scrutinee's place, then each tested box's, and whether that value keeps a counted
    /// part nothing moves or tests.
    rest: Vec<Place>,
    kept: Vec<bool>,
}

impl Parts {
    /// Records the box at `place`, of the enum `ty`, as tested: a token, and a value of its own.
    fn token(&mut self, ty: Type, place: &Place) -> usize {
        self.tokens.push((ty, place.clone()));
        self.rest.push(place.clone());
        self.kept.push(false);
        self.rest.len() - 1
    }
}
