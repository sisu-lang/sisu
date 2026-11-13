//! Assignment: places, which root may be written, the temps of compound operators, and spills.

use std::rc::Rc;

use super::array::{innermost, is_optional_array, optional_array, optional_element};
use super::expr::{if_some, literal_type, local, operator};
use super::member::{Access, FieldOwner, FoundField, safe_value};
use super::operator::Operator;
use super::reports::Reported;
use super::unused_names::{Copied, Used};
use super::{
    Binding, BindingKind, Checked, Checker, Declared, Expectation, Origin, Poisoned, as_optional,
    block_expr, known_type, payload, type_of,
};
use crate::ast;
use crate::diagnostic::{DiagnosticKind, Immutable, IndexUsage, UnwrapHelp, Written};
use crate::parser::{is_place, is_range_target};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, ExprKind, Type, TypeKind};

/// A place, flattened: its root, the object of a field root, and each step from the root
/// outwards.
pub(super) struct Path {
    pub(super) root: Root,
    /// For a `Root::Field`: the object whose field the root is.
    pub(super) base: Option<tir::Expr>,
    pub(super) steps: Vec<PathStep>,
}

/// A step of a path past its root.
pub(super) enum PathStep {
    /// Field `index` of a struct.
    Field(usize),
    /// The element at the index.
    Index(tir::Expr),
}

impl Path {
    /// The path of the local `id`.
    fn local(id: tir::LocalId) -> Self {
        Self {
            root: Root::Local(id),
            base: None,
            steps: Vec::new(),
        }
    }

    /// The operands of the path in evaluation order, the base and then each index, and the
    /// shape that takes them back.
    pub(super) fn split(self) -> (Shape, Vec<tir::Expr>) {
        let mut operands: Vec<_> = self.base.into_iter().collect();
        let steps = (self.steps.into_iter())
            .map(|step| match step {
                PathStep::Field(index) => Some(index),
                PathStep::Index(index) => {
                    operands.push(index);
                    None
                }
            })
            .collect();
        let shape = Shape {
            root: self.root,
            steps,
        };
        (shape, operands)
    }
}

/// A path with its operands taken out: a field index for each struct field, `None` for each
/// index.
#[derive(Clone)]
pub(super) struct Shape {
    pub(super) root: Root,
    steps: Vec<Option<usize>>,
}

impl Shape {
    /// The path again, with `operands` in the order that `Path::split` gave them.
    pub(super) fn path(self, operands: Vec<tir::Expr>) -> Path {
        let mut operands = operands.into_iter();
        let base = matches!(self.root, Root::Field(_)).then(|| {
            (operands.next()).expect("a field root's base should be the first operand of its path")
        });
        let steps = (self.steps.into_iter())
            .map(|step| match step {
                Some(index) => PathStep::Field(index),
                None => PathStep::Index(
                    (operands.next()).expect("each index of a path should have its operand"),
                ),
            })
            .collect();
        Path {
            root: self.root,
            base,
            steps,
        }
    }
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
    /// `x.n = v` or `x.n[i] = v`, where `x` is the root of the place and `n` a struct field.
    Field,
    /// `mut x` as an argument: the place gives its cause when it cannot change, and reports none.
    MutArg,
}

impl Change {
    /// How a field access of this change uses the field.
    fn access(self) -> Access {
        match self {
            Change::Push => Access::Push,
            Change::MutArg => Access::Mut,
            Change::Assign | Change::Element | Change::Field => Access::Assign,
        }
    }

