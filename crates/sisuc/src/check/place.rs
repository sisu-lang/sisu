//! Assignment: places, which root may be written, the temps of compound operators, and spills.

use super::expr::{literal_type, local, operator};
use super::operator::Operator;
use super::{
    BindingKind, Checked, Checker, Declared, Poisoned, UNKNOWN, block_expr, payload, type_of,
};
use crate::ast;
use crate::diagnostic::{DiagnosticKind, Immutable};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, ExprKind, Type, TypeKind};

/// A place, flattened. `operands` are its own operands in evaluation order: a field root's
/// base expression, then each index from the root outwards.
struct Path {
    root: Root,
    operands: Vec<tir::Expr>,
}

#[derive(Clone, Copy)]
enum Root {
    Local(tir::LocalId),
    /// Field `index` of the object that is the first operand.
    Field(usize),
}

struct CheckedPlace {
    /// `None` when a part of the place failed.
    path: Option<Path>,
    /// The type a value written to it needs; `None` is `Error`.
    ty: Option<Type>,
    /// `false` for a root that is a field without `mut`. A name root that cannot be written is
    /// reported but still lowers, as in milestone 2.
    writable: bool,
}

impl CheckedPlace {
    fn failed() -> Self {
        Self {
            path: None,
            ty: None,
            writable: false,
        }
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
        let place = self.place(span, target, false);
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
                Ok(self.write(span, path.root, path.operands, value))
            }
            _ => Err(Poisoned::typed(Type::UNIT)),
        }
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
        let place = self.place(span, target, false);
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
        Ok(tir::Stmt::Expr(block_expr(lets, None, Type::UNIT, span)))
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
        let place = self.place(span, target, false);
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
    fn constant_place(&mut self, span: Span, name: Symbol, element: bool) -> Option<CheckedPlace> {
        let Declared::Known(constant) = self.constants.get(&name)? else {
            return Some(CheckedPlace::failed());
        };
        let (declared, ty) = (constant.span, constant.ty);
        self.report(DiagnosticKind::AssignToImmutable {
            name: name.to_string(),
            at: span,
            element,
            cause: Immutable::Constant { declared },
        });
        Some(CheckedPlace {
            path: None,
            ty,
            writable: false,
        })
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

    /// The place `target` names, which `span` assigns. `element` when it is the base of an
    /// index, so the write replaces an element of it.
    fn place(&mut self, span: Span, target: &ast::Expr, element: bool) -> CheckedPlace {
        match &target.kind {
            &ast::ExprKind::Name(name) => {
                let stamp = self.write_stamp();
                let Some(b) = self.lookup(name) else {
                    if let Some(place) = self.constant_place(span, name, element) {
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
                b.written = stamp;
                b.reassigned |= b.kind == BindingKind::Var;
                let (local, ty, kind, declared) = (b.local, b.ty, b.kind, b.span);
                let cause = match kind {
                    BindingKind::Var => None,
                    BindingKind::Let => Some(Immutable::Let {
                        declared,
                        redeclared: b.shadows_mut,
                    }),
                    BindingKind::Param => Some(Immutable::Param { declared }),
                    BindingKind::Unwrapped(keyword) => {
                        Some(Immutable::Unwrapped { keyword, declared })
                    }
                };
                if let Some(cause) = cause {
                    self.report(DiagnosticKind::AssignToImmutable {
                        name: name.to_string(),
                        at: span,
                        element,
                        cause,
                    });
                }
                CheckedPlace {
                    path: Some(Path {
                        root: Root::Local(local),
                        operands: Vec::new(),
                    }),
                    ty,
                    writable: true,
                }
            }
            ast::ExprKind::Field { base, name, .. } => {
                self.stamp_root(base);
                // A `?.` anywhere in the target's chain would not parse.
                let outer = std::mem::replace(&mut self.body.in_target, true);
                let checked = self.value(base, None);
                self.body.in_target = outer;
                let Some((class, index, ty)) = self.find_field(type_of(&checked), base, name, true)
                else {
                    return CheckedPlace::failed();
                };
                let field = &self.classes[class.0].fields[index];
                let writable = field.mutable;
                if !writable {
                    let (name, declared) = (field.name.name.to_string(), field.name.span);
                    self.report(DiagnosticKind::AssignToImmutable {
                        name,
                        at: span,
                        element,
                        cause: Immutable::Field { declared },
                    });
                }
                CheckedPlace {
                    path: checked.ok().map(|base| Path {
                        root: Root::Field(index),
                        operands: vec![base],
                    }),
                    ty,
                    writable,
                }
            }
            ast::ExprKind::Index { base, index } => {
                let inner = self.place(span, base, true);
                let element = self.element(base, inner.ty);
                let (index, mismatched) = self.operand(index, Some(Type::I64));
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
                    writable: inner.writable,
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
        };
        tir::Expr { kind, ty, span }
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
