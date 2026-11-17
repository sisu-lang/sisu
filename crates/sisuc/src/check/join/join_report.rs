//! The report of a join. A join with no required type and no candidate reports one E0032 at its
//! head. It has a label for each group of members by type, and the fix that fits.
//!
//! A join under a required type reports what that type does not take. When the members agree and
//! only the required type differs, it is one E0032 at the whole join. Else each member that does
//! not convert gets a directed E0032.
//!
//! A `unit` member that no type takes is E0043. A `break` with no value, in a loop that gives one,
//! is E0043 `BareBreak`.

use super::{
    Checker, Conflict, ExprKind, JoinForm, JoinMember, Joined, Place, Reported, Site, Span, Symbol,
    Type, TypeArgs, TypeKind, ensure_stack, member_type, part_at, target, tir, value_positions,
};
use crate::check::expectation::{Origin, own_name};
use crate::check::expr::{callee, takes_unit, valueless};
use crate::check::{beyond_one_layer, number_conversion};
use crate::diagnostic::{
    Conversion, DiagnosticKind, Expected, JoinFix, JoinFormName, MismatchFix, NoValueCause,
};
use crate::tir::InterfaceId;

/// What the leaves of a value are, as far as `unit` goes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Units {
    /// Each leaf never finishes.
    Skipped,
    /// No leaf is `unit`.
    Free,
    /// Some leaves are `unit`, and some are not.
    Mixed,
    /// Each leaf is `unit`.
    Only,
}

/// A group of leaves of one type, as the report shows it.
struct Shown {
    ty: Type,
    leaves: Vec<Span>,
    whole: Vec<Span>,
}

/// Where the member itself, not a leaf inside it, is reported.
#[derive(Clone, Copy)]
struct Top {
    /// The member converts by unification only: no fix wraps it.
    fixed: bool,
    /// The member is a part of its argument: the type of the place, and the type parameter.
    part: Option<(Type, Symbol)>,
    /// The type that the join requires.
    required: Type,
}

/// A span's place in the source, so that the labels of a report have one order.
fn place(span: Span) -> (usize, usize) {
    (span.start(), span.end())
}

/// The conversion that widens `got` to `want` with no loss: a `u8` or a `char` to an `i64` or an
/// `f64`. A conversion that can fail, and an `i64` to an `f64`, are no such conversion.
fn widening(want: Type, got: Type) -> Option<(&'static str, Option<&'static str>)> {
    if matches!(want.kind(), TypeKind::Optional(_)) || (want == Type::F64 && got == Type::I64) {
        return None;
    }
    number_conversion(want, got)
}

