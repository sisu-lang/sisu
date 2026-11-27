//! Array expressions: literals and their element join, `[v; n]`, indexing and methods.

use super::branch::branch_expectation;
use super::expr::{branch_free, no_member, path, read_jump, typed};
use super::generic::{mismatch_poison, mismatched};
use super::infer::Kind;
use super::join::{Heading, JoinForm};
use super::place::{Change, first_safe, is_struct};
use super::reports::Reported;
use super::{Checked, Checker, Declared, Expectation, Misfit, Origin, Poisoned, payload, type_of};
use crate::ast;
use crate::diagnostic::{
    Definition, Diagnostic, DiagnosticKind, Expected, IndexUsage, Literal, UnwrapHelp,
};
use crate::source::Span;
use crate::tir::{self, Type, TypeArgs, TypeKind};

/// The methods of an array that the compiler writes, with their parameter counts. `push` is not
/// answered here: it writes its receiver's place, which `Checker::push` checks. The prelude's
/// `extend` blocks give an array its other methods.
pub(super) const METHODS: &[(&str, usize)] = &[("len", 0), ("push", 1)];

impl Checker<'_> {
    /// `[a, b, ...]`: the elements are the members of one join (`JoinForm::Array`), under the
    /// element of the expected array type (see `element_expectation`). A `[]` takes its type
    /// from `expected` (see `empty_array`).
    pub(super) fn array_lit(
        &mut self,
        span: Span,
        elements: &[ast::Expr],
        expected: Expectation,
    ) -> Checked {
        if elements.is_empty() {
            return self.empty_array(span, expected);
        }
        let inner = self.element_expectation(expected);
        let member = branch_expectation(inner);
        let values: Vec<Checked> = (elements.iter())
            .map(|e| self.element_value(e, member))
            .collect();
        // The join takes blocks: each element is the value of a block of its own, which the join
        // settles inside, as it settles the value of a branch.
        let blocks = (values.into_iter())
            .map(|value| {
                value.map(|e| tir::Block {
                    stmts: Vec::new(),
                    ty: e.ty,
                    value: Some(Box::new(e)),
                })
            })
            .collect();
        let spans: Vec<Span> = elements.iter().map(|e| e.span).collect();
        let heading = Heading {
            head: self.source.head(span, "[".len()),
            whole: span,
            returns: None,
        };
        let binding = self.let_name(span);
        let (blocks, element) = self.join_blocks(
            (JoinForm::Array, heading, binding),
            blocks,
            (&spans, false, false),
            inner,
        );
        let checked: Vec<Checked> = (blocks.into_iter())
            .map(|block| {
                block.map(|block| {
                    *block
                        .value
                        .expect("an element's block should hold its value")
                })
            })
            .collect();
        match element {
            // An element that the element type rejected makes the array `Error`, as a branch that
            // the type of an `if` rejects makes the `if` one: no report follows from its type.
            Ok(element) => {
                let rejected = checked.iter().any(mismatched);
                let array = self.array(span, element, checked, rejected);
                if let Ok(e) = &array
                    && branch_free(e, &self.body.branch_free)
                {
                    self.body.branch_free.insert(span);
                }
                array
            }
            Err(poisoned) => Err(Poisoned {
                ty: poisoned.ty.map(Type::array),
                found: (poisoned.found).map(|found| Misfit {
                    ty: Type::array(found.ty),
                    ..found
                }),
                failed_some: false,
                ..poisoned
            }),
        }
    }

    /// What each element of an array under `expected` takes: the element `E` of an expected
    /// `[E]` or `[E]?`, required, from the origin of `expected`. A type that is no array, or a bare
    /// variable as from a `[]` beside the array, is no type to meet. Then the elements join, and
    /// the context meets their type.
    fn element_expectation(&self, expected: Expectation) -> Expectation {
        match expected {
            Expectation::Required(..) => match expected_element(expected.ty()) {
                Some(element) if !self.bare_variable(element) => {
                    Expectation::Required(element, expected.origin())
                }
                _ => Expectation::None,
            },
            other => other,
        }
    }

    /// The value of the element `e` under `expected`: not `never`, and no `unit` where the
    /// context takes none (see `value_of`).
    fn element_value(&mut self, e: &ast::Expr, expected: Expectation) -> Checked {
        let checked = read_jump(self.expr(e, expected));
        self.value_of(e, checked, expected, false)
    }

    /// `[]` under `expected`: an array of the expected element type. An expected optional of an
    /// open variable, and a member, give it an open element, which the join gives its type.
    fn empty_array(&mut self, span: Span, expected: Expectation) -> Checked {
        if let Some(element) = expected_element(expected.ty()) {
            return self.array(span, element, Vec::new(), false);
        }
        // An optional of an open variable takes the `[]` as it is: its element is open.
        let open = expected.ty().is_some_and(|ty| {
            let mut ty = self.table.resolve(ty);
            let optional = matches!(ty.kind(), TypeKind::Optional(_));
            while let TypeKind::Optional(inner) = ty.kind() {
                ty = inner;
            }
            optional && matches!(ty.kind(), TypeKind::Infer(_))
        });
        let d = match expected {
            // A member's `[]` is an `[Any]`: the join gives its element.
            _ if open => {
                let element = self.table.fresh(Kind::Any, span);
                return self.array(span, element, Vec::new(), false);
            }
            Expectation::Member => {
                let element = self.table.fresh(Kind::Any, span);
                return self.array(span, element, Vec::new(), false);
            }
            Expectation::Poisoned => {
                return Err(Poisoned::untyped(self.poisoned_context(span)));
            }
            Expectation::Required(ty, _) => DiagnosticKind::Mismatch {
                want: Expected::Types(vec![self.type_name(ty)]),
                got: "[]".to_string(),
                at: span,
                origin: self.origin_fact(expected.origin()),
                fix: None,
                conversion: false,
                layers: false,
            },
            Expectation::None => {
                let reported = self.report_uninferred(Literal::EmptyArray, span);
                return Err(Poisoned::untyped(reported));
            }
        };
        Err(mismatch_poison(self.error(d)))
    }

    /// The `ArrayLit` of `checked`, an `[element]`, unless an element failed. `mismatched`
    /// when the literal's rule reported an element, which makes it `Error`.
    pub(super) fn array(
        &mut self,
        span: Span,
        element: Type,
        checked: Vec<Checked>,
        mismatched: bool,
    ) -> Checked {
        let ty = Type::array(element);
        match checked.into_iter().collect::<Result<Vec<_>, _>>() {
            Ok(elements) => Ok(self.spilled(span, ty, elements, tir::ExprKind::ArrayLit)),
            Err(failed) => Err(Poisoned {
                ty: (!mismatched).then_some(ty),
                mismatch: mismatched,
                ..Poisoned::untyped(failed.reported())
            }),
        }
    }

    /// `[value; len]`: `value` is checked with the expected element type as its expectation, then
    /// against it, and `len` against `i64`. A `value` of a class, under any layers of optional,
    /// is reported, as every slot would hold the same object; the literal `None` is not. That
    /// rule sees `value`'s own type, before a mismatch with the element type erases it.
    pub(super) fn array_fill(
        &mut self,
        span: Span,
        value: &ast::Expr,
        len: &ast::Expr,
        expected: Expectation,
    ) -> Checked {
        let element = expected_element(expected.ty());
        // Without an element type, a `None` or `[]` cannot be inferred, unless `expected` is
        // `Poisoned`. A member's value is a member too: the join gives the payload of its `None`.
        // The whole value of a `let` without a type keeps an integer literal open, as a member;
        // `None` and `[]` stay on `main`'s path until the stage 2 switch.
        let open = matches!(value.kind, ast::ExprKind::Int { .. })
            && expected == Expectation::None
            && self.open_let_value(span);
        let hint = match element {
            Some(element) => Expectation::Required(element, expected.origin()),
            None if open => Expectation::Member,
            None if matches!(expected, Expectation::Poisoned | Expectation::Member) => expected,
            None => Expectation::None,
        };
        // `operand`, split so the class rule runs between the check and the wrap.
        let checked = self.value(value, hint);
        let class = type_of(&checked).map(innermost).filter(|ty| {
            matches!(
                ty.kind(),
                TypeKind::Class(..) | TypeKind::Param(..) | TypeKind::Assoc(..) | TypeKind::Any(..)
            ) && !self.none_written(value)
        });
        let mut same = None;
        if let Some(class) = class {
            let any = matches!(class.kind(), TypeKind::Any(..));
            // An `any` of a class-only interface holds an object, as a class is one.
            let may_be_class = matches!(class.kind(), TypeKind::Param(..) | TypeKind::Assoc(..))
                || (any && !self.conforms(class, self.lang.any_object, TypeArgs::NONE));
            same = Some(self.error(DiagnosticKind::SameObject {
                class: self.type_name(class),
                at: value.span,
                param: may_be_class,
                any,
            }));
        }
        let (checked, mismatched) = match element {
            Some(_) => self.close(value, checked, hint),
            None => (checked, false),
        };
        let (len, len_mismatched) =
            self.operand(len, Expectation::Required(Type::I64, Origin::None));
        let failed = mismatched || same.is_some() || len_mismatched;
        let ty = type_of(&checked).map(Type::array);
        match (checked, len, ty) {
            (Ok(value), Ok(len), Some(ty)) if !failed => {
                let fill = self.spilled(span, ty, vec![value, len], |operands| {
                    let (value, len) = pair(operands);
                    tir::ExprKind::ArrayFill { value, len }
                });
                // As for an array literal: the walk of `without_value` does not enter it again.
                if branch_free(&fill, &self.body.branch_free) {
                    self.body.branch_free.insert(span);
                }
                Ok(fill)
            }
            (Err(part), _, ty) | (_, Err(part), ty) => {
                Err(Poisoned::new(ty.filter(|_| !failed), part.reported()))
            }
            // Both parts check, so the class rule failed, and reported.
            _ => Err(Poisoned::untyped(same.expect(
                "an array fill whose parts check should fail by its class rule",
            ))),
        }
    }

    /// `base[index]`: `base` is an array, not an optional one, and `index` an `i64`.
    pub(super) fn index(&mut self, span: Span, base: &ast::Expr, index: &ast::Expr) -> Checked {
        if let ast::ExprKind::Range(range) = &index.kind {
            return self.range_index(span, base, range);
        }
        let outer = std::mem::replace(&mut self.body.indexed, true);
        let checked = self.value_as(base, self.chain_use());
        // An optional array is reported, and its element goes on typed, but no code reads it.
        let optional = type_of(&checked).is_some_and(is_optional_array);
        let class_write = (self.body.write_chain.is_some() && optional)
            .then(|| self.class_unwrap(base, &checked))
            .flatten();
        self.body.indexed = outer;
        let (element, report) = match (class_write, type_of(&checked)) {
            (Some(unwrap), Some(ty)) => {
                let reported = self.diagnostics.error(must_unwrap(base, unwrap));
                (optional_element(ty), Some(reported))
            }
            _ => self.element(
                base,
                (type_of(&checked), checked.is_err()),
                IndexUsage::Read,
            ),
        };
        // The index is not part of the chain that it ends.
        self.body.indexed = false;
        let chain = std::mem::take(&mut self.body.write_chain);
        // A base with no element, such as a string, takes no `i64` index either.
        let origin = if element.is_some() {
            Origin::Index
        } else {
            Origin::None
        };
        let (index, mismatched) = self.operand(index, Expectation::Required(Type::I64, origin));
        self.body.indexed = outer;
        self.body.write_chain = chain;
        match (checked, index, element) {
            (Ok(base), Ok(index), Some(element)) if !optional => {
                Ok(self.spilled(span, element, vec![base, index], |operands| {
                    let (base, index) = pair(operands);
                    tir::ExprKind::Index { base, index }
                }))
            }
            // A failed index is untyped: a repair to an index differs from one to a range.
            (base, index, element) => {
                let ty = element.filter(|_| !mismatched && type_of(&index).is_some());
                let reported = match (base, index) {
                    (Err(part), _) | (_, Err(part)) => part.reported(),
                    // A base that checks and gives no element, or an optional one, is reported.
                    (Ok(_), Ok(_)) => {
                        report.expect("an index whose parts check should fail by its base's report")
                    }
                };
                Err(Poisoned::new(ty, reported))
            }
        }
    }

    /// The element type of `base`, which is indexed and has type `ty` (`None` is `Error`). An
    /// optional array is reported and gives the element of its payload. A string, or a value that
    /// is no array, is reported and gives `Error`. With the proof of the report, when there is one.
    /// A base that `failed` under the chain of a write to a struct has reported that write.
    pub(super) fn element(
        &mut self,
        base: &ast::Expr,
        (ty, failed): (Option<Type>, bool),
        usage: IndexUsage,
    ) -> (Option<Type>, Option<Reported>) {
        let Some(ty) = ty else {
            return (None, None);
        };
        let d: Diagnostic = match ty.kind() {
            TypeKind::Array(element) => return (Some(element), None),
            TypeKind::Optional(payload) if matches!(payload.kind(), TypeKind::Array(_)) => {
                let element = optional_element(ty);
                let target_elem = element.map(|e| {
                    if self.body.indexed {
                        indexed_leaf(e)
                    } else {
                        innermost(e)
                    }
                });
                // A struct, enum, or generic element under a write chain after `?.`: part 1 cannot
                // write through `?.`, and an unwrap gives a copy.
                let on_struct = self
                    .body
                    .write_chain
                    .is_some_and(|(_, change)| match change {
                        Change::Receiver(_, true) => target_elem.is_some_and(|e| {
                            matches!(
                                e.kind(),
                                TypeKind::Struct(..)
                                    | TypeKind::Enum(..)
                                    | TypeKind::Param(..)
                                    | TypeKind::Assoc(..)
                            )
                        }),
                        _ => target_elem.is_some_and(is_struct),
                    });
                if on_struct && failed {
                    return (element, None);
                }
                let chain = (self.body.write_chain)
                    .filter(|&(_, change)| on_struct && change != Change::MutArg)
                    .zip(first_safe(base));
                let unwrap = if on_struct {
                    UnwrapHelp::Nothing
                } else {
                    UnwrapHelp::It
                };
                let reported = match chain {
                    Some(((_, Change::Receiver(method, true)), object)) => {
                        let kind = self.chain_value_kind(target_elem);
                        let d = self.chain_call(base, object, (method, kind));
                        let d = self.keep_chain_replace(d, base);
                        self.error(d)
                    }
                    Some((write, object)) => {
                        let d = self.chain_write(write, base, object);
                        let d = self.keep_chain_replace(d, base);
                        self.error(d)
                    }
                    None => self.diagnostics.error(must_unwrap(base, unwrap)),
                };
                return (element, Some(reported));
            }
            _ if ty == Type::STRING => DiagnosticKind::StringIndex {
                at: base.span,
                base: self.source.quote(base.span),
                usage,
            }
            .into(),
            _ => DiagnosticKind::CannotIndex {
                ty: self.type_name(ty),
                at: base.span,
                range: false,
            }
            .into(),
        };
        (None, Some(self.diagnostics.error(d)))
    }

    /// `receiver.method(args)` on `receiver`, an array of type `ty`, when no block of the prelude
    /// gives `ty` the method: `len()`, which the compiler writes; a method of a block of the
    /// prelude whose bounds fail for `ty` (`unmet_block_bound`); or no method, which is E0069 when
    /// a block gives the method to another array type.
    pub(super) fn array_method(
        &mut self,
        span: Span,
        receiver: Checked,
        ty: Type,
        method: &ast::Ident,
        args: &[ast::Arg],
    ) -> Checked {
        let name = method.name.as_str();
        let built_in = METHODS.iter().find(|(m, _)| *m == name);
        if built_in.is_none()
            && let Some(Declared::Known(func)) = self.own_method(ty, method.name)
        {
            return self.unmet_block_bound(span, func, ty, method, args);
        }
        let Some(&(_, arity)) = built_in else {
            if let Some(&reported) = self.rejected_methods.get(&method.name) {
                return self.call_of_rejected_method(method, args, reported);
            }
            let prelude = self.prelude_method_names(ty);
            let targets = self.prelude_targets(method.name);
            let d = if !prelude.contains(&name) && !targets.is_empty() {
                DiagnosticKind::SumNeedsNumbers {
                    method: name.to_string(),
                    ty: self.type_name(ty),
                    targets: targets.into_iter().map(|t| self.type_name(t)).collect(),
                    at: method.span,
                    function: false,
                }
                .into()
            } else {
                let names = METHODS.iter().map(|&(m, _)| m).chain(prelude);
                no_member(Definition::Method, self.type_name(ty), method, names)
            };
            return self.rejected_lookup(d, method, args);
        };
        if args.len() != arity {
            let d = DiagnosticKind::ArgumentCount {
                name: self.method_name(ty, name),
                want: arity,
                found: args.len(),
                at: span,
            };
            return self.rejected(d.into(), args, Some(Type::I64));
        }
        let kind = receiver
            .map(|r| tir::ExprKind::Len(Box::new(r)))
            .map_err(|failed| failed.reported());
        typed(span, Type::I64, false, kind)
    }

    /// A call at `span` of the method `func` of a block of the prelude that covers the array type
    /// `ty` and whose bounds fail for it: E0154 at the call, with no label in the prelude. A call
    /// with the wrong number of arguments is the call-count error. An argument that is not of the
    /// type the method wants is the mistake before the bound, as it is for `==`: a conformance
    /// would not make the program right. So is an argument that failed on its own, unless a type
    /// parameter is in what the method wants. The bound is reported after the arguments.
    fn unmet_block_bound(
        &mut self,
        span: Span,
        func: tir::FuncId,
        ty: Type,
        method: &ast::Ident,
        args: &[ast::Arg],
    ) -> Checked {
        let sig = &self.signatures[func.0];
        let params = sig.params.clone();
        let extension = sig
            .extension
            .expect("a method whose block bounds fail should have a block");
        self.labels(args);
        if params.len() != args.len() {
            let name = self.method_name(ty, method.name.as_str());
            return self.wrong_count(span, name, params.len(), args, None);
        }
        let stands = self.owner_args(func, ty).get().to_vec();
        let mut own_mistake = None;
        for (arg, param) in args.iter().zip(params) {
            let want = param.and_then(|param| self.subst(param, &stands));
            let (argument, mismatched) =
                self.operand(&arg.value, Expectation::or_poisoned(want, Origin::None));
            // An argument with no type has failed, but a type parameter keeps its report beside
            // the error of such an argument.
            let failed = type_of(&argument).is_none() && !want.is_some_and(Type::has_param);
            if mismatched || failed {
                own_mistake = own_mistake.or_else(|| argument.err().map(|part| part.reported()));
            }
        }
        let reported = match own_mistake {
            Some(reported) => reported,
            None => self.report_block_bound(extension, ty, span, method.name.as_str()),
        };
        Err(Poisoned::untyped(reported))
    }

    /// A call of the method `method` that a rejected `extend` block may have given (see
    /// `Checker.rejected_methods`): poisoned, with no report of its own. `reported` is the proof
    /// of the block's report.
    pub(super) fn call_of_rejected_method(
        &mut self,
        method: &ast::Ident,
        args: &[ast::Arg],
        reported: Reported,
    ) -> Checked {
        self.body.rejected_calls.insert(method.span);
        self.labels(args);
        self.stray(args);
        Err(Poisoned::untyped(reported))
    }

    /// Reports `d` for a method call that cannot be checked, and its arguments' own errors. `ret`
    /// is the type of the call when its method fixes it.
    pub(super) fn rejected(
        &mut self,
        d: Diagnostic,
        args: &[ast::Arg],
        ret: Option<Type>,
    ) -> Checked {
        let reported = self.diagnostics.error(d);
        self.labels(args);
        self.stray(args);
        Err(Poisoned::new(ret, reported))
    }
}