    /// This change, made to a part of a struct: a write writes a part of the root, and a `push`
    /// and a `mut` argument stay.
    fn of_part(self) -> Change {
        match self {
            Change::Assign | Change::Element | Change::Field => Change::Field,
            Change::Push | Change::MutArg => self,
        }
    }

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
            Change::Assign | Change::Element | Change::Field => DiagnosticKind::AssignToImmutable {
                name,
                at,
                written: match self {
                    Change::Element => Written::Element,
                    Change::Field => Written::Part,
                    _ => Written::Whole,
                },
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
    /// The read of a place inside a value that a call or a construction gives, which no write
    /// can reach: a `push` calls a method of its own type on it.
    pub(super) temporary: Option<tir::Expr>,
}

/// The `?.` along the path of a `mut` argument.
pub(super) struct Chain {
    /// The text before the first `?.`, when it is on one line.
    pub(super) name: Option<String>,
    /// Whether `?.` takes a class object out of its optional: only then does an unwrap of the
    /// object make its field a place.
    pub(super) class_object: bool,
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
            temporary: None,
        }
    }

    /// Whether the `?.` of the place takes a class object out of its optional. `locals` are the
    /// locals of the body.
    pub(super) fn guards_class(&self, locals: &[tir::Local]) -> bool {
        self.guard
            .as_ref()
            .is_some_and(|guard| matches!(locals[guard.bind.0].ty.kind(), TypeKind::Class(..)))
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
        self.copy_written(
            target,
            target.span,
            !matches!(target.kind, ast::ExprKind::Name(_)),
        );
        // A local's type comes from where its name is declared. A field or an index waits.
        let declared = match target.kind {
            ast::ExprKind::Name(name) => self.lookup(name).map(|b| b.span),
            _ => None,
        };
        if let (Some(want), Some(text)) = (place.ty, &place.shown) {
            self.show(value.span, want, text);
            if let Some(at) = declared {
                self.show(at, want, text);
            }
        }
        let origin = match (declared, place.ty) {
            (Some(at), Some(ty)) => Origin::Assigned { at, ty },
            _ => Origin::None,
        };
        let value = self.expect(value, Expectation::or_poisoned(place.ty, origin));
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
                let write = self.write(span, path, value);
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
    /// class, an enum, a struct, a type parameter or an associated type is not stamped. The call
    /// may be a method of its own, which writes nothing.
    pub(super) fn push_receiver(&mut self, receiver: &ast::Expr) -> Checked {
        let mut place = self.place(receiver.span, receiver, Change::Push);
        if !matches!(
            place.ty.map(innermost).map(Type::kind),
            Some(
                TypeKind::Class(..)
                    | TypeKind::Enum(..)
                    | TypeKind::Struct(..)
                    | TypeKind::Param(..)
                    | TypeKind::Assoc(..)
            )
        ) {
            self.stamp_push(receiver);
        }
        // A place of type `Error` has reported itself.
        let read = match (place.path.take(), place.ty) {
            (Some(path), Some(_)) => Ok(self.read(build(path), receiver.span)),
            (None, Some(_)) if place.temporary.is_some() => Ok(place
                .temporary
                .take()
                .expect("the guard should hold a temporary")),
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
            let reported = self.error(d);
            // A `?.` on a struct value held the report of its write, which is the only one.
            if let Some(guard) = &place.guard
                && matches!(
                    self.body.locals[guard.bind.0].ty.kind(),
                    TypeKind::Struct(..)
                )
            {
                return Err(Poisoned::new(type_of(&read), reported));
            }
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
    /// neither. It walks each step once, and a root that is no binding, such as the value of a
    /// call, reads nothing.
    pub(super) fn read_pushed(&mut self, read: &tir::Expr, e: &ast::Expr) {
        #[cfg(test)]
        super::PATH_STEPS.set(super::PATH_STEPS.get() + 1);
        match (&e.kind, &read.kind) {
            (ast::ExprKind::Index { base, .. }, ExprKind::Index { base: read, .. }) => {
                self.read_pushed(read, base);
            }
            (ast::ExprKind::Index { base, .. }, ExprKind::Block(block)) => {
                if let Some(read) = spilled_base(block) {
                    self.read_pushed(read, base);
                }
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
            (ast::ExprKind::Field { base: inner, .. }, ExprKind::StructField { base, index }) => {
                let TypeKind::Struct(id, _) = base.ty.kind() else {
                    unreachable!("the base of a checked struct field should be a struct")
                };
                self.record(Used::StructField(id, *index), e.span);
                self.read_pushed(base, inner);
            }
            // `self` is no binding that a read marks, and another root is a temporary.
            _ => {}
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
        let value = self.expect(&arg.value, Expectation::or_poisoned(element, Origin::None));
        match (value, read, place) {
            (Ok(value), Ok(read), Some(place)) if place.writable => {
                self.copy_written(receiver_expr, span, true);
                let path = unread(read, receiver_expr);
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
        let (shape, mut operands) = path.split();
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
        let mut place = build(shape.path(operands));
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
        self.copy_written(
            target,
            target.span,
            !matches!(target.kind, ast::ExprKind::Name(_)),
        );
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
            let (shape, operands) = path.split();
            let temps: Vec<_> = operands
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
            (shape, temps)
        });
        let reads = |temps: &[(tir::LocalId, Type, Span)]| {
            temps
                .iter()
                .map(|(temp, ty, span)| local(*temp, *ty, *span))
                .collect::<Vec<_>>()
        };
        let read = match &path {
            Some((shape, temps)) => {
                Ok(self.read(build(shape.clone().path(reads(temps))), target.span))
            }
            None => Err(Poisoned::typed(
                ty,
                place
                    .reported
                    .expect("a place with no path should have failed"),
            )),
        };
        let value = self.compound_value(span, (op, op_span), target, value, (ty, read));
        let failed = (value.as_ref().err().map(Poisoned::reported)).or(place.reported);
        let (Some((shape, temps)), Ok(value), true) = (path, value, place.writable) else {
            return Err(unit(failed.expect(
                "a compound assignment that fails should have a failed part",
            )));
        };
        let operands = reads(&temps);
        lets.push(self.write(span, shape.path(operands), value));
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
            Expectation::or_poisoned(want, Origin::None),
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
        let (checked, _) = self.settled(
            &expanded,
            checked,
            Expectation::or_poisoned(want, Origin::None),
        );
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
                Expectation::Required(ty, Origin::None)
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
                self.settled(value, checked, Expectation::Required(ty, Origin::None))
            };
            self.concat(span, (read, true), value, mismatched)
        } else {
            // The place's own mismatch waits for `arithmetic`, which drops it beside a `char`.
            let start = self.diagnostics.mark();
            let place = self.settled_by(
                target,
                read,
                Some(want),
                Origin::None,
                Some(operator.takes()),
            );
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
            temporary: None,
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
    /// the root of a path that crosses no class object is reassigned, as `reassign_root` says.
    /// The root of a class field's base is written.
    pub(super) fn stamp_push(&mut self, target: &ast::Expr) {
        let (&ast::ExprKind::Name(name), false) =
            (&place_root(target).kind, self.crosses_class(target))
        else {
            return self.stamp_root(target);
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
                let reported = self.not_found(name, target.span);
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
        let (local, ty) = (b.local, b.ty);
        let shown = b.shown.clone();
        let cause = binding_cause(b);
        let ((held, forbidden), refused) = match cause {
            Some(cause) if change == Change::MutArg => ((None, None), Some(cause)),
            Some(cause) => (self.forbid(change, name.to_string(), span, cause), None),
            None => ((None, None), None),
        };
        let reported = forbidden.or_else(|| ty.is_none().then(|| self.untyped_binding(span)));
        CheckedPlace {
            path: Some(Path::local(local)),
            ty,
            shown,
            writable: true,
            held,
            held_ty: None,
            guard: None,
            refused,
            chain: None,
            reported,
            temporary: None,
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
            // An unwrap of a struct gives a copy: only a class object makes its field a place.
            (
                ast::ExprKind::Field {
                    base: object,
                    safe: true,
                    ..
                },
                Some(guard),
                Some(TypeKind::Array(_)),
            ) if self.one_line(object.span)
                && matches!(
                    self.body.locals[guard.bind.0].ty.kind(),
                    TypeKind::Class(..)
                ) =>
            {
                UnwrapHelp::Object(self.source.text(object.span).to_string())
            }
            _ => UnwrapHelp::Nothing,
        };
        self.diagnostics.error(optional_array(base, unwrap))
    }

    /// The place `target` names, which `span` changes as `change` says: `Element` when it is the
    /// base of an index, so the write replaces an element of it.
    pub(super) fn place(&mut self, span: Span, target: &ast::Expr, change: Change) -> CheckedPlace {
        match &target.kind {
            &ast::ExprKind::Name(name) => self.name_place(span, target, name, change),
            ast::ExprKind::Field {
                base, name, safe, ..
            } => self.field_place(span, target, (base, name, *safe), change),
            ast::ExprKind::Index { base, index } => {
                let inner_change = match change {
                    Change::Push | Change::MutArg => change,
                    Change::Assign | Change::Element | Change::Field => Change::Element,
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
                    None => self.element(base, (inner.value_ty(), false), change.index_usage()),
                };
                // The index is not part of the chain that it ends.
                self.body.indexed = false;
                // A base with no element, such as a string, takes no `i64` index either.
                let origin = if element.is_some() {
                    Origin::PlaceIndex
                } else {
                    Origin::None
                };
                let (index, mismatched) =
                    self.operand(index, Expectation::Required(Type::I64, origin));
                self.body.indexed = outer;
                let reported = (inner.reported.or(element_report))
                    .or_else(|| index.as_ref().err().map(Poisoned::reported));
                let index = index.ok();
                // The read of a temporary goes on to the element.
                let temporary = (inner.temporary)
                    .zip(element.filter(|_| !mismatched))
                    .zip(index.as_ref())
                    .map(|((base, element), index)| {
                        let kind = ExprKind::Index {
                            base: Box::new(base),
                            index: Box::new(index.clone()),
                        };
                        tir::Expr::new(kind, element, target.span)
                    });
                let path = inner
                    .path
                    .filter(|_| element.is_some() && optional.is_none())
                    .zip(index)
                    .map(|(mut path, index)| {
                        path.steps.push(PathStep::Index(index));
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
                    temporary,
                }
            }
            _ => unreachable!("the parser should make only a name, a field or an index a place"),
        }
    }

    /// The place `target`, the field `name` of `base`, which `span` changes as `change` says.
    /// `safe` makes it `base?.name`. The base is checked once, as a value. The type that holds
    /// the field and the checked tree then decide the place, so a class path is as it was.
    fn field_place(
        &mut self,
        span: Span,
        target: &ast::Expr,
        (base, name, safe): (&ast::Expr, &ast::Ident, bool),
        change: Change,
    ) -> CheckedPlace {
        let access = change.access();
        if change != Change::Push {
            self.stamp_root(base);
        }
        let root = place_root(base);
        // The base of a `mut` argument is checked as the base of an index is: a `?.` fix
        // there leads to a path that is no place (E0170).
        let indexed = self.body.indexed || change == Change::MutArg;
        // The root name is no read until the path says whether the write reads it.
        let quiet = match root.kind {
            ast::ExprKind::Name(_) => Some(root.span),
            _ => self.body.quiet,
        };
        let outer = (
            std::mem::replace(&mut self.body.indexed, indexed),
            std::mem::replace(&mut self.body.quiet, quiet),
            self.body.write_chain.replace((span, change)),
        );
        let checked = self.value(base, Expectation::None);
        (self.body.indexed, self.body.quiet, self.body.write_chain) = outer;
        // A `?.` on a base that failed, such as the `?.` of a write before it, reports nothing
        // more.
        if (!safe || checked.is_err())
            && type_of(&checked).is_some_and(|ty| is_struct(innermost(ty)))
        {
            return self.struct_place(span, target, (base, name), checked, change);
        }
        self.read_root(root);
        let mut guard = None;
        // `?.` takes the object out of its optional, where the place starts. The write of the
        // place is the branch where the object is present.
        let checked = if safe {
            match (self.safe_payload(&checked, base, name), checked) {
                (Ok(payload), Ok(scrutinee)) if is_struct(payload) => {
                    return self.safe_struct_place(
                        span,
                        target,
                        (base, name),
                        (payload, scrutinee),
                        change,
                    );
                }
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
                    self.reassign_root(target);
                    return CheckedPlace::failed(Some(reported));
                }
            }
        } else {
            checked
        };
        let ((owner, index, ty), on_optional) =
            match self.field_target(known_type(&checked), base, name, (access, change)) {
                Ok(found) => found,
                Err(reported) => {
                    self.reassign_root(target);
                    return CheckedPlace::failed(Some(reported));
                }
            };
        let FieldOwner::Class(class) = owner else {
            unreachable!("a field of a struct should take the struct arms")
        };
        let shown = self.field_shown(owner, index);
        let (writable, held, refused, forbidden) = self.field_rule(span, change, (class, index));
        // A field with no type has the error of its declaration.
        let untyped = ty.is_none() && on_optional.is_none();
        let on_optional_report = on_optional;
        let on_optional = on_optional.is_some();
        // The copy-in reads the field.
        if change == Change::MutArg {
            self.record(owner.used(index), target.span);
        }
        // The first error of the place: its base's, if the base failed.
        let reported = (checked.as_ref().err().map(Poisoned::reported))
            .or(on_optional_report)
            .or(forbidden)
            .or_else(|| untyped.then(|| self.proof(name.span))); // proof: declaration
        CheckedPlace {
            path: checked.ok().filter(|_| !on_optional).map(|base| Path {
                root: Root::Field(index),
                base: Some(base),
                steps: Vec::new(),
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
            temporary: None,
        }
    }

    /// The write rule of field `index` of the class `class`, which `span` changes as `change`
    /// says (ADR 0011). It gives whether the field can change and the held E0001 of a `push`. It
    /// also gives the cause of a `mut` argument and the token of the E0001 that it reported.
    fn field_rule(
        &mut self,
        span: Span,
        change: Change,
        (class, index): (tir::ClassId, usize),
    ) -> (
        bool,
        Option<DiagnosticKind>,
        Option<Immutable>,
        Option<Reported>,
    ) {
        let field = &self.classes[class.0].fields[index];
        if field.mutable {
            return (true, None, None, None);
        }
        let (name, declared) = (field.name.name.to_string(), field.name.span);
        let cause = Immutable::Field { declared };
        if change == Change::MutArg {
            return (false, None, Some(cause), None);
        }
        let (held, forbidden) = self.forbid(change, name, span, cause);
        (false, held, None, forbidden)
    }

    /// The type of field `index` of `owner` as its declaration writes it, when it names an alias.
    fn field_shown(&self, owner: FieldOwner, index: usize) -> Option<Rc<str>> {
        self.owner_info(owner).fields[index].shown.clone()
    }

    /// Marks the binding `root`, the root of a place, as read.
    fn read_root(&mut self, root: &ast::Expr) {
        if let ast::ExprKind::Name(name) = root.kind
            && let Some(b) = self.lookup(name)
        {
            b.read = true;
        }
    }

    /// The place `target`, the field `name` of `base`, whose checked value `checked` has a struct
    /// type, or an optional of one. A step through a struct is as writable as its base: the root
    /// of the path decides. A path that ends in no place is a temporary (E0021).
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per kind of change, so the place rules read in one place"
    )]
    fn struct_place(
        &mut self,
        span: Span,
        target: &ast::Expr,
        (base, name): (&ast::Expr, &ast::Ident),
        checked: Checked,
        change: Change,
    ) -> CheckedPlace {
        let root = place_root(base);
        let holder = type_of(&checked).expect("the caller should have found a struct type");
        if matches!(holder.kind(), TypeKind::Optional(_)) {
            self.read_root(root);
            return self.optional_struct_place(span, target, (base, name), &checked, change);
        }
        let (owner, index, ty) = match self.find_field(holder, base, name, change.access()) {
            Ok(found) => found,
            Err(reported) => {
                self.read_root(root);
                self.reassign_root(target);
                return CheckedPlace::failed(Some(reported));
            }
        };
        let shown = self.field_shown(owner, index);
        // The copy-in reads the field.
        if change == Change::MutArg {
            self.record(owner.used(index), target.span);
        }
        let read = match checked {
            Ok(read) => read,
            // The base reported its error. The place keeps the field's type for the value.
            Err(failed) => {
                self.read_root(root);
                self.reassign_root(base);
                return CheckedPlace {
                    ty,
                    shown,
                    reported: Some(failed.reported()),
                    ..CheckedPlace::failed(None)
                };
            }
        };
        let untyped = |checker: &mut Self| ty.is_none().then(|| checker.proof(name.span)); // proof: declaration
        if !is_path(&read, base) {
            self.read_root(root);
            self.reassign_root(target);
            let at = target.span;
            let read = ty.map(|ty| {
                let kind = ExprKind::StructField {
                    base: Box::new(read),
                    index,
                };
                tir::Expr::new(kind, ty, at)
            });
            // A write inside a value that a call gives is lost. A `push` may be a method of the
            // field's own type, which reads it.
            let (held, reported, temporary) = match change {
                Change::Assign | Change::Element | Change::Field => {
                    let d = DiagnosticKind::AssignToExpression { at, range: None };
                    (None, Some(self.error(d)), None)
                }
                Change::Push => (
                    Some(DiagnosticKind::PushToTemporary { at, range: None }),
                    None,
                    read,
                ),
                Change::MutArg => (None, None, read),
            };
            return CheckedPlace {
                ty,
                shown,
                held,
                reported: reported.or_else(|| untyped(self)),
                temporary,
                ..CheckedPlace::failed(None)
            };
        }
        let mut path = try_unread(read, base).expect("`is_path` should have found a path");
        path.steps.push(PathStep::Field(index));
        let written = change.of_part();
        let (writable, held, refused, reported) = match (path.root, &root.kind) {
            (Root::Local(_), &ast::ExprKind::Name(root_name)) => {
                let named = self.name_place(span, root, root_name, written);
                (named.writable, named.held, named.refused, named.reported)
            }
            // The `self` of a struct method: a `mut self`, with its E0018, can change.
            (Root::Local(_), _) if self.body.self_mut => (true, None, None, None),
            (Root::Local(_), _) => {
                let cause = Immutable::StructSelf;
                if change == Change::MutArg {
                    (true, None, Some(cause), None)
                } else {
                    let (held, forbidden) = self.forbid(written, "self".to_string(), span, cause);
                    (true, held, None, forbidden)
                }
            }
            // A field of a class object: its `mut` decides.
            (Root::Field(field), _) => {
                self.read_root(root);
                let object =
                    (path.base.as_ref()).expect("a field root's base should be in its path");
                let TypeKind::Class(class, _) = object.ty.kind() else {
                    unreachable!("the object of a field root should be a class")
                };
                let (writable, held, refused, forbidden) =
                    self.field_rule(span, written, (class, field));
                (writable, held, refused, forbidden)
            }
        };
        CheckedPlace {
            path: ty.map(|_| path),
            ty,
            shown,
            writable,
            held,
            held_ty: None,
            guard: None,
            refused,
            chain: None,
            reported: reported.or_else(|| untyped(self)),
            temporary: None,
        }
    }

    /// The place `target`, the field `name` of `base`, whose checked value `checked` has the type
    /// of an optional struct, with no `?.` at this step. After a `?.` in `base`, a write and a
    /// `push` onto an array report the write that the `?.` leads to. Else `field_target` reports
    /// the E0062 of the `.`.
    fn optional_struct_place(
        &mut self,
        span: Span,
        target: &ast::Expr,
        (base, name): (&ast::Expr, &ast::Ident),
        checked: &Checked,
        change: Change,
    ) -> CheckedPlace {
        let holder = innermost(type_of(checked).expect("the caller should have found a type"));
        let TypeKind::Struct(id, _) = holder.kind() else {
            unreachable!("the caller should have found an optional struct")
        };
        let found = (self.structs[id.0].field(name.name))
            .map(|index| (index, self.field_type(holder, index)));
        // The base reported its error, such as the `?.` of a write before it. The place keeps
        // the field's type for the value.
        if let (Some(_), Err(failed)) = (found, checked) {
            self.reassign_root(base);
            return CheckedPlace::failed(Some(failed.reported()));
        }
        if let (Some((index, ty)), Ok(_), Some(chain)) = (found, checked, first_safe(base)) {
            let array = ty.is_some_and(|ty| matches!(payload(ty).kind(), TypeKind::Array(_)));
            let writes = match change {
                Change::Push => array,
                Change::MutArg => false,
                Change::Assign | Change::Element | Change::Field => true,
            };
            if writes {
                let d = self.chain_write((span, change), base, chain);
                return CheckedPlace {
                    ty,
                    shown: self.field_shown(FieldOwner::Struct(id), index),
                    reported: Some(self.error(d)),
                    ..CheckedPlace::failed(None)
                };
            }
        }
        let ((owner, index, _), on_optional) =
            match self.field_target(known_type(checked), base, name, (change.access(), change)) {
                Ok(found) => found,
                Err(reported) => {
                    self.reassign_root(target);
                    return CheckedPlace::failed(Some(reported));
                }
            };
        if change == Change::MutArg {
            self.record(owner.used(index), target.span);
        }
        // The fix of the E0062 decides the place.
        CheckedPlace::failed(on_optional)
    }

    /// The place `target`, `base?.name` with the struct `payload` that `?.` takes out of
    /// `scrutinee`. Part 1 cannot write through it: a write reports at once, and a `push` holds
    /// the report, which only an array makes. A `mut` argument keeps its guard for E0170.
    fn safe_struct_place(
        &mut self,
        span: Span,
        target: &ast::Expr,
        (base, name): (&ast::Expr, &ast::Ident),
        (payload, scrutinee): (Type, tir::Expr),
        change: Change,
    ) -> CheckedPlace {
        let (owner, index, ty) = match self.find_field(payload, base, name, change.access()) {
            Ok(found) => found,
            Err(reported) => {
                self.reassign_root(target);
                return CheckedPlace::failed(Some(reported));
            }
        };
        if matches!(change, Change::Assign | Change::Element | Change::Field) {
            let d = self.chain_write((span, change), base, (base, name));
            return CheckedPlace {
                ty,
                shown: self.field_shown(owner, index),
                reported: Some(self.error(d)),
                ..CheckedPlace::failed(None)
            };
        }
        let held = if change == Change::Push {
            Some(self.chain_write((span, change), base, (base, name)))
        } else {
            // The copy-in reads the field.
            self.record(owner.used(index), target.span);
            None
        };
        let bind = self.fresh(payload);
        let mut path = Path::local(bind);
        path.steps.push(PathStep::Field(index));
        CheckedPlace {
            path: ty.map(|_| path),
            ty,
            writable: change == Change::MutArg,
            held,
            guard: Some(Guard {
                bind,
                scrutinee: Ok(scrutinee),
            }),
            reported: ty.is_none().then(|| self.proof(name.span)), // proof: declaration
            ..CheckedPlace::failed(None)
        }
    }

    /// The report of a write, at `span` by `change`, that the `?.` after `object` before `name`
    /// leads to. There, `?.` takes a struct value out of an optional. `base` holds the `?.`. A
    /// root that cannot change is E0001. Else it is E0062 at the `?.`, with no fix: part 1
    /// cannot write through it. Its help replaces the value, when the value is a place.
    pub(super) fn chain_write(
        &mut self,
        (span, change): (Span, Change),
        base: &ast::Expr,
        (object, name): (&ast::Expr, &ast::Ident),
    ) -> DiagnosticKind {
        let root = match self.struct_root(base) {
            StructRoot::Binding(name) => Some(name.to_string()),
            StructRoot::SelfValue => Some("self".to_string()),
            StructRoot::Other => None,
        };
        if let (Some(root), Some(cause)) = (root, self.chain_root_cause(base)) {
            return change.of_part().immutable(root, span, cause);
        }
        // The value to replace: a place on one line whose `?.` takes out a struct.
        let replace = self
            .one_line(object.span)
            .then(|| self.replaceable(object))
            .flatten()
            .filter(|&(can, ty)| {
                can && matches!(ty.kind(), TypeKind::Optional(payload) if is_struct(payload))
            })
            .map(|_| self.source.text(object.span).to_string());
        DiagnosticKind::ChainWriteOnStruct {
            at: Span::new(object.span.end(), name.span.end()),
            replace,
        }
    }

    /// Why the root of the place `target` cannot change, when a write through its struct path
    /// reaches it. A path that crosses a class object writes no root. The root binding is
    /// reassigned: W0002's fix would lead to this E0001.
    fn chain_root_cause(&mut self, target: &ast::Expr) -> Option<Immutable> {
        match self.struct_root(target) {
            StructRoot::Binding(name) => {
                let b = self
                    .lookup(name)
                    .expect("a root that `struct_root` found should be bound");
                b.reassigned |= b.kind == BindingKind::Var;
                binding_cause(b)
            }
            StructRoot::SelfValue => (!self.body.self_mut).then_some(Immutable::StructSelf),
            StructRoot::Other => None,
        }
    }

    /// What W0016 needs of the `let mut` `name` whose initializer is `init`: the place it copies,
    /// when that place has a root, crosses no class object and holds no range index (a slice is
    /// a new value). It holds why the root cannot change, and whether the root has the name of the
    /// copy: `let mut a = a` copies an outer `a` that the copy hides. Call it before the local is
    /// declared, because the place may name an outer local of the same name.
    pub(super) fn copy_of(&self, init: &ast::Expr, name: Symbol) -> Option<Copied> {
        let (root, cause) = match self.struct_root(init) {
            StructRoot::Binding(root) => (root, binding_cause(self.binding(root)?)),
            StructRoot::SelfValue => (
                Symbol::intern("self"),
                (!self.body.self_mut).then_some(Immutable::StructSelf),
            ),
            StructRoot::Other => return None,
        };
        Some(Copied {
            init: init.span,
            root,
            cause,
            bare: matches!(init.kind, ast::ExprKind::Name(_) | ast::ExprKind::SelfValue),
            hidden: root == name,
            write: None,
            other: false,
        })
    }

    /// Records a use of the local at the root of the place `target`, written at `at`, for the
    /// W0016 of that local. A `part` is a write to a part of it: a field, an element, or a
    /// `push`. It counts when the path stays in structs and arrays. Any other write, the whole
    /// local, a `mut` argument or a path through a class object, rules W0016 out.
    pub(super) fn copy_written(&mut self, target: &ast::Expr, at: Span, part: bool) {
        let ast::ExprKind::Name(name) = place_root(target).kind else {
            return;
        };
        if self.binding(name).is_none_or(|b| b.copy.is_none()) {
            return;
        }
        let stays = part && matches!(self.struct_root(target), StructRoot::Binding(_));
        let Some(copy) = self.lookup(name).and_then(|b| b.copy.as_mut()) else {
            return;
        };
        if stays {
            copy.write.get_or_insert(at);
        } else {
            copy.other = true;
        }
    }

    /// Marks the binding at the root of the place `target` as reassigned. A path that crosses a
    /// class object marks nothing: its write needs no `mut` of the root. A path that the declared
    /// types do not decide also marks it, such as one with a missing field. The fix of its error
    /// needs the `mut`.
    pub(super) fn reassign_root(&mut self, target: &ast::Expr) {
        if let ast::ExprKind::Name(name) = place_root(target).kind
            && !self.crosses_class(target)
            && let Some(b) = self.lookup(name)
        {
            b.reassigned |= b.kind == BindingKind::Var;
        }
    }

    /// Whether the path of the place `target` crosses a class object, as `struct_walk` finds it.
    fn crosses_class(&self, target: &ast::Expr) -> bool {
        matches!(self.struct_walk(target), Err(WalkStop::Class))
    }

    /// The root of the place `target`, through each field of a struct and each index, by the
    /// declared types alone and with no diagnostics. A class step, a path that the types do not
    /// decide, or another root, is `Other`.
    pub(super) fn struct_root(&self, target: &ast::Expr) -> StructRoot {
        self.struct_walk(target)
            .map_or(StructRoot::Other, |(root, _)| root)
    }

    /// The root of `e` and the type of `e`, as `struct_root` walks them, or why the walk stops.
    fn struct_walk(&self, e: &ast::Expr) -> Result<(StructRoot, Type), WalkStop> {
        #[cfg(test)]
        super::PATH_STEPS.set(super::PATH_STEPS.get() + 1);
        match &e.kind {
            ast::ExprKind::Field { base, name, .. } => {
                let (root, ty) = self.struct_walk(base)?;
                let holder = innermost(ty);
                let id = match holder.kind() {
                    TypeKind::Struct(id, _) => id,
                    TypeKind::Class(..) => return Err(WalkStop::Class),
                    _ => return Err(WalkStop::Undecided),
                };
                let index = (self.structs[id.0].field(name.name)).ok_or(WalkStop::Undecided)?;
                let ty = self.field_type(holder, index).ok_or(WalkStop::Undecided)?;
                Ok((root, ty))
            }
            ast::ExprKind::Index { base, index } => {
                let (root, ty) = self.struct_walk(base)?;
                // A range gives a slice: a new value of the same type, which has no root. A class
                // step after it still stops the walk.
                if matches!(index.kind, ast::ExprKind::Range(_)) {
                    return Ok((StructRoot::Other, ty));
                }
                match innermost(ty).kind() {
                    TypeKind::Array(element) => Ok((root, element)),
                    _ => Err(WalkStop::Undecided),
                }
            }
            &ast::ExprKind::Name(name) => (self.binding(name).and_then(|b| b.ty))
                .map(|ty| (StructRoot::Binding(name), ty))
                .ok_or(WalkStop::Undecided),
            ast::ExprKind::SelfValue => match self.body.self_type.known() {
                Some(ty) if is_struct(ty) => Ok((StructRoot::SelfValue, ty)),
                _ => Err(WalkStop::Undecided),
            },
            _ => Err(WalkStop::Undecided),
        }
    }

    /// The type of the place `e`, and whether a write can replace its value, by the declared
    /// types alone. The root is a `let mut` binding, and each struct step and index keeps its
    /// base's answer. A class step's `mut` field decides there. A help that replaces a value
    /// names only such a place.
    pub(super) fn replaceable(&self, e: &ast::Expr) -> Option<(bool, Type)> {
        match &e.kind {
            ast::ExprKind::Field {
                base,
                name,
                safe: false,
                ..
            } => {
                let (can, ty) = self.replaceable(base)?;
                let (info, can) = match ty.kind() {
                    TypeKind::Struct(id, _) => (&self.structs[id.0], Some(can)),
                    TypeKind::Class(id, _) => (&self.classes[id.0], None),
                    _ => return None,
                };
                let index = info.field(name.name)?;
                let can = can.unwrap_or(info.fields[index].mutable);
                Some((can, self.field_type(ty, index)?))
            }
            ast::ExprKind::Index { base, .. } => {
                let (can, ty) = self.replaceable(base)?;
                match ty.kind() {
                    TypeKind::Array(element) => Some((can, element)),
                    _ => None,
                }
            }
            &ast::ExprKind::Name(name) => {
                let b = self.binding(name)?;
                Some((b.kind == BindingKind::Var, b.ty?))
            }
            _ => None,
        }
    }

    /// `Assign { place, value }`, the place of `path`, after `spill`
    /// binds what it must of the operands and the value. A field or element place whose value
    /// is `P + t`, `P` reading the place and `t` unable to exit, is `Append { place, t }`
    /// instead; nothing in it can exit, so nothing spills.
    fn write(&mut self, span: Span, path: Path, value: tir::Expr) -> tir::Stmt {
        let (shape, mut operands) = path.split();
        operands.push(value);
        let (mut stmts, mut operands) = self.spill(operands);
        let value = operands
            .pop()
            .expect("`spill` should return one operand per input");
        let place = build(shape.path(operands));
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
            tir::Place::StructField { base, index } => {
                let base = self.read(*base, span);
                let ty = self.field_type(base.ty, index).expect(
                    "a place whose type is known should not be a field whose type is `Error`",
                );
                let base = Box::new(base);
                (ExprKind::StructField { base, index }, ty)
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
        (
            tir::Place::StructField { base, index },
            ExprKind::StructField {
                base: read,
                index: field,
            },
        ) => index == field && same_place(base, read),
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
        (
            ExprKind::StructField { base: a, index: i },
            ExprKind::StructField { base: b, index: j },
        ) => i == j && same_read(a, b),
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

/// The path of the place that `read` holds, the receiver `e` of a `push`: the inverse of
/// `Checker::read`.
fn unread(read: tir::Expr, e: &ast::Expr) -> Path {
    try_unread(read, e).expect("`push_receiver` should read a place as a path")
}

/// The path that `read`, the checked `e`, reads: a chain of struct fields and indexes, in step
/// with the syntax, that ends in a local or in a field of a class object. `None` for any other
/// end, such as a call, whose value is a temporary. An index whose operands `spilled` bound is a
/// block that reads them, and its path takes the operands back.
pub(super) fn try_unread(read: tir::Expr, e: &ast::Expr) -> Option<Path> {
    #[cfg(test)]
    super::PATH_STEPS.set(super::PATH_STEPS.get() + 1);
    match (&e.kind, read.kind) {
        (ast::ExprKind::Name(_) | ast::ExprKind::SelfValue, ExprKind::Local(id)) => {
            Some(Path::local(id))
        }
        (ast::ExprKind::Field { .. }, ExprKind::Field { base, index }) => Some(Path {
            root: Root::Field(index),
            base: Some(*base),
            steps: Vec::new(),
        }),
        (ast::ExprKind::Field { base: inner, .. }, ExprKind::StructField { base, index }) => {
            let mut path = try_unread(*base, inner)?;
            path.steps.push(PathStep::Field(index));
            Some(path)
        }
        (ast::ExprKind::Index { base: inner, .. }, ExprKind::Index { base, index }) => {
            let mut path = try_unread(*base, inner)?;
            path.steps.push(PathStep::Index(*index));
            Some(path)
        }
        (ast::ExprKind::Index { base: inner, .. }, ExprKind::Block(block)) => {
            let (base, index) = spilled_index(block)?;
            let mut path = try_unread(base, inner)?;
            path.steps.push(PathStep::Index(index));
            Some(path)
        }
        _ => None,
    }
}

/// Whether `try_unread` finds a path in `read`, the checked `e`.
fn is_path(read: &tir::Expr, e: &ast::Expr) -> bool {
    #[cfg(test)]
    super::PATH_STEPS.set(super::PATH_STEPS.get() + 1);
    match (&e.kind, &read.kind) {
        (ast::ExprKind::Name(_) | ast::ExprKind::SelfValue, ExprKind::Local(_))
        | (ast::ExprKind::Field { .. }, ExprKind::Field { .. }) => true,
        (ast::ExprKind::Field { base: inner, .. }, ExprKind::StructField { base, .. })
        | (ast::ExprKind::Index { base: inner, .. }, ExprKind::Index { base, .. }) => {
            is_path(base, inner)
        }
        (ast::ExprKind::Index { base: inner, .. }, ExprKind::Block(block)) => {
            spilled_base(block).is_some_and(|base| is_path(base, inner))
        }
        _ => false,
    }
}

/// The base and the index of the block that `spilled` makes of an index: `let`s that bind its
/// operands, in order, then an `Index` that reads them. An operand that it did not bind is in
/// the `Index` itself.
fn spilled_index(block: tir::Block) -> Option<(tir::Expr, tir::Expr)> {
    let mut lets = (block.stmts.into_iter())
        .map(|stmt| match stmt {
            tir::Stmt::Let { local, init } => Some((local, init)),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    let ExprKind::Index { base, index } = block.value?.kind else {
        return None;
    };
    let mut bound = |e: tir::Expr| match e.kind {
        ExprKind::Local(id) => match lets.iter().position(|(local, _)| *local == id) {
            Some(at) => lets.remove(at).1,
            None => e,
        },
        _ => e,
    };
    let base = bound(*base);
    let index = bound(*index);
    lets.is_empty().then_some((base, index))
}

/// The base that `spilled_index` gives for `block`, read in place.
fn spilled_base(block: &tir::Block) -> Option<&tir::Expr> {
    let ExprKind::Index { base, .. } = &block.value.as_ref()?.kind else {
        return None;
    };
    if !(block.stmts.iter()).all(|stmt| matches!(stmt, tir::Stmt::Let { .. })) {
        return None;
    }
    let ExprKind::Local(id) = base.kind else {
        return Some(base);
    };
    let init = block.stmts.iter().find_map(|stmt| match stmt {
        tir::Stmt::Let { local, init } if *local == id => Some(init),
        _ => None,
    });
    Some(init.unwrap_or(base))
}

/// The place that `path` names.
pub(super) fn build(path: Path) -> tir::Place {
    let mut place = match path.root {
        Root::Local(id) => tir::Place::Local(id),
        Root::Field(index) => tir::Place::Field {
            base: (path.base).expect("a field root's base should be in its path"),
            index,
        },
    };
    for step in path.steps {
        place = match step {
            PathStep::Field(index) => tir::Place::StructField {
                base: Box::new(place),
                index,
            },
            PathStep::Index(index) => tir::Place::Index {
                base: Box::new(place),
                index,
            },
        };
    }
    place
}

/// The root of a struct path, as `Checker::struct_root` finds it.
#[derive(Clone, Copy)]
pub(super) enum StructRoot {
    Binding(Symbol),
    SelfValue,
    Other,
}

/// Why `Checker::struct_walk` stops before the root of a place.
#[derive(Clone, Copy)]
enum WalkStop {
    /// The path crosses a class object. A write there changes the object, not the root.
    Class,
    /// The declared types do not decide the path. It has a missing or untyped field, or a root
    /// with no type. Or it takes a field or an element of a value that is no struct or array.
    Undecided,
}

/// Why the binding `b` cannot change, if it cannot.
fn binding_cause(b: &Binding) -> Option<Immutable> {
    let declared = b.span;
    match b.kind {
        BindingKind::Var => None,
        BindingKind::Let => Some(Immutable::Let {
            declared,
            redeclared: b.shadows_mut,
        }),
        BindingKind::Param => Some(Immutable::Param { declared }),
        BindingKind::Unwrapped(keyword) => Some(Immutable::Unwrapped { keyword, declared }),
    }
}

/// Whether `ty` is a struct.
pub(super) fn is_struct(ty: Type) -> bool {
    matches!(ty.kind(), TypeKind::Struct(..))
}

/// The deepest `?.` in the chain of fields and indexes of `e`: its object and its field's name.
pub(super) fn first_safe(mut e: &ast::Expr) -> Option<(&ast::Expr, &ast::Ident)> {
    let mut found = None;
    loop {
        #[cfg(test)]
        super::PATH_STEPS.set(super::PATH_STEPS.get() + 1);
        match &e.kind {
            ast::ExprKind::Field {
                base, name, safe, ..
            } => {
                if *safe {
                    found = Some((&**base, name));
                }
                e = base;
            }
            ast::ExprKind::Index { base, .. } => e = base,
            _ => return found,
        }
    }
}
