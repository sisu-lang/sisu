//! Assignment: places, which root may be written, the temps of compound operators, and spills.

use std::rc::Rc;

use super::array::{innermost, is_optional_array, optional_array};
use super::expr::{if_some, literal_type, local, operator};
use super::member::{Access, safe_value};
use super::operator::Operator;
use super::unused_names::Used;
use super::{
    BindingKind, Checked, Checker, Declared, Poisoned, UNKNOWN, as_optional, block_expr, payload,
    type_of,
};
use crate::ast;
use crate::diagnostic::{DiagnosticKind, Immutable, UnwrapHelp};
use crate::parser::is_place;
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, ExprKind, Type, TypeKind};

/// A place, flattened. `operands` are its own operands in evaluation order: a field root's
/// base expression, then each index from the root outwards.
struct Path {
    root: Root,
    operands: Vec<tir::Expr>,
}

/// What a write to a place does to the root's binding, which decides the wording of E0001.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Change {
    /// `x = v`.
    Assign,
    /// `x[i] = v`, where `x` is the root of the place.
    Element,
    /// `x.push(v)`.
    Push,
}

impl Change {
    /// E0001 for the root `name`, whose write at `at` its `cause` forbids.
    fn immutable(self, name: String, at: Span, cause: Immutable) -> DiagnosticKind {
        match self {
            Change::Push => DiagnosticKind::PushToImmutable { name, at, cause },
            Change::Assign | Change::Element => DiagnosticKind::AssignToImmutable {
                name,
                at,
                element: self == Change::Element,
                cause,
            },
        }
    }
}

#[derive(Clone, Copy)]
enum Root {
    Local(tir::LocalId),
    /// Field `index` of the object that is the first operand.
    Field(usize),
}

/// The object that the `?.` of a place's root field takes out of an optional. `bind` holds it
/// where `scrutinee` has one, and the place starts there. A write or a `push` of the place runs
/// only there.
struct Guard {
    bind: tir::LocalId,
    scrutinee: tir::Expr,
}

pub(super) struct CheckedPlace {
    /// `None` when a part of the place failed.
    path: Option<Path>,
    /// The type a value written to it needs, the type of what the place stores; `None` is
    /// `Error`.
    ty: Option<Type>,
    /// That type as its declaration writes it, when it names an alias.
    shown: Option<Rc<str>>,
    /// `false` for a root that is a field without `mut`. A name root that cannot be written is
    /// reported but still lowers, as in milestone 2.
    writable: bool,
    /// For a `push`, whose receiver may turn out to be no array: the E0001 of a root that cannot
    /// change, which only an array reports.
    held: Option<DiagnosticKind>,
    /// The object that a `?.` of the root field takes out of an optional. The write of the place
    /// is the branch where it is present.
    guard: Option<Guard>,
}

impl CheckedPlace {
    fn failed() -> Self {
        Self {
            path: None,
            ty: None,
            shown: None,
            writable: false,
            held: None,
            guard: None,
        }
    }

    /// The type of the place as a value: a `?.` of its root field makes it an optional.
    fn value_ty(&self) -> Option<Type> {
        self.ty.map(|ty| {
            if self.guard.is_some() {
                as_optional(ty)
            } else {
                ty
            }
        })
    }
}