impl Checker<'_> {
    /// Reports the join `site` that has no candidate and no required type: E0043 at each `unit`
    /// leaf when `unit` is one group beside another; else one E0032.
    pub(super) fn report_conflict(
        &mut self,
        site: Site,
        binding: Option<Span>,
        members: &mut [JoinMember<'_>],
        conflict: &Conflict,
    ) -> Reported {
        let unit = (conflict.groups.iter()).any(|group| group.ty == Type::UNIT);
        let beside = (conflict.groups.iter()).any(|group| group.ty != Type::UNIT)
            || !conflict.parts.is_empty();
        if unit && beside {
            let mut units = Vec::new();
            for member in members.iter_mut() {
                self.member_units(member, None, &mut units);
            }
            return self.no_values(units, site.head);
        }
        let kind = self.conflict_kind(site, binding, conflict);
        self.error(kind)
    }

    /// Reports the join `site` under the required type, which a member does not take.
    pub(super) fn report_required(
        &mut self,
        site: Site,
        (required, origin): (Type, Origin),
        members: &mut [JoinMember<'_>],
    ) -> Reported {
        let taking: Vec<usize> = (0..members.len())
            .filter(|&i| member_type(members[i].value).is_some_and(|ty| ty != Type::NEVER))
            .collect();
        let mut failing = Vec::new();
        for &i in &taking {
            if self.fails(site, i, &mut members[i], required) {
                failing.push(i);
            }
        }
        // A loop with a bare `break` gives `unit`: the first value that is not `unit` is the label.
        if let Origin::BareBreak { at } = origin {
            let valued = failing.first().map_or(site.head, |&i| members[i].label);
            return self.error(DiagnosticKind::BareBreak { at, valued });
        }
        if !takes_unit(required) {
            let want = Some(self.table.default_open(required));
            let (mut units, mut all) = (Vec::new(), true);
            for &i in &taking {
                let found = self.member_units(&mut members[i], want, &mut units);
                all &= matches!(found, Units::Only | Units::Skipped);
            }
            if !units.is_empty() {
                // The branches that each give `unit` are one mistake at the whole `if` or `match`.
                if all && matches!(site.form, JoinForm::If | JoinForm::Match) {
                    units = vec![(site.head, NoValueCause::Plain)];
                }
                return self.no_values(units, site.head);
            }
        }
        // The members agree with each other: only the required type differs.
        if taking.len() >= 2
            && let Ok(Joined::Typed(got)) = self.join_members(members, site.hole, site.fixed, None)
            && self.table.resolve(got) != required
        {
            let (mut want, mut got) = (required, self.table.default_open(got));
            if matches!(site.form, JoinForm::Array) {
                (want, got) = (Type::array(want), Type::array(got));
            }
            return self.directed(site.head, (want, got), origin, None, false);
        }
        let mut last = None;
        for &i in &failing {
            let member = &mut members[i];
            let target = target(member.at, site.hole, required);
            let top = Top {
                fixed: site.fixed == Some(i),
                part: match (member.at, site.hole) {
                    (Place::Part { place, .. }, Some(hole)) => Some((place, hole)),
                    _ => None,
                },
                required,
            };
            let label = member.label;
            last = match &mut *member.value {
                Ok(e) => self.report_leaves(e, target, origin, Some(top)),
                Err(poisoned) => {
                    let got = self.table.default_open(poisoned.ty.unwrap_or(required));
                    Some(self.directed(label, (required, got), origin, None, false))
                }
            }
            .or(last);
        }
        let first = failing.first().and_then(|&i| member_type(members[i].value));
        last.unwrap_or_else(|| match first {
            Some(got) => {
                let got = self.table.default_open(got);
                self.directed(site.head, (required, got), origin, None, false)
            }
            None => self.proof(site.head),
        })
    }

    /// Whether `member` does not convert to `ty` as the join `site` takes it. It binds nothing.
    fn fails(&mut self, site: Site, i: usize, member: &mut JoinMember<'_>, ty: Type) -> bool {
        let target = target(member.at, site.hole, ty);
        let snapshot = self.table.snapshot();
        let wraps = self.converts(member.value, target, site.fixed == Some(i));
        self.table.rollback(snapshot);
        wraps.is_none()
    }

    /// Whether the value position `e` does not convert to `target`. It binds nothing.
    fn fails_at(&mut self, e: &mut tir::Expr, target: Type) -> bool {
        let snapshot = self.table.snapshot();
        let wraps = self.converts_at(e, target);
        self.table.rollback(snapshot);
        wraps.is_none()
    }

    /// Reports each leaf of `e` that does not convert to `target`: the leaf, against the part of
    /// `target` at its position. `top` is set when `e` is the member itself.
    fn report_leaves(
        &mut self,
        e: &mut tir::Expr,
        target: Type,
        origin: Origin,
        top: Option<Top>,
    ) -> Option<Reported> {
        if e.ty == Type::NEVER {
            return None;
        }
        ensure_stack(|| {
            if let Some((part, _)) = self.enter(e, target) {
                let mut last = None;
                value_positions(e, &mut |child| {
                    if self.fails_at(child, part) {
                        last = self.report_leaves(child, part, origin, None).or(last);
                    }
                });
                return last;
            }
            let convertible = !matches!(top, Some(Top { fixed: true, .. }))
                && !matches!(top, Some(Top { part: Some(_), .. }))
                && !matches!(
                    e.kind,
                    ExprKind::If { .. } | ExprKind::IfSome { .. } | ExprKind::Match { .. }
                );
            let (want, got) = match top {
                Some(Top {
                    part: Some((place, hole)),
                    required,
                    ..
                }) => {
                    let got = part_at(self.table.resolve(place), hole, self.table.resolve(e.ty));
                    (required, got.unwrap_or(e.ty))
                }
                _ => (target, e.ty),
            };
            let (want, got) = (self.table.default_open(want), self.table.default_open(got));
            let fix = (convertible.then(|| number_conversion(want, got)).flatten()).map(
                |(to, through)| Conversion {
                    to,
                    through,
                    operand: e.span,
                },
            );
            let conversion = matches!(got.kind(), TypeKind::Optional(p) if p == want)
                && matches!(e.kind, ExprKind::Convert(_));
            Some(self.directed(e.span, (want, got), origin, fix, conversion))
        })
    }

    /// "expected `want`, found `got`" at `at`, as a mismatch with a written type reports it.
    fn directed(
        &mut self,
        at: Span,
        (want, got): (Type, Type),
        origin: Origin,
        fix: Option<Conversion>,
        conversion: bool,
    ) -> Reported {
        self.error(DiagnosticKind::Mismatch {
            want: Expected::Types(vec![self.want_name(at, want)]),
            got: self.type_name(got),
            at,
            origin: self.origin_fact(origin),
            fix: fix.map(MismatchFix::Convert),
            conversion,
            layers: beyond_one_layer(want, got),
        })
    }

    /// The `unit` leaves of `member`, pushed to `out`, and what its leaves are.
    fn member_units(
        &self,
        member: &mut JoinMember<'_>,
        want: Option<Type>,
        out: &mut Vec<(Span, NoValueCause)>,
    ) -> Units {
        match &mut *member.value {
            Ok(e) => self.units(e, want, out),
            Err(poisoned) if poisoned.ty == Some(Type::UNIT) => {
                out.push((member.label, NoValueCause::Plain));
                Units::Only
            }
            Err(poisoned) if poisoned.ty == Some(Type::NEVER) => Units::Skipped,
            Err(_) => Units::Free,
        }
    }

    /// The `unit` leaves of `e`, pushed to `out` with the cause of each. An `if` or a `match`
    /// whose leaves are each `unit` is one leaf at its head.
    fn units(
        &self,
        e: &mut tir::Expr,
        want: Option<Type>,
        out: &mut Vec<(Span, NoValueCause)>,
    ) -> Units {
        if e.ty == Type::NEVER {
            return Units::Skipped;
        }
        let start = out.len();
        // How many children are free, mixed and only `unit`.
        let mut seen = [0_usize; 3];
        let mut inner = false;
        ensure_stack(|| {
            value_positions(e, &mut |child| {
                inner = true;
                match self.units(child, want, out) {
                    Units::Skipped => {}
                    Units::Free => seen[0] += 1,
                    Units::Mixed => seen[1] += 1,
                    Units::Only => seen[2] += 1,
                }
            });
        });
        if !inner {
            if e.ty != Type::UNIT {
                return Units::Free;
            }
            out.push((e.span, self.unit_cause(e, want)));
            return Units::Only;
        }
        let [free, mixed, only] = seen;
        if free == 0 && mixed == 0 {
            if only == 0 {
                return Units::Skipped;
            }
            if matches!(
                e.kind,
                ExprKind::If { .. } | ExprKind::IfSome { .. } | ExprKind::Match { .. }
            ) {
                out.truncate(start);
                out.push((e.span, self.unit_cause(e, want)));
            }
            return Units::Only;
        }
        if only == 0 && mixed == 0 {
            Units::Free
        } else {
            Units::Mixed
        }
    }

    /// Why the `unit` value `e` has no value, as `no_value_cause` says it for a value in the
    /// source.
    fn unit_cause(&self, e: &tir::Expr, want: Option<Type>) -> NoValueCause {
        match &e.kind {
            ExprKind::If {
                else_block: None, ..
            }
            | ExprKind::IfSome {
                else_block: None, ..
            } => {
                let if_let = matches!(e.kind, ExprKind::IfSome { .. });
                let optional = want.is_some_and(|ty| matches!(ty.kind(), TypeKind::Optional(_)));
                if !optional && valueless(e).is_none() {
                    NoValueCause::UnitIf {
                        if_let,
                        want: want.map(|ty| self.type_name(ty)),
                    }
                } else {
                    NoValueCause::NoElse {
                        if_let,
                        returning: false,
                    }
                }
            }
            ExprKind::Dbg { operand, .. } => self.unit_cause(operand, want),
            ExprKind::Call { .. } | ExprKind::MutCall { .. } | ExprKind::Block(_) => {
                let Some(func) = callee(e) else {
                    return NoValueCause::Plain;
                };
                let signature = &self.signatures[func.0];
                let declared =
                    (!signature.span.in_prelude()).then(|| match self.written_ret(func) {
                        Some((at, shown)) => (at, Some(shown)),
                        None => (signature.span, None),
                    });
                NoValueCause::Call {
                    name: own_name(&signature.name).to_string(),
                    declared,
                }
            }
            _ => NoValueCause::Plain,
        }
    }

    /// Reports each of `units` as E0043. `head` stands in when there is none.
    fn no_values(&mut self, units: Vec<(Span, NoValueCause)>, head: Span) -> Reported {
        let mut last = None;
        for (at, cause) in units {
            last = Some(self.no_value(at, cause));
        }
        last.unwrap_or_else(|| self.proof(head))
    }

    /// The E0032 of a join that has no candidate: its groups in the order of the source, the
    /// parts of its arguments, the declared interfaces that fit each member, and the fix.
    pub(super) fn conflict_kind(
        &mut self,
        site: Site,
        binding: Option<Span>,
        conflict: &Conflict,
    ) -> DiagnosticKind {
        // A literal that is still open takes the type of a group that holds a number.
        let held =
            (conflict.groups.iter()).any(|g| matches!(g.ty, Type::U8 | Type::I64 | Type::F64));
        let mut shown: Vec<Shown> = Vec::new();
        for group in &conflict.groups {
            let open = matches!(group.ty.kind(), TypeKind::Infer(_));
            if held && open && self.table.default_open(group.ty) != group.ty {
                continue;
            }
            let ty = self.table.default_open(group.ty);
            let at = if let Some(at) = shown.iter().position(|one| one.ty == ty) {
                at
            } else {
                shown.push(Shown {
                    ty,
                    leaves: Vec::new(),
                    whole: Vec::new(),
                });
                shown.len() - 1
            };
            shown[at].leaves.extend(&group.leaves);
            shown[at].whole.extend(&group.whole);
        }
        for one in &mut shown {
            one.leaves.sort_by_key(|&span| place(span));
            one.whole.sort_by_key(|&span| place(span));
        }
        shown.sort_by_key(|one| one.leaves.first().map(|&span| place(span)));
        let mut parts: Vec<(Span, Type)> = (conflict.parts.iter())
            .map(|&(at, ty)| (at, self.table.default_open(ty)))
            .collect();
        parts.sort_by_key(|&(at, _)| place(at));
        let types: Vec<Type> = (shown.iter().map(|one| one.ty))
            .chain(parts.iter().map(|&(_, ty)| ty))
            .collect();
        let interfaces = self.fitting_interfaces(&types);
        let conversion = if shown.len() == 2 && parts.is_empty() {
            (0..2).find_map(|want| {
                let (want, got) = (&shown[want], &shown[1 - want]);
                let (to, through) = widening(want.ty, got.ty)?;
                // A value that is no whole value, such as a leaf of a nested join, has no wrap.
                (got.whole.len() == got.leaves.len()).then(|| {
                    JoinFix::Convert(
                        (got.whole.iter())
                            .map(|&operand| Conversion {
                                to,
                                through,
                                operand,
                            })
                            .collect(),
                    )
                })
            })
        } else {
            None
        };
        let fix = conversion.or_else(|| match (interfaces.as_slice(), binding) {
            ([name], Some(binding)) => {
                let ty = format!("any {name}");
                Some(JoinFix::Declare {
                    name: binding,
                    ty: match site.form {
                        JoinForm::Array => format!("[{ty}]"),
                        _ => ty,
                    },
                })
            }
            _ => None,
        });
        DiagnosticKind::JoinMismatch {
            form: self.form_name(site),
            head: site.head,
            groups: (shown.into_iter())
                .map(|one| (self.type_name(one.ty), one.leaves))
                .collect(),
            parts: (parts.into_iter())
                .map(|(at, ty)| (at, self.type_name(ty)))
                .collect(),
            interfaces,
            fix,
        }
    }

    /// The names of the interfaces that the program declares, which each of `types` conforms to.
    /// A language interface does not count, and neither does an interface with type parameters.
    fn fitting_interfaces(&mut self, types: &[Type]) -> Vec<String> {
        if types.iter().any(|ty| ty.holds_infer()) {
            return Vec::new();
        }
        let mut found = Vec::new();
        for i in 0..self.interfaces.len() {
            let info = &self.interfaces[i];
            if info.name.span.in_prelude() || info.type_params.len() != 1 {
                continue;
            }
            let name = info.name.name.to_string();
            if types
                .iter()
                .all(|&ty| self.conforms(ty, InterfaceId(i), TypeArgs::NONE))
            {
                found.push(name);
            }
        }
        found
    }

    /// The form of the join `site`, as its headline names it.
    fn form_name(&self, site: Site) -> JoinFormName {
        match site.form {
            JoinForm::If => JoinFormName::If,
            JoinForm::Match => JoinFormName::Match,
            JoinForm::Array => JoinFormName::Array,
            JoinForm::Loop => JoinFormName::Loop,
            JoinForm::Coalesce => JoinFormName::Coalesce,
            JoinForm::Operator(_) => {
                JoinFormName::Operator(self.source.text(site.head).to_string())
            }
            JoinForm::Compare => JoinFormName::Compare,
            JoinForm::Compound { .. } => {
                JoinFormName::Compound(self.source.text(site.head).to_string())
            }
            JoinForm::Call { param } => JoinFormName::Call(param.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{array, at, eye, if_else, int, local, none, orders};
    use super::super::*;
    use crate::check::expectation::Origin;
    use crate::check::infer::Kind;
    use crate::check::operator::Operator;
    use crate::check::tests::joins::with_params;
    use crate::diagnostic::{Code, Diagnostic, DiagnosticKind};
    use crate::tir::FuncId;

    /// `Seen`, with the structs `Eye` and `Lid`, which conform to it.
    const EYE_LID: &str = "interface Seen {\n    fn seen(self) -> i64\n}\nstruct Eye: Seen {\n    v: i64\n    fn seen(self) -> i64 { self.v }\n}\nstruct Lid: Seen {\n    v: i64\n    fn seen(self) -> i64 { self.v }\n}\n";

    /// `Seen` and `Heard`, which both `Eye` and `Lid` conform to.
    const EYE_LID_HEARD: &str = "interface Seen {\n    fn seen(self) -> i64\n}\ninterface Heard {\n    fn heard(self) -> i64\n}\nstruct Eye: Seen, Heard {\n    v: i64\n    fn seen(self) -> i64 { self.v }\n    fn heard(self) -> i64 { self.v }\n}\nstruct Lid: Seen, Heard {\n    v: i64\n    fn seen(self) -> i64 { self.v }\n    fn heard(self) -> i64 { self.v }\n}\n";

    /// The functions `u` and `g`, which give `unit`.
    const UNITS: &str = "fn u() {}\nfn g() {}\n";

    fn head() -> Span {
        Span::new(10, 12)
    }

    /// Each of `values` as a whole member.
    fn whole(values: &mut [Checked]) -> Vec<JoinMember<'_>> {
        (values.iter_mut())
            .map(|value| {
                let label = match value {
                    Ok(e) => e.span,
                    Err(_) => at(0),
                };
                JoinMember {
                    value,
                    at: Place::Whole,
                    label,
                }
            })
            .collect()
    }

    /// The join of `values`, each a whole member, with the head `head()`.
    fn join_at(
        c: &mut Checker<'_>,
        form: JoinForm,
        values: &mut [Checked],
        expected: Expectation,
        binding: Option<Span>,
    ) -> Result<Type, Poisoned> {
        let mut members = whole(values);
        c.join_form(form, head(), &mut members, expected, binding)
    }

    fn required(ty: Type) -> Expectation {
        Expectation::Required(ty, Origin::None)
    }

    /// The diagnostics so far.
    fn diagnostics(c: &Checker<'_>) -> Vec<Diagnostic> {
        c.diagnostics.iter().cloned().collect()
    }

    /// The code and the primary span of each diagnostic so far.
    fn found(c: &Checker<'_>) -> Vec<(Code, Span)> {
        (diagnostics(c).iter())
            .map(|d| (d.code(), d.span()))
            .collect()
    }

    /// The span of each secondary label of the only diagnostic so far.
    fn labels(c: &Checker<'_>) -> Vec<Span> {
        let [d] = diagnostics(c).try_into().unwrap_or_else(|all: Vec<_>| {
            panic!("one diagnostic should be reported, not {}", all.len())
        });
        d.secondary().iter().map(|&(span, _)| span).collect()
    }

    /// The call `f()` of the function `name`, which gives `unit`.
    fn unit_call(c: &Checker<'_>, name: &str, n: usize) -> tir::Expr {
        let func = (c.signatures.iter())
            .position(|signature| signature.name == name)
            .expect("the program should declare the function");
        let kind = ExprKind::Call {
            func: FuncId(func),
            types: TypeArgs::NONE,
            args: Vec::new(),
        };
        tir::Expr::new(kind, Type::UNIT, at(n))
    }

    fn string(n: usize) -> tir::Expr {
        tir::Expr::new(ExprKind::Str("s".to_owned()), Type::STRING, at(n))
    }

    #[test]
    fn conflict_should_put_the_primary_label_at_the_head() {
        with_params("", "a: u8, s: string", |c, types| {
            // GIVEN the members `a: u8` and `s: string`
            let mut values = vec![Ok(local(types[0], 1)), Ok(local(types[1], 2))];
            // WHEN they join, with no required type
            let joined = join_at(c, JoinForm::If, &mut values, Expectation::None, None);
            // THEN one E0032 at the head, and the join is untyped poison
            assert_eq!(found(c), vec![(Code::Mismatch, head())]);
            assert!(joined.err().is_some_and(|poisoned| poisoned.ty.is_none()));
        });
    }

    #[test]
    fn conflict_should_label_each_group_in_the_same_order_for_each_member_order() {
        let run = |order: &[usize]| {
            with_params("", "a: u8, s: string, b: bool", |c, types| {
                let mut made: Vec<Option<Checked>> =
                    (0..3).map(|i| Some(Ok(local(types[i], i + 1)))).collect();
                let mut values: Vec<Checked> = (order.iter())
                    .map(|&i| made[i].take().expect("each index should come once"))
                    .collect();
                let _ = join_at(c, JoinForm::If, &mut values, Expectation::None, None);
                labels(c)
            })
        };
        // GIVEN three members of three types, WHEN they join in each order
        // THEN the labels are the same, one at each member, in the order of the source
        for order in orders(3) {
            assert_eq!(run(&order), vec![at(1), at(2), at(3)], "order {order:?}");
        }
    }

    #[test]
    fn conflict_should_name_each_member_at_each_depth() {
        with_params(EYE_LID, "e: string, x: Eye, y: Lid", |c, types| {
            // GIVEN `if c { e } else if d { Eye(v: 1) } else { Lid(v: 2) }`, with `e: string`
            let inner_ty = c.table.fresh(Kind::Any, at(4));
            let inner = if_else(eye(types[1], 2), eye(types[2], 3), inner_ty, 4);
            let outer_ty = c.table.fresh(Kind::Any, at(5));
            let outer = if_else(local(types[0], 1), inner, outer_ty, 5);
            let mut values = vec![Ok(outer)];
            // WHEN it joins
            let _ = join_at(c, JoinForm::If, &mut values, Expectation::None, None);
            // THEN one E0032 at the head, with a label at `e`, at `Eye(v: 1)` and at `Lid(v: 2)`
            assert_eq!(found(c), vec![(Code::Mismatch, head())]);
            assert_eq!(labels(c), vec![at(1), at(2), at(3)]);
        });
    }

    #[test]
    fn part_member_should_get_the_argument_label() {
        with_params("", "names: [string], n: i64", |c, types| {
            // GIVEN `names` at a parameter `[T]` and `n` at a parameter `T`
            let hole = Symbol::intern("T");
            let place = Type::array(Type::intern(TypeKind::Param(hole, 0)));
            let (mut first, mut second) = (Ok(local(types[0], 2)), Ok(local(types[1], 3)));
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
            let joined = c.join_form(form, head(), &mut members, Expectation::None, None);
            // THEN one E0032 at the head, with one label at `names` and one at `n`
            assert!(joined.is_err());
            assert_eq!(found(c), vec![(Code::Mismatch, head())]);
            assert_eq!(labels(c), vec![at(2), at(3)]);
        });
    }

    /// The source text of `if c { a } else { b }`, with `a` at 7 and `b` at 18.
    const IF_TEXT: &str = "if c { a } else { b }";

    /// The text of `IF_TEXT` after the fix of its join of `a` and `b`, in each order of the two.
    fn fixed_text(params: &str) -> Option<String> {
        let mut texts = Vec::new();
        for order in orders(2) {
            let text = with_params("", params, |c, types| {
                let spans = [Span::new(7, 8), Span::new(18, 19)];
                let mut values: Vec<Checked> = (order.iter())
                    .map(|&i| {
                        let mut value = local(types[i], 0);
                        value.span = spans[i];
                        Ok(value)
                    })
                    .collect();
                let _ = join_at(c, JoinForm::If, &mut values, Expectation::None, None);
                let [d] = diagnostics(c).try_into().unwrap_or_else(|all: Vec<_>| {
                    panic!("one diagnostic should be reported, not {}", all.len())
                });
                match d.fixes() {
                    [] => None,
                    [fix] => {
                        assert!(!fix.safe(), "a conversion is no safe fix");
                        Some(fix.apply(IF_TEXT))
                    }
                    more => panic!("{} fixes", more.len()),
                }
            });
            texts.push(text);
        }
        assert_eq!(texts[0], texts[1], "the fix should not depend on the order");
        texts.swap_remove(0)
    }

    #[test]
    fn conversion_fix_should_widen_u8_to_i64() {
        // GIVEN `if c { a } else { b }` with `a: u8` and `b: i64`
        // WHEN the branches do not join, in each order
        // THEN the fix converts `a`, and `u8` to `f64` goes through `i64`
        assert_eq!(
            fixed_text("a: u8, b: i64").as_deref(),
            Some("if c { i64(a) } else { b }")
        );
        assert_eq!(
            fixed_text("a: u8, b: f64").as_deref(),
            Some("if c { f64(i64(a)) } else { b }")
        );
    }

    #[test]
    fn conversion_fix_should_not_exist_for_i64_and_f64() {
        // GIVEN branches of `i64` and `f64`, or of `i64` and `string`, THEN there is no fix
        assert_eq!(fixed_text("a: i64, b: f64"), None);
        assert_eq!(fixed_text("a: i64, b: string"), None);
    }

    #[test]
    fn conversion_fix_should_skip_the_assigned_side() {
        let compound = |params: &str| {
            with_params("", params, |c, types| {
                // GIVEN `s += a`, where the member `s` is the place it writes to
                let (mut s, mut a) = (local(types[0], 0), local(types[1], 0));
                (s.span, a.span) = (Span::new(0, 1), Span::new(5, 6));
                let mut values = vec![Ok(s), Ok(a)];
                let op = Operator::Plus { compound: true };
                let form = JoinForm::Compound { op, assigned: 0 };
                let mut members = whole(&mut values);
                // WHEN they join
                let joined =
                    c.join_form(form, Span::new(2, 4), &mut members, Expectation::None, None);
                assert!(joined.is_err());
                let [d] = diagnostics(c).try_into().unwrap_or_else(|all: Vec<_>| {
                    panic!("one diagnostic should be reported, not {}", all.len())
                });
                d.fixes().first().map(|fix| fix.apply("s += a"))
            })
        };
        // THEN with `s: i64` and `a: u8` the fix converts `a`
        assert_eq!(compound("s: i64, a: u8").as_deref(), Some("s += i64(a)"));
        // AND with `s: u8` and `b: i64` there is no fix: it would convert `s`
        assert_eq!(compound("s: u8, a: i64"), None);
    }

    /// The only diagnostic of the join of `eye` and `lid` (`Eye(v: 1)` and `Lid(v: 2)`), with
    /// `binding` as the name of a `let` without a type.
    fn eye_lid(src: &str, form: JoinForm, binding: Option<Span>) -> Diagnostic {
        with_params(src, "x: Eye, y: Lid", |c, types| {
            let mut values = vec![Ok(eye(types[0], 1)), Ok(eye(types[1], 2))];
            let joined = join_at(c, form, &mut values, Expectation::None, binding);
            assert!(joined.is_err());
            let [d] = diagnostics(c).try_into().unwrap_or_else(|all: Vec<_>| {
                panic!("one diagnostic should be reported, not {}", all.len())
            });
            d
        })
    }

    /// `let x = ...`, with the name at 4.
    const LET_TEXT: &str = "let x = ...";

    #[test]
    fn one_declared_interface_should_give_an_unsafe_fix_at_a_let() {
        // GIVEN `let x = if c { Eye(v: 1) } else { Lid(v: 2) }`: only `Seen` fits both, and the
        // language interface `Display` fits too
        let binding = Some(Span::new(4, 5));
        let d = eye_lid(EYE_LID, JoinForm::If, binding);
        // THEN the fix writes `: any Seen` after `x`, and it is unsafe
        let [fix] = d.fixes() else {
            panic!("one fix should be offered, not {}", d.fixes().len());
        };
        assert!(!fix.safe());
        assert_eq!(fix.apply(LET_TEXT), "let x: any Seen = ...");
        // AND an array of them writes `[any Seen]`
        let d = eye_lid(EYE_LID, JoinForm::Array, binding);
        let [fix] = d.fixes() else {
            panic!("one fix should be offered, not {}", d.fixes().len());
        };
        assert_eq!(fix.apply(LET_TEXT), "let x: [any Seen] = ...");
        // AND with no `let` it is a help with no fix
        let d = eye_lid(EYE_LID, JoinForm::If, None);
        assert_eq!(d.fixes().len(), 0);
        assert!(d.help().is_some());
    }

    #[test]
    fn two_declared_interfaces_should_give_no_fix() {
        // GIVEN `let x = if c { Eye(v: 1) } else { Lid(v: 2) }`, where `Seen` and `Heard` fit both
        let d = eye_lid(EYE_LID_HEARD, JoinForm::If, Some(Span::new(4, 5)));
        // THEN the help names them, and there is no fix
        assert_eq!(d.fixes().len(), 0);
        assert!(d.help().is_some());
    }

    #[test]
    fn unit_member_should_give_e0043_no_value() {
        with_params(UNITS, "", |c, _| {
            // GIVEN `[u(), 1]`, WHEN it joins with no required type
            let one = int(c, 1, 2);
            let mut values = vec![Ok(unit_call(c, "u", 1)), Ok(one)];
            let _ = join_at(c, JoinForm::Array, &mut values, Expectation::None, None);
            // THEN E0043 at `u()`, in place of E0032
            assert_eq!(found(c), vec![(Code::NoValue, at(1))]);
        });
        with_params(UNITS, "", |c, _| {
            // GIVEN `if c { u() } else { 1 }`, THEN the same
            let one = int(c, 1, 2);
            let mut values = vec![Ok(unit_call(c, "u", 1)), Ok(one)];
            let _ = join_at(c, JoinForm::If, &mut values, Expectation::None, None);
            assert_eq!(found(c), vec![(Code::NoValue, at(1))]);
        });
        with_params(UNITS, "", |c, _| {
            // GIVEN `let b: u8 = id(u())`: one member at a `T`
            let mut values = vec![Ok(unit_call(c, "u", 1))];
            let form = JoinForm::Call {
                param: Symbol::intern("T"),
            };
            let _ = join_at(c, form, &mut values, required(Type::U8), None);
            // THEN E0043 at `u()`
            assert_eq!(found(c), vec![(Code::NoValue, at(1))]);
        });
        with_params(UNITS, "", |c, _| {
            // GIVEN `let x: u8 = if c { u() } else { u() }`, and the same `match`
            let mut values = vec![Ok(unit_call(c, "u", 1)), Ok(unit_call(c, "u", 2))];
            let _ = join_at(c, JoinForm::If, &mut values, required(Type::U8), None);
            // THEN one E0043 at the whole `if`
            assert_eq!(found(c), vec![(Code::NoValue, head())]);
        });
        with_params(UNITS, "", |c, _| {
            let mut values = vec![Ok(unit_call(c, "u", 1)), Ok(unit_call(c, "u", 2))];
            let _ = join_at(c, JoinForm::Match, &mut values, required(Type::U8), None);
            assert_eq!(found(c), vec![(Code::NoValue, head())]);
        });
        with_params(UNITS, "", |c, _| {
            // GIVEN `let xs: [u8] = [u(), u()]`, THEN E0043 at each `u()`
            let mut values = vec![Ok(unit_call(c, "u", 1)), Ok(unit_call(c, "u", 2))];
            let _ = join_at(c, JoinForm::Array, &mut values, required(Type::U8), None);
            assert_eq!(
                found(c),
                vec![(Code::NoValue, at(1)), (Code::NoValue, at(2))]
            );
        });
        with_params(UNITS, "", |c, _| {
            // GIVEN `let x: u8 = pick(u(), u())`, THEN E0043 at each `u()`
            let mut values = vec![Ok(unit_call(c, "u", 1)), Ok(unit_call(c, "u", 2))];
            let form = JoinForm::Call {
                param: Symbol::intern("T"),
            };
            let _ = join_at(c, form, &mut values, required(Type::U8), None);
            assert_eq!(
                found(c),
                vec![(Code::NoValue, at(1)), (Code::NoValue, at(2))]
            );
        });
        with_params(UNITS, "", |c, _| {
            // GIVEN `let xs: [u8] = [if c { u() } else { u() }]`: a nested join of `unit` leaves
            let nested_ty = c.table.fresh(Kind::Any, at(3));
            let nested = if_else(unit_call(c, "u", 1), unit_call(c, "u", 2), nested_ty, 3);
            let mut values = vec![Ok(nested)];
            let _ = join_at(c, JoinForm::Array, &mut values, required(Type::U8), None);
            // THEN one E0043, at the head of the nested `if`
            assert_eq!(found(c), vec![(Code::NoValue, at(3))]);
        });
        with_params(UNITS, "", |c, _| {
            // GIVEN `let y: unit? = if c { g() } else { g() }`
            let mut values = vec![Ok(unit_call(c, "g", 1)), Ok(unit_call(c, "g", 2))];
            let optional = Type::optional(Type::UNIT);
            let joined = join_at(c, JoinForm::If, &mut values, required(optional), None);
            // THEN it is valid
            assert_eq!(joined.ok(), Some(optional));
            assert_eq!(found(c).len(), 0);
        });
    }

    #[test]
    fn join_under_a_required_type_should_report_once_when_the_members_agree() {
        let hole = Symbol::intern("T");
        let forms = [
            JoinForm::If,
            JoinForm::Array,
            JoinForm::Call { param: hole },
            JoinForm::Loop,
        ];
        for form in forms {
            with_params("", "a: u8, b: u8", |c, types| {
                // GIVEN the members `a: u8` and `b: u8` under the required type `string`
                let mut values = vec![Ok(local(types[0], 1)), Ok(local(types[1], 2))];
                // WHEN they join
                let _ = join_at(c, form, &mut values, required(Type::STRING), None);
                // THEN one E0032, at the whole join
                assert_eq!(found(c), vec![(Code::Mismatch, head())]);
            });
        }
    }

    #[test]
    fn join_under_a_required_type_should_report_at_the_member_when_one_member_gives_the_value() {
        with_params("", "n: u8", |c, types| {
            // GIVEN `let b: string = id(n)`: one member `n: u8` at a `T`
            let mut values = vec![Ok(local(types[0], 1))];
            let form = JoinForm::Call {
                param: Symbol::intern("T"),
            };
            // WHEN it joins under the required type `string`
            let _ = join_at(c, form, &mut values, required(Type::STRING), None);
            // THEN the directed E0032 stays at `n`, not at the call
            assert_eq!(found(c), vec![(Code::Mismatch, at(1))]);
        });
    }

    #[test]
    fn join_under_a_required_type_should_report_each_member_that_does_not_convert() {
        with_params("", "a: u8", |c, types| {
            // GIVEN `[a, "s"]` with `a: u8`, under the element type `string` that a `let` declares
            let origin = Origin::Declared {
                at: at(30),
                ty: Type::array(Type::STRING),
            };
            let mut values = vec![Ok(local(types[0], 1)), Ok(string(2))];
            // WHEN it joins
            let expected = Expectation::Required(Type::STRING, origin);
            let _ = join_at(c, JoinForm::Array, &mut values, expected, None);
            // THEN one E0032, at `a`, with a label at the declaration
            assert_eq!(found(c), vec![(Code::Mismatch, at(1))]);
            assert_eq!(labels(c), vec![at(30)]);
        });
    }

    /// The members `values` of a loop, each with its `break` at `labels`.
    fn break_members<'a>(values: &'a mut [Checked], labels: &[Span]) -> Vec<JoinMember<'a>> {
        (values.iter_mut().zip(labels))
            .map(|(value, &label)| JoinMember {
                value,
                at: Place::Whole,
                label,
            })
            .collect()
    }

    /// The diagnostics after the join of the `break` values that `build` makes. The first bare
    /// `break` is at 9. The result is the code and span of each, and the labels of the first.
    fn bare_break(
        build: fn(&mut Checker<'_>, &[Type]) -> Vec<Checked>,
        labels: &[Span],
    ) -> (Vec<(Code, Span)>, Vec<Span>) {
        with_params(UNITS, "o: unit?", |c, types| {
            let mut values = build(c, types);
            let mut members = break_members(&mut values, labels);
            let origin = Origin::BareBreak { at: at(9) };
            let expected = Expectation::Required(Type::UNIT, origin);
            let _ = c.join_form(JoinForm::Loop, head(), &mut members, expected, None);
            let all = diagnostics(c);
            let spans = all.first().map_or_else(Vec::new, |d| {
                d.secondary().iter().map(|&(span, _)| span).collect()
            });
            (found(c), spans)
        })
    }

    #[test]
    fn bare_break_should_give_one_e0043_at_the_first_bare_break() {
        // GIVEN a loop with a bare `break` at 9 and `break 1`, whose `1` joins `unit`
        let (reported, labelled) = bare_break(|c, _| vec![Ok(int(c, 1, 5))], &[at(4)]);
        // THEN one E0043 at the bare `break`, with its label at the `break 1`
        assert_eq!(reported, vec![(Code::NoValue, at(9))]);
        assert_eq!(labelled, vec![at(4)]);
        // GIVEN two values `1` and `2`, THEN it is still one E0043, labelled at the first
        let (reported, labelled) = bare_break(
            |c, _| vec![Ok(int(c, 1, 5)), Ok(int(c, 2, 6))],
            &[at(4), at(7)],
        );
        assert_eq!(reported, vec![(Code::NoValue, at(9))]);
        assert_eq!(labelled, vec![at(4)]);
        // GIVEN `break None` and a `break` of a `unit?` value, THEN one E0043
        let (reported, labelled) = bare_break(
            |c, types| vec![Ok(none(c, 5)), Ok(local(types[0], 6))],
            &[at(4), at(7)],
        );
        assert_eq!(reported, vec![(Code::NoValue, at(9))]);
        assert_eq!(labelled, vec![at(4)]);
        // GIVEN a value that is a nested join of `unit` values, THEN there is no report
        let (reported, _) = bare_break(
            |c, _| {
                let open = c.table.fresh(Kind::Any, at(5));
                let nested = if_else(unit_call(c, "u", 1), unit_call(c, "u", 2), open, 3);
                vec![Ok(nested)]
            },
            &[at(4)],
        );
        assert_eq!(reported, vec![]);
    }

    /// The groups of the conflict of the members that `build` makes, as the report shows them.
    fn shown_groups(
        build: fn(&mut Checker<'_>, &[Type]) -> Vec<Checked>,
    ) -> Vec<(String, Vec<Span>)> {
        with_params("", "a: u8", |c, types| {
            let mut values = build(c, types);
            let mut members = whole(&mut values);
            let Err(conflict) = c.join_members(&mut members, None, None, None) else {
                panic!("the members should have no candidate");
            };
            let site = Site {
                form: JoinForm::If,
                head: head(),
                hole: None,
                fixed: None,
            };
            let DiagnosticKind::JoinMismatch { groups, .. } =
                c.conflict_kind(site, None, &conflict)
            else {
                panic!("a conflict should give a `JoinMismatch`");
            };
            groups
        })
    }

    #[test]
    fn open_literal_should_show_as_i64() {
        // GIVEN the members `[1]` and `"s"`, THEN the group of the array shows `[i64]`
        let groups = shown_groups(|c, _| {
            let literal = int(c, 1, 1);
            let element = literal.ty;
            vec![Ok(array(vec![literal], element, 2)), Ok(string(3))]
        });
        assert_eq!(
            groups,
            vec![
                ("[i64]".to_owned(), vec![at(1)]),
                ("string".to_owned(), vec![at(3)])
            ]
        );
        // GIVEN `1`, `a: u8` and `"s"`, THEN `1` has no group: the type of `a` holds it
        let groups =
            shown_groups(|c, types| vec![Ok(int(c, 1, 1)), Ok(local(types[0], 2)), Ok(string(3))]);
        assert_eq!(
            groups,
            vec![
                ("u8".to_owned(), vec![at(2)]),
                ("string".to_owned(), vec![at(3)])
            ]
        );
    }
}
