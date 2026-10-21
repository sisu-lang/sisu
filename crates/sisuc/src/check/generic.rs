//! Generic declarations: type parameters, the type arguments written for them, and what a value
//! of a type parameter may not do.

use std::fmt;

use rustc_hash::FxHashMap;

use super::prelude::PRELUDE;
use super::{
    Checked, Checker, Declared, Definition, DiagnosticKind, Poisoned, built_in_type, type_of,
};
use crate::ast;
use crate::diagnostic::{ArgumentsForm, MainForm};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{Type, TypeArgs, TypeKind};

impl Checker<'_> {
    /// The type parameters `owner`, a method's class's or enum's, then `own`, a declaration's.
    /// Reports each of `own` that takes a built-in type's name, or the name of one before it;
    /// `owner`'s are reported where they are declared. Every name keeps its place, so that a
    /// `Param`'s index is its place.
    pub(super) fn type_params(&mut self, owner: &[ast::Ident], own: &[ast::Ident]) -> Vec<Symbol> {
        let mut first = FxHashMap::default();
        for param in owner {
            first
                .entry(param.name)
                .or_insert((param.span, Definition::Type));
        }
        for param in own {
            if !self.built_in(param) {
                self.first_definition(&mut first, Definition::Type, None, param);
            }
        }
        owner.iter().chain(own).map(|param| param.name).collect()
    }

    /// The type arguments `written` in `angles` after `name`, whose declaration's type
    /// parameters are `declared`. `None`, reported, when they do not fit `declared`, or one is
    /// `Error` or cannot be a type argument. A list after a name that takes none is reported,
    /// and the name stands without it.
    pub(super) fn type_args(
        &mut self,
        name: &ast::Ident,
        declared: &[Symbol],
        written: &[ast::TypeExpr],
        angles: Span,
    ) -> Option<TypeArgs> {
        let form = match (declared.len(), written.len()) {
            (0, 0) => return Some(TypeArgs::NONE),
            (0, _) => {
                self.not_generic(name.name, angles);
                return Some(TypeArgs::NONE);
            }
            (_, 0) => ArgumentsForm::Missing {
                params: declared.iter().map(Symbol::to_string).collect(),
            },
            (want, found) if want != found => ArgumentsForm::Count { want, found },
            _ => {
                // Each argument is checked, so this collects into a `Vec` before it gives up.
                let args: Vec<_> = written.iter().map(|arg| self.type_argument(arg)).collect();
                return args
                    .into_iter()
                    .collect::<Option<Vec<_>>>()
                    .map(|args| TypeArgs::intern(&args));
            }
        };
        let at = match form {
            ArgumentsForm::Missing { .. } => name.span,
            _ => angles,
        };
        self.report(DiagnosticKind::TypeArguments {
            name: name.name.to_string(),
            at,
            form,
        });
        None
    }

    /// Reports the type arguments in `angles` after `name`, which takes none: the fix removes
    /// them.
    pub(super) fn not_generic(&mut self, name: Symbol, angles: Span) {
        let safe = !self.comment_in(&[angles.range()]);
        self.report(DiagnosticKind::TypeArguments {
            name: name.to_string(),
            at: angles,
            form: ArgumentsForm::NotGeneric { safe },
        });
    }

    /// The type of the type argument `arg`: `None`, reported, for `unit`, `never` or an
    /// optional, which no type parameter takes.
    fn type_argument(&mut self, arg: &ast::TypeExpr) -> Option<Type> {
        if let ast::TypeKind::Name(name) = arg.kind
            && matches!(name.as_str(), "unit" | "never")
            && !self.body.type_params.contains(&name)
            && !self.class_ids.contains_key(&name)
            && !self.enum_ids.contains_key(&name)
        {
            self.report(DiagnosticKind::TypeArgument {
                ty: arg.to_string(),
                at: arg.span,
                optional: false,
            });
            return None;
        }
        let ty = self.resolve(arg)?;
        if arg.optional {
            self.report(DiagnosticKind::TypeArgument {
                ty: self.type_name(ty),
                at: arg.span,
                optional: true,
            });
            return None;
        }
        Some(ty)
    }

    /// Reports `main`, which has type parameters. The fix removes them, unless a comment is
    /// among them or the rest of `main` names one.
    pub(super) fn main_type_params(&mut self, main: &ast::Function) {
        let angles = main.angles;
        let rest = &self.source.text[angles.end()..main.span.end()];
        let named = rest
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .any(|word| main.type_params.iter().any(|p| p.name.as_str() == word));
        let remove = (!named && !self.comment_in(&[angles.range()])).then_some(angles);
        self.report(DiagnosticKind::MainSignature {
            at: angles,
            form: MainForm::TypeParams { remove },
        });
    }

    /// Reports `op` on the operand at `at`, of type `ty`, which names a type parameter: no
    /// operator takes one.
    pub(super) fn param_lacks(&mut self, at: Span, ty: Type, op: impl fmt::Display) {
        self.report(DiagnosticKind::ParamLacks {
            ty: self.type_name(ty),
            op: op.to_string(),
            at,
            contains: false,
        });
    }

    /// Reports `name<...>`, a value with type arguments in `angles`: the `<` and `>` were
    /// comparisons. `name` is the value as written, a name or a field, and `first` the first
    /// type argument, which the help puts in parentheses with `name`.
    pub(super) fn misread(&mut self, name: Span, first: &ast::TypeExpr) -> Poisoned {
        self.body.misread += 1;
        let text = self.source.text;
        self.report(DiagnosticKind::TypeArguments {
            name: text[name.start()..name.end()].to_string(),
            at: name,
            form: ArgumentsForm::Value {
                compare: text[name.start()..first.span.end()].to_string(),
            },
        });
        Poisoned::error()
    }

    /// The type arguments `types`, in `angles`, written after `name` in an expression, which
    /// are reported unless a generic declaration takes them. `Some` when `name` is a value,
    /// whose `<` and `>` were comparisons: the expression is poisoned.
    pub(super) fn named_args(
        &mut self,
        name: &ast::Ident,
        types: &[ast::TypeExpr],
        angles: Span,
    ) -> Option<Poisoned> {
        let first = types.first()?;
        let n = name.name;
        let params = match (
            self.class_ids.get(&n),
            self.function_ids.get(&n),
            self.enum_ids.get(&n),
        ) {
            (Some(&Declared::Known(id)), ..) => Some(self.classes[id.0].type_params.len()),
            (_, Some(&Declared::Known(id)), _) => Some(self.signatures[id.0].type_params.len()),
            (.., Some(&Declared::Known(id))) => Some(self.enums[id.0].type_params.len()),
            _ => None,
        };
        match params {
            Some(0) => self.not_generic(n, angles),
            Some(_) => panic!("type arguments in expressions land in Task 4"),
            None if PRELUDE.contains(&n.as_str()) || built_in_type(n.as_str()).is_some() => {
                self.not_generic(n, angles);
            }
            None if self.lookup(n).is_some()
                || matches!(self.constants.get(&n), Some(Declared::Known(_))) =>
            {
                return Some(self.misread(name.span, first));
            }
            // A broken or unknown name: its use reports it.
            None => {}
        }
        None
    }

    /// Whether `receiver` before `.method` is a path, not a value: an enum, or `math` or a
    /// built-in type that no local takes.
    pub(super) fn path_method(&mut self, receiver: &ast::Expr, method: &ast::Ident) -> bool {
        let name = match &receiver.kind {
            ast::ExprKind::Name(name) => *name,
            ast::ExprKind::Applied { name, .. } => name.name,
            _ => return false,
        };
        if let Some(&Declared::Known(id)) = self.enum_ids.get(&name) {
            let info = &self.enums[id.0];
            if let Some(&Declared::Known(func)) = info.methods.get(&method.name) {
                assert!(
                    self.signatures[func.0].type_params.len() == info.type_params.len(),
                    "type arguments in expressions land in Task 4"
                );
            }
            return true;
        }
        (built_in_type(name.as_str()).is_some() || matches!(name.as_str(), "math" | "Math"))
            && self.lookup(name).is_none()
    }

    /// Reports the type arguments in `angles` of `receiver.method<first, ...>`, `receiver`
    /// checked from `receiver_expr`: a method that takes none, or a field, a value whose `<` and
    /// `>` were comparisons, which is `Some` and poisons the expression. A member the receiver
    /// does not have is reported as that alone.
    pub(super) fn method_args(
        &mut self,
        receiver: &Checked,
        receiver_expr: &ast::Expr,
        method: &ast::Ident,
        first: &ast::TypeExpr,
        angles: Span,
    ) -> Option<Poisoned> {
        let (methods, owner_params, field) = match type_of(receiver)?.kind() {
            TypeKind::Class(id, _) => {
                let info = &self.classes[id.0];
                let field = info.fields.iter().any(|f| f.name.name == method.name);
                (&info.methods, info.type_params.len(), field)
            }
            TypeKind::Enum(id, _) => {
                let info = &self.enums[id.0];
                (&info.methods, info.type_params.len(), false)
            }
            TypeKind::Array(_) | TypeKind::String | TypeKind::I64 | TypeKind::F64 => {
                self.not_generic(method.name, angles);
                return None;
            }
            _ => return None,
        };
        match methods.get(&method.name).copied() {
            Some(Declared::Known(func)) => {
                assert!(
                    self.signatures[func.0].type_params.len() == owner_params,
                    "type arguments in expressions land in Task 4"
                );
                self.not_generic(method.name, angles);
                None
            }
            None if field => Some(self.misread(receiver_expr.span.to(method.span), first)),
            _ => None,
        }
    }
}