impl Checker<'_> {
    /// `target = value`; a root that cannot be written is reported, and `value` still checked.
    pub(super) fn assign(
        &mut self,
        span: Span,
        target: &ast::Expr,
        value: &ast::Expr,
    ) -> Result<tir::Stmt, Poisoned> {
        let place = self.place(span, target, Change::Assign);
        if let (Some(want), Some(text)) = (place.ty, &place.shown) {
            self.show(value.span, want, text);
        }
        let value = self.expect(value, place.ty);
        self.assigned(span, place, value)
    }

    /// The write of the checked `value` to the checked `place`.
    fn assigned(
        &mut self,
        span: Span,
        place: CheckedPlace,
        value: Checked,
    ) -> Result<tir::Stmt, Poisoned> {
        match (place.path, value) {
            (Some(path), Ok(value)) if place.writable => {
                let write = self.write(span, path.root, path.operands, value);
                Ok(match place.guard {
                    None => write,
                    guard => {
                        let block = block_expr(vec![write], None, Type::UNIT, span);
                        tir::Stmt::Expr(guarded(guard, block, span))
                    }
                })
            }
            _ => Err(Poisoned::typed(Type::UNIT)),
        }
    }

    /// Whether a `push` checks its receiver `e` as a place. That holds for a field, or for an
    /// index of a local or of a field. A `?.` may lead to it only when `safe` says the call is
    /// `?.push`: `e?.push(x)` holds a value only where `e` does. An index of such a chain is a
    /// place too: it always fails there, so the help for an element target applies. The place
    /// stamps by the rule of `push_receiver`.
    pub(super) fn is_push_place(&mut self, e: &ast::Expr, safe: bool) -> bool {
        // A receiver checked ahead of its turn is a value already.
        is_place(e)
            && (safe
                || !goes_through_safe_access(e)
                || matches!(e.kind, ast::ExprKind::Index { .. }))
            && !self.body.ahead_at.contains_key(&(e.span, false))
            && self.rooted(e)
    }

    /// Whether the chain of indexes of the place `e` ends in a field or in a local: not in a
    /// constant, a function or an unknown name.
    fn rooted(&mut self, e: &ast::Expr) -> bool {
        match &e.kind {
            ast::ExprKind::Index { base, .. } => self.rooted(base),
            &ast::ExprKind::Name(name) => self.lookup(name).is_some(),
            _ => true,
        }
    }

    /// The receiver of a `push`, checked once as a place. The place waits in `body.pushed` for
    /// `push`, or for the method lookup that finds the receiver is no array. The result is the
    /// value the place holds, which owns the operands of the place.
    ///
    /// A call of a mutating method counts as a change of the place as soon as the receiver is
    /// checked, whatever the call then gets wrong: `stamp_push` stamps it here. A place of a
    /// class, an enum, a type parameter or an associated type is not stamped, because the call
    /// may be a method of its own, which writes nothing.
    pub(super) fn push_receiver(&mut self, receiver: &ast::Expr) -> Checked {
        let mut place = self.place(receiver.span, receiver, Change::Push);
        if !matches!(
            place.ty.map(innermost).map(Type::kind),
            Some(
                TypeKind::Class(..)
                    | TypeKind::Enum(..)
                    | TypeKind::Param(..)
                    | TypeKind::Assoc(..)
            )
        ) {
            self.stamp_push(receiver);
        }
        // A place of type `Error` has reported itself.
        let read = match (place.path.take(), place.ty) {
            (Some(path), Some(_)) => Ok(self.read(build(path.root, path.operands), receiver.span)),
            (_, ty) => Err(Poisoned::of(ty)),
        };
        self.body.pushed = Some((receiver.span, place));
        read
    }

    /// Takes the place that `push_receiver` checked for `receiver`.
    pub(super) fn take_pushed(&mut self, receiver: &ast::Expr) -> Option<CheckedPlace> {
        let (at, place) = self.body.pushed.take()?;
        (at == receiver.span).then_some(place)
    }

    /// The receiver of `receiver?.push(arg)`, which `push_receiver` checked as `read`. The place
    /// may hold an array that is optional, because its own type or a `?.` of its root field makes
    /// it so. Then the call writes the place. The result is `Ok` of the array, as `?.` hands it to
    /// the call, and the place waits for `push`. Else the call is a method of whatever the place
    /// holds. The result is then `Err` of the place read as a value, as a `?.` reads a receiver.
    /// The root field's `?.` binds its object for that value.
    pub(super) fn safe_push_receiver(
        &mut self,
        receiver: &ast::Expr,
        read: Checked,
    ) -> Result<tir::Expr, Checked> {
        let guarded = self
            .body
            .pushed
            .as_ref()
            .is_some_and(|(_, place)| place.guard.is_some());
        let held = type_of(&read).map(|ty| if guarded { as_optional(ty) } else { ty });
        let in_place = held.is_some_and(is_optional_array);
        let read = match read {
            Ok(mut array) if in_place => {
                // The array inside the optional that the place stores.
                if let TypeKind::Optional(payload) = array.ty.kind() {
                    array.ty = payload;
                }
                return Ok(array);
            }
            read => read,
        };
        let place = self
            .take_pushed(receiver)
            .expect("`push_receiver` should have kept the place of a receiver that is a place");
        if let Ok(read) = &read {
            self.read_pushed(read, receiver);
        }
        Err(match place.guard {
            Some(Guard { bind, scrutinee }) => safe_value(receiver.span, bind, Ok(scrutinee), read),
            None => read,
        })
    }

    /// Records the reads of the receiver `e` of a `push` that is no array's, whose value is
    /// `read`. The local is read. The field at the root of the place is read. A write reads
    /// neither.
    pub(super) fn read_pushed(&mut self, read: &tir::Expr, e: &ast::Expr) {
        match (&e.kind, &read.kind) {
            (ast::ExprKind::Index { base, .. }, ExprKind::Index { base: read, .. }) => {
                self.read_pushed(read, base);
            }
            (&ast::ExprKind::Name(name), ExprKind::Local(_)) => {
                let binding = self.lookup(name);
                binding
                    .expect("the root of a checked place should be a binding")
                    .read = true;
            }
            (ast::ExprKind::Field { .. }, ExprKind::Field { base, index }) => {
                let TypeKind::Class(class, _) = base.ty.kind() else {
                    unreachable!("the base of a checked field should be a class")
                };
                self.record(Used::Field(class, *index), e.span);
            }
            _ => unreachable!("the read of a push receiver should match its syntax"),
        }
    }

    /// `receiver.push(arg)`, `receiver` the checked `receiver_expr`. The place of the receiver
    /// is `place`, if `push_receiver` checked it. The receiver is an array, or its type is
    /// unknown. It gives a `unit` block that holds `Push` of the place, after the spills of its
    /// operands and the value. A receiver that no place takes is reported before the argument.
    /// `safe` makes it `receiver?.push(arg)`: a place of an optional type is pushed to where it
    /// holds a value, and a root field's `?.` holds its object in `place.guard`.
    pub(super) fn push(
        &mut self,
        span: Span,
        safe: bool,
        (receiver, place): (Checked, Option<CheckedPlace>),
        receiver_expr: &ast::Expr,
        args: &[ast::Arg],
    ) -> Checked {
        let element = match type_of(&receiver).map(Type::kind) {
            Some(TypeKind::Array(element)) => Some(element),
            _ => None,
        };
        // A receiver of unknown type may be a class whose `push` gives a value, so its failure
        // says nothing about the call's value.
        let unit = || {
            Err(if element.is_some() {
                Poisoned::typed(Type::UNIT)
            } else {
                Poisoned::error()
            })
        };
        let not_pushed = |checker: &mut Self, d: Option<DiagnosticKind>| {
            if let Some(d) = d {
                checker.report(d);
            }
            checker.stray(args);
            unit()
        };
        // `push_receiver` stamped the place that it checked. Only a receiver it did not check
        // is stamped here, so a receiver is stamped once.
        if place.is_none() && is_place(receiver_expr) {
            self.stamp_push(receiver_expr);
        }
        // A receiver that failed has reported itself.
        if receiver.is_err() {
            return not_pushed(self, None);
        }
        if !is_place(receiver_expr) {
            let at = receiver_expr.span;
            return not_pushed(self, Some(DiagnosticKind::PushToTemporary { at }));
        }
        // Only a receiver checked ahead of its turn has no place: its call is an argument, whose
        // `unit` is reported.
        let (Ok(read), Some(mut place)) = (receiver, place) else {
            return not_pushed(self, None);
        };
        if let Some(d) = place.held.take() {
            self.report(d);
        }
        let [arg] = args else {
            let d = DiagnosticKind::ArgumentCount {
                name: self.method_name(read.ty, "push"),
                want: 1,
                found: args.len(),
                at: span,
            };
            return self.rejected(d.into(), args);
        };
        // A label or a mismatch is reported, and an error withholds the `tir`.
        self.labels(args);
        let value = self.expect(&arg.value, element);
        match value {
            Ok(value) if place.writable => {
                let path = unread(read);
                // The place stores an optional that `?.` takes the array out of.
                let some = safe
                    && place
                        .ty
                        .is_some_and(|ty| matches!(ty.kind(), TypeKind::Optional(_)));
                Ok(self.pushed(span, path, (some, place.guard), value))
            }
            _ => unit(),
        }
    }

    /// The unit block of `Push` of the place `path`, after the spill of its operands, and of
    /// `value` unless `some`, as `write` makes a store. With `some`, the place stores an optional
    /// and is written where it holds a value. A `guard` makes the block the branch where its
    /// object is present.
    fn pushed(
        &mut self,
        span: Span,
        path: Path,
        (some, guard): (bool, Option<Guard>),
        value: tir::Expr,
    ) -> tir::Expr {
        let mut operands = path.operands;
        let mut stmts;
        let mut value = value;
        if some {
            // The value runs after the test that the optional holds an array, which mir puts
            // in front of it, so it stays in the `Push`. Only the path is bound, where an exit
            // in the value comes after a counted operand.
            let exits = value.exits();
            (stmts, operands) = self.spill_before(operands, exits);
        } else {
            operands.push(value);
            (stmts, operands) = self.spill(operands);
            value = operands
                .pop()
                .expect("`spill` should return one operand per input");
        }
        let mut place = build(path.root, operands);
        if some {
            place = tir::Place::Some(Box::new(place));
        }
        stmts.push(tir::Stmt::Push { place, value, span });
        let block = block_expr(stmts, None, Type::UNIT, span);
        guarded(guard, block, span)
    }

    /// `x op= e` is `x = x op e`. On any other place, each of its operands is bound to a fresh
    /// local first, in source order, so it runs once: `p[e] op= v` is
    /// `Block { let t = e; Assign { p[t], p[t] op v } }`. The root binding is never copied, and
    /// neither is an operand that is an immutable local, which `v` cannot change: `p[i] op= v`
    /// is `Assign { p[i], p[i] op v }`.
    /// `op` takes `u8`s when the place holds a `u8`, else `i64`s; `+=` on a `string` joins. A place
    /// is reported as `binary` reports a local of its type, and a `char` value is reported alone:
    /// a `string?` beside `+=` against `string`; an `i64?`, `u8?` or `f64?` against its payload; a
    /// place of any other type than `op` takes against all the types it takes. `op` comes with its
    /// token's span.
    pub(super) fn compound_assign(
        &mut self,
        span: Span,
        (op, op_span): (ast::BinaryOp, Span),
        target: &ast::Expr,
        value: &ast::Expr,
    ) -> Result<tir::Stmt, Poisoned> {
        let unit = || Poisoned::typed(Type::UNIT);
        if let ast::ExprKind::Name(name) = target.kind {
            // The target is checked as a value only to give its value back to it: no read.
            let outer = self.body.quiet.replace(target.span);
            let result = self.compound_assign_name(span, (op, op_span), (target, name), value);
            self.body.quiet = outer;
            return result;
        }
        let place = self.place(span, target, Change::Assign);
        let Some(ty) = place.ty else {
            // Only its own mistakes are left to report.
            drop(self.expr(value, Some(UNKNOWN)));
            return Err(unit());
        };
        if ty == Type::CHAR {
            self.char_compound(op_span, value);
            return Err(unit());
        }
        let (op, _) = operator(op).expect(
            "the parser should produce only arithmetic operators for a compound assignment",
        );
        let mut lets = Vec::new();
        let path = place.path.map(|path| {
            let temps: Vec<_> = path
                .operands
                .into_iter()
                .map(|init| {
                    // Nothing can reassign an immutable local, so it is read twice.
                    if let ExprKind::Local(l) = init.kind
                        && !self.body.locals[l.0].mutable
                    {
                        return (l, init.ty, init.span);
                    }
                    let temp = self.fresh(init.ty);
                    let read = (temp, init.ty, init.span);
                    lets.push(tir::Stmt::Let { local: temp, init });
                    read
                })
                .collect();
            (path.root, temps)
        });
        let reads = |temps: &[(tir::LocalId, Type, Span)]| {
            temps
                .iter()
                .map(|(temp, ty, span)| local(*temp, *ty, *span))
                .collect::<Vec<_>>()
        };
        let read = match &path {
            Some((root, temps)) => Ok(self.read(build(*root, reads(temps)), target.span)),
            None => Err(Poisoned::typed(ty)),
        };
        let value = self.compound_value(span, (op, op_span), target, value, (ty, read));
        let (Some((root, temps)), Ok(value), true) = (path, value, place.writable) else {
            return Err(unit());
        };
        let operands = reads(&temps);
        lets.push(self.write(span, root, operands, value));
        let block = block_expr(lets, None, Type::UNIT, span);
        Ok(tir::Stmt::Expr(guarded(place.guard, block, span)))
    }

    /// `compound_assign` of the name `target`, which is `name`.
    fn compound_assign_name(
        &mut self,
        span: Span,
        (op, op_span): (ast::BinaryOp, Span),
        (target, name): (&ast::Expr, Symbol),
        value: &ast::Expr,
    ) -> Result<tir::Stmt, Poisoned> {
        // `assign` already reports an unknown target; a synthetic read would repeat it.
        if self.lookup(name).is_none() {
            return self.assign(span, target, value);
        }
        // A `char` has no arithmetic, and a rewrite of the statement would break the
        // assignment: `target` stands for the value, which `assign` accepts.
        if self.lookup(name).is_some_and(|b| b.ty == Some(Type::CHAR)) {
            self.char_compound(op_span, value);
            return self.assign(span, target, target);
        }
        // `assign` of `target op value`, whose `op` spans the statement.
        let place = self.place(span, target, Change::Assign);
        let want = place.ty;
        let checked = self.binary(
            span,
            (op, op_span),
            target,
            value,
            Some(want.unwrap_or(UNKNOWN)),
            true,
        );
        let expanded = ast::Expr {
            kind: ast::ExprKind::Binary {
                op,
                op_span,
                lhs: Box::new(target.clone()),
                rhs: Box::new(value.clone()),
            },
            span,
        };
        let (checked, _) = self.settled(&expanded, checked, want);
        self.assigned(span, place, checked)
    }

    /// The operator `op` of a compound assignment over the place `target`, of type `ty`, whose
    /// current value is `read`, and `value`: a join if the place holds a `string` or a `string?`
    /// and `op` is `+`, else a number operator. A place that is none of the operand types is
    /// reported as `integers` does, after `value`: a `char` value is reported alone.
    fn compound_value(
        &mut self,
        span: Span,
        (op, op_span): (tir::BinaryOp, Span),
        target: &ast::Expr,
        value: &ast::Expr,
        (ty, read): (Type, Checked),
    ) -> Checked {
        let want = literal_type(Some(ty));
        let operator = if op == tir::BinaryOp::Add {
            Operator::Plus { compound: true }
        } else {
            Operator::Arithmetic
        };
        if payload(ty) == Type::STRING && op == tir::BinaryOp::Add {
            let start = self.diagnostics.len();
            // A `string?` place is what `binary` reports first, and the value is checked with no
            // expected type.
            let checked = self.value(value, Some(if ty == Type::STRING { ty } else { UNKNOWN }));
            if ty != Type::STRING && type_of(&checked) == Some(Type::CHAR) {
                self.report(DiagnosticKind::ArithmeticOnChar { at: op_span });
                return Err(Poisoned::error());
            }
            self.label_join_literals(start, [target, value]);
            let (value, mismatched) = if ty != Type::STRING {
                let d = self.concat_mismatch(target, ty, &[target, value], true, type_of(&checked));
                self.diagnostics.push(d);
                (Err(Poisoned::error()), true)
            } else if let Some(got) = type_of(&checked).filter(|&got| got != ty) {
                // As in `binary`: the place is the `string` that the value is joined to.
                // A rejected expected type leaves no type, so `got` is the value's own.
                let d = self.concat_mismatch(value, got, &[target, value], true, Some(ty));
                self.diagnostics.push(d);
                (Err(Poisoned::error()), true)
            } else {
                self.settled(value, checked, Some(ty))
            };
            self.concat(span, read, value, mismatched)
        } else {
            // The place's own mismatch waits for `arithmetic`, which drops it beside a `char`.
            let start = self.diagnostics.len();
            let place = self.settled_by(target, read, Some(want), Some(operator.takes()));
            let held = self.diagnostics.drain(start..).collect();
            self.arithmetic(span, (op, op_span), want, place, value, held)
        }
    }

    /// Reports the compound assignment, at its operator `op_span`, to a `char` place, and checks
    /// `value` for its own mistakes.
    fn char_compound(&mut self, op_span: Span, value: &ast::Expr) {
        self.report(DiagnosticKind::ArithmeticOnChar { at: op_span });
        // Only its diagnostics matter.
        drop(self.expr(value, Some(UNKNOWN)));
    }

    /// The constant `name` as the root of a place that `span` assigns, reported, if `name` names
    /// one. The value is still checked against the constant's type. A broken constant poisons
    /// the assignment without a diagnostic.
    fn constant_place(&mut self, span: Span, name: Symbol, change: Change) -> Option<CheckedPlace> {
        let Declared::Known(constant) = self.constants.get(&name)? else {
            return Some(CheckedPlace::failed());
        };
        let (declared, ty) = (constant.span, constant.ty);
        self.report(change.immutable(name.to_string(), span, Immutable::Constant { declared }));
        Some(CheckedPlace {
            path: None,
            ty,
            shown: None,
            writable: false,
            held: None,
            guard: None,
        })
    }

    /// Reports E0001 for the root `name` that `change` writes at `at`, or for a `push` holds it
    /// back: its receiver may be no array.
    fn forbid(
        &mut self,
        change: Change,
        name: String,
        at: Span,
        cause: Immutable,
    ) -> Option<DiagnosticKind> {
        let d = change.immutable(name, at, cause);
        if change == Change::Push {
            return Some(d);
        }
        self.report(d);
        None
    }

    /// Stamps what the place `target` of a `push` writes, as `place` stamps a write. A binding at
    /// the root of a chain of indexes is reassigned. The root of a field's base is written.
    fn stamp_push(&mut self, target: &ast::Expr) {
        let mut root = target;
        while let ast::ExprKind::Index { base, .. } = &root.kind {
            root = base;
        }
        let &ast::ExprKind::Name(name) = &root.kind else {
            return self.stamp_root(root);
        };
        let stamp = self.write_stamp();
        if let Some(b) = self.lookup(name) {
            b.written = stamp;
            b.reassigned |= b.kind == BindingKind::Var;
        }
    }

    /// Counts a write, and gives the stamp to put on what it writes.
    fn write_stamp(&mut self) -> u32 {
        self.body.writes += 1;
        self.body.writes
    }

    /// Stamps the binding, or `self`, at the root of `base`, the base of a written field, which
    /// this follows to its root once. A root that is neither stamps nothing.
    fn stamp_root(&mut self, mut base: &ast::Expr) {
        loop {
            match &base.kind {
                ast::ExprKind::Field { base: inner, .. }
                | ast::ExprKind::Index { base: inner, .. } => {
                    base = inner;
                }
                &ast::ExprKind::Name(name) => {
                    let stamp = self.write_stamp();
                    if let Some(b) = self.lookup(name) {
                        b.written = stamp;
                    }
                    return;
                }
                ast::ExprKind::SelfValue => {
                    self.body.self_written = self.write_stamp();
                    return;
                }
                _ => return,
            }
        }
    }

    /// The place of the local, constant or unknown name `name`, the target `target` that `span`
    /// changes as `change` says.
    fn name_place(
        &mut self,
        span: Span,
        target: &ast::Expr,
        name: Symbol,
        change: Change,
    ) -> CheckedPlace {
        // A `push` stamps by the rule of `push_receiver`.
        let stamp = (change != Change::Push).then(|| self.write_stamp());
        let Some(b) = self.lookup(name) else {
            if let Some(place) = self.constant_place(span, name, change) {
                return place;
            } else if self.is_function(name) {
                self.report(DiagnosticKind::AssignToFunction {
                    name: name.to_string(),
                    at: target.span,
                });
            } else {
                let d = self.not_found(name, target.span);
                self.diagnostics.push(d);
                // The name may have meant any open binding; none of them warns as
                // unassigned.
                for b in self.body.scopes.iter_mut().flat_map(|s| s.values_mut()) {
                    b.reassigned = true;
                }
            }
            return CheckedPlace::failed();
        };
        if let Some(stamp) = stamp {
            b.written = stamp;
            b.reassigned |= b.kind == BindingKind::Var;
        }
        let (local, ty, kind, declared) = (b.local, b.ty, b.kind, b.span);
        let shown = b.shown.clone();
        let cause = match kind {
            BindingKind::Var => None,
            BindingKind::Let => Some(Immutable::Let {
                declared,
                redeclared: b.shadows_mut,
            }),
            BindingKind::Param => Some(Immutable::Param { declared }),
            BindingKind::Unwrapped(keyword) => Some(Immutable::Unwrapped { keyword, declared }),
        };
        let held = cause.and_then(|cause| self.forbid(change, name.to_string(), span, cause));
        CheckedPlace {
            path: Some(Path {
                root: Root::Local(local),
                operands: Vec::new(),
            }),
            ty,
            shown,
            writable: true,
            held,
            guard: None,
        }
    }

    /// Reports E0062 at `base`, the optional array of an element target whose place is `inner`.
    /// An `if let` of the array binds a copy, which is no place. The help names the object of the
    /// `?.` only when that object is what makes the array optional: the `?.` field stores an
    /// array, and the object is a class, so its field is a place. Else no unwrap gives a place.
    fn optional_array_target(&mut self, base: &ast::Expr, inner: &CheckedPlace) {
        let unwrap = match (&base.kind, &inner.guard, inner.ty.map(Type::kind)) {
            (
                ast::ExprKind::Field {
                    base: object,
                    safe: true,
                    ..
                },
                Some(_),
                Some(TypeKind::Array(_)),
            ) if self.one_line(object.span) => {
                UnwrapHelp::Object(self.source.text(object.span).to_string())
            }
            _ => UnwrapHelp::Nothing,
        };
        self.diagnostics.push(optional_array(base, unwrap));
    }

    /// The place `target` names, which `span` changes as `change` says: `Element` when it is the
    /// base of an index, so the write replaces an element of it.
    fn place(&mut self, span: Span, target: &ast::Expr, change: Change) -> CheckedPlace {
        match &target.kind {
            &ast::ExprKind::Name(name) => self.name_place(span, target, name, change),
            ast::ExprKind::Field {
                base, name, safe, ..
            } => {
                let access = if change == Change::Push {
                    Access::Push
                } else {
                    Access::Assign
                };
                if change != Change::Push {
                    self.stamp_root(base);
                }
                let checked = self.value(base, None);
                let mut guard = None;
                // `?.` takes the object out of its optional, where the place starts. The write
                // of the place is the branch where the object is present.
                let checked = if *safe {
                    match (self.safe_payload(&checked, base, name), checked) {
                        (Ok(payload), Ok(scrutinee)) => {
                            let bind = self.fresh(payload);
                            guard = Some(Guard { bind, scrutinee });
                            Ok(local(bind, payload, base.span))
                        }
                        (payload, _) => {
                            // As `?.` does, report the field and give no type.
                            self.find_field(
                                payload.err().and_then(|failed| failed.ty),
                                base,
                                name,
                                access,
                            );
                            return CheckedPlace::failed();
                        }
                    }
                } else {
                    checked
                };
                let Some((class, index, ty)) =
                    self.find_field(type_of(&checked), base, name, access)
                else {
                    return CheckedPlace::failed();
                };
                let field = &self.classes[class.0].fields[index];
                let (writable, shown) = (field.mutable, field.shown.clone());
                let held = if writable {
                    None
                } else {
                    let (name, declared) = (field.name.name.to_string(), field.name.span);
                    self.forbid(change, name, span, Immutable::Field { declared })
                };
                CheckedPlace {
                    path: checked.ok().map(|base| Path {
                        root: Root::Field(index),
                        operands: vec![base],
                    }),
                    ty,
                    shown,
                    writable,
                    held,
                    guard,
                }
            }
            ast::ExprKind::Index { base, index } => {
                let inner_change = if change == Change::Push {
                    Change::Push
                } else {
                    Change::Element
                };
                let outer = std::mem::replace(&mut self.body.indexed, true);
                let inner = self.place(span, base, inner_change);
                let element = match inner.value_ty() {
                    Some(ty) if is_optional_array(ty) => {
                        self.optional_array_target(base, &inner);
                        None
                    }
                    ty => self.element(base, ty),
                };
                // The index is not part of the chain that it ends.
                self.body.indexed = false;
                let (index, mismatched) = self.operand(index, Some(Type::I64));
                self.body.indexed = outer;
                let path = inner
                    .path
                    .filter(|_| element.is_some())
                    .zip(index.ok())
                    .map(|(mut path, index)| {
                        path.operands.push(index);
                        path
                    });
                CheckedPlace {
                    path,
                    ty: element.filter(|_| !mismatched),
                    shown: None,
                    writable: inner.writable,
                    held: inner.held,
                    guard: None,
                }
            }
            _ => unreachable!("the parser should make only a name, a field or an index a place"),
        }
    }

    /// `Assign { place, value }`, the place built from `root` and `operands`, after `spill`
    /// binds what it must of the operands and the value. A field or element place whose value
    /// is `P + t`, `P` reading the place and `t` unable to exit, is `Append { place, t }`
    /// instead; nothing in it can exit, so nothing spills.
    fn write(
        &mut self,
        span: Span,
        root: Root,
        mut operands: Vec<tir::Expr>,
        value: tir::Expr,
    ) -> tir::Stmt {
        operands.push(value);
        let (mut stmts, mut operands) = self.spill(operands);
        let value = operands
            .pop()
            .expect("`spill` should return one operand per input");
        let place = build(root, operands);
        // `!rhs.exits()` is defence: `concat` already spills an rhs that can exit, so this never
        // sees one (`places::does_not_recognise`'s `(if c { return } ...)` row covers the spill).
        let append = !matches!(place, tir::Place::Local(_))
            && matches!(&value.kind, ExprKind::Concat { lhs, rhs }
                if !rhs.exits() && same_place(&place, lhs));
        let assign = match value.kind {
            ExprKind::Concat { rhs, .. } if append => tir::Stmt::Append { place, value: *rhs },
            _ => tir::Stmt::Assign { place, value },
        };
        if stmts.is_empty() {
            return assign;
        }
        stmts.push(assign);
        tir::Stmt::Expr(block_expr(stmts, None, Type::UNIT, span))
    }

    /// The value `place` holds, read at `span`.
    fn read(&self, place: tir::Place, span: Span) -> tir::Expr {
        let (kind, ty) = match place {
            tir::Place::Local(id) => (ExprKind::Local(id), self.body.locals[id.0].ty),
            tir::Place::Field { base, index } => {
                let ty = self.field_type(base.ty, index).expect(
                    "a place whose type is known should not be a field whose type is `Error`",
                );
                let base = Box::new(base);
                (ExprKind::Field { base, index }, ty)
            }
            tir::Place::Index { base, index } => {
                let base = self.read(*base, span);
                let TypeKind::Array(element) = base.ty.kind() else {
                    panic!("an indexed place should hold an array, as `element` checked")
                };
                let ty = element;
                let (base, index) = (Box::new(base), Box::new(index));
                (ExprKind::Index { base, index }, ty)
            }
            tir::Place::Some(_) => {
                unreachable!("only a `push` has a `Some` step, and a `push` reads no place")
            }
        };
        tir::Expr { kind, ty, span }
    }
}

