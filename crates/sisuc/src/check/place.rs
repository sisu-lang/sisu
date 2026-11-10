//! Assignment: places, which root may be written, the temps of compound operators, and spills.

use std::rc::Rc;

use super::array::{innermost, is_optional_array, optional_array, optional_element};
use super::expr::{if_some, literal_type, local, operator};
use super::member::{Access, FoundField, safe_value};
use super::operator::Operator;
use super::reports::Reported;
use super::unused_names::Used;
use super::{
    BindingKind, Checked, Checker, Declared, Expectation, Poisoned, as_optional, block_expr,
    known_type, payload, type_of,
};
use crate::ast;
use crate::diagnostic::{DiagnosticKind, Immutable, IndexUsage, UnwrapHelp};
use crate::parser::{is_place, is_range_target};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, ExprKind, Type, TypeKind};

/// A place, flattened. `operands` are its own operands in evaluation order: a field root's
/// base expression, then each index from the root outwards.
pub(super) struct Path {
    pub(super) root: Root,
    pub(super) operands: Vec<tir::Expr>,
}

/// What a write to a place does to the root's binding, which decides the wording of E0001.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Change {
    /// `x = v`.
    Assign,
    /// `x[i] = v`, where `x` is the root of the place.
    Element,
    /// `x.push(v)`.
    Push,
    /// `mut x` as an argument: the place gives its cause when it cannot change, and reports none.
    MutArg,
}

impl Change {
    /// How the index that this change reaches uses its element: an assignment writes it, when
    /// it is the whole target. A `push` and a deeper index only read it.
    fn index_usage(self) -> IndexUsage {
        if self == Change::Assign {
            IndexUsage::Write
        } else {
            IndexUsage::Read
        }
    }

