//! The join: the one type that the members of a form convert to. The forms are an `if`, a
//! `match`, an array literal, and a loop's `break` values. They are also the sides of `??` and
//! of an operator, and the arguments at one type parameter of a call. `join_form` is the one
//! entry. It picks the candidate that each member converts to, with the fewest wraps, and then
//! settles each member to it: it writes each `Wrap` and `ToAny` at the value positions of the
//! member, through `conversion` and `apply`.

use rustc_hash::{FxHashMap, FxHashSet};

mod join_report;

use super::any::Conversion;
use super::infer::Table;
use super::operator::Operator;
use super::reports::Reported;
use super::{Checked, Checker, Expectation, Poisoned};
use crate::conformance::ensure_stack;
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, ExprKind, Type, TypeArgs, TypeKind};
use crate::types::MARK_INDEX;

#[cfg(test)]
thread_local! {
    /// How many candidates `join` has tested, on this thread.
    pub(super) static CANDIDATES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many value positions `settle` has settled, on this thread.
    pub(super) static SETTLED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many nodes `value_positions` has visited, on this thread.
    pub(super) static VALUE_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many times `nested_leaves` has put a position around a leaf, on this thread.
    pub(super) static LEAF_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many distinct types `fill` has filled, on this thread.
    pub(super) static FILLED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many times `part_in` has compared a part of a place with a part of a type, on this
    /// thread.
    pub(super) static PART_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A member of a join: a checked value, where it stands, and the span of the label of an E0043
/// `BareBreak` (the `break` of a loop's member, else the value). A poisoned member takes part with
/// its type, or is left out when it has none.
pub(super) struct JoinMember<'a> {
    pub(super) value: &'a mut Checked,
    pub(super) at: Place,
    pub(super) label: Span,
}

/// Where a member stands in its join.
#[derive(Clone, Copy)]
pub(super) enum Place {
    /// The member is its own value: a branch's value, an element, a `break` value, an operand,
    /// or an argument at a bare `T`.
    Whole,
    /// An argument at a parameter of type `place`, which names the call's `T` inside it (`[T]`,
    /// `T?`, `Box<T>`). The member is the part of the argument at `T`, and `argument` is the
    /// argument's span.
    Part { place: Type, argument: Span },
    /// An argument at a place that names two or more parameters of the call, whose type unified
    /// with that place once, before its first join. The member is `part`, its part at `T`, which
    /// converts to a type of `T` only by unification. So a join of each parameter does not walk
    /// the whole argument again.
    Split { part: Type, argument: Span },
}

impl JoinMember<'_> {
    /// The type that the member takes part with: its part when it is split, else the type of its
    /// value.
    pub(super) fn ty(&self) -> Option<Type> {
        match self.at {
            Place::Split { part, .. } => Some(part),
            Place::Whole | Place::Part { .. } => member_type(self.value),
        }
    }
}

/// The form of a join, which its report names.
#[derive(Clone, Copy)]
#[expect(dead_code, reason = "the forms come in Task 5")]
pub(super) enum JoinForm {
    If,
    Match,
    Array,
    Loop,
    Coalesce,
    Operator(Operator),
    Compare,
    /// `+=`: the member at `assigned` is the place it writes to, which does not convert.
    Compound {
        op: Operator,
        assigned: usize,
    },
    /// The arguments at the places of the type parameter `param` of a call.
    Call {
        param: Symbol,
    },
}

/// What `join` found when no conflict stopped it.
enum Joined {
    /// Each member converts to this type.
    Typed(Type),
    /// An untyped poisoned member is left out, and no member that is left states a type.
    Untyped,
    /// Each member is `never`.
    Never,
}

/// The members of a join that has no candidate, in groups by type.
struct Conflict {
    groups: Vec<Group>,
    /// Each member that is a made part of its argument, at the argument's span, with its part's
    /// type. A call labels it as the argument that gives its type parameter that type.
    parts: Vec<(Span, Type)>,
}

/// The leaves of the members of a conflict that have one type, by their spans: a member of a
/// nested join is named at its own depth.
struct Group {
    ty: Type,
    leaves: Vec<Span>,
    /// The leaves that are a whole value, which a conversion can wrap: not the assigned side of a
    /// compound assignment, and not an `if` or a `match`.
    whole: Vec<Span>,
    /// The type and the value of each leaf that is an integer literal.
    literals: Vec<(Type, i64)>,
}

/// Where a join stands in the source.
#[derive(Clone, Copy)]
pub(super) struct Heading {
    /// The primary label of a conflict: the `if`, the `[` of an array, the call's name.
    pub(super) head: Span,
    /// The whole expression, which a report of the type of the join as a whole points at.
    pub(super) whole: Span,
    /// For a call, its return type with the type parameter of the join in it, when that is not the
    /// bare parameter: the type that a fix writes after a `let` is that type with the interface.
    pub(super) returns: Option<Type>,
}

/// What a join reports about: its form, where it stands, and how its members take part.
#[derive(Clone, Copy)]
struct Site {
    form: JoinForm,
    heading: Heading,
    /// The `T` of a call's `Place::Part` members.
    hole: Option<Symbol>,
    /// The member that converts by unification only.
    fixed: Option<usize>,
}

/// A value at the end of the value positions of a member, with its span and its type. `around` is
/// its type inside the positions around it (`[t]` for an element, `t?` for a `Some` payload).
/// `lifted` is its type with `?` added inside those positions (rule A).
struct Leaf {
    at: Span,
    ty: Type,
    around: Type,
    lifted: Type,
    /// The value of an integer literal that the leaf is.
    literal: Option<i64>,
}

impl Leaf {
    fn new(at: Span, ty: Type) -> Leaf {
        Leaf {
            at,
            ty,
            around: ty,
            lifted: Type::optional(ty),
            literal: None,
        }
    }

    /// The leaf that the value `e` is.
    fn of(e: &tir::Expr) -> Leaf {
        let literal = match e.kind {
            ExprKind::Int(value) => Some(value),
            _ => None,
        };
        Leaf {
            literal,
            ..Leaf::new(e.span, e.ty)
        }
    }
}