/// "`m` may be `None`", at the optional `base` that an index, a `for` or a write through a class
/// `?.` needs unwrapped. `unwrap` says what the help unwraps.
pub(super) fn must_unwrap(base: &ast::Expr, unwrap: UnwrapHelp) -> Diagnostic {
    DiagnosticKind::MayBeNone {
        value: path(base),
        at: base.span,
        access: None,
        write: false,
        peel: None,
        unwrap,
    }
    .into()
}

/// Whether `ty` is `[T]?`.
pub(super) fn is_optional_array(ty: Type) -> bool {
    optional_element(ty).is_some()
}

/// `T`, when `ty` is `[T]?`.
pub(super) fn optional_element(ty: Type) -> Option<Type> {
    let TypeKind::Optional(payload) = ty.kind() else {
        return None;
    };
    match payload.kind() {
        TypeKind::Array(element) => Some(element),
        _ => None,
    }
}

/// `E`, when `expected` is `[E]` or `[E]?`.
fn expected_element(expected: Option<Type>) -> Option<Type> {
    match expected.map(payload).map(Type::kind) {
        Some(TypeKind::Array(element)) => Some(element),
        _ => None,
    }
}

/// The two operands `spilled` hands back for a node of two.
pub(super) fn pair(operands: Vec<tir::Expr>) -> (Box<tir::Expr>, Box<tir::Expr>) {
    let [a, b]: [tir::Expr; 2] = operands
        .try_into()
        .expect("`spilled` should hand its closure one operand per input, and there are two");
    (Box::new(a), Box::new(b))
}

/// `ty` with every leading optional layer taken off.
pub(super) fn innermost(ty: Type) -> Type {
    ty.without_optionals()
}

/// The leaf type inside nested array and optional layers of `ty`.
fn indexed_leaf(mut ty: Type) -> Type {
    while let TypeKind::Array(inner) | TypeKind::Optional(inner) = ty.kind() {
        ty = inner;
    }
    ty
}
