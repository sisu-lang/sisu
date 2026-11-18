//! The report of a join. A join with no required type and no candidate reports one E0032 at its
//! head. It has a label for each group of members by type, and the fix that fits.
//!
//! A join under a required type reports what that type does not take. When the members agree and
//! only the required type differs, it is one E0032 at the whole join. Else each value position that
//! does not convert gets a directed E0032, or an E0043 when it is a `unit`.
//!
//! A `break` with no value, in a loop that gives one, is E0043 `BareBreak`.

use super::{
    Checker, Conflict, ExprKind, FxHashMap, JoinForm, JoinMember, Joined, Leaf, Place, Reported,
    Site, Span, Symbol, Type, TypeArgs, TypeKind, ensure_stack, fill, member_type, nested_leaves,
    part_at, target, tir, value_positions,
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

/// Whether the integer literal `value` fits `ty`, as E0040 judges it.
fn fits(ty: Type, value: i64) -> bool {
    match ty {
        Type::U8 => u8::try_from(value).is_ok(),
        Type::F64 => value.unsigned_abs() <= 1_u64 << f64::MANTISSA_DIGITS,
        Type::I64 => true,
        other => other.holds_infer(),
    }
}

/// `text` with the name of each open variable (`infer#3`) as `_`.
fn without_variables(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("infer#") {
        out.push_str(&rest[..at]);
        out.push('_');
        rest = rest[at + "infer#".len()..].trim_start_matches(|c: char| c.is_ascii_digit());
    }
    out.push_str(rest);
    out
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
            return self.no_values(units, site.heading.head);
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
        let head = site.heading.head;
        let mut taking = 0;
        let mut fails = vec![false; members.len()];
        for (i, failed) in fails.iter_mut().enumerate() {
            if member_type(members[i].value).is_some_and(|ty| ty != Type::NEVER) {
                taking += 1;
                *failed = self.fails(site, i, &mut members[i], required);
            }
        }
        let first_failing = fails.iter().position(|&failed| failed);
        // A loop with a bare `break` gives `unit`: the first value that is not `unit` is the label.
        if let Origin::BareBreak { at } = origin {
            let valued = first_failing.map_or(head, |i| members[i].label);
            return self.error(DiagnosticKind::BareBreak { at, valued });
        }
        // A call names the first argument that converts as the one that gives the parameter a type.
        let origin = self
            .inferred_origin(site, members, &fails)
            .unwrap_or(origin);
        // Members agree only when each that takes part fails: one that takes `required`, as
        // `None` takes `i64?`, leaves the report at those that do not.
        let failing = fails.iter().filter(|&&failed| failed).count();
        if failing == taking
            && let Some(reported) = self.agreed(site, (required, origin), members, taking)
        {
            return reported;
        }
        let mut last = None;
        for (i, _) in fails.iter().enumerate().filter(|&(_, &failed)| failed) {
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
            let own = member_type(member.value).map_or(required, |ty| self.table.default_open(ty));
            let reported = match &mut *member.value {
                Ok(e) => self.report_leaves(e, target, origin, Some(top), false),
                Err(_) => None,
            };
            let reported = reported.unwrap_or_else(|| {
                if own == Type::UNIT && !takes_unit(required) {
                    self.no_value(label, NoValueCause::Plain)
                } else {
                    self.directed(label, (required, own), origin, None, false)
                }
            });
            last = Some(reported);
        }
        last.unwrap_or_else(|| self.proof(head))
    }

    /// The report of members that agree with each other, and differ from the required type only.
    /// `taking` is how many members take part: one member is reported at itself.
    fn agreed(
        &mut self,
        site: Site,
        (required, origin): (Type, Origin),
        members: &mut [JoinMember<'_>],
        taking: usize,
    ) -> Option<Reported> {
        if taking < 2 {
            return None;
        }
        let Ok(Joined::Typed(got)) = self.join_members(members, site.hole, site.fixed, None) else {
            return None;
        };
        let got = self.table.resolve(got);
        if got == required {
            return None;
        }
        if got == Type::UNIT {
            // The branches that each give `unit` are one mistake at the whole `if` or `match`. The
            // other forms report each `unit` value.
            let whole =
                matches!(site.form, JoinForm::If | JoinForm::Match) && !takes_unit(required);
            return whole.then(|| self.no_value(site.heading.whole, NoValueCause::Plain));
        }
        let kind = self.agreed_kind(site, (required, got), origin);
        Some(self.error(kind))
    }

    /// The E0032 of members that have the type `got`, where `required` is the type of each. An
    /// array is reported as one: its members are its elements. It is at the whole expression.
    pub(super) fn agreed_kind(
        &self,
        site: Site,
        (required, got): (Type, Type),
        origin: Origin,
    ) -> DiagnosticKind {
        let got = self.table.default_open(got);
        let (want, got) = match site.form {
            JoinForm::Array => (Type::array(required), Type::array(got)),
            _ => (required, got),
        };
        self.directed_kind(site.heading.whole, (want, got), origin, None, false)
    }

    /// For a call, the origin "this argument gives `T` the type `ty`" of the first member that
    /// converts. `None` when no member converts, or the join is no call.
    fn inferred_origin(
        &self,
        site: Site,
        members: &[JoinMember<'_>],
        fails: &[bool],
    ) -> Option<Origin> {
        let param = site.hole?;
        let (i, ty) = members.iter().enumerate().find_map(|(i, member)| {
            // A literal gives `T` no type of its own: the call gives it the required type.
            let ty = member_type(&*member.value)
                .filter(|&ty| ty != Type::NEVER && !self.table.resolve(ty).holds_infer())?;
            (!fails[i]).then_some((i, ty))
        })?;
        let (at, ty) = match members[i].at {
            Place::Part { place, argument } => {
                let own = self.table.resolve(ty);
                (
                    argument,
                    part_at(self.table.resolve(place), param, own).unwrap_or(own),
                )
            }
            Place::Split { part, argument } => (argument, self.table.resolve(part)),
            Place::Whole => (members[i].label, ty),
        };
        let ty = self.table.default_open(ty);
        Some(Origin::Inferred { at, param, ty })
    }

    /// The origin "this argument gives `param` the type `ty`" of a call's join of `param`. It is
    /// the first member that does not fail (`fails`) and whose part at `param` is known. The rest
    /// of its type can be open, unlike in `inferred_origin`.
    pub(in crate::check) fn binder_origin(
        &self,
        param: Symbol,
        members: &[JoinMember<'_>],
        fails: &[bool],
    ) -> Option<Origin> {
        members.iter().zip(fails).find_map(|(member, &failed)| {
            // A literal gives `param` no type of its own: the call gives it the required type.
            let ty = member_type(&*member.value).filter(|&ty| ty != Type::NEVER && !failed)?;
            let own = self.table.resolve(ty);
            let (at, ty) = match member.at {
                Place::Part { place, argument } => (
                    argument,
                    part_at(self.table.resolve(place), param, own).unwrap_or(own),
                ),
                Place::Split { part, argument } => (argument, self.table.resolve(part)),
                Place::Whole => (member.label, own),
            };
            // Only the part at `param` must be known: `mk()` of `Result<i64, E>` gives `T` of
            // `Result<T, string>` the type `i64`.
            (!ty.holds_infer()).then(|| Origin::Inferred {
                at,
                param,
                ty: self.table.default_open(ty),
            })
        })
    }

    /// Whether `member` does not convert to `ty` as the join `site` takes it. It binds nothing.
    fn fails(&mut self, site: Site, i: usize, member: &mut JoinMember<'_>, ty: Type) -> bool {
        let target = target(member.at, site.hole, ty);
        let snapshot = self.table.snapshot();
        let wraps = self.member_converts(member, target, site.fixed == Some(i));
        self.table.rollback(snapshot);
        wraps.is_none()
    }

    /// Whether the value `e`, which has no value position, does not convert to `target`. It binds
    /// nothing.
    fn leaf_fails(&mut self, e: &mut tir::Expr, target: Type) -> bool {
        let snapshot = self.table.snapshot();
        let wraps = self.converts_at(e, target);
        self.table.rollback(snapshot);
        wraps.is_none()
    }

    /// Reports each leaf of `e` that does not convert to `target`: the leaf, against the part of
    /// `target` at its position. One walk visits each value position once. `top` is set when `e` is
    /// the member itself. `in_join` is set below an `if` or a `match`. The first one of a chain
    /// that has one type for each leaf is one value: it is reported at itself.
    fn report_leaves(
        &mut self,
        e: &mut tir::Expr,
        target: Type,
        origin: Origin,
        top: Option<Top>,
        in_join: bool,
    ) -> Option<Reported> {
        if e.ty == Type::NEVER {
            return None;
        }
        ensure_stack(|| {
            let Some((part, _)) = self.enter(e, target) else {
                if !self.leaf_fails(e, target) {
                    return None;
                }
                return Some(self.report_value(e, e.ty, target, origin, top));
            };
            let node = matches!(
                e.kind,
                ExprKind::If { .. } | ExprKind::IfSome { .. } | ExprKind::Match { .. }
            );
            if node
                && !in_join
                && let Some(got) = self.agreed_leaves(e)
            {
                return Some(self.report_value(e, got, target, origin, top));
            }
            let mut last = None;
            value_positions(e, &mut |child| {
                last = self
                    .report_leaves(child, part, origin, None, in_join || node)
                    .or(last);
            });
            last
        })
    }

    /// The type of each leaf of `e`, when it is one type.
    fn agreed_leaves(&self, e: &mut tir::Expr) -> Option<Type> {
        let mut leaves: Vec<Leaf> = Vec::new();
        nested_leaves(e, &mut leaves);
        let first = leaves.first()?.around;
        let key = self.table.key(first);
        (leaves.iter().all(|leaf| self.table.key(leaf.around) == key))
            .then(|| self.table.resolve(first))
    }

    /// Reports the value `e`, of type `got`, against `target`: E0043 for a `unit` where none is
    /// wanted, else E0032 with the conversion that fits.
    fn report_value(
        &mut self,
        e: &tir::Expr,
        got: Type,
        target: Type,
        origin: Origin,
        top: Option<Top>,
    ) -> Reported {
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
                // The whole place, when it is known, as on `main`; else the part at `hole`.
                let whole = self.table.default_open(self.table.resolve(target));
                if whole.holds_infer() {
                    let part = part_at(self.table.resolve(place), hole, self.table.resolve(got));
                    (required, part.unwrap_or(got))
                } else {
                    (whole, got)
                }
            }
            _ => (target, got),
        };
        let (want, got) = (self.table.default_open(want), self.table.default_open(got));
        if got == Type::UNIT && !takes_unit(want) {
            let cause = self.unit_cause(e, Some(want));
            return self.no_value(e.span, cause);
        }
        // A generic call that gives its one argument's type is a nested join. When that argument
        // is still open, it would take the required type, so the report goes to it, at each depth.
        let mut at = e.span;
        if matches!(e.kind, ExprKind::Call { .. }) {
            while let Some((inner, ty)) = self.lone_arg(at)
                && self.table.resolve(ty).holds_infer()
            {
                at = inner;
            }
        }
        let fix =
            (convertible.then(|| number_conversion(want, got)).flatten()).map(|(to, through)| {
                Conversion {
                    to,
                    through,
                    operand: at,
                }
            });
        let conversion = matches!(got.kind(), TypeKind::Optional(p) if p == want)
            && matches!(e.kind, ExprKind::Convert(_));
        self.directed(at, (want, got), origin, fix, conversion)
    }

    /// "expected `want`, found `got`" at `at`, as a mismatch with a written type reports it.
    fn directed(
        &mut self,
        at: Span,
        types: (Type, Type),
        origin: Origin,
        fix: Option<Conversion>,
        conversion: bool,
    ) -> Reported {
        let kind = match self.any_misfit(at, types.0, types.1, origin) {
            Some(kind) => kind,
            None => self.directed_kind(at, types, origin, fix, conversion),
        };
        self.error(kind)
    }

    /// The `Mismatch` of `directed`.
    fn directed_kind(
        &self,
        at: Span,
        (want, got): (Type, Type),
        origin: Origin,
        fix: Option<Conversion>,
        conversion: bool,
    ) -> DiagnosticKind {
        let want_text = if want.holds_infer() {
            self.open_name(want)
        } else {
            self.want_name(at, want)
        };
        DiagnosticKind::Mismatch {
            want: Expected::Types(vec![want_text]),
            got: self.open_name(got),
            at,
            origin: self.origin_fact(origin),
            fix: fix.map(MismatchFix::Convert),
            conversion,
            layers: beyond_one_layer(want, got),
        }
    }

    /// `ty` as a diagnostic names it. A part that no type reached is written as the value that
    /// leaves it open: `None` for the payload of an optional, `[]` for the element of an array.
    fn open_name(&self, ty: Type) -> String {
        if !ty.holds_infer() {
            return self.type_name(ty);
        }
        match ty.kind() {
            TypeKind::Optional(payload) if matches!(payload.kind(), TypeKind::Infer(_)) => {
                "None".to_string()
            }
            TypeKind::Array(element) if matches!(element.kind(), TypeKind::Infer(_)) => {
                "[]".to_string()
            }
            TypeKind::Optional(payload) => format!("{}?", self.open_name(payload)),
            TypeKind::Array(element) => format!("[{}]", self.open_name(element)),
            _ => without_variables(&self.type_name(ty)),
        }
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

    /// Why the `unit` value `e` has no value. `no_value_cause` (`expr.rs`) states the same rule on
    /// the source tree, which a join does not hold: a change to one is a change to the other.
    /// A call that the `tir` does not name keeps `Plain`.
    fn unit_cause(&self, e: &tir::Expr, want: Option<Type>) -> NoValueCause {
        let called = |name: &str| NoValueCause::Call {
            name: name.to_string(),
            declared: None,
        };
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
            // `units` and `report_leaves` go into the operand of a `dbg` first: only a `dbg` that
            // is a member itself gets here.
            ExprKind::Dbg { operand, .. } => self.unit_cause(operand, want),
            ExprKind::Print { stream, .. } => called(match stream {
                tir::Stream::Stdout => "print",
                tir::Stream::Stderr => "eprint",
            }),
            ExprKind::InterfaceCall {
                interface, method, ..
            }
            | ExprKind::DynCall {
                interface, method, ..
            } => called(
                self.interfaces[interface.0].requirements[*method as usize]
                    .name
                    .name
                    .as_str(),
            ),
            ExprKind::Block(block)
                if matches!(block.stmts.last(), Some(tir::Stmt::Push { .. })) =>
            {
                called("push")
            }
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

    /// Whether the type of `other` holds each literal of `group`, which has an open integer type.
    /// It binds nothing.
    fn holds_literals(&mut self, other: Type, group: &super::Group) -> bool {
        let snapshot = self.table.snapshot();
        let holds = self.unify(group.ty, other)
            && (group.literals.iter()).all(|&(ty, value)| fits(self.table.resolve(ty), value));
        self.table.rollback(snapshot);
        holds
    }

    /// The E0032 of a join that has no candidate: its groups in the order of the source, the
    /// parts of its arguments, the declared interfaces that fit each member, and the fix. It is
    /// `pub(super)` so that a test reads its fields, which the diagnostic text hides.
    pub(super) fn conflict_kind(
        &mut self,
        site: Site,
        binding: Option<Span>,
        conflict: &Conflict,
    ) -> DiagnosticKind {
        // A literal or a call that is still open is in no group when the type of another group
        // holds it: it decides nothing, and its type would show `_`.
        let mut shown: Vec<Shown> = Vec::new();
        let mut index: FxHashMap<Type, usize> = FxHashMap::default();
        for group in &conflict.groups {
            let open = self.table.resolve(group.ty).holds_infer();
            if open
                && (conflict.groups.iter())
                    // Only a closed type holds a literal: two open literals hold each other.
                    .any(|other| {
                        other.ty != group.ty
                            && !self.table.resolve(other.ty).holds_infer()
                            && self.holds_literals(other.ty, group)
                    })
            {
                continue;
            }
            let ty = self.table.default_open(group.ty);
            let at = if let Some(&at) = index.get(&ty) {
                at
            } else {
                shown.push(Shown {
                    ty,
                    leaves: Vec::new(),
                    whole: Vec::new(),
                });
                index.insert(ty, shown.len() - 1);
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
            ([(interface, name)], Some(binding)) => {
                let any = Type::intern(TypeKind::Any(*interface, TypeArgs::NONE));
                let ty = match (site.form, site.heading.returns, site.hole) {
                    (JoinForm::Array, ..) => format!("[any {name}]"),
                    (JoinForm::Call { .. }, Some(returns), Some(hole)) => {
                        self.type_name(fill(returns, hole, any))
                    }
                    _ => format!("any {name}"),
                };
                Some(JoinFix::Declare { name: binding, ty })
            }
            _ => None,
        });
        // A call labels the first argument as the one that gives its type parameter that type.
        if site.hole.is_some()
            && parts.is_empty()
            && let Some(one) = shown.first_mut()
            && !one.leaves.is_empty()
        {
            parts.push((one.leaves.remove(0), one.ty));
            if one.leaves.is_empty() {
                shown.remove(0);
            }
        }
        DiagnosticKind::JoinMismatch {
            form: self.form_name(site),
            head: site.heading.head,
            groups: (shown.into_iter())
                .map(|one| (self.open_name(one.ty), one.leaves))
                .collect(),
            parts: (parts.into_iter())
                .map(|(at, ty)| (at, self.open_name(ty)))
                .collect(),
            interfaces: interfaces.into_iter().map(|(_, name)| name).collect(),
            fix,
        }
    }

    /// The interfaces that the program declares, which each of `types` conforms to and that can
    /// be written after `any`. A language interface does not count, and neither does an interface
    /// with type parameters.
    fn fitting_interfaces(&mut self, types: &[Type]) -> Vec<(InterfaceId, String)> {
        if types.iter().any(|ty| ty.holds_infer()) {
            return Vec::new();
        }
        let mut found = Vec::new();
        for i in 0..self.interfaces.len() {
            let info = &self.interfaces[i];
            if info.name.span.in_prelude() || info.type_params.len() != 1 {
                continue;
            }
            let (id, name) = (InterfaceId(i), info.name.name.to_string());
            if types
                .iter()
                .all(|&ty| self.conforms(ty, id, TypeArgs::NONE))
                && self.why_not_any(id).is_none()
            {
                found.push((id, name));
            }
        }
        found
    }

    /// The form of the join `site`, as its headline names it.
    fn form_name(&self, site: Site) -> JoinFormName {
        let head = site.heading.head;
        match site.form {
            JoinForm::If => JoinFormName::If,
            JoinForm::Match => JoinFormName::Match,
            JoinForm::Array => JoinFormName::Array,
            JoinForm::Loop => JoinFormName::Loop,
            JoinForm::Coalesce => JoinFormName::Coalesce,
            JoinForm::Operator(_) => JoinFormName::Operator(self.source.text(head).to_string()),
            JoinForm::Compare => JoinFormName::Compare,
            JoinForm::Compound { .. } => JoinFormName::Compound(self.source.text(head).to_string()),
            JoinForm::Call { param } => JoinFormName::Call(param.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{
        array, at, eye, heading, if_else, if_without_else, int, local, none, orders,
    };
    use super::super::*;
    use crate::check::expectation::Origin;
    use crate::check::infer::Kind;
    use crate::check::operator::Operator;
    use crate::check::tests::joins::with_params;
    use crate::diagnostic::{Code, Diagnostic, DiagnosticKind, Expected, JoinFix, NoValueCause};
    use crate::mir::tests::assert_near_linear;
    use crate::tir::FuncId;

    /// `Seen`, with the structs `Eye` and `Lid`, which conform to it.
    const EYE_LID: &str = "interface Seen {\n    fn seen(self) -> i64\n}\nstruct Eye: Seen {\n    v: i64\n    fn seen(self) -> i64 { self.v }\n}\nstruct Lid: Seen {\n    v: i64\n    fn seen(self) -> i64 { self.v }\n}\n";

    /// `Seen` and `Heard`, which both `Eye` and `Lid` conform to.
    const EYE_LID_HEARD: &str = "interface Seen {\n    fn seen(self) -> i64\n}\ninterface Heard {\n    fn heard(self) -> i64\n}\nstruct Eye: Seen, Heard {\n    v: i64\n    fn seen(self) -> i64 { self.v }\n    fn heard(self) -> i64 { self.v }\n}\nstruct Lid: Seen, Heard {\n    v: i64\n    fn seen(self) -> i64 { self.v }\n    fn heard(self) -> i64 { self.v }\n}\n";

    /// `Say`, which `Eye` conforms to, and which gives `unit`.
    const EYE_LID_SAY: &str = "fn u() {}\ninterface Say {\n    fn say(self)\n}\nstruct Eye: Say {\n    v: i64\n    fn say(self) {}\n}\n";

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
        c.join_form(form, heading(head()), &mut members, expected, binding)
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
            let joined = c.join_form(form, heading(head()), &mut members, Expectation::None, None);
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
                let joined = c.join_form(
                    form,
                    heading(Span::new(2, 4)),
                    &mut members,
                    Expectation::None,
                    None,
                );
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
        assert!(d.help().is_none(), "the fix says it: no help beside it");
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
            let _ = c.join_form(
                JoinForm::Loop,
                heading(head()),
                &mut members,
                expected,
                None,
            );
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

    /// The `Site` of a join of `form`: a call has the type parameter `T`.
    fn site(form: JoinForm) -> Site {
        let hole = match form {
            JoinForm::Call { param } => Some(param),
            _ => None,
        };
        Site {
            form,
            heading: heading(head()),
            hole,
            fixed: None,
        }
    }

    fn call() -> JoinForm {
        JoinForm::Call {
            param: Symbol::intern("T"),
        }
    }

    /// The kind that the conflict of the members `build` makes gives, with `binding` as the name
    /// of a `let` without a type.
    fn conflict_of(
        (src, params): (&str, &str),
        form: JoinForm,
        binding: Option<Span>,
        build: fn(&mut Checker<'_>, &[Type]) -> Vec<Checked>,
    ) -> DiagnosticKind {
        with_params(src, params, |c, types| {
            let mut values = build(c, types);
            let mut members = whole(&mut values);
            let site = site(form);
            let Err(conflict) = c.join_members(&mut members, site.hole, None, None) else {
                panic!("the members should have no candidate");
            };
            c.conflict_kind(site, binding, &conflict)
        })
    }

    /// The groups of the conflict of the members that `build` makes, as the report shows them.
    fn shown_groups(
        build: fn(&mut Checker<'_>, &[Type]) -> Vec<Checked>,
    ) -> Vec<(String, Vec<Span>)> {
        let kind = conflict_of(("", "a: u8, t: bool"), JoinForm::If, None, build);
        let DiagnosticKind::JoinMismatch { groups, .. } = kind else {
            panic!("a conflict should give a `JoinMismatch`");
        };
        groups
    }

    fn group(text: &str, at_: usize) -> (String, Vec<Span>) {
        (text.to_owned(), vec![at(at_)])
    }

    #[test]
    fn open_literal_should_show_as_i64() {
        // GIVEN the members `[1]` and `"s"`, THEN the group of the array shows `[i64]`
        let groups = shown_groups(|c, _| {
            let literal = int(c, 1, 1);
            let element = literal.ty;
            vec![Ok(array(vec![literal], element, 2)), Ok(string(3))]
        });
        assert_eq!(groups, vec![group("[i64]", 1), group("string", 3)]);
        // GIVEN `None`, `a: u8` and `"s"`, THEN the group shows `None`, as a program writes it
        let groups =
            shown_groups(|c, types| vec![Ok(none(c, 1)), Ok(local(types[0], 2)), Ok(string(3))]);
        assert_eq!(
            groups,
            vec![group("None", 1), group("u8", 2), group("string", 3)]
        );
        // GIVEN `[]` and `"s"`, THEN the group shows `[]`
        let groups = shown_groups(|c, _| {
            let open = c.table.fresh(Kind::Any, at(1));
            vec![Ok(array(Vec::new(), open, 1)), Ok(string(2))]
        });
        assert_eq!(groups, vec![group("[]", 1), group("string", 2)]);
        // GIVEN `1`, `a: u8` and `"s"`, THEN `1` has no group: the type of `a` holds it
        let groups =
            shown_groups(|c, types| vec![Ok(int(c, 1, 1)), Ok(local(types[0], 2)), Ok(string(3))]);
        assert_eq!(groups, vec![group("u8", 2), group("string", 3)]);
    }

    #[test]
    fn open_literals_should_share_one_group_beside_another_type() {
        // GIVEN `[1, 2, "s"]`, THEN `1` and `2` are one group `i64`, and keep their labels
        let groups = shown_groups(|c, _| vec![Ok(int(c, 1, 1)), Ok(int(c, 2, 2)), Ok(string(3))]);
        assert_eq!(
            groups,
            vec![("i64".to_owned(), vec![at(1), at(2)]), group("string", 3)]
        );
    }

    #[test]
    fn open_literal_should_join_the_group_that_holds_it_inside_an_array() {
        // GIVEN `[1]`, `xs: [u8]` and `"s"`, THEN the literal `1` has no group
        let groups = shown_groups(|c, _| {
            let literal = int(c, 1, 1);
            let element = literal.ty;
            let held = Type::array(Type::U8);
            vec![
                Ok(array(vec![literal], element, 1)),
                Ok(local(held, 2)),
                Ok(string(3)),
            ]
        });
        assert_eq!(groups, vec![group("[u8]", 2), group("string", 3)]);
    }

    #[test]
    fn open_literal_should_keep_its_group_when_the_held_type_cannot_hold_it() {
        // GIVEN `300`, `a: u8` and `t: bool`, THEN `300` keeps its label: `u8` cannot hold it
        let groups = shown_groups(|c, types| {
            vec![
                Ok(int(c, 300, 1)),
                Ok(local(types[0], 2)),
                Ok(local(types[1], 3)),
            ]
        });
        assert_eq!(
            groups,
            vec![group("i64", 1), group("u8", 2), group("bool", 3)]
        );
    }

    #[test]
    fn left_side_of_coalesce_should_be_a_group_at_its_payload() {
        with_params("", "o: u8?", |c, types| {
            // GIVEN `o ?? "s"` with `o: u8?`: `o` is a part at `v?`, and `"s"` is whole
            let v = c.table.fresh(Kind::Any, at(1));
            let (mut left, mut right) = (Ok(local(types[0], 1)), Ok(string(2)));
            let mut members = [
                JoinMember {
                    value: &mut left,
                    at: Place::Part {
                        place: Type::optional(v),
                        argument: at(1),
                    },
                    label: at(1),
                },
                JoinMember {
                    value: &mut right,
                    at: Place::Whole,
                    label: at(2),
                },
            ];
            // WHEN the conflict of the members is made
            let conflict = c.conflict(&mut members, &[0, 1], None, None);
            // THEN `o` is a leaf of the group `u8`, and no argument part: only a call has them
            assert_eq!(conflict.parts.len(), 0);
            let groups: Vec<(Type, Vec<Span>)> = (conflict.groups.iter())
                .map(|group| (group.ty, group.leaves.clone()))
                .collect();
            assert_eq!(
                groups,
                vec![(Type::U8, vec![at(1)]), (Type::STRING, vec![at(2)])]
            );
        });
    }

    #[test]
    fn left_side_of_coalesce_should_get_no_conversion_fix() {
        with_params("", "o: u8?, b: i64", |c, types| {
            // GIVEN `o ?? b` with `o: u8?` and `b: i64`: `o` is a part at `v?`
            let v = c.table.fresh(Kind::Any, at(1));
            let (mut left, mut right) = (Ok(local(types[0], 1)), Ok(local(types[1], 2)));
            let mut members = [
                JoinMember {
                    value: &mut left,
                    at: Place::Part {
                        place: Type::optional(v),
                        argument: at(1),
                    },
                    label: at(1),
                },
                JoinMember {
                    value: &mut right,
                    at: Place::Whole,
                    label: at(2),
                },
            ];
            // WHEN the report of the conflict is made
            let conflict = c.conflict(&mut members, &[0, 1], None, None);
            let kind = c.conflict_kind(site(JoinForm::Coalesce), None, &conflict);
            // THEN no fix converts `o`: it is no whole value
            let DiagnosticKind::JoinMismatch { fix, .. } = kind else {
                panic!("a conflict should give a `JoinMismatch`");
            };
            assert!(fix.is_none());
        });
    }

    #[test]
    fn call_should_label_the_first_argument_as_the_one_that_gives_the_type() {
        // GIVEN `pick(a, "s")` with `a: u8`, WHEN the arguments for `T` do not join
        let kind = conflict_of(("", "a: u8"), call(), None, |_, types| {
            vec![Ok(local(types[0], 1)), Ok(string(2))]
        });
        let DiagnosticKind::JoinMismatch { groups, parts, .. } = kind else {
            panic!("a conflict should give a `JoinMismatch`");
        };
        // THEN `a` has the label of an argument, and `"s"` the label of its type
        assert_eq!(parts, vec![(at(1), "u8".to_owned())]);
        assert_eq!(groups, vec![group("string", 2)]);
        // GIVEN `index_of(names, n)`, where `names: [string]` is at a place `[T]`
        let kind = conflict_of(("", "n: i64"), call(), None, |c, types| {
            vec![Ok(local(types[0], 3)), Ok(string(2)), Ok(none(c, 4))]
        });
        let DiagnosticKind::JoinMismatch { parts, .. } = kind else {
            panic!("a conflict should give a `JoinMismatch`");
        };
        // THEN the first argument in the source is the one that gives `T` its type
        assert_eq!(parts.first().map(|&(at, _)| at), Some(at(2)));
    }

    #[test]
    fn call_under_a_required_type_should_label_the_first_argument_that_converts() {
        let run = |required_ty: Type| {
            with_params("", "a: u8", |c, types| {
                // GIVEN `let y: u8 = pick(a, "s")`, the declaration at 30
                let origin = Origin::Declared {
                    at: at(30),
                    ty: required_ty,
                };
                let mut values = vec![Ok(local(types[0], 1)), Ok(string(2))];
                let expected = Expectation::Required(required_ty, origin);
                // WHEN the call joins its arguments under the declared type
                let _ = join_at(c, call(), &mut values, expected, None);
                // THEN one E0032 at `"s"`, with the label of `a`, and none of the declaration
                assert_eq!(found(c), vec![(Code::Mismatch, at(2))]);
                labels(c)
            })
        };
        assert_eq!(run(Type::U8), vec![at(1)]);
        assert_eq!(run(Type::optional(Type::U8)), vec![at(1)]);
    }

    #[test]
    fn call_under_a_required_type_should_not_label_a_literal_argument() {
        with_params("", "", |c, _| {
            // GIVEN `let y: u8 = pick(1, "s")`, the declaration at 30
            let origin = Origin::Declared {
                at: at(30),
                ty: Type::U8,
            };
            let mut values = vec![Ok(int(c, 1, 1)), Ok(string(2))];
            let expected = Expectation::Required(Type::U8, origin);
            // WHEN the call joins its arguments under the declared type
            let _ = join_at(c, call(), &mut values, expected, None);
            // THEN one E0032 at `"s"`, and no label at `1`: the literal gives `T` no type
            assert_eq!(found(c), vec![(Code::Mismatch, at(2))]);
            assert!(!labels(c).contains(&at(1)));
        });
    }

    #[test]
    fn unit_value_should_not_hide_the_mistake_at_another_member() {
        // GIVEN `let xs: [u8] = [u(), "s"]`
        with_params(UNITS, "", |c, _| {
            let mut values = vec![Ok(unit_call(c, "u", 1)), Ok(string(2))];
            // WHEN the elements join under `u8`
            let _ = join_at(c, JoinForm::Array, &mut values, required(Type::U8), None);
            // THEN E0043 at `u()` and E0032 at `"s"`
            assert_eq!(
                found(c),
                vec![(Code::NoValue, at(1)), (Code::Mismatch, at(2))]
            );
        });
    }

    #[test]
    fn unit_value_should_not_blame_a_member_that_converts() {
        // GIVEN `let x: [[unit]] = [[u()], [1]]`
        with_params(UNITS, "", |c, _| {
            let one = int(c, 1, 5);
            let element = one.ty;
            let mut values = vec![
                Ok(array(vec![unit_call(c, "u", 1)], Type::UNIT, 2)),
                Ok(array(vec![one], element, 4)),
            ];
            // WHEN the elements join under `[unit]`
            let required = required(Type::array(Type::UNIT));
            let _ = join_at(c, JoinForm::Array, &mut values, required, None);
            // THEN E0032 at `1` alone: `[u()]` converts, so `u()` has no mistake
            assert_eq!(found(c), vec![(Code::Mismatch, at(5))]);
        });
    }

    #[test]
    fn nested_join_under_a_required_type_should_be_one_member() {
        let nested = |c: &mut Checker<'_>, types: &[Type]| {
            let open = c.table.fresh(Kind::Any, at(3));
            if_else(local(types[0], 1), local(types[0], 2), open, 3)
        };
        // GIVEN `[if c { a } else { b }]` with `a, b: u8`, under `string`
        with_params("", "a: u8", |c, types| {
            let mut values = vec![Ok(nested(c, types))];
            let _ = join_at(
                c,
                JoinForm::Array,
                &mut values,
                required(Type::STRING),
                None,
            );
            // THEN one E0032 at the `if`
            assert_eq!(found(c), vec![(Code::Mismatch, at(3))]);
        });
        // GIVEN `[if c { a } else { b }, "s"]`, THEN the same
        with_params("", "a: u8", |c, types| {
            let mut values = vec![Ok(nested(c, types)), Ok(string(4))];
            let _ = join_at(
                c,
                JoinForm::Array,
                &mut values,
                required(Type::STRING),
                None,
            );
            assert_eq!(found(c), vec![(Code::Mismatch, at(3))]);
        });
        // GIVEN `id(if c { a } else { b })`, THEN the same
        with_params("", "a: u8", |c, types| {
            let mut values = vec![Ok(nested(c, types))];
            let _ = join_at(c, call(), &mut values, required(Type::STRING), None);
            assert_eq!(found(c), vec![(Code::Mismatch, at(3))]);
        });
    }

    /// The steps of the report of the join of `[chain]` under `string`. The `chain` is an
    /// `else if` chain of `n` branches. Each gives `string`, but the last gives `u8`.
    fn required_chain(n: usize) -> usize {
        with_params("", "a: u8", |c, types| {
            let mut chain = local(types[0], 0);
            for i in 1..n {
                let open = c.table.fresh(Kind::Any, at(i));
                chain = if_else(local(Type::STRING, i), chain, open, i);
            }
            let mut values = vec![Ok(chain)];
            LEAF_STEPS.set(0);
            VALUE_STEPS.set(0);
            let _ = join_at(
                c,
                JoinForm::Array,
                &mut values,
                required(Type::STRING),
                None,
            );
            assert_eq!(
                found(c).len(),
                1,
                "the one leaf that fails should be reported"
            );
            LEAF_STEPS.get() + VALUE_STEPS.get()
        })
    }

    #[test]
    fn required_report_should_cost_linear_steps_on_an_else_if_chain() {
        // GIVEN an `else if` chain of n, 2n and 3n branches: only the last fails
        let steps = [
            required_chain(100),
            required_chain(200),
            required_chain(300),
        ];
        // THEN the report visits each value position once, so the steps grow linearly
        assert_near_linear(steps[0], steps[1], steps[2]);
    }

    #[test]
    fn declared_interface_that_cannot_be_any_should_give_no_help_and_no_fix() {
        let src = "interface Copy {\n    fn copy(self) -> Self\n}\nstruct A: Copy {\n    v: i64\n    fn copy(self) -> A { self }\n}\nstruct B: Copy {\n    v: i64\n    fn copy(self) -> B { self }\n}\n";
        with_params(src, "x: A, y: B", |c, types| {
            let mut values = vec![Ok(eye(types[0], 1)), Ok(eye(types[1], 2))];
            // GIVEN `let x = if c { A(v: 1) } else { B(v: 2) }`, where `Copy` returns `Self`
            let binding = Some(Span::new(4, 5));
            let _ = join_at(c, JoinForm::If, &mut values, Expectation::None, binding);
            // THEN `any Copy` is no type: no fix, and no help
            let [d] = diagnostics(c).try_into().unwrap_or_else(|all: Vec<_>| {
                panic!("one diagnostic should be reported, not {}", all.len())
            });
            assert_eq!(d.fixes().len(), 0);
            assert!(d.help().is_none());
        });
    }

    #[test]
    fn built_in_unit_value_should_keep_the_cause_of_its_call() {
        let cause = |build: &dyn Fn(&mut Checker<'_>, &[Type]) -> tir::Expr| {
            with_params(EYE_LID_SAY, "x: Eye", |c, types| {
                let e = build(c, types);
                c.unit_cause(&e, None)
            })
        };
        let named = |cause: NoValueCause, text: &str| matches!(cause, NoValueCause::Call { ref name, .. } if name == text);
        let print = |stream| {
            let operand = Box::new(tir::Expr::new(ExprKind::Int(1), Type::I64, at(2)));
            ExprKind::Print { stream, operand }
        };
        // `[print(1), 1]` and `[eprint(1), 1]` name the call that they make
        let stdout = cause(&|_, _| tir::Expr::new(print(tir::Stream::Stdout), Type::UNIT, at(1)));
        assert!(named(stdout, "print"));
        let stderr = cause(&|_, _| tir::Expr::new(print(tir::Stream::Stderr), Type::UNIT, at(1)));
        assert!(named(stderr, "eprint"));
        // `dbg(print(1))` is the cause of its operand
        let dbg = cause(&|_, _| {
            let operand = Box::new(tir::Expr::new(
                print(tir::Stream::Stdout),
                Type::UNIT,
                at(1),
            ));
            let kind = ExprKind::Dbg {
                at: at(1),
                text: String::new(),
                operand,
            };
            tir::Expr::new(kind, Type::UNIT, at(1))
        });
        assert!(named(dbg, "print"));
        // `u()` names the function, and its declaration
        let user = cause(&|c, _| unit_call(c, "u", 1));
        assert!(matches!(user, NoValueCause::Call { ref name, declared: Some(_) } if name == "u"));
        // `x.say()` is a call of an interface method: its name, and no declaration
        let method = cause(&|c, types| {
            let say = (c.interfaces.iter())
                .position(|interface| interface.name.name.as_str() == "Say")
                .expect("the program should declare `Say`");
            let receiver = local(types[0], 2);
            let kind = ExprKind::InterfaceCall {
                interface: tir::InterfaceId(say),
                iargs: TypeArgs::NONE,
                method: 0,
                types: TypeArgs::NONE,
                args: vec![receiver],
            };
            tir::Expr::new(kind, Type::UNIT, at(1))
        });
        assert!(matches!(method, NoValueCause::Call { ref name, declared: None } if name == "say"));
        // `xs.push(1)` is a block that ends in a `Push`
        let push = cause(&|_, _| {
            let value = tir::Expr::new(ExprKind::Int(1), Type::I64, at(2));
            let place = tir::Place::Local(tir::LocalId(0));
            let stmt = tir::Stmt::Push {
                place,
                value,
                span: at(1),
            };
            let block = tir::Block {
                stmts: vec![stmt],
                value: None,
                ty: Type::UNIT,
            };
            tir::Expr::new(ExprKind::Block(block), Type::UNIT, at(1))
        });
        assert!(named(push, "push"));
        // `if c { print(0) }` has no `else`
        let no_else = cause(&|_, _| if_without_else(1));
        assert!(matches!(
            no_else,
            NoValueCause::NoElse { if_let: false, .. } | NoValueCause::UnitIf { if_let: false, .. }
        ));
    }

    #[test]
    fn declare_fix_should_write_the_return_type_of_a_call() {
        // GIVEN `let x = wrap(Eye(v: 1), Lid(v: 2))` with `fn wrap<T>(a: T, b: T) -> [T]`
        let kind = with_params(EYE_LID, "x: Eye, y: Lid", |c, types| {
            let mut values = vec![Ok(eye(types[0], 1)), Ok(eye(types[1], 2))];
            let mut members = whole(&mut values);
            let site = Site {
                heading: Heading {
                    head: head(),
                    whole: head(),
                    returns: Some(Type::array(Type::intern(TypeKind::Param(
                        Symbol::intern("T"),
                        0,
                    )))),
                },
                ..site(call())
            };
            let Err(conflict) = c.join_members(&mut members, site.hole, None, None) else {
                panic!("the members should have no candidate");
            };
            c.conflict_kind(site, Some(Span::new(4, 5)), &conflict)
        });
        // THEN the fix writes the return type with `any Seen` for `T`
        let DiagnosticKind::JoinMismatch { fix, .. } = kind else {
            panic!("a conflict should give a `JoinMismatch`");
        };
        let Some(JoinFix::Declare { ty, .. }) = fix else {
            panic!("the fix should write a type after the name");
        };
        assert_eq!(ty, "[any Seen]");
    }

    #[test]
    fn agreeing_members_should_report_the_types_of_the_join() {
        let kind = |form: JoinForm| {
            with_params("", "", |c, _| {
                let site = site(form);
                let origin = Origin::None;
                c.agreed_kind(site, (Type::STRING, Type::U8), origin)
            })
        };
        let wanted = |kind: DiagnosticKind| match kind {
            DiagnosticKind::Mismatch {
                want: Expected::Types(want),
                got,
                ..
            } => (want, got),
            _ => panic!("the members that agree should give a `Mismatch`"),
        };
        // GIVEN the members `a` and `b` of type `u8`, which join under `string`
        // THEN the report names `string` and `u8`, and for an array `[string]` and `[u8]`
        assert_eq!(
            wanted(kind(JoinForm::If)),
            (vec!["string".to_owned()], "u8".to_owned())
        );
        assert_eq!(
            wanted(kind(JoinForm::Array)),
            (vec!["[string]".to_owned()], "[u8]".to_owned())
        );
    }

    #[test]
    fn agreeing_members_should_be_reported_at_the_whole_expression() {
        let heading = Heading {
            head: Span::new(10, 12),
            whole: Span::new(20, 30),
            returns: None,
        };
        let run = |form: JoinForm, build: fn(&mut Checker<'_>, &[Type]) -> Vec<Checked>| {
            with_params(UNITS, "a: u8", |c, types| {
                let mut values = build(c, types);
                let mut members = whole(&mut values);
                let expected = required(Type::STRING);
                let _ = c.join_form(form, heading, &mut members, expected, None);
                found(c)
            })
        };
        // GIVEN `a` and `a` of type `u8` under `string`, THEN E0032 at the whole `[a, a]`
        let both = |_: &mut Checker<'_>, types: &[Type]| {
            vec![Ok(local(types[0], 1)), Ok(local(types[0], 2))]
        };
        assert_eq!(
            run(JoinForm::Array, both),
            vec![(Code::Mismatch, Span::new(20, 30))]
        );
        // GIVEN the branches `u()` and `u()` under `string`, THEN E0043 at the whole `if`
        let units = |c: &mut Checker<'_>, _: &[Type]| {
            vec![Ok(unit_call(c, "u", 1)), Ok(unit_call(c, "u", 2))]
        };
        assert_eq!(
            run(JoinForm::If, units),
            vec![(Code::NoValue, Span::new(20, 30))]
        );
    }
}