/// Whether a `?.` sits anywhere in the chain of fields and indexes that makes up the place `e`.
fn goes_through_safe_access(e: &ast::Expr) -> bool {
    match &e.kind {
        ast::ExprKind::Field { base, safe, .. } => *safe || goes_through_safe_access(base),
        ast::ExprKind::Index { base, .. } => goes_through_safe_access(base),
        _ => false,
    }
}

/// `block`, the write of a place, as the branch where the object of the place's `guard` is
/// present: the right side runs there only. Without a `guard`, `block` as it is.
fn guarded(guard: Option<Guard>, block: tir::Expr, span: Span) -> tir::Expr {
    match guard {
        Some(Guard { bind, scrutinee }) => if_some(bind, scrutinee, block, None, span),
        None => block,
    }
}

/// Whether `read` reads `place`: the same root local, the same fields, and index
/// expressions that are the same local or the same integer literal.
fn same_place(place: &tir::Place, read: &tir::Expr) -> bool {
    match (place, &read.kind) {
        (tir::Place::Local(local), ExprKind::Local(read)) => local == read,
        (
            tir::Place::Field { base, index },
            ExprKind::Field {
                base: read,
                index: field,
            },
        ) => index == field && same_read(base, read),
        (
            tir::Place::Index { base, index },
            ExprKind::Index {
                base: read,
                index: at,
            },
        ) => same_index(index, at) && same_place(base, read),
        _ => false,
    }
}