    /// E0001 for the root `name`, whose write at `at` its `cause` forbids.
    fn immutable(self, name: String, at: Span, cause: Immutable) -> DiagnosticKind {
        match self {
            Change::Push => DiagnosticKind::PushToImmutable { name, at, cause },
            Change::MutArg => {
                unreachable!("a `mut` argument gives its cause back, and reports none")
            }
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
pub(super) enum Root {
    Local(tir::LocalId),
    /// Field `index` of the object that is the first operand.
    Field(usize),
}

/// The object that the `?.` of a place's root field takes out of an optional. `bind` holds it
/// where `scrutinee` has one, and the place starts there. A write or a `push` of the place runs
/// only there. The `scrutinee` of an object that an E0062 typed is `Err`: its place has no path.
pub(super) struct Guard {
    bind: tir::LocalId,
    scrutinee: Checked,
}

pub(super) struct CheckedPlace {
    /// `None` when a part of the place failed.
    pub(super) path: Option<Path>,
    /// The type a value written to it needs, the type of what the place stores; `None` is
    /// `Error`.
    pub(super) ty: Option<Type>,
    /// That type as its declaration writes it, when it names an alias.
    shown: Option<Rc<str>>,
    /// `false` for a root that is a field without `mut`. A name root that cannot be written is
    /// reported but still lowers, as in milestone 2.
    writable: bool,
    /// For a `push`, whose receiver may turn out to be no array: the E0001 of a root that cannot
    /// change, which only an array reports.
    held: Option<DiagnosticKind>,
    /// For a `push`: the type that a root field under a failed `.` declares, through the indexes
    /// of the place that do not fail. The place has no `ty`, but its field still has this type.
    held_ty: Option<Type>,
    /// The object that a `?.` of the root field takes out of an optional. The write of the place
    /// is the branch where it is present.
    pub(super) guard: Option<Guard>,
    /// For a `mut` argument: why the place cannot change, which E0169 reports.
    pub(super) refused: Option<Immutable>,
    /// For a `mut` argument: the `?.` along the path, which makes the place's read type and
    /// write type differ.
    pub(super) chain: Option<Chain>,
    /// The token of the first error of the place: a failed part, a part with no type, or a root
    /// that cannot change. A `push` holds the E0001 of its root in `held`.
    pub(super) reported: Option<Reported>,
}

/// The `?.` along the path of a `mut` argument.
pub(super) struct Chain {
    /// The text before the first `?.`, when it is on one line.
    pub(super) name: Option<String>,
}

impl CheckedPlace {
    /// A place that failed, by the error `reported` when it gave one.
    pub(super) fn failed(reported: Option<Reported>) -> Self {
        Self {
            path: None,
            ty: None,
            shown: None,
            writable: false,
            held: None,
            held_ty: None,
            guard: None,
            refused: None,
            chain: None,
            reported,
        }
    }

    /// The type of the place as a value: a `?.` of its root field makes it an optional.
    pub(super) fn value_ty(&self) -> Option<Type> {
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
        if let Some(reported) = self.assign_to_range(target, None, value) {
            return reported;
        }
        let place = self.place(span, target, Change::Assign);
        if let (Some(want), Some(text)) = (place.ty, &place.shown) {
            self.show(value.span, want, text);
        }
        let value = self.expect(value, Expectation::or_poisoned(place.ty));
        self.inferred_by_write(target, &value, span.end());
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
            (_, value) => {
                let reported = (value.err().map(|failed| failed.reported()))
                    .or(place.reported)
                    .expect("a write that fails should have a failed part");
                Err(Poisoned::typed(Type::UNIT, reported))
            }
        }
    }

    /// Whether a `push` checks its receiver `e` as a place. That holds for a field or an index
    /// whose chain `rooted` accepts, whether or not a `?.` leads to it. The place stamps by the
    /// rule of `push_receiver`.
    pub(super) fn is_push_place(&mut self, e: &ast::Expr) -> bool {
        // A receiver checked ahead of its turn is a value already.
        is_place(e) && !self.body.ahead_at.contains_key(&(e.span, false)) && self.rooted(e)
    }

    /// Whether the chain of indexes and fields of the place `e` ends in a local or in an
    /// expression such as a call, whose field is a place. It ends in no constant, function, type
    /// or unknown name.
    pub(super) fn rooted(&mut self, e: &ast::Expr) -> bool {
        match &e.kind {
            ast::ExprKind::Index { base, .. } | ast::ExprKind::Field { base, .. } => {
                self.rooted(base)
            }
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
            (_, ty) => Err(Poisoned::new(
                ty,
                place
                    .reported
                    .expect("a place with no path or type should have failed"),
            )),
        };
        self.body.pushed = Some((receiver.span, place));
        read
    }

    /// Takes the place that `push_receiver` checked for `receiver`.
    pub(super) fn take_pushed(&mut self, receiver: &ast::Expr) -> Option<CheckedPlace> {
        let (at, place) = self.body.pushed.take()?;
        (at == receiver.span).then_some(place)
    }

    /// Whether the `?.` of the root field of the place that `push_receiver` checked makes its
    /// value optional.
    fn pushed_guarded(&self) -> bool {
        self.body
            .pushed
            .as_ref()
            .is_some_and(|(_, place)| place.guard.is_some())
    }

    /// The receiver of `receiver?.push(arg)`, which `push_receiver` checked as `read`. The place
    /// may hold an array that is optional, because its own type or a `?.` of its root field makes
    /// it so. Then the call writes the place. The result is `Ok` of the array, as `?.` hands it to
    /// the call, and the place waits for `push`. Else the call is a method of whatever the place
    /// holds. The result is then `Err` of the place read as a value (`pushed_value`), as a `?.`
    /// reads a receiver.
    pub(super) fn safe_push_receiver(
        &mut self,
        receiver: &ast::Expr,
        read: Checked,
    ) -> Result<tir::Expr, Checked> {
        let in_place = held_array_layers(type_of(&read), self.pushed_guarded()) == 1;
        match read {
            Ok(mut array) if in_place => {
                // The array inside the optional that the place stores.
                if let TypeKind::Optional(payload) = array.ty.kind() {
                    array.ty = payload;
                }
                Ok(array)
            }
            read => Err(self.pushed_value(receiver, read)),
        }
    }

    /// Whether a `.push` on the place that `push_receiver` checked, with the value `read`, goes
    /// through `pushed_value`: the place holds an optional, or a `?.` of its root field makes it
    /// one.
    pub(super) fn pushed_may_be_none(&self, read: &Checked) -> bool {
        self.pushed_guarded()
            || type_of(read).is_some_and(|ty| matches!(ty.kind(), TypeKind::Optional(_)))
    }

    /// Takes the place that `push_receiver` checked for `receiver`. Gives it as the value `read`
    /// that a call reads. A `?.` of its root field wraps `read` where its object is present. A
    /// place that holds an optional array, of any depth, reports its E0001 now, before the call
    /// reports E0062. The place is then no more: the call writes nothing.
    pub(super) fn pushed_value(&mut self, receiver: &ast::Expr, read: Checked) -> Checked {
        #[cfg(test)]
        super::PUSHED_VALUES.set(super::PUSHED_VALUES.get() + 1);
        let mut place = self
            .take_pushed(receiver)
            .expect("`push_receiver` should have kept the place of a receiver that is a place");
        // A place under a failed `.` is untyped, but its field still declares one.
        if held_array_layers(type_of(&read).or(place.held_ty), place.guard.is_some()) > 0
            && let Some(d) = place.held.take()
        {
            self.error(d);
        }
        if let Ok(read) = &read {
            self.read_pushed(read, receiver);
        }
        match place.guard {
            Some(Guard { bind, scrutinee }) => safe_value(receiver.span, bind, scrutinee, read),
            None => read,
        }
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
    /// A receiver that is `Err` but an array, such as the payload of an optional that E0062
    /// reported, gets the same checks and pushes nothing. `safe` makes it `receiver?.push(arg)`:
    /// a place of an optional type is pushed to where it holds a value, and a root field's `?.`
    /// holds its object in `place.guard`.
    pub(super) fn push(
        &mut self,
        span: Span,
        safe: bool,
        (receiver, place): (Checked, Option<CheckedPlace>),
        receiver_expr: &ast::Expr,
        args: &[ast::Arg],
    ) -> Checked {
        let array = type_of(&receiver);
        let element = match array.map(Type::kind) {
            Some(TypeKind::Array(element)) => Some(element),
            _ => None,
        };
        // A receiver of unknown type may be a class whose `push` gives a value, so its failure
        // says nothing about the call's value.
        let unit = |reported| Err(Poisoned::new(element.map(|_| Type::UNIT), reported));
        let not_pushed = |checker: &mut Self, reported| {
            checker.stray(args);
            unit(reported)
        };
        // `push_receiver` stamped the place that it checked. Only a receiver it did not check
        // is stamped here, so a receiver is stamped once.
        // So is a range index: `xs[0..true].push(4)` reports its bound alone, with no warning that
        // `xs` is never reassigned.
        if place.is_none() && (is_place(receiver_expr) || is_range_target(receiver_expr)) {
            self.stamp_push(receiver_expr);
        }
        // A receiver that failed has reported itself.
        if let Err(failed) = &receiver {
            let failed = failed.reported();
            // A local with no type is not pushed to, but the element gives its type. The element
            // is checked once, as `stray` checks it.
            if let [arg] = args
                && let Some(local) = self.pushed_to_uninferred(receiver_expr)
            {
                if let Ok(pushed) = self.expr(&arg.value, Expectation::Poisoned) {
                    self.inferred_by_use(local, pushed.ty, arg.value.span.end());
                }
                return unit(failed);
            }
            // An array that no value reads, such as the payload of an optional, is checked as the
            // `Ok` path checks it, with no value to push.
            if element.is_none() {
                // A field under a failed `.` still names its array, so a field that cannot
                // change gets its E0001.
                if let Some(place) = place
                    && place
                        .held_ty
                        .is_some_and(|t| matches!(payload(t).kind(), TypeKind::Array(_)))
                    && let Some(d) = place.held
                {
                    self.error(d);
                }
                return not_pushed(self, failed);
            }
        }
        if !is_place(receiver_expr) {
            let at = receiver_expr.span;
            let range = self.push_range(receiver_expr, args);
            let reported = self.error(DiagnosticKind::PushToTemporary { at, range });
            return not_pushed(self, reported);
        }
        let (read, mut place) = match (receiver, place) {
            (Ok(read), Some(place)) => (Ok(read), Some(place)),
            // Only a receiver checked ahead of its turn has no place: its call is an argument,
            // whose `unit` is reported.
            (Ok(_), None) => {
                let reported = self.proof(span); // proof: held misfit
                return not_pushed(self, reported);
            }
            (Err(failed), place) => (Err(failed.reported()), place),
        };
        let held = (place.as_mut().and_then(|place| place.held.take())).map(|d| self.error(d));
        let [arg] = args else {
            let d = DiagnosticKind::ArgumentCount {
                name: self.method_name(
                    array.expect(
                        "a receiver that is `Ok`, or `Err` with an element, should have a type",
                    ),
                    "push",
                ),
                want: 1,
                found: args.len(),
                at: span,
            };
            return self.rejected(d.into(), args, element.map(|_| Type::UNIT));
        };
        // A label or a mismatch is reported, and an error withholds the `tir`.
        self.labels(args);
        let value = self.expect(&arg.value, Expectation::or_poisoned(element));
        match (value, read, place) {
            (Ok(value), Ok(read), Some(place)) if place.writable => {
                let path = unread(read);
                // The place stores an optional that `?.` takes the array out of.
                let some = safe
                    && place
                        .ty
                        .is_some_and(|ty| matches!(ty.kind(), TypeKind::Optional(_)));
                Ok(self.pushed(span, path, (some, place.guard), value))
            }
            (value, read, place) => {
                let reported = (value.err().map(|failed| failed.reported()))
                    .or_else(|| read.err())
                    .or(held)
                    .or_else(|| place.and_then(|place| place.reported))
                    .expect("a `push` that fails should have a failed part");
                unit(reported)
            }
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
        let unit = |reported| Poisoned::typed(Type::UNIT, reported);
        if let Some(reported) = self.assign_to_range(target, Some(op), value) {
            return reported;
        }
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
            drop(self.expr(value, Expectation::Poisoned));
            return Err(unit(
                place
                    .reported
                    .expect("a place with no type should have failed"),
            ));
        };
        if ty == Type::CHAR {
            let reported = self.char_compound(op_span, value);
            return Err(unit(reported));
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
            None => Err(Poisoned::typed(
                ty,
                place
                    .reported
                    .expect("a place with no path should have failed"),
            )),
        };
        let value = self.compound_value(span, (op, op_span), target, value, (ty, read));
        let failed = (value.as_ref().err().map(Poisoned::reported)).or(place.reported);
        let (Some((root, temps)), Ok(value), true) = (path, value, place.writable) else {
            return Err(unit(failed.expect(
                "a compound assignment that fails should have a failed part",
            )));
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
            Expectation::or_poisoned(want),
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
        let (checked, _) = self.settled(&expanded, checked, Expectation::or_poisoned(want));
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
            let expected = if ty == Type::STRING {
                Expectation::Required(ty)
            } else {
                Expectation::Poisoned
            };
            let checked = self.value(value, expected);
            if ty != Type::STRING && type_of(&checked) == Some(Type::CHAR) {
                let reported = self.error(DiagnosticKind::ArithmeticOnChar { at: op_span });
                return Err(Poisoned::untyped(reported));
            }
            self.label_join_literals(start, [target, value]);
            let (value, mismatched) = if ty != Type::STRING {
                let d = self.concat_mismatch(target, ty, &[target, value], true, type_of(&checked));
                (Err(Poisoned::untyped(self.diagnostics.error(d))), true)
            } else if let Some(got) = type_of(&checked).filter(|&got| got != ty) {
                // As in `binary`: the place is the `string` that the value is joined to.
                // A rejected expected type leaves no type, so `got` is the value's own.
                let d = self.concat_mismatch(value, got, &[target, value], true, Some(ty));
                (Err(Poisoned::untyped(self.diagnostics.error(d))), true)
            } else {
                self.settled(value, checked, Expectation::Required(ty))
            };
            self.concat(span, (read, true), value, mismatched)
        } else {
            // The place's own mismatch waits for `arithmetic`, which drops it beside a `char`.
            let start = self.diagnostics.mark();
            let place = self.settled_by(target, read, Some(want), Some(operator.takes()));
            // `arithmetic` restores them, or cuts them with the place, which is `Error` then.
            let held = self.diagnostics.take_since(start);
            self.arithmetic(span, (op, op_span), want, place, value, held)
        }
    }

    /// Reports the compound assignment, at its operator `op_span`, to a `char` place, and checks
    /// `value` for its own mistakes.
    fn char_compound(&mut self, op_span: Span, value: &ast::Expr) -> Reported {
        let reported = self.error(DiagnosticKind::ArithmeticOnChar { at: op_span });
        // Only its diagnostics matter.
        drop(self.expr(value, Expectation::Poisoned));
        reported
    }

    /// The constant `name` as the root of a place that `span` assigns, reported, if `name` names
    /// one. The value is still checked against the constant's type. A broken constant poisons
    /// the assignment without a diagnostic.
    fn constant_place(&mut self, span: Span, name: Symbol, change: Change) -> Option<CheckedPlace> {
        let id = match self.names.constant(name)? {
            Declared::Known(id) => id,
            Declared::Poisoned(reported) => return Some(CheckedPlace::failed(Some(reported))),
        };
        let constant = &self.const_infos[id.0];
        let (declared, ty) = (constant.span, constant.ty);
        let cause = Immutable::Constant { declared };
        let (refused, reported) = if change == Change::MutArg {
            (Some(cause), None)
        } else {
            (
                None,
                Some(self.error(change.immutable(name.to_string(), span, cause))),
            )
        };
        Some(CheckedPlace {
            path: None,
            ty,
            shown: None,
            writable: false,
            held: None,
            held_ty: None,
            guard: None,
            refused,
            chain: None,
            reported,
        })
    }

    /// Reports E0001 for the root `name` that `change` writes at `at`, and gives the held report
    /// and the token of the report. A `push` holds the report back (`held`): its receiver may be
    /// no array.
    fn forbid(
        &mut self,
        change: Change,
        name: String,
        at: Span,
        cause: Immutable,
    ) -> (Option<DiagnosticKind>, Option<Reported>) {
        let d = change.immutable(name, at, cause);
        if change == Change::Push {
            return (Some(d), None);
        }
        (None, Some(self.error(d)))
    }

    /// Stamps what the place `target` of a `push` writes, as `place` stamps a write. A binding at
    /// the root of a chain of indexes is reassigned. The root of a field's base is written.
    pub(super) fn stamp_push(&mut self, target: &ast::Expr) {
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
    pub(super) fn stamp_root(&mut self, base: &ast::Expr) {
        match &place_root(base).kind {
            &ast::ExprKind::Name(name) => {
                let stamp = self.write_stamp();
                if let Some(b) = self.lookup(name) {
                    b.written = stamp;
                }
            }
            ast::ExprKind::SelfValue => self.body.self_written = self.write_stamp(),
            _ => {}
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
            }
            // A poisoned item's own error is reported, and the name is no typo for a binding.
            let reported = if let Some(reported) = self.use_poison(name, target.span) {
                reported
            } else if self.is_function(name) {
                self.error(DiagnosticKind::AssignToFunction {
                    name: name.to_string(),
                    at: target.span,
                })
            } else {
                let d = self.not_found(name, target.span);
                let reported = self.diagnostics.error(d);
                // The name may have meant any open binding; none of them warns as
                // unassigned.
                for b in self.body.scopes.iter_mut().flat_map(|s| s.values_mut()) {
                    b.reassigned = true;
                }
                reported
            };
            return CheckedPlace::failed(Some(reported));
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
        let ((held, forbidden), refused) = match cause {
            Some(cause) if change == Change::MutArg => ((None, None), Some(cause)),
            Some(cause) => (self.forbid(change, name.to_string(), span, cause), None),
            None => ((None, None), None),
        };
        let untyped = ty.is_none().then(|| self.untyped_binding(span));
        CheckedPlace {
            path: Some(Path {
                root: Root::Local(local),
                operands: Vec::new(),
            }),
            ty,
            shown,
            writable: true,
            held,
            held_ty: None,
            guard: None,
            refused,
            chain: None,
            reported: untyped.or(forbidden),
        }
    }

    /// The field `name` of the base of a place, a value of type `ty`, as `find_field` finds it. It
    /// also gives the token of the E0062 when `ty` is an optional. `ty` is `Err` when the base
    /// failed. A `.` on an optional checks the field on its payload and writes nothing. A `push`
    /// reads the place only to find that its receiver is no array: it finds no field there, unless
    /// the field holds an array, whose write rule and elements the `push` still checks. `Err` holds
    /// the token of the first error.
    fn field_target(
        &mut self,
        ty: Result<Type, Reported>,
        base: &ast::Expr,
        name: &ast::Ident,
        (access, change): (Access, Change),
    ) -> Result<(FoundField, Option<Reported>), Reported> {
        let ty = ty?;
        let TypeKind::Optional(payload) = ty.kind() else {
            return Ok((self.find_field(ty, base, name, access)?, None));
        };
        let (found, reported) = self.field_of_payload(payload, base, name, access)?;
        // The field is an array or an optional array, which the `push` writes.
        let array = found.2.is_some_and(|held| {
            matches!(held.kind(), TypeKind::Array(_)) || is_optional_array(held)
        });
        if change == Change::Push && !array {
            return Err(reported);
        }
        Ok((found, Some(reported)))
    }

    /// Reports E0062 at `base`, the optional array of an element place whose place is `inner`.
    /// An `if let` of the array binds a copy, which is no place. The help names the object of the
    /// `?.` only when that object is what makes the array optional: the `?.` field stores an
    /// array, and the object is a class, so its field is a place. Else no unwrap gives a place.
    /// `Checker::pushed_array_unwrap` states the same rule for a `push`: change both.
    fn optional_array_target(&mut self, base: &ast::Expr, inner: &CheckedPlace) -> Reported {
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
        self.diagnostics.error(optional_array(base, unwrap))
    }

    /// The place `target` names, which `span` changes as `change` says: `Element` when it is the
    /// base of an index, so the write replaces an element of it.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per kind of place, which share what its change decides"
    )]
    pub(super) fn place(&mut self, span: Span, target: &ast::Expr, change: Change) -> CheckedPlace {
        match &target.kind {
            &ast::ExprKind::Name(name) => self.name_place(span, target, name, change),
            ast::ExprKind::Field {
                base, name, safe, ..
            } => {
                let access = match change {
                    Change::Push => Access::Push,
                    Change::MutArg => Access::Mut,
                    Change::Assign | Change::Element => Access::Assign,
                };
                if change != Change::Push {
                    self.stamp_root(base);
                }
                // The base of a `mut` argument is checked as the base of an index is: a `?.` fix
                // there leads to a path that is no place (E0170).
                let outer = self.body.indexed;
                self.body.indexed |= change == Change::MutArg;
                let checked = self.value(base, Expectation::None);
                self.body.indexed = outer;
                let mut guard = None;
                // `?.` takes the object out of its optional, where the place starts. The write
                // of the place is the branch where the object is present.
                let checked = if *safe {
                    match (self.safe_payload(&checked, base, name), checked) {
                        (Ok(payload), Ok(scrutinee)) => {
                            let bind = self.fresh(payload);
                            guard = Some(Guard {
                                bind,
                                scrutinee: Ok(scrutinee),
                            });
                            Ok(local(bind, payload, base.span))
                        }
                        (payload, scrutinee) => {
                            let failed = (payload.as_ref().err())
                                .or_else(|| scrutinee.as_ref().err())
                                .expect("a `?.` that fails should have a failed part");
                            let (ty, reported) = (failed.ty, failed.reported());
                            // As `?.` does, report the field and give no type.
                            if let Some(ty) = ty {
                                let _ = self.find_field(ty, base, name, access);
                            }
                            return CheckedPlace::failed(Some(reported));
                        }
                    }
                } else {
                    checked
                };
                let ((class, index, ty), on_optional) =
                    match self.field_target(known_type(&checked), base, name, (access, change)) {
                        Ok(found) => found,
                        Err(reported) => return CheckedPlace::failed(Some(reported)),
                    };
                let field = &self.classes[class.0].fields[index];
                let (writable, shown) = (field.mutable, field.shown.clone());
                let (mut held, mut forbidden, mut refused) = (None, None, None);
                if !writable {
                    let (name, declared) = (field.name.name.to_string(), field.name.span);
                    let cause = Immutable::Field { declared };
                    if change == Change::MutArg {
                        refused = Some(cause);
                    } else {
                        (held, forbidden) = self.forbid(change, name, span, cause);
                    }
                }
                // A field with no type has the error of its declaration.
                let untyped = ty.is_none() && on_optional.is_none();
                let untyped = untyped.then(|| self.proof(name.span)); // proof: declaration
                let on_optional_report = on_optional;
                let on_optional = on_optional.is_some();
                // The copy-in reads the field.
                if change == Change::MutArg {
                    self.record(Used::Field(class, index), target.span);
                }
                // The first error of the place: its base's, if the base failed.
                let reported = (checked.as_ref().err().map(Poisoned::reported))
                    .or(on_optional_report)
                    .or(untyped)
                    .or(forbidden);
                CheckedPlace {
                    path: checked.ok().filter(|_| !on_optional).map(|base| Path {
                        root: Root::Field(index),
                        operands: vec![base],
                    }),
                    // A place under a failed `.` is untyped: the fix of the E0062 decides it.
                    ty: ty.filter(|_| !on_optional),
                    shown,
                    writable,
                    held,
                    held_ty: ty.filter(|_| on_optional),
                    guard,
                    refused,
                    chain: None,
                    reported,
                }
            }
            ast::ExprKind::Index { base, index } => {
                let inner_change = match change {
                    Change::Push | Change::MutArg => change,
                    Change::Assign | Change::Element => Change::Element,
                };
                let outer = std::mem::replace(&mut self.body.indexed, true);
                let inner = self.place(span, base, inner_change);
                // The array of an optional array is no place, but its element has a type.
                let optional = inner.value_ty().filter(|&ty| is_optional_array(ty));
                let (element, element_report) = match optional {
                    Some(ty) => {
                        let reported = self.optional_array_target(base, &inner);
                        (optional_element(ty), Some(reported))
                    }
                    None => self.element(base, inner.value_ty(), change.index_usage()),
                };
                // The index is not part of the chain that it ends.
                self.body.indexed = false;
                let (index, mismatched) = self.operand(index, Expectation::Required(Type::I64));
                self.body.indexed = outer;
                let reported = (inner.reported.or(element_report))
                    .or_else(|| index.as_ref().err().map(Poisoned::reported));
                let path = inner
                    .path
                    .filter(|_| element.is_some() && optional.is_none())
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
                    held_ty: inner.held_ty.filter(|_| !mismatched).and_then(|t| {
                        match payload(t).kind() {
                            TypeKind::Array(element) => Some(element),
                            _ => None,
                        }
                    }),
                    guard: None,
                    refused: inner.refused,
                    chain: None,
                    reported,
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
    pub(super) fn read(&self, place: tir::Place, span: Span) -> tir::Expr {
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
        tir::Expr::new(kind, ty, span)
    }
}

/// The expression at the root of the place `e`: `e` with each field and index taken off.
pub(super) fn place_root(mut e: &ast::Expr) -> &ast::Expr {
    while let ast::ExprKind::Index { base, .. } | ast::ExprKind::Field { base, .. } = &e.kind {
        e = base;
    }
    e
}

/// The layers of optional around the array that a `.push` receiver holds, or 0 when it holds no
/// optional array. The receiver holds the type `ty`, made optional where a `?.` of the root
/// field of its place `guarded` it. `?.push` takes the place as one array only when there is one
/// layer. A place with any layer reports its held write before E0062.
fn held_array_layers(ty: Option<Type>, guarded: bool) -> usize {
    let Some(mut ty) = ty else {
        return 0;
    };
    if guarded {
        ty = as_optional(ty);
    }
    let mut layers = 0;
    while let TypeKind::Optional(payload) = ty.kind() {
        layers += 1;
        ty = payload;
    }
    if matches!(ty.kind(), TypeKind::Array(_)) {
        layers
    } else {
        0
    }
}

/// `block`, the write of a place, as the branch where the object of the place's `guard` is
/// present: the right side runs there only. Without a `guard`, `block` as it is.
fn guarded(guard: Option<Guard>, block: tir::Expr, span: Span) -> tir::Expr {
    match guard {
        Some(Guard {
            bind,
            scrutinee: Ok(scrutinee),
        }) => if_some(bind, scrutinee, block, None, span),
        _ => block,
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
pub(super) fn build(root: Root, operands: Vec<tir::Expr>) -> tir::Place {
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