impl Checker<'_> {
    /// The one entry of a join: the type of the members, each settled to it. Under a required type
    /// that is the only candidate. `heading` is where the form stands, and `binding` the name of a
    /// `let` without a type, which the fix of a conflict writes after. A conflict, or a required
    /// type that a member does not take, is reported here: a conflict is untyped poison (typed by
    /// a required type). When an untyped poisoned member is left out, a typed join is poison of its
    /// type (ADR 0049).
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the other forms come in Task 5")
    )]
    pub(super) fn join_form(
        &mut self,
        form: JoinForm,
        heading: Heading,
        members: &mut [JoinMember<'_>],
        expected: Expectation,
        binding: Option<Span>,
    ) -> Result<Type, Poisoned> {
        let returns = |_: &mut Self| heading.returns;
        self.join_form_with(form, (heading, returns), members, expected, binding)
    }

    /// `join_form`, where `returns` gives the `returns` of the heading. It is called only for a
    /// report, so a call builds its result type only when its join fails.
    pub(super) fn join_form_with(
        &mut self,
        form: JoinForm,
        (heading, returns): (Heading, impl FnOnce(&mut Self) -> Option<Type>),
        members: &mut [JoinMember<'_>],
        expected: Expectation,
        binding: Option<Span>,
    ) -> Result<Type, Poisoned> {
        let hole = match form {
            JoinForm::Call { param } => Some(param),
            _ => None,
        };
        let fixed = match form {
            JoinForm::Compound { assigned, .. } => Some(assigned),
            _ => None,
        };
        let mut site = Site {
            form,
            heading,
            hole,
            fixed,
        };
        let required = match expected {
            Expectation::Required(ty, origin) => Some((ty, origin)),
            Expectation::None
            | Expectation::Poisoned
            | Expectation::Hint { .. }
            | Expectation::Member => None,
        };
        // The first member that is left out: untyped poison.
        let left_out = members.iter().find_map(|member| match &*member.value {
            Err(poisoned) if poisoned.ty.is_none() => Some(poisoned.reported()),
            _ => None,
        });
        match self.join_members(members, hole, fixed, required.map(|(ty, _)| ty)) {
            Ok(Joined::Typed(ty)) => {
                for (i, member) in members.iter_mut().enumerate() {
                    let target = target(member.at, hole, ty);
                    self.settle_member(member, target, fixed == Some(i));
                }
                let ty = self.table.resolve(ty);
                match left_out {
                    Some(reported) => Err(Poisoned::typed(ty, reported)),
                    None => Ok(ty),
                }
            }
            Ok(Joined::Never) => Ok(Type::NEVER),
            Ok(Joined::Untyped) => {
                let reported = left_out.expect("an untyped join should have an untyped member");
                Err(Poisoned::untyped(reported))
            }
            Err(conflict) => {
                site.heading.returns = returns(self);
                if let Some((ty, origin)) = required {
                    let reported = self.report_required(site, (ty, origin), members);
                    Err(Poisoned::typed(ty, reported))
                } else {
                    let reported = self.report_conflict(site, binding, members, &conflict);
                    Err(Poisoned::untyped(reported))
                }
            }
        }
    }

    /// The type that the join of `members` takes, with no report and no bind: `None` when it has
    /// no candidate, no type or no value. Under `required`, that type when each member takes it.
    pub(super) fn join_type(
        &mut self,
        members: &mut [JoinMember<'_>],
        hole: Option<Symbol>,
        required: Option<Type>,
    ) -> Option<Type> {
        match self.join_members(members, hole, None, required) {
            Ok(Joined::Typed(ty)) => Some(ty),
            Ok(Joined::Untyped | Joined::Never) | Err(_) => None,
        }
    }

    /// The candidate that each member takes with the fewest wraps. Each test runs under a
    /// snapshot, which it rolls back: the join binds nothing. A required type decides the join
    /// also when a member is left out. `hole` is the `T` of a call's
    /// `Place::Part` members, and the member at `fixed` converts by unification only.
    fn join_members(
        &mut self,
        members: &mut [JoinMember<'_>],
        hole: Option<Symbol>,
        fixed: Option<usize>,
        required: Option<Type>,
    ) -> Result<Joined, Conflict> {
        let mut taking = Vec::new();
        let mut left_out = false;
        for (i, member) in members.iter().enumerate() {
            match member_type(member.value) {
                Some(Type::NEVER) => {}
                Some(_) => taking.push(i),
                None => left_out = true,
            }
        }
        if taking.is_empty() && !left_out {
            return Ok(Joined::Never);
        }
        // Without its poisoned member, a join is typed only by a required type or by a member that
        // states its type.
        let stated = |c: &Self, i: usize| {
            member_type(members[i].value).is_some_and(|ty| !c.table.resolve(ty).holds_infer())
        };
        if required.is_none() && left_out && !taking.iter().any(|&i| stated(self, i)) {
            return Ok(Joined::Untyped);
        }
        let candidates = match required {
            Some(ty) => vec![ty],
            None => self.candidates(members, &taking, hole),
        };
        let mut best: Option<(usize, Type)> = None;
        for candidate in candidates {
            #[cfg(test)]
            CANDIDATES.set(CANDIDATES.get() + 1);
            let snapshot = self.table.snapshot();
            let bounds = self.body.candidate_bounds.len();
            let mut wraps = Some(0);
            for &i in &taking {
                let target = target(members[i].at, hole, candidate);
                let Some(more) = self.member_converts(&mut members[i], target, fixed == Some(i))
                else {
                    wraps = None;
                    break;
                };
                wraps = wraps.map(|n| n + more);
            }
            // A bound of an open form that the candidate closes can fail. Then the candidate
            // fails, so the result does not depend on the order of the members.
            if !self.candidate_bounds_hold(bounds) {
                wraps = None;
            }
            self.table.rollback(snapshot);
            if let Some(wraps) = wraps
                && best.is_none_or(|(least, _)| wraps < least)
            {
                best = Some((wraps, candidate));
            }
        }
        match best {
            Some((_, ty)) => Ok(Joined::Typed(ty)),
            None => Err(self.conflict(members, &taking, hole, fixed)),
        }
    }

    /// The candidates of a join, one per key (`Table::key`): the type of each member, and of each
    /// nested member (a leaf) inside the positions around it. A nested member is a member. So
    /// when a member is optional, each other member's type with `?` added at its position is a
    /// candidate too (rule A). For a `Place::Part` member, each is its part at `hole`.
    fn candidates(
        &self,
        members: &mut [JoinMember<'_>],
        taking: &[usize],
        hole: Option<Symbol>,
    ) -> Vec<Type> {
        let optional =
            |c: &Self, ty: Type| matches!(c.table.resolve(ty).kind(), TypeKind::Optional(_));
        // Each member's top, then its nested members: the candidate, the candidate with `?`
        // added, and whether the member's own type is optional.
        let mut found: Vec<(Type, Option<Type>, bool)> = Vec::new();
        for &i in taking {
            let member = &mut members[i];
            // A split member is one top: its part, which has no nested member.
            if let Place::Split { part, .. } = member.at {
                let part = self.table.resolve(part);
                found.push((part, Some(Type::optional(part)), optional(self, part)));
                continue;
            }
            let mut leaves = Vec::new();
            match &mut *member.value {
                Ok(e) => {
                    leaves.push(Leaf::of(e));
                    nested_leaves(e, &mut leaves);
                }
                Err(poisoned) => leaves.extend(poisoned.ty.map(|ty| Leaf::new(member.label, ty))),
            }
            for (k, leaf) in leaves.into_iter().enumerate() {
                let part = |ty: Type| match member.at {
                    Place::Whole => Some(ty),
                    Place::Part { place, .. } => {
                        let (place, ty) = (self.table.resolve(place), self.table.resolve(ty));
                        hole.and_then(|hole| part_at(place, hole, ty))
                    }
                    Place::Split { part, .. } => Some(part),
                };
                let Some(around) = part(leaf.around) else {
                    continue;
                };
                // A top is optional by its part at `hole`, a nested member by its own type. The
                // `?` of a top goes on its part, as `?` added at `[T]` has no part at `T`.
                let (own, lifted) = if k == 0 {
                    (around, Some(Type::optional(around)))
                } else {
                    (leaf.ty, part(leaf.lifted))
                };
                found.push((around, lifted, optional(self, own)));
            }
        }
        let optionals = found.iter().filter(|&&(.., own)| own).count();
        let mut keys = FxHashSet::default();
        let mut candidates = Vec::new();
        let mut add = |c: &Self, ty: Type| {
            if keys.insert(c.table.key(ty)) {
                candidates.push(ty);
            }
        };
        for &(around, lifted, own) in &found {
            add(self, around);
            if let Some(lifted) = lifted.filter(|_| optionals > usize::from(own)) {
                add(self, lifted);
            }
        }
        candidates
    }

    /// `converts` for `member`, whose target is `target`. A split member only unifies its part.
    pub(super) fn member_converts(
        &mut self,
        member: &mut JoinMember<'_>,
        target: Type,
        fixed: bool,
    ) -> Option<usize> {
        match member.at {
            Place::Split { part, .. } => self.unify(part, target).then_some(0),
            Place::Whole | Place::Part { .. } => self.converts(member.value, target, fixed),
        }
    }

    /// Whether `e` converts to `target` only at its top: it has no value position.
    pub(super) fn converts_at_top(&self, e: &mut tir::Expr, target: Type) -> bool {
        let mut inner = false;
        value_positions(e, &mut |_| inner = true);
        !inner && self.enter(e, target).is_none()
    }

    /// The wraps that `value` takes to convert to `target`, or `None`. It binds what it unifies:
    /// `join` calls it under a snapshot. The member at a `fixed` place only unifies.
    pub(super) fn converts(
        &mut self,
        value: &mut Checked,
        target: Type,
        fixed: bool,
    ) -> Option<usize> {
        match value {
            Ok(e) if fixed => self.unify(e.ty, target).then_some(0),
            Ok(e) => self.converts_at(e, target),
            Err(poisoned) => {
                let ty = poisoned.ty?;
                if fixed {
                    self.unify(ty, target).then_some(0)
                } else {
                    self.conversion(ty, target).map(|steps| steps.len())
                }
            }
        }
    }

    /// `converts` for the value position `e`: a made value converts at its top, and a value that
    /// has value positions converts at each of them. It unifies what `settle_at` unifies, in the
    /// same order, so that `settle_at` takes the steps that this counts.
    pub(super) fn converts_at(&mut self, e: &mut tir::Expr, target: Type) -> Option<usize> {
        if e.ty == Type::NEVER {
            return Some(0);
        }
        ensure_stack(|| match self.enter(e, target) {
            Some((part, wrapped)) => {
                let mut wraps = Some(usize::from(wrapped));
                value_positions(e, &mut |child| {
                    wraps = wraps.and_then(|n| Some(n + self.converts_at(child, part)?));
                });
                if wraps.is_some() {
                    let own = if wrapped { Type::array(part) } else { target };
                    self.unify(e.ty, own);
                }
                wraps
            }
            None => match self.table.open_call(e) {
                // An open call converts as its closed form when it can, else at the candidate.
                Some(call) => {
                    let snapshot = self.table.snapshot();
                    if let Some(steps) = self.conversion(e.ty, target) {
                        self.keep_snapshot(snapshot);
                        return Some(steps.len());
                    }
                    self.table.rollback(snapshot);
                    self.open_at(call, e, target)
                }
                None => self.conversion(e.ty, target).map(|steps| steps.len()),
            },
        })
    }

    /// Changes the value of `member` to `target`, which `join` chose. A member with no value to
    /// lift becomes a value poisoned by its E0043, of the type `target`. A split member only
    /// unifies its part.
    fn settle_member(&mut self, member: &mut JoinMember<'_>, target: Type, fixed: bool) {
        if let Place::Split { part, .. } = member.at {
            self.unify(part, target);
            return;
        }
        let value = &mut *member.value;
        match value {
            Ok(e) if fixed => {
                self.unify(e.ty, target);
            }
            Ok(e) => {
                if let Err(reported) = self.settle_at(e, target) {
                    *value = Err(Poisoned::typed(self.table.resolve(target), reported));
                }
            }
            Err(poisoned) => {
                if let Some(ty) = poisoned.ty
                    && ty != Type::NEVER
                {
                    if fixed {
                        self.unify(ty, target);
                    } else {
                        self.conversion(ty, target);
                    }
                    poisoned.ty = Some(target);
                }
            }
        }
    }

    /// Settles the value position `e` to `target`: it adds each `Wrap` and `ToAny` at the position
    /// where it applies, and gives each value position on the way its type. `Err` holds the proof
    /// of an E0043.
    pub(super) fn settle_at(&mut self, e: &mut tir::Expr, target: Type) -> Result<(), Reported> {
        #[cfg(test)]
        SETTLED.set(SETTLED.get() + 1);
        if e.ty == Type::NEVER {
            return Ok(());
        }
        ensure_stack(|| match self.enter(e, target) {
            Some((part, wrapped)) => {
                let mut failed = Ok(());
                value_positions(e, &mut |child| {
                    let settled = self.settle_at(child, part);
                    failed = failed.and(settled);
                });
                let own = if wrapped { Type::array(part) } else { target };
                if !self.unify(e.ty, own) {
                    e.ty = own;
                }
                failed?;
                if wrapped {
                    self.convert_in_place(e, &[Conversion::Wrap])?;
                }
                Ok(())
            }
            None => match self.table.open_call(e) {
                // As `converts_at`: the closed form when it converts, else the open form.
                Some(call) => {
                    let snapshot = self.table.snapshot();
                    if let Some(steps) = self.conversion(e.ty, target) {
                        self.keep_snapshot(snapshot);
                        return self.convert_in_place(e, &steps);
                    }
                    self.table.rollback(snapshot);
                    self.settle_open(call, e, target)
                }
                None => match self.conversion(e.ty, target) {
                    Some(steps) => self.convert_in_place(e, &steps),
                    // `converts_at` took this value under the same bindings, so no value gets
                    // here.
                    None => Err(self.proof(e.span)),
                },
            },
        })
    }

    /// Applies `steps` to `e` in place.
    fn convert_in_place(
        &mut self,
        e: &mut tir::Expr,
        steps: &[Conversion],
    ) -> Result<(), Reported> {
        if steps.is_empty() {
            return Ok(());
        }
        let placeholder = tir::Expr::new(ExprKind::Unit, Type::UNIT, e.span);
        let taken = std::mem::replace(e, placeholder);
        *e = self
            .apply(taken, steps)
            .map_err(|poisoned| poisoned.reported())?;
        Ok(())
    }

    /// The part of `target` at the value positions of `e`, and whether `e` itself then wraps, or
    /// `None` when `e` converts at its top: a made value, or an array or a `Some` whose `target`
    /// has another shape.
    fn enter(&self, e: &tir::Expr, target: Type) -> Option<(Type, bool)> {
        let target = self.table.resolve(target);
        match &e.kind {
            ExprKind::If {
                else_block: Some(_),
                ..
            }
            | ExprKind::IfSome {
                else_block: Some(_),
                ..
            }
            | ExprKind::Block(_)
            | ExprKind::Loop(_)
            | ExprKind::Dbg { .. } => Some((target, false)),
            ExprKind::Match { arms, .. } if !arms.iter().any(|arm| arm.made) => {
                Some((target, false))
            }
            ExprKind::ArrayLit(_) | ExprKind::ArrayFill { .. } => match target.kind() {
                TypeKind::Array(element) => Some((element, false)),
                TypeKind::Optional(payload) => match payload.kind() {
                    TypeKind::Array(element) => Some((element, true)),
                    _ => None,
                },
                _ => None,
            },
            ExprKind::Wrap(_) => match target.kind() {
                TypeKind::Optional(payload) => Some((payload, false)),
                _ => None,
            },
            // No value position: the value converts at its top. Each kind is named, so that a new
            // kind is put on one side or the other.
            ExprKind::If { .. }
            | ExprKind::IfSome { .. }
            | ExprKind::Match { .. }
            | ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Char(_)
            | ExprKind::Str(_)
            | ExprKind::None
            | ExprKind::Unit
            | ExprKind::Local(_)
            | ExprKind::Call { .. }
            | ExprKind::InterfaceCall { .. }
            | ExprKind::New { .. }
            | ExprKind::Variant { .. }
            | ExprKind::Field { .. }
            | ExprKind::Struct { .. }
            | ExprKind::StructField { .. }
            | ExprKind::ToAny(_)
            | ExprKind::DynCall { .. }
            | ExprKind::Convert(_)
            | ExprKind::Print { .. }
            | ExprKind::Panic(_)
            | ExprKind::ReadLine
            | ExprKind::Index { .. }
            | ExprKind::Slice { .. }
            | ExprKind::Len(_)
            | ExprKind::Chars(_)
            | ExprKind::Bytes(_)
            | ExprKind::Words(_)
            | ExprKind::FromChars(_)
            | ExprKind::Math { .. }
            | ExprKind::Unary { .. }
            | ExprKind::Binary { .. }
            | ExprKind::Interpolate { .. }
            | ExprKind::Concat { .. }
            | ExprKind::Compare { .. }
            | ExprKind::Equal { .. }
            | ExprKind::Is { .. }
            | ExprKind::Break { .. }
            | ExprKind::Continue { .. }
            | ExprKind::Return(_)
            | ExprKind::MutCall { .. } => None,
        }
    }

    /// The groups of the leaves of the taking members, by type. A leaf has the type that it has
    /// inside the positions around it (`[i64]` for the literal `1` in `[1]`). A member that is a
    /// made part of its argument is no leaf: it is in `parts`, with the type of its part.
    fn conflict(
        &mut self,
        members: &mut [JoinMember<'_>],
        taking: &[usize],
        hole: Option<Symbol>,
        fixed: Option<usize>,
    ) -> Conflict {
        let mut groups: Vec<Group> = Vec::new();
        let mut parts = Vec::new();
        let mut index: FxHashMap<Type, usize> = FxHashMap::default();
        for &i in taking {
            let member = &mut members[i];
            // A split member is a made part of its argument, as a `Place::Part` top.
            if let Place::Split { part, argument } = member.at {
                if hole.is_some() {
                    parts.push((argument, self.table.resolve(part)));
                }
                continue;
            }
            let mut leaves = Vec::new();
            // Whether the one leaf is the member's own value, and a conversion can wrap it.
            let (mut top, mut whole) = (false, false);
            match &mut *member.value {
                Ok(e) if e.ty != Type::NEVER => {
                    top = !nested_leaves(e, &mut leaves);
                    if top {
                        leaves.push(Leaf::of(e));
                        whole = fixed != Some(i)
                            && !matches!(
                                e.kind,
                                ExprKind::If { .. }
                                    | ExprKind::IfSome { .. }
                                    | ExprKind::Match { .. }
                            );
                    }
                }
                Ok(_) => {}
                Err(poisoned) => {
                    top = true;
                    leaves.extend(poisoned.ty.map(|ty| Leaf::new(member.label, ty)));
                }
            }
            for leaf in leaves {
                let ty = self.table.resolve(leaf.around);
                let ty = match (member.at, hole) {
                    (Place::Part { place, .. }, Some(hole)) => {
                        let place = self.table.resolve(place);
                        // An open part that meets a known part of the place shows it, as `X` of
                        // `Two<X, Two<X, Y>>` takes `i64` from `Two<T, Two<i64, U>>`.
                        let ty = if top && ty.holds_infer() {
                            self.shown_against(ty, place)
                        } else {
                            ty
                        };
                        part_at(place, hole, ty).unwrap_or(ty)
                    }
                    // The left side of `??` is at its payload.
                    (Place::Part { .. } | Place::Split { .. }, None) => match ty.kind() {
                        TypeKind::Optional(payload) => payload,
                        _ => ty,
                    },
                    _ => ty,
                };
                // Only a call labels an argument that gives its type parameter a type.
                if let (Place::Part { argument, .. }, true, true) = (member.at, top, hole.is_some())
                {
                    parts.push((argument, ty));
                    continue;
                }
                let at = *index.entry(ty).or_insert_with(|| {
                    groups.push(Group {
                        ty,
                        leaves: Vec::new(),
                        whole: Vec::new(),
                        literals: Vec::new(),
                    });
                    groups.len() - 1
                });
                groups[at].leaves.push(leaf.at);
                if whole && matches!(member.at, Place::Whole) {
                    groups[at].whole.push(leaf.at);
                }
                if let Some(value) = leaf.literal {
                    groups[at]
                        .literals
                        .push((self.table.resolve(leaf.ty), value));
                }
            }
        }
        Conflict { groups, parts }
    }
}

/// The type of a member that takes part: its value's, or its poison's when that is typed.
fn member_type(value: &Checked) -> Option<Type> {
    match value {
        Ok(e) => Some(e.ty),
        Err(poisoned) => poisoned.ty,
    }
}

/// The type that a member at `at` converts to when the join is `ty`: `ty` itself, or the type of
/// its parameter with `hole` replaced by `ty`. A split member is its part, so it converts to `ty`.
pub(super) fn target(at: Place, hole: Option<Symbol>, ty: Type) -> Type {
    match (at, hole) {
        (Place::Part { place, .. }, Some(hole)) => fill(place, hole, ty),
        (Place::Part { place, .. }, None) => place,
        (Place::Whole | Place::Split { .. }, _) => ty,
    }
}

/// Each leaf at the value positions of `e`, which is `e` itself when it has none.
fn collect_leaves(e: &mut tir::Expr, out: &mut Vec<Leaf>) {
    if e.ty != Type::NEVER && !nested_leaves(e, out) {
        out.push(Leaf::of(e));
    }
}

/// The leaves at the value positions inside `e`, each with the positions of `e` around it.
/// `false` when `e` has no value position. Only an element and a payload change the type around
/// a leaf, so a leaf costs one step per array or `Some` above it: the depth of its type.
fn nested_leaves(e: &mut tir::Expr, out: &mut Vec<Leaf>) -> bool {
    let start = out.len();
    let mut inner = false;
    ensure_stack(|| {
        value_positions(e, &mut |child| {
            inner = true;
            collect_leaves(child, out);
        });
    });
    let around: fn(Type) -> Type = match e.kind {
        ExprKind::ArrayLit(_) | ExprKind::ArrayFill { .. } => Type::array,
        ExprKind::Wrap(_) => Type::optional,
        _ => return inner,
    };
    for leaf in &mut out[start..] {
        #[cfg(test)]
        LEAF_STEPS.set(LEAF_STEPS.get() + 1);
        leaf.around = around(leaf.around);
        leaf.lifted = around(leaf.lifted);
    }
    inner
}

/// Whether `e`, or a value at a value position inside it, is a call whose type holds an open
/// variable of `ty`: a generic call that waits for the context of the call around it.
pub(super) fn waiting_call(table: &Table, e: &mut tir::Expr, ty: Type) -> bool {
    fn waits(table: &Table, e: &mut tir::Expr, roots: &[u32]) -> bool {
        if matches!(e.kind, ExprKind::Call { .. })
            && (table.open_roots(e.ty).iter()).any(|root| roots.contains(root))
        {
            return true;
        }
        let mut found = false;
        value_positions(e, &mut |inner| found = found || waits(table, inner, roots));
        found
    }
    waits(table, e, &table.open_roots(ty))
}

/// Calls `f` on each value position directly in `e`. These are each branch's value of an `if` or
/// an `if let` with an `else` and of a `match`, and a block's value. They are also each element
/// of an array literal, the fill value of `[v; n]`, the payload of a `Some` and the operand of
/// `dbg`. In a loop, they are each `break` value that ends that loop. A block with no value gives
/// `f` a `unit` at the span of `e`, which the block keeps only when `f` changes it. A `match`
/// with the made arm of an `if let` without an `else` has no value, so it has no value position.
fn value_positions(e: &mut tir::Expr, f: &mut impl FnMut(&mut tir::Expr)) {
    #[cfg(test)]
    VALUE_STEPS.set(VALUE_STEPS.get() + 1);
    let span = e.span;
    match &mut e.kind {
        ExprKind::If {
            then_block,
            else_block: Some(else_block),
            ..
        }
        | ExprKind::IfSome {
            then_block,
            else_block: Some(else_block),
            ..
        } => {
            block_positions(then_block, span, f);
            block_positions(else_block, span, f);
        }
        ExprKind::Match { arms, .. } if !arms.iter().any(|arm| arm.made) => {
            for arm in arms {
                block_positions(&mut arm.body, span, f);
            }
        }
        ExprKind::Block(block) => block_positions(block, span, f),
        ExprKind::ArrayLit(elements) => {
            for element in elements {
                f(element);
            }
        }
        ExprKind::ArrayFill { value, .. }
        | ExprKind::Wrap(value)
        | ExprKind::Dbg { operand: value, .. } => f(value),
        ExprKind::Loop(body) => body.each_expr_mut(&mut |child| breaks(child, 0, f)),
        // No value position: the value converts at its top. Each kind is named, so that a new
        // kind is put on one side or the other.
        ExprKind::If { .. }
        | ExprKind::IfSome { .. }
        | ExprKind::Match { .. }
        | ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Char(_)
        | ExprKind::Str(_)
        | ExprKind::None
        | ExprKind::Unit
        | ExprKind::Local(_)
        | ExprKind::Call { .. }
        | ExprKind::InterfaceCall { .. }
        | ExprKind::New { .. }
        | ExprKind::Variant { .. }
        | ExprKind::Field { .. }
        | ExprKind::Struct { .. }
        | ExprKind::StructField { .. }
        | ExprKind::ToAny(_)
        | ExprKind::DynCall { .. }
        | ExprKind::Convert(_)
        | ExprKind::Print { .. }
        | ExprKind::Panic(_)
        | ExprKind::ReadLine
        | ExprKind::Index { .. }
        | ExprKind::Slice { .. }
        | ExprKind::Len(_)
        | ExprKind::Chars(_)
        | ExprKind::Bytes(_)
        | ExprKind::Words(_)
        | ExprKind::FromChars(_)
        | ExprKind::Math { .. }
        | ExprKind::Unary { .. }
        | ExprKind::Binary { .. }
        | ExprKind::Interpolate { .. }
        | ExprKind::Concat { .. }
        | ExprKind::Compare { .. }
        | ExprKind::Equal { .. }
        | ExprKind::Is { .. }
        | ExprKind::Break { .. }
        | ExprKind::Continue { .. }
        | ExprKind::Return(_)
        | ExprKind::MutCall { .. } => {}
    }
}

/// `value_positions` for the value of `block`, in an expression at `span`. A `never` block has no
/// value. The block takes the type that `f` gives its value.
fn block_positions(block: &mut tir::Block, span: Span, f: &mut impl FnMut(&mut tir::Expr)) {
    if block.ty == Type::NEVER {
        return;
    }
    if let Some(value) = &mut block.value {
        f(value);
        block.ty = value.ty;
    } else {
        let mut unit = tir::Expr::new(ExprKind::Unit, Type::UNIT, span);
        f(&mut unit);
        if !matches!(unit.kind, ExprKind::Unit) {
            block.ty = unit.ty;
            block.value = Some(Box::new(unit));
        }
    }
}

/// Calls `f` on each `break` value in `e` that ends the loop `depth` loops out from `e`, as
/// `wrap_breaks` counts the depth.
// shortcut: a nested loop is entered when it holds any jump (`exits` reads a kept flag). So each
// join of an enclosing loop walks again into the nested loops that jump, and a deep nest costs n
// squared. Task 5's `crossed` flag enters a loop only when a `break` in it ends an outer loop.
fn breaks(e: &mut tir::Expr, depth: usize, f: &mut impl FnMut(&mut tir::Expr)) {
    #[cfg(test)]
    VALUE_STEPS.set(VALUE_STEPS.get() + 1);
    let nested = matches!(e.kind, ExprKind::Loop(_));
    if nested && !e.exits() {
        return;
    }
    let inner = depth + usize::from(nested);
    ensure_stack(|| e.each_child_mut(&mut |child| breaks(child, inner, f)));
    if let ExprKind::Break {
        depth: target,
        value: Some(value),
    } = &mut e.kind
        && *target == depth
    {
        f(value);
    }
}

/// `place` with each `hole` replaced by `with`. The hole is the join's mark, the `Param` of that
/// name at `MARK_INDEX`: a parameter of the caller with the same name stays. Each distinct type
/// inside `place` is filled once, however many paths lead to it.
fn fill(place: Type, hole: Symbol, with: Type) -> Type {
    fill_in(place, hole, with, &mut FxHashMap::default())
}

/// `fill`, with `memo` holding each type that the walk has already filled.
fn fill_in(place: Type, hole: Symbol, with: Type, memo: &mut FxHashMap<Type, Type>) -> Type {
    if !place.has_param() {
        return place;
    }
    if let Some(&done) = memo.get(&place) {
        return done;
    }
    #[cfg(test)]
    FILLED.set(FILLED.get() + 1);
    let done = ensure_stack(|| {
        let args = |list: TypeArgs, memo: &mut FxHashMap<Type, Type>| {
            let filled: Vec<Type> = (list.get().iter())
                .map(|&arg| fill_in(arg, hole, with, memo))
                .collect();
            TypeArgs::intern(&filled)
        };
        match place.kind() {
            TypeKind::Param(name, MARK_INDEX) if name == hole => with,
            TypeKind::Array(inner) => Type::array(fill_in(inner, hole, with, memo)),
            TypeKind::Optional(inner) => Type::optional(fill_in(inner, hole, with, memo)),
            TypeKind::Class(id, list) => Type::intern(TypeKind::Class(id, args(list, memo))),
            TypeKind::Struct(id, list) => Type::intern(TypeKind::Struct(id, args(list, memo))),
            TypeKind::Enum(id, list) => Type::intern(TypeKind::Enum(id, args(list, memo))),
            TypeKind::Any(id, list) => Type::intern(TypeKind::Any(id, args(list, memo))),
            TypeKind::Assoc(base, interface, list, k) => {
                let base = fill_in(base, hole, with, memo);
                Type::intern(TypeKind::Assoc(base, interface, args(list, memo), k))
            }
            _ => place,
        }
    });
    memo.insert(place, done);
    done
}

/// The part of `ty`, an argument's type, at `hole` in its parameter's type `place`. The hole is
/// the join's mark, as in `fill`. An argument
/// that is not optional against a `place` of `T?` converts at its top, so its part is its own
/// type. `None` when `ty` does not have the shape of `place`.
pub(super) fn part_at(place: Type, hole: Symbol, ty: Type) -> Option<Type> {
    let mut misses = FxHashSet::default();
    match (place.kind(), ty.kind()) {
        (TypeKind::Optional(inner), kind) if !matches!(kind, TypeKind::Optional(_)) => {
            part_in(inner, hole, ty, &mut misses)
        }
        _ => part_in(place, hole, ty, &mut misses),
    }
}

/// `part_at` below the top. `misses` holds each pair of a place and a type that gave `None`. So a
/// type with shared parts is walked once for each distinct pair, not for each path.
fn part_in(
    place: Type,
    hole: Symbol,
    ty: Type,
    misses: &mut FxHashSet<(Type, Type)>,
) -> Option<Type> {
    #[cfg(test)]
    PART_STEPS.set(PART_STEPS.get() + 1);
    if !place.has_param() || misses.contains(&(place, ty)) {
        return None;
    }
    let found = ensure_stack(|| {
        let lists = match (place.kind(), ty.kind()) {
            (TypeKind::Param(name, MARK_INDEX), _) if name == hole => return Some(ty),
            (TypeKind::Array(ours), TypeKind::Array(theirs))
            | (TypeKind::Optional(ours), TypeKind::Optional(theirs)) => {
                return part_in(ours, hole, theirs, misses);
            }
            (TypeKind::Class(a, ours), TypeKind::Class(b, theirs)) if a == b => (ours, theirs),
            (TypeKind::Struct(a, ours), TypeKind::Struct(b, theirs)) if a == b => (ours, theirs),
            (TypeKind::Enum(a, ours), TypeKind::Enum(b, theirs)) if a == b => (ours, theirs),
            (TypeKind::Any(a, ours), TypeKind::Any(b, theirs)) if a == b => (ours, theirs),
            _ => return None,
        };
        (lists.0.get().iter().zip(lists.1.get()))
            .find_map(|(&ours, &theirs)| part_in(ours, hole, theirs, misses))
    });
    if found.is_none() {
        misses.insert((place, ty));
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::expectation::Origin;
    use crate::check::infer::{Kind, LiteralCheck};
    use crate::check::tests::joins::with_params;
    use crate::diagnostic::Code;
    use crate::mir::tests::assert_near_linear;
    use crate::tir::{Block, LocalId, StructId};

    /// An interface, and a struct that conforms to it.
    pub(super) const SEEN: &str = "interface Seen {\n    fn seen(self) -> i64\n}\nstruct Eye: Seen {\n    v: i64\n    fn seen(self) -> i64 { self.v }\n}\n";

    pub(super) fn at(n: usize) -> Span {
        Span::new(n, n + 1)
    }

    /// The heading of a join whose head is the whole expression, and which is no call.
    pub(super) fn heading(head: Span) -> Heading {
        Heading {
            head,
            whole: head,
            returns: None,
        }
    }

    /// The integer literal `value`, whose type is open.
    pub(super) fn int(c: &mut Checker<'_>, value: u64, n: usize) -> tir::Expr {
        let ty = c.table.fresh(Kind::Int, at(n));
        let check = LiteralCheck {
            span: at(n),
            literal: at(n),
            value,
            negated: None,
        };
        c.table.literal(ty, check);
        let value = i64::try_from(value).expect("the test literal should fit i64");
        tir::Expr::new(ExprKind::Int(value), ty, at(n))
    }

    /// `None`, whose payload is open.
    pub(super) fn none(c: &mut Checker<'_>, n: usize) -> tir::Expr {
        let payload = c.table.fresh(Kind::Any, at(n));
        tir::Expr::new(ExprKind::None, Type::optional(payload), at(n))
    }

    pub(super) fn local(ty: Type, n: usize) -> tir::Expr {
        tir::Expr::new(ExprKind::Local(LocalId(n)), ty, at(n))
    }

    pub(super) fn some(payload: tir::Expr) -> tir::Expr {
        let (ty, span) = (Type::optional(payload.ty), payload.span);
        tir::Expr::new(ExprKind::Wrap(Box::new(payload)), ty, span)
    }

    pub(super) fn array(elements: Vec<tir::Expr>, element: Type, n: usize) -> tir::Expr {
        tir::Expr::new(ExprKind::ArrayLit(elements), Type::array(element), at(n))
    }

    fn block(value: tir::Expr) -> Block {
        let ty = value.ty;
        Block {
            stmts: Vec::new(),
            value: Some(Box::new(value)),
            ty,
        }
    }

    /// `if true { then } else { other }`, of the type `ty`.
    pub(super) fn if_else(then: tir::Expr, other: tir::Expr, ty: Type, n: usize) -> tir::Expr {
        let kind = ExprKind::If {
            cond: Box::new(tir::Expr::new(ExprKind::Bool(true), Type::BOOL, at(n))),
            then_block: block(then),
            else_block: Some(block(other)),
        };
        tir::Expr::new(kind, ty, at(n))
    }

    /// `if true { print(0) }`, which has no `else`.
    pub(super) fn if_without_else(n: usize) -> tir::Expr {
        let operand = tir::Expr::new(ExprKind::Int(0), Type::I64, at(n));
        let print = ExprKind::Print {
            stream: tir::Stream::Stdout,
            operand: Box::new(operand),
        };
        let then_block = Block {
            stmts: vec![tir::Stmt::Expr(tir::Expr::new(print, Type::UNIT, at(n)))],
            value: None,
            ty: Type::UNIT,
        };
        let kind = ExprKind::If {
            cond: Box::new(tir::Expr::new(ExprKind::Bool(true), Type::BOOL, at(n))),
            then_block,
            else_block: None,
        };
        tir::Expr::new(kind, Type::UNIT, at(n))
    }

    /// `Eye(v: 1)`, of the struct type `eye`.
    pub(super) fn eye(eye: Type, n: usize) -> tir::Expr {
        let TypeKind::Struct(id, _) = eye.kind() else {
            panic!("the type should be the struct `Eye`");
        };
        let id: StructId = id;
        let v = tir::Expr::new(ExprKind::Int(1), Type::I64, at(n));
        tir::Expr::new(ExprKind::Struct { id, args: vec![v] }, eye, at(n))
    }

    /// The join of `values`, each a whole member, with no expected type.
    fn join_all(
        c: &mut Checker<'_>,
        form: JoinForm,
        values: &mut [Checked],
    ) -> Result<Type, Poisoned> {
        join_under(c, form, values, Expectation::None)
    }

    /// `join_all` under the expectation `expected`.
    fn join_under(
        c: &mut Checker<'_>,
        form: JoinForm,
        values: &mut [Checked],
        expected: Expectation,
    ) -> Result<Type, Poisoned> {
        let mut members = whole(values);
        c.join_form(form, heading(at(0)), &mut members, expected, None)
    }

    /// Each of `values` as a whole member.
    fn whole(values: &mut [Checked]) -> Vec<JoinMember<'_>> {
        (values.iter_mut())
            .map(|value| JoinMember {
                value,
                at: Place::Whole,
                label: at(0),
            })
            .collect()
    }

    /// `e` with each type in it as its key, so that two runs compare. `each_type_mut` walks the
    /// whole tree.
    fn keyed(c: &Checker<'_>, e: &mut tir::Expr) {
        e.each_type_mut(&mut |ty| *ty = c.table.key(*ty));
    }

    /// The text of a member after its join, with each type as its key.
    fn shown(c: &Checker<'_>, value: &Checked) -> String {
        match value {
            Ok(e) => {
                let mut e = e.clone();
                keyed(c, &mut e);
                format!("{e:?}")
            }
            Err(poisoned) => format!("poisoned {:?}", poisoned.ty.map(|ty| c.table.key(ty))),
        }
    }

    /// Each order of `0..n`.
    pub(super) fn orders(n: usize) -> Vec<Vec<usize>> {
        if n == 0 {
            return vec![Vec::new()];
        }
        let mut all = Vec::new();
        for shorter in orders(n - 1) {
            for i in 0..n {
                let mut order = shorter.clone();
                order.insert(i, n - 1);
                all.push(order);
            }
        }
        all
    }

    /// Joins the members that `build` makes, in each order, and asserts that each order gives the
    /// same type and the same members. Gives the type as its key, or `None` for poison.
    fn each_order(
        src: &str,
        params: &str,
        build: fn(&mut Checker<'_>, &[Type]) -> Vec<Checked>,
    ) -> Option<Type> {
        let count = with_params(src, params, |c, types| build(c, types).len());
        let mut first: Option<(Option<Type>, Vec<String>)> = None;
        for order in orders(count) {
            let run = with_params(src, params, |c, types| {
                let mut made: Vec<Option<Checked>> =
                    build(c, types).into_iter().map(Some).collect();
                let mut values: Vec<Checked> = (order.iter())
                    .map(|&i| made[i].take().expect("each index should come once"))
                    .collect();
                let ty = join_all(c, JoinForm::If, &mut values).ok();
                let mut shown_in_order = vec![String::new(); count];
                for (value, &i) in values.iter().zip(&order) {
                    shown_in_order[i] = shown(c, value);
                }
                (ty.map(|ty| c.table.key(ty)), shown_in_order)
            });
            match &first {
                Some(first) => assert!(
                    *first == run,
                    "the order {order:?} should give the same join"
                ),
                None => first = Some(run),
            }
        }
        first.and_then(|(ty, _)| ty)
    }

    #[test]
    fn join_should_take_the_type_that_each_member_converts_to() {
        // GIVEN the members `1` and `a: u8`
        // WHEN they join, in each order
        let ty = each_order("", "a: u8", |c, types| {
            vec![Ok(int(c, 1, 1)), Ok(local(types[0], 2))]
        });
        // THEN the join is `u8`
        assert_eq!(ty, Some(Type::U8));
    }

    #[test]
    fn join_should_unify_before_it_wraps() {
        // GIVEN the members `None` and `o: i64??`
        // WHEN they join, in each order
        let ty = each_order("", "", |c, _| {
            let twice = Type::optional(Type::optional(Type::I64));
            vec![Ok(none(c, 1)), Ok(local(twice, 2))]
        });
        // THEN the join is `i64??`: `None` takes the type by unification, with no wrap
        assert_eq!(ty, Some(Type::optional(Type::optional(Type::I64))));
        with_params("", "", |c, _| {
            let twice = Type::optional(Type::optional(Type::I64));
            let mut values = vec![Ok(none(c, 1)), Ok(local(twice, 2))];
            assert!(join_all(c, JoinForm::If, &mut values).is_ok());
            let Ok(first) = &values[0] else {
                panic!("`None` should stay a value");
            };
            assert!(matches!(first.kind, ExprKind::None));
            assert_eq!(c.table.resolve(first.ty), twice);
        });
    }

    #[test]
    fn join_should_add_the_optional_candidate() {
        // GIVEN the members `s: any Seen` and `Some(Eye(v: 1))`
        // WHEN they join, in each order
        let ty = each_order(SEEN, "s: any Seen, e: Eye", |_, types| {
            vec![Ok(local(types[0], 1)), Ok(some(eye(types[1], 2)))]
        });
        // THEN the join is `(any Seen)?`, which no member has: rule A adds it
        let seen = with_params(SEEN, "s: any Seen", |_, types| types[0]);
        assert_eq!(ty, Some(Type::optional(seen)));
    }

    #[test]
    fn join_should_convert_a_made_value_only_at_its_top() {
        // GIVEN the members `xs: [i64]` and `[m]`, where `m: i64?`
        with_params("", "", |c, _| {
            let optional = Type::optional(Type::I64);
            let mut values = vec![
                Ok(local(Type::array(Type::I64), 1)),
                Ok(array(vec![local(optional, 2)], optional, 3)),
            ];
            // WHEN they join
            let joined = join_all(c, JoinForm::Array, &mut values);
            // THEN no candidate takes both: `xs` is made, so its elements do not wrap
            let Err(poisoned) = joined else {
                panic!("the join should be a conflict");
            };
            assert_eq!(poisoned.ty, None);
        });
    }

    #[test]
    fn join_should_leave_out_never_and_untyped_poison() {
        with_params("", "a: u8", |c, types| {
            // GIVEN a `never` member and `a: u8`
            let mut values = vec![Ok(local(Type::NEVER, 1)), Ok(local(types[0], 2))];
            // WHEN they join, THEN the join is `u8`
            assert_eq!(join_all(c, JoinForm::If, &mut values).ok(), Some(Type::U8));
            // GIVEN only `never` members, THEN the join is `never`
            let mut values = vec![Ok(local(Type::NEVER, 1)), Ok(local(Type::NEVER, 2))];
            assert_eq!(
                join_all(c, JoinForm::If, &mut values).ok(),
                Some(Type::NEVER)
            );
            // GIVEN an untyped poisoned member and `a: u8`, THEN the join is poison of type `u8`
            let poisoned = Poisoned::untyped(c.proof(at(3)));
            let mut values = vec![Err(poisoned), Ok(local(types[0], 2))];
            let joined = join_all(c, JoinForm::If, &mut values);
            assert_eq!(
                joined.err().and_then(|poisoned| poisoned.ty),
                Some(Type::U8)
            );
            // GIVEN an untyped poisoned member and `1`, which states no type
            let poisoned = Poisoned::untyped(c.proof(at(3)));
            let mut values = vec![Err(poisoned), Ok(int(c, 1, 4))];
            // THEN the join is untyped poison, and the literal stays open
            let Err(poisoned) = join_all(c, JoinForm::If, &mut values) else {
                panic!("the join should be poisoned");
            };
            assert_eq!(poisoned.ty, None);
            let Ok(literal) = &values[1] else {
                panic!("the literal should stay a value");
            };
            assert!(c.table.resolve(literal.ty).holds_infer());
        });
    }

    /// The type of `T` when `first` is an argument at a parameter `[T]` and `second` an argument
    /// at a parameter `T`.
    fn join_at_array_place(
        src: &str,
        params: &str,
        build: fn(&mut Checker<'_>, &[Type]) -> (Checked, Checked),
    ) -> Option<Type> {
        with_params(src, params, |c, types| {
            let hole = Symbol::intern("T");
            let place = Type::array(Type::intern(TypeKind::Param(hole, MARK_INDEX)));
            let (mut first, mut second) = build(c, types);
            let mut members = [
                JoinMember {
                    value: &mut first,
                    at: Place::Part {
                        place,
                        argument: at(2),
                    },
                    label: at(2),
                },
                JoinMember {
                    value: &mut second,
                    at: Place::Whole,
                    label: at(3),
                },
            ];
            let form = JoinForm::Call { param: hole };
            let ty = c.join_form(form, heading(at(0)), &mut members, Expectation::None, None);
            ty.ok().map(|ty| c.table.resolve(ty))
        })
    }

    #[test]
    fn join_should_take_the_part_of_a_made_argument_as_a_candidate() {
        // GIVEN `opts: [u8?]` at a parameter `[T]` and the literal `3` at `T`
        // WHEN they join at `T`
        let ty = join_at_array_place("", "opts: [u8?]", |c, types| {
            (Ok(local(types[0], 2)), Ok(int(c, 3, 3)))
        });
        // THEN `T` is `u8?`, the part of the argument
        assert_eq!(ty, Some(Type::optional(Type::U8)));
        // GIVEN `es: [any Seen]` at `[T]` and `Eye(v: 1)` at `T`, THEN `T` is `any Seen`
        let ty = join_at_array_place(SEEN, "es: [any Seen], e: Eye", |_, types| {
            (Ok(local(types[0], 2)), Ok(eye(types[1], 3)))
        });
        let seen = with_params(SEEN, "s: any Seen", |_, types| types[0]);
        assert_eq!(ty, Some(seen));
    }

    #[test]
    fn join_should_take_the_part_of_an_argument() {
        with_params("", "a: u8", |c, types| {
            // GIVEN the arguments `[1]` at a parameter `[T]` and `a: u8` at a parameter `T`
            let hole = Symbol::intern("T");
            let place = Type::array(Type::intern(TypeKind::Param(hole, MARK_INDEX)));
            let literal = int(c, 1, 1);
            let element = literal.ty;
            let mut first = Ok(array(vec![literal], element, 2));
            let mut second = Ok(local(types[0], 3));
            let mut members = [
                JoinMember {
                    value: &mut first,
                    at: Place::Part {
                        place,
                        argument: at(2),
                    },
                    label: at(2),
                },
                JoinMember {
                    value: &mut second,
                    at: Place::Whole,
                    label: at(3),
                },
            ];
            // WHEN they join at `T`
            let form = JoinForm::Call { param: hole };
            let ty = c.join_form(form, heading(at(0)), &mut members, Expectation::None, None);
            // THEN `T` is `u8`, and the element of the array is `u8`
            assert_eq!(ty.ok(), Some(Type::U8));
            assert_eq!(c.table.resolve(element), Type::U8);
        });
    }

    #[test]
    fn rejected_candidate_should_leave_no_binding() {
        with_params("", "s: string", |c, types| {
            // GIVEN the members `None`, `s: string` and `t: i64?`, which no candidate takes: a
            // candidate binds the payload of `None` before a later member rejects it
            let mut values = vec![
                Ok(none(c, 1)),
                Ok(local(types[0], 2)),
                Ok(local(Type::optional(Type::I64), 3)),
            ];
            // WHEN they join
            let joined = join_all(c, JoinForm::If, &mut values);
            // THEN the join is a conflict, and the payload of `None` is still open
            assert!(joined.is_err());
            let Ok(first) = &values[0] else {
                panic!("`None` should stay a value");
            };
            assert!(c.table.resolve(first.ty).holds_infer());
        });
    }

    #[test]
    fn join_should_test_one_candidate_per_distinct_type() {
        with_params("", "a: u8", |c, types| {
            // GIVEN 1000 members `a: u8` and one `None`
            let mut values: Vec<Checked> = (0..1000).map(|n| Ok(local(types[0], n))).collect();
            values.push(Ok(none(c, 1000)));
            CANDIDATES.set(0);
            SETTLED.set(0);
            // WHEN they join
            let ty = join_all(c, JoinForm::Array, &mut values);
            // THEN the join is `u8?`, it tests `u8`, `u8?` and the type of `None` once each, and
            // it settles each member once
            assert_eq!(ty.ok(), Some(Type::optional(Type::U8)));
            assert!(
                CANDIDATES.get() <= 3,
                "the join tested {} candidates",
                CANDIDATES.get()
            );
            assert_eq!(SETTLED.get(), 1001);
        });
    }

    /// The steps of the join of `[if true { a } else { b }, ...]`, `n` times, with `ys: [i64?]`,
    /// and how many value positions it settled.
    fn ifs_in_an_array(n: usize) -> (usize, usize) {
        with_params("", "", |c, _| {
            let ifs: Vec<tir::Expr> = (0..n)
                .map(|i| if_else(local(Type::I64, i), local(Type::I64, i), Type::I64, i))
                .collect();
            let optional = Type::optional(Type::I64);
            let mut values = vec![
                Ok(array(ifs, Type::I64, n)),
                Ok(local(Type::array(optional), n + 1)),
            ];
            VALUE_STEPS.set(0);
            SETTLED.set(0);
            let ty = join_all(c, JoinForm::Array, &mut values);
            assert_eq!(ty.ok(), Some(Type::array(optional)));
            (VALUE_STEPS.get(), SETTLED.get())
        })
    }

    #[test]
    fn settle_should_visit_each_value_position_once() {
        // GIVEN an array of `n` ifs whose branches are `i64`, and `ys: [i64?]`
        // WHEN they join to `[i64?]`
        let runs = [
            ifs_in_an_array(100),
            ifs_in_an_array(200),
            ifs_in_an_array(300),
        ];
        // THEN the settle takes the array, each `if`, each branch and `ys` once
        for (run, n) in runs.iter().zip([100, 200, 300]) {
            assert_eq!(run.1, 3 * n + 2);
        }
        // AND the walk of the value positions is linear
        assert_near_linear(runs[0].0, runs[1].0, runs[2].0);
    }

    #[test]
    fn settle_should_wrap_at_the_value_position() {
        with_params("", "", |c, _| {
            // GIVEN the members `[m]`, where `m: i64?`, and `[1]`
            let optional = Type::optional(Type::I64);
            let literal = int(c, 1, 3);
            let element = literal.ty;
            let mut values = vec![
                Ok(array(vec![local(optional, 1)], optional, 2)),
                Ok(array(vec![literal], element, 4)),
            ];
            // WHEN they join
            let ty = join_all(c, JoinForm::Array, &mut values);
            // THEN the join is `[i64?]`, and the `1` in the array is wrapped where it stands
            assert_eq!(ty.ok(), Some(Type::array(optional)));
            let Ok(second) = &values[1] else {
                panic!("the array should stay a value");
            };
            assert_eq!(c.table.resolve(second.ty), Type::array(optional));
            let ExprKind::ArrayLit(elements) = &second.kind else {
                panic!("the member should stay an array literal");
            };
            let ExprKind::Wrap(inner) = &elements[0].kind else {
                panic!("the element should be wrapped");
            };
            assert_eq!(c.table.resolve(elements[0].ty), optional);
            assert!(matches!(inner.kind, ExprKind::Int(1)));
            assert_eq!(c.table.resolve(inner.ty), Type::I64);
        });
    }

    #[test]
    fn apply_should_report_e0043_for_an_if_with_no_else() {
        with_params("", "", |c, _| {
            // GIVEN the members `if true { print(0) }`, with no `else`, and `o: unit?`
            let optional = Type::optional(Type::UNIT);
            let mut values = vec![Ok(if_without_else(1)), Ok(local(optional, 2))];
            // WHEN they join to `unit?`, which wraps the `if`
            let ty = join_all(c, JoinForm::If, &mut values);
            // THEN the `if` has no value to wrap: E0043 at the `if`, and the member is poison of
            // the join's type
            assert_eq!(ty.ok(), Some(optional));
            let found: Vec<(Code, Span)> = (c.diagnostics.iter())
                .map(|d| (d.code(), d.span()))
                .collect();
            assert_eq!(found, vec![(Code::NoValue, at(1))]);
            let Err(poisoned) = &values[0] else {
                panic!("the `if` should be poisoned");
            };
            assert_eq!(poisoned.ty, Some(optional));
        });
    }

    /// The spans of the diagnostics of the unused names of `SEEN`, after `f` runs in `main`.
    fn unused_after(f: fn(&mut Checker<'_>, &[Type])) -> Vec<Span> {
        with_params(SEEN, "s: any Seen, e: Eye", |c, types| {
            f(c, types);
            c.unused()
                .iter()
                .map(crate::diagnostic::Diagnostic::span)
                .collect()
        })
    }

    #[test]
    fn apply_should_record_the_used_interface() {
        let seen = SEEN.find("Seen").expect("SEEN should name `Seen`");
        let seen = Span::new(seen, seen + "Seen".len());
        // GIVEN `main`, which uses no name, THEN `Seen` is unused
        assert!(unused_after(|_, _| {}).contains(&seen));
        // GIVEN `main`, with the join of `s: any Seen` and `Eye(v: 1)`, which converts `Eye`
        // WHEN the unused names are found
        let spans = unused_after(|c, types| {
            let mut values = vec![Ok(local(types[0], 1)), Ok(eye(types[1], 2))];
            let ty = join_all(c, JoinForm::If, &mut values);
            assert_eq!(ty.ok(), Some(types[0]));
        });
        // THEN `Seen` is used
        assert!(!spans.contains(&seen));
    }

    #[test]
    fn required_type_should_decide_a_join_with_untyped_poison() {
        with_params("", "", |c, _| {
            // GIVEN untyped poison (`nope`) and the literal `300`, under the required type `u8`
            let poisoned = Poisoned::untyped(c.proof(at(1)));
            let mut values = vec![Err(poisoned), Ok(int(c, 300, 2))];
            // WHEN they join
            let required = Expectation::Required(Type::U8, Origin::None);
            let joined = join_under(c, JoinForm::If, &mut values, required);
            // THEN the literal meets `u8`: E0040 at `300`, and the join is poison of type `u8`
            let Err(poisoned) = joined else {
                panic!("the join should be poisoned");
            };
            assert_eq!(poisoned.ty, Some(Type::U8));
            let found: Vec<(Code, Span)> = (c.diagnostics.iter())
                .map(|d| (d.code(), d.span()))
                .collect();
            assert_eq!(found, vec![(Code::IntegerDoesNotFit, at(2))]);
        });
    }

    #[test]
    fn join_should_add_the_optional_candidate_of_a_nested_member() {
        // GIVEN the members `Some(Eye(v: 1))` and `if d { s } else { Eye(v: 1) }`, with
        // `s: any Seen`
        // WHEN they join, in each order
        let ty = each_order(SEEN, "s: any Seen, e: Eye", |c, types| {
            let open = c.table.fresh(Kind::Any, at(3));
            let nested = if_else(local(types[0], 3), eye(types[1], 4), open, 5);
            vec![Ok(some(eye(types[1], 1))), Ok(nested)]
        });
        // THEN the join is `(any Seen)?`: the nested member `s` gives `?`-added candidate
        let seen = with_params(SEEN, "s: any Seen", |_, types| types[0]);
        assert_eq!(ty, Some(Type::optional(seen)));
    }

    #[test]
    fn apply_should_report_e0043_for_a_nested_if_with_no_else() {
        with_params("", "", |c, _| {
            // GIVEN the members `if c { None } else { if d { print(0) } }` and `o: unit?`
            let optional = Type::optional(Type::UNIT);
            let open = c.table.fresh(Kind::Any, at(3));
            let outer = if_else(none(c, 1), if_without_else(2), open, 3);
            let mut values = vec![Ok(outer), Ok(local(optional, 4))];
            // WHEN they join to `unit?`, which wraps the inner `if`
            let ty = join_all(c, JoinForm::If, &mut values);
            // THEN one E0043 at the inner `if`, and the outer member is poison of type `unit?`
            assert_eq!(ty.ok(), Some(optional));
            let found: Vec<(Code, Span)> = (c.diagnostics.iter())
                .map(|d| (d.code(), d.span()))
                .collect();
            assert_eq!(found, vec![(Code::NoValue, at(2))]);
            let Err(poisoned) = &values[0] else {
                panic!("the outer `if` should be poisoned");
            };
            assert_eq!(poisoned.ty, Some(optional));
        });
    }

    /// `Some(1)`, `a: u8` and `None`.
    fn some_a_none(c: &mut Checker<'_>, types: &[Type]) -> Vec<Checked> {
        vec![
            Ok(some(int(c, 1, 1))),
            Ok(local(types[0], 2)),
            Ok(none(c, 3)),
        ]
    }

    #[test]
    fn candidate_tests_should_leave_the_table_as_it_was() {
        // GIVEN the members `Some(1)`, `a: u8` and `None`
        // WHEN they join, in each order
        let ty = each_order("", "a: u8", some_a_none);
        // THEN the join is `u8?`
        assert_eq!(ty, Some(Type::optional(Type::U8)));
        with_params("", "a: u8", |c, types| {
            let mut values = some_a_none(c, types);
            let before: Vec<String> = values.iter().map(|value| shown(c, value)).collect();
            // WHEN only the candidates are tested
            let mut members = whole(&mut values);
            let joined = c.join_members(&mut members, None, None, None);
            assert!(matches!(joined, Ok(Joined::Typed(_))));
            // THEN each type in the members is as it was: no test left a binding
            let after: Vec<String> = values.iter().map(|value| shown(c, value)).collect();
            assert_eq!(before, after);
        });
    }

    /// The steps of the leaves and value positions of the join of `[c]`, where `c` is an `else if`
    /// chain of `n` branches, with `xs: [i64]`.
    fn else_if_chain(n: usize) -> usize {
        with_params("", "", |c, _| {
            let mut chain = local(Type::I64, 0);
            for i in 1..n {
                chain = if_else(local(Type::I64, i), chain, Type::I64, i);
            }
            let mut values = vec![
                Ok(array(vec![chain], Type::I64, n)),
                Ok(local(Type::array(Type::I64), n + 1)),
            ];
            LEAF_STEPS.set(0);
            VALUE_STEPS.set(0);
            let ty = join_all(c, JoinForm::Array, &mut values);
            assert_eq!(ty.ok(), Some(Type::array(Type::I64)));
            LEAF_STEPS.get() + VALUE_STEPS.get()
        })
    }

    #[test]
    fn leaves_of_an_else_if_chain_should_cost_linear_steps() {
        // GIVEN an `else if` chain of n, 2n and 3n branches in an array, beside `xs: [i64]`
        // WHEN each joins
        let steps = [else_if_chain(100), else_if_chain(200), else_if_chain(300)];
        // THEN the leaves cost one step per array around them, so the steps grow linearly
        assert_near_linear(steps[0], steps[1], steps[2]);
    }

    /// How many types `fill` fills in `P_level<T>`, where `P_0<T>` is `T` and `P_n<T>` is
    /// `Two<P_(n-1)<T>, P_(n-1)<T>>`.
    fn filled_at(level: usize) -> usize {
        let two = "struct Two<A, B> {\n    a: A\n    b: B\n}\n";
        with_params(two, "t: Two<i64, i64>", |_, types| {
            let TypeKind::Struct(two, _) = types[0].kind() else {
                panic!("the parameter should be the struct `Two`");
            };
            let hole = Symbol::intern("T");
            let mut place = Type::intern(TypeKind::Param(hole, MARK_INDEX));
            for _ in 0..level {
                place = Type::intern(TypeKind::Struct(two, TypeArgs::intern(&[place, place])));
            }
            FILLED.set(0);
            let filled = fill(place, hole, Type::I64);
            assert!(!filled.has_param());
            FILLED.get()
        })
    }

    #[test]
    fn fill_should_fill_each_distinct_type_once() {
        // GIVEN `P_n<T>`, which holds n + 1 distinct types and 2^n paths to `T`
        // WHEN `fill` puts `i64` for `T` at levels 10 and 12
        // THEN it fills each distinct type once
        assert_eq!(filled_at(10), 11);
        assert_eq!(filled_at(12), 13);
    }
}