/// Whether `a` and `b` read the same place, as `same_place` decides.
fn same_read(a: &tir::Expr, b: &tir::Expr) -> bool {
    match (&a.kind, &b.kind) {
        (ExprKind::Local(a), ExprKind::Local(b)) => a == b,
        (ExprKind::Field { base: a, index: i }, ExprKind::Field { base: b, index: j }) => {
            i == j && same_read(a, b)
        }
        (ExprKind::Index { base: a, index: i }, ExprKind::Index { base: b, index: j }) => {
            same_index(i, j) && same_read(a, b)
        }
        _ => false,
    }
}

/// Whether two indexes are the same local or the same integer literal.
fn same_index(a: &tir::Expr, b: &tir::Expr) -> bool {
    match (&a.kind, &b.kind) {
        (ExprKind::Local(a), ExprKind::Local(b)) => a == b,
        (ExprKind::Int(a), ExprKind::Int(b)) => a == b,
        _ => false,
    }
}

/// The path of the place that `read` holds, the inverse of `Checker::read`.
fn unread(read: tir::Expr) -> Path {
    match read.kind {
        ExprKind::Local(id) => Path {
            root: Root::Local(id),
            operands: Vec::new(),
        },
        ExprKind::Field { base, index } => Path {
            root: Root::Field(index),
            operands: vec![*base],
        },
        ExprKind::Index { base, index } => {
            let mut path = unread(*base);
            path.operands.push(*index);
            path
        }
        _ => unreachable!("`push_receiver` should read a place as a local, a field or an index"),
    }
}

/// The place `root` names, with `operands` as its field base and indexes.
fn build(root: Root, operands: Vec<tir::Expr>) -> tir::Place {
    let mut operands = operands.into_iter();
    let mut place = match root {
        Root::Local(id) => tir::Place::Local(id),
        Root::Field(index) => tir::Place::Field {
            base: operands
                .next()
                .expect("a field root's base should be the first operand of its path"),
            index,
        },
    };
    for index in operands {
        place = tir::Place::Index {
            base: Box::new(place),
            index,
        };
    }
    place
}
