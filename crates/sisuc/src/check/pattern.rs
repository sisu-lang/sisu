//! Patterns, `match`, and the patterns of `if let` and `while let`.

use std::collections::hash_map::Entry;
use std::iter;

use rustc_hash::{FxHashMap, FxHashSet};

use super::branch::Branch;
use super::enums::reserved_name;
use super::exhaust::{self, Report, Row};
use super::generic::Ahead;
use super::stmt::BlockSoFar;
use super::{Checked, CheckedBlock, Checker, Declared, Poisoned, type_of};
use crate::ast;
use crate::diagnostic::{
    BadRange, Definition, DiagnosticKind, GuardHelp, MismatchSource, Unwrap, suggestion,
};
use crate::source::Span;
use crate::symbol::Symbol;
use crate::tir::{self, Type, TypeKind};

/// The names one pattern binds, at `top`, as its check goes; `arm` is the `match` arm it
/// starts, if any.
struct Bound<'a> {
    top: Span,
    arm: Option<&'a ast::Arm>,
    /// Each name bound so far on the way through the pattern, where, and its local; an `|`
    /// counts as its first alternative once it is checked.
    names: FxHashMap<Symbol, (Span, tir::LocalId)>,
    /// The names of `names` in the order they were bound.
    order: Vec<Symbol>,
    /// The pattern's locals in order, each once.
    locals: Vec<tir::LocalId>,
    /// For each `|` whose later alternatives are being checked, innermost last: the locals of
    /// its first alternative, which the later ones bind again.
    alternatives: Vec<Alternatives>,
}

/// The names the first alternative of an `|` binds, and the first name a later alternative
/// binds that it does not.
struct Alternatives {
    first: FxHashMap<Symbol, tir::LocalId>,
    extra: Option<Symbol>,
}

/// What `if let` or `while let` does with its value, as `let_value` decides.
pub(super) enum LetValue {
    /// Unwraps an optional, binding `name` to its payload, of type `bound` (`None` is `Error`).
    Unwrap {
        name: ast::Ident,
        bound: Option<Type>,
    },
    /// Tests the value, of type `ty` (`None` is `Error`), against the pattern.
    Match { ty: Option<Type> },
}

/// The checked pattern of an `if let`, a `while let` or a `match` arm, from `bind_pattern`.
pub(super) enum LetPattern {
    /// An optional's payload bound to `bind`, as `IfSome` binds it.
    Some { bind: tir::LocalId },
    /// An enum tested against `pattern`, which binds `bindings`; `None` when it failed.
    Match {
        pattern: Option<(tir::Pattern, Vec<tir::LocalId>)>,
    },
}

/// Whether `ty` is one `match` takes.
pub(super) fn matchable(ty: Type) -> bool {
    matches!(
        ty.kind(),
        TypeKind::Enum(..)
            | TypeKind::I64
            | TypeKind::U8
            | TypeKind::Bool
            | TypeKind::Char
            | TypeKind::String
    )
}

/// The key under which ranking keeps the branch of `arm` (see `Ahead::Bound`). A block is kept
/// under its own span. A block in parentheses has a body that starts at `(`, so the key is the
/// block's span, not the body's.
pub(super) fn arm_key(arm: &ast::Arm) -> Span {
    match &arm.body.kind {
        ast::ExprKind::Block(block) => block.span,
        _ => arm.body.span,
    }
}

/// The path of a variant pattern as written: `Num` or `Expr.Num`.
fn written(qualifier: Option<&ast::Ident>, name: &ast::Ident) -> String {
    match qualifier {
        Some(q) => format!("{}.{}", q.name, name.name),
        None => name.name.to_string(),
    }
}

/// An empty block of type `unit`: the `_` arm an `if let` with no `else` lowers to.
fn empty_block() -> tir::Block {
    tir::Block {
        stmts: Vec::new(),
        value: None,
        ty: Type::UNIT,
    }
}

impl Checker<'_> {
    /// `match scrutinee { arms }`, whose keyword is at `keyword` and `}` at `close`. Each arm is
    /// checked in a scope that holds its bindings, and the arms join as an `if`'s branches do
    /// (`join`). When every pattern checked, the arms are checked for coverage and reachability.
    pub(super) fn match_expr(
        &mut self,
        span: Span,
        keyword: Span,
        close: Span,
        scrutinee: &ast::Expr,
        arms: &[ast::Arm],
        expected: Option<Type>,
    ) -> Checked {
        let checked = self.value_kept(scrutinee);
        let mut position = type_of(&checked);
        let cannot = position.take_if(|&mut ty| !matchable(ty));
        if let Some(ty) = cannot {
            let ty = self.type_name(ty);
            self.report(DiagnosticKind::CannotMatch {
                ty,
                at: scrutinee.span,
            });
        }
        let mismatched = cannot.is_some();
        let sources: Vec<_> = arms
            .iter()
            .map(|arm| match &arm.body.kind {
                ast::ExprKind::Block(block) => Branch::of_block(block),
                _ => Branch::of_expr(&arm.body),
            })
            .collect();
        let mut patterns: Vec<_> = arms.iter().map(|_| None).collect();
        let mut guards: Vec<_> = arms.iter().map(|_| None).collect();
        let form = LetValue::Match { ty: position };
        let (blocks, ty) = self.join(span, &sources, expected, |c, i, expected| {
            let arm = &arms[i];
            let (bound, guard, so_far) = c.bound_branch(arm_key(arm), |c| {
                c.bind_pattern(None, &form, &arm.pattern, Some(arm))
            });
            let LetPattern::Match { pattern } = bound else {
                panic!("a `match` arm should test its value, which `bind_pattern` never unwraps");
            };
            patterns[i] = pattern;
            guards[i] = guard;
            if let ast::ExprKind::Block(block) = &arm.body.kind {
                return c.resume_block(block, expected, so_far);
            }
            let value = c.expr(&arm.body, expected);
            c.pop_scope();
            value.map(|value| tir::Block {
                stmts: Vec::new(),
                ty: value.ty,
                value: Some(Box::new(value)),
            })
        });
        // A scrutinee `match` cannot take fails the `match`, as a failed condition fails an `if`.
        // With no arms, only a scrutinee of a known type makes it `never`.
        let ty = match ty {
            Ok(Type::NEVER) if mismatched && !arms.is_empty() => Err(Poisoned::typed(Type::NEVER)),
            Ok(_) if mismatched || (arms.is_empty() && position.is_none()) => {
                Err(Poisoned::error())
            }
            ty => ty,
        };
        let mut reachable = vec![true; arms.len()];
        if let Some(ty) = position {
            let rows: Option<Vec<_>> = patterns
                .iter()
                .zip(arms)
                .map(|(p, arm)| {
                    p.as_ref().map(|(pattern, _)| Row {
                        pattern,
                        guarded: arm.guard.is_some(),
                    })
                })
                .collect();
            if let Some(rows) = rows {
                let report = exhaust::check(&self.lowered_enums, ty, &rows);
                self.report_coverage(keyword, close, arms, report, &mut reachable);
            }
        }
        let arms: Option<Vec<_>> = patterns
            .into_iter()
            .zip(guards)
            .zip(blocks)
            .zip(reachable)
            .map(|(((pattern, guard), body), reachable)| {
                let (pattern, bindings) = pattern?;
                let guard = match guard {
                    Some(guard) => Some(guard.ok()?),
                    None => None,
                };
                Some(tir::Arm {
                    pattern,
                    bindings,
                    guard,
                    body: body.ok()?,
                    reachable,
                })
            })
            .collect();
        match (checked, arms, ty) {
            (Ok(scrutinee), Some(arms), Ok(ty)) => Ok(tir::Expr {
                kind: tir::ExprKind::Match {
                    temp: self.fresh(scrutinee.ty),
                    scrutinee: Box::new(scrutinee),
                    arms,
                },
                ty,
                span,
            }),
            (.., ty) => Err(ty.map_or_else(|poisoned| poisoned, Poisoned::typed)),
        }
    }

    /// Reports what `report` found on the `arms` of the `match` whose keyword is at `keyword`
    /// and `}` at `close`, and clears `reachable` for each arm that no value reaches.
    fn report_coverage(
        &mut self,
        keyword: Span,
        close: Span,
        arms: &[ast::Arm],
        report: Report,
        reachable: &mut [bool],
    ) {
        let source = self.source.file;
        let line_start = |at: usize| source.slice(0..at).rfind('\n').map_or(0, |n| n + 1);
        // Each entry is an arm's pattern or an alternative in it, in arm order.
        let mut rest = arms.iter().enumerate();
        let mut current = rest.next();
        for found in report.unreachable {
            let within = |arm: &ast::Arm| {
                let pattern = arm.pattern.span;
                pattern.start() <= found.at.start() && found.at.end() <= pattern.end()
            };
            while current.is_some_and(|(_, arm)| !within(arm)) {
                current = rest.next();
            }
            let Some((index, arm)) = current else {
                break;
            };
            reachable[index] &= found.alternative.is_some();
            let remove = found.alternative.unwrap_or_else(|| {
                // Arms are separated by a line break or a `;` (ADR 0028), so an arm may share
                // its line. The first arm is always reached, so a previous arm exists.
                let start = arm.pattern.span.start();
                let end = arm.body.span.end();
                let next = arms.get(index + 1).map(|next| next.pattern.span.start());
                if !source.slice(line_start(start)..start).trim().is_empty() {
                    // The arm follows another on its line: take the `;` before it too.
                    let previous = index.checked_sub(1).and_then(|i| arms.get(i));
                    let previous = previous.expect("an unreachable arm should follow an arm");
                    return Span::new(previous.body.span.end(), end);
                }
                if let Some(next) = next.filter(|&next| line_start(next) == line_start(end)) {
                    // The next arm starts on the line where this body ends: take this arm up
                    // to it, so the fix leaves the next arm in place.
                    return Span::new(start, next);
                }
                // This arm starts its line and the next starts its own. The fix takes the
                // whole line, a comment after the body too, unless the `match`'s `}` ends it:
                // then from the line break before the arm through the body, so the `}` stays.
                let line_end = source
                    .rest(end)
                    .find('\n')
                    .map_or(source.end(), |n| end + n);
                if close.start() < line_end {
                    Span::new(line_start(start) - 1, end)
                } else {
                    Span::new(line_start(start), (line_end + 1).min(source.end()))
                }
            });
            self.report(DiagnosticKind::UnreachableArm {
                at: found.at,
                alternative: found.alternative.is_some(),
                covered_by: found.covered_by,
                remove,
            });
        }
        let Some(missing) = report.missing else {
            return;
        };
        // What comes before `at` on its line, when that is only whitespace.
        let blank =
            |at: usize| Some(source.slice(line_start(at)..at)).filter(|s| s.trim().is_empty());
        let line = source.slice(line_start(keyword.start())..keyword.start());
        let match_indent = &line[..line.len() - line.trim_start().len()];
        let close = close.start();
        let indent = match arms.first().and_then(|arm| blank(arm.pattern.span.start())) {
            Some(indent) => indent.to_string(),
            None => format!("{match_indent}    "),
        };
        // A `}` on a line of its own keeps it; one after the last arm moves to a new line, and
        // the fix replaces the space before it.
        let (insert, close) = if blank(close).is_some() {
            (Span::new(line_start(close), line_start(close)), None)
        } else {
            let start = arms.last().map_or(close, |arm| arm.body.span.end());
            (Span::new(start, close), Some(match_indent.to_string()))
        };
        self.report(DiagnosticKind::NonExhaustive {
            missing: missing.pattern,
            partly: missing.partly,
            guarded: missing.guarded,
            gap: missing.gap,
            at: keyword,
            insert,
            indent,
            close,
        });
    }

    /// `value`, which `keyword` tests against `pattern`, and what it does with it: unwraps an
    /// optional when `pattern` is a name or `_`, else matches it. A value that is neither an
    /// enum nor optional is reported, as the `bool` says, and becomes `Error`; the pattern is
    /// then checked at an `Error` position, so its names are poisoned.
    pub(super) fn let_value(
        &mut self,
        keyword: Unwrap,
        pattern: &ast::Pattern,
        value: &ast::Expr,
    ) -> (Checked, LetValue, bool) {
        let checked = self.value(value, None);
        let ty = type_of(&checked);
        let name = match &pattern.kind {
            ast::PatternKind::Name(name) => Some(name.clone()),
            ast::PatternKind::Wild => Some(ast::Ident {
                name: Symbol::intern("_"),
                span: pattern.span,
            }),
            ast::PatternKind::None
            | ast::PatternKind::Variant { .. }
            | ast::PatternKind::Literal(_)
            | ast::PatternKind::Range { .. }
            | ast::PatternKind::Or(_) => None,
        };
        match (ty.map(|ty| (ty, ty.kind())), name) {
            (Some((_, TypeKind::Optional(payload))), Some(name)) => {
                let bound = Some(payload);
                (checked, LetValue::Unwrap { name, bound }, false)
            }
            // `pattern_in_scope` reports any other pattern on an optional.
            (Some((_, TypeKind::Optional(_) | TypeKind::Enum(..))) | None, _) => {
                (checked, LetValue::Match { ty }, false)
            }
            (Some((ty, kind)), _) => {
                let compare = matches!(
                    kind,
                    TypeKind::I64
                        | TypeKind::U8
                        | TypeKind::Bool
                        | TypeKind::String
                        | TypeKind::Char
                );
                let found = self.type_name(ty);
                self.report(DiagnosticKind::UnwrapNeedsOptional {
                    keyword,
                    found,
                    at: value.span,
                    compare,
                });
                (Err(Poisoned::error()), LetValue::Match { ty: None }, true)
            }
        }
    }

    /// `block`, in a scope holding what `pattern` binds as `let_value` decided, for `keyword`.
    pub(super) fn let_block(
        &mut self,
        keyword: Unwrap,
        value: &LetValue,
        pattern: &ast::Pattern,
        block: &ast::Block,
        expected: Option<Type>,
    ) -> (LetPattern, CheckedBlock) {
        let (bound, _, so_far) = self.bound_branch(block.span, |c| {
            c.bind_pattern(Some(keyword), value, pattern, None)
        });
        (bound, self.resume_block(block, expected, so_far))
    }

    /// The branch at `key` with its pattern bound, in its scope, now open: as ranking kept it
    /// (see `Ahead::Bound`), else by `bind` in a fresh scope. What the pattern bound, the
    /// branch's checked guard, and its statements checked so far.
    pub(super) fn bound_branch(
        &mut self,
        key: Span,
        bind: impl FnOnce(&mut Self) -> (LetPattern, Option<Checked>),
    ) -> (LetPattern, Option<Checked>, BlockSoFar) {
        if let Some((_, Ahead::Bound(bound, guard, scope, so_far))) = self.kept(key, true) {
            self.body.scopes.push(scope);
            return (bound, guard, so_far);
        }
        self.body.scopes.push(FxHashMap::default());
        let (bound, guard) = bind(self);
        (bound, guard, BlockSoFar::default())
    }

    /// What `pattern` binds of `value`, declared in the scope just opened, and the checked
    /// guard of `arm`, the `match` arm it starts, if any. `keyword` is the `if let` or
    /// `while let` that tests `value`, and reports a pattern that always matches; it is `None`
    /// for an arm. The guard is checked here, before the body, so it sees no name the body
    /// declares.
    pub(super) fn bind_pattern(
        &mut self,
        keyword: Option<Unwrap>,
        value: &LetValue,
        pattern: &ast::Pattern,
        arm: Option<&ast::Arm>,
    ) -> (LetPattern, Option<Checked>) {
        let word = keyword.map_or("match", Unwrap::word);
        let bound = match value {
            LetValue::Unwrap { name, bound } => LetPattern::Some {
                bind: self.bind_unwrapped(word, name, *bound),
            },
            &LetValue::Match { ty } => {
                let pattern = self.pattern_in_scope(pattern, ty, word, arm);
                // An optional takes only a name or `_`, which unwraps it: only an enum gets here.
                if let (Some(keyword), Some(ty), Some((pattern, _))) = (keyword, ty, &pattern)
                    && let TypeKind::Enum(..) = ty.kind()
                {
                    let rows = [Row {
                        pattern,
                        guarded: false,
                    }];
                    if exhaust::check(&self.lowered_enums, ty, &rows)
                        .missing
                        .is_none()
                    {
                        self.report(DiagnosticKind::AlwaysMatches {
                            keyword,
                            at: pattern.span,
                        });
                    }
                }
                LetPattern::Match { pattern }
            }
        };
        let guard = arm
            .and_then(|arm| arm.guard.as_ref())
            .map(|guard| self.expect(guard, Some(Type::BOOL)));
        (bound, guard)
    }

    /// `pattern` at a position of type `ty` (`None` is `Error`), its names declared in the
    /// innermost scope as `keyword` binds them; `arm` is the `match` arm it starts, if any.
    /// `None` when it failed.
    fn pattern_in_scope(
        &mut self,
        pattern: &ast::Pattern,
        ty: Option<Type>,
        keyword: &'static str,
        arm: Option<&ast::Arm>,
    ) -> Option<(tir::Pattern, Vec<tir::LocalId>)> {
        let mut bound = Bound {
            top: pattern.span,
            arm,
            names: FxHashMap::default(),
            order: Vec::new(),
            locals: Vec::new(),
            alternatives: Vec::new(),
        };
        let pattern = self.pattern(pattern, ty, keyword, &mut bound)?;
        Some((pattern, bound.locals))
    }

    /// `p` at a position of type `ty`; see `pattern_in_scope`. A failed pattern still declares
    /// its names, poisoned, so the arm's body does not report them.
    fn pattern(
        &mut self,
        p: &ast::Pattern,
        ty: Option<Type>,
        keyword: &'static str,
        bound: &mut Bound<'_>,
    ) -> Option<tir::Pattern> {
        let pattern = |kind| tir::Pattern { kind, span: p.span };
        // An `Error` position matches anything; its own error is reported.
        let Some(ty) = ty else {
            self.poison(p, keyword, bound);
            return None;
        };
        let names_only = !matchable(ty);
        match &p.kind {
            ast::PatternKind::None
            | ast::PatternKind::Literal(_)
            | ast::PatternKind::Range { .. }
                if names_only =>
            {
                self.only_names(ty, p.span);
                None
            }
            ast::PatternKind::Literal(literal) => {
                self.literal_pattern(literal, p.span, ty).map(pattern)
            }
            ast::PatternKind::Range {
                start,
                end,
                inclusive,
            } => self
                .range_pattern(p.span, start, end, *inclusive, ty)
                .map(pattern),
            ast::PatternKind::Or(alternatives) if names_only => {
                self.only_names(ty, p.span);
                for alternative in alternatives {
                    self.poison(alternative, keyword, bound);
                }
                None
            }
            ast::PatternKind::Or(alternatives) => self
                .alternatives(alternatives, ty, keyword, bound)
                .map(pattern),
            ast::PatternKind::Wild => Some(pattern(tir::PatternKind::Wild)),
            ast::PatternKind::Name(name) => {
                self.name_pattern(name, ty, keyword, bound).map(pattern)
            }
            ast::PatternKind::None => {
                let want = self.type_name(ty);
                self.report(DiagnosticKind::PatternMismatch {
                    want,
                    found: "None".to_string(),
                    at: p.span,
                });
                None
            }
            ast::PatternKind::Variant {
                qualifier,
                name,
                fields,
                parens,
            } => {
                let kind = if names_only {
                    self.only_names(ty, p.span);
                    None
                } else {
                    self.variant_pattern(p.span, ty, qualifier.as_ref(), name, fields, *parens)
                };
                let Some((variant, types)) = kind else {
                    for field in fields {
                        self.poison(field, keyword, bound);
                    }
                    return None;
                };
                // Every field is checked, so the names after a failed one are still bound.
                let fields: Vec<_> = fields
                    .iter()
                    .zip(&types)
                    .map(|(field, &ty)| self.pattern(field, ty, keyword, bound))
                    .collect();
                Some(pattern(tir::PatternKind::Variant {
                    variant,
                    fields: fields.into_iter().collect::<Option<_>>()?,
                }))
            }
        }
    }

    /// The name pattern `name` at a position of type `ty`: a variant of `ty`'s enum, else a
    /// binding, unless it is a constant or an upper-case name, which `ty` has no variant of.
    fn name_pattern(
        &mut self,
        name: &ast::Ident,
        ty: Type,
        keyword: &'static str,
        bound: &mut Bound<'_>,
    ) -> Option<tir::PatternKind> {
        let text = name.name.as_str();
        let enum_id = match ty.kind() {
            TypeKind::Enum(id, _) => Some(id.0),
            _ => None,
        };
        if let Some(id) = enum_id
            && let Some(&variant) = self.enums[id].index.get(&name.name)
        {
            if self.enums[id].variants[variant].fields.is_empty() {
                return Some(tir::PatternKind::Variant {
                    variant,
                    fields: Vec::new(),
                });
            }
            self.report(DiagnosticKind::VariantForm {
                path: text.to_string(),
                payload: true,
                held: false,
                at: name.span,
            });
            return None;
        }
        // A name the prelude takes is a binding, which `declare` reports.
        if reserved_name(text).is_none() {
            if self.constants.contains_key(&name.name) {
                let candidates =
                    iter::once("n".to_string()).chain((2..).map(|n: u32| format!("n{n}")));
                // Bound in one alternative of an `|`, the name would be missing from the others.
                let guard = if bound.arm.is_some_and(|arm| has_or(&arm.pattern)) {
                    Some(GuardHelp::InWords)
                } else {
                    self.guard_help(bound, candidates).map(|(fresh, own)| {
                        GuardHelp::Rewritten(format!("{fresh} if {fresh} == {text}{own}"))
                    })
                };
                self.report(DiagnosticKind::ConstantInPattern {
                    name: text.to_string(),
                    at: name.span,
                    guard,
                });
                return None;
            }
            if text.starts_with(char::is_uppercase) {
                let kind = if let Some(id) = enum_id {
                    let info = &self.enums[id];
                    let variants = info
                        .variants
                        .iter()
                        .map(|v| (v.name.name.as_str(), Definition::Variant));
                    DiagnosticKind::UnknownVariant {
                        enum_name: info.name.name.to_string(),
                        name: text.to_string(),
                        at: name.span,
                        suggestion: suggestion(text, variants),
                    }
                } else {
                    // Read as the binding the fix makes it, so the body does not cascade. The
                    // name is not checked as a binding's: this error is the one it gets.
                    if let Entry::Vacant(entry) = bound.names.entry(name.name) {
                        let kind = super::BindingKind::Unwrapped(keyword);
                        let local = self.insert_local(name, None, kind, None);
                        entry.insert((name.span, local));
                        bound.order.push(name.name);
                        bound.locals.push(local);
                    }
                    DiagnosticKind::NotVariantHere {
                        name: text.to_string(),
                        at: name.span,
                    }
                };
                self.report(kind);
                return None;
            }
        }
        self.bind(name, Some(ty), keyword, bound)
            .map(tir::PatternKind::Bind)
    }

    /// The literal `literal`, at `at`, at a position of type `ty`, which takes more than names:
    /// an integer at `i64` or `u8`, else a literal of the position's own type.
    fn literal_pattern(
        &mut self,
        literal: &ast::Literal,
        at: Span,
        ty: Type,
    ) -> Option<tir::PatternKind> {
        match (literal, ty) {
            (
                &ast::Literal::Int {
                    value,
                    text,
                    negative,
                },
                Type::I64 | Type::U8,
            ) => {
                let value = self.int_value(value, text, negative, ty)?;
                Some(tir::PatternKind::Range {
                    lo: value,
                    hi: value,
                })
            }
            (&ast::Literal::Char(c), Type::CHAR) => {
                let value = i64::from(u32::from(c));
                Some(tir::PatternKind::Range {
                    lo: value,
                    hi: value,
                })
            }
            (&ast::Literal::Bool(value), Type::BOOL) => Some(tir::PatternKind::Bool(value)),
            (ast::Literal::Str(text), Type::STRING) => Some(tir::PatternKind::Str(text.clone())),
            _ => {
                self.pattern_mismatch(ty, at);
                None
            }
        }
    }

    /// The integer `value`, its digits at `text` after the `-` at `negative`, at a position of
    /// type `ty`, `i64` or `u8`; `None` when `u8` cannot hold it, which is reported.
    fn int_value(
        &mut self,
        value: i64,
        text: Span,
        negative: Option<Span>,
        ty: Type,
    ) -> Option<i64> {
        if ty == Type::U8 {
            if let Some(minus) = negative {
                self.report(DiagnosticKind::NegatedU8 { at: minus.to(text) });
                return None;
            }
            if value > i64::from(u8::MAX) {
                self.report(DiagnosticKind::IntegerDoesNotFit {
                    text: self.source.file.text(text).to_string(),
                    at: text,
                });
                return None;
            }
        }
        Some(value)
    }

    /// The range `start..end`, or `start..=end` when `inclusive`, at `at`, at a position of
    /// type `ty`, which takes more than names: two literals of `ty`, which is `i64`, `u8` or
    /// `char`, with at least one value between them. It becomes the inclusive range.
    fn range_pattern(
        &mut self,
        at: Span,
        start: &ast::Bound,
        end: &ast::Bound,
        inclusive: bool,
        ty: Type,
    ) -> Option<tir::PatternKind> {
        for bound in [start, end] {
            if let ast::Bound::Name(name) = bound {
                // `n == LIMIT` would test one value, not the range, so the help would mislead.
                let kind = if self.constants.contains_key(&name.name) {
                    DiagnosticKind::ConstantInPattern {
                        name: name.name.to_string(),
                        at: name.span,
                        guard: None,
                    }
                } else {
                    DiagnosticKind::Expected {
                        what: "a literal".to_string(),
                        found: format!("`{}`", name.name),
                        at: name.span,
                    }
                };
                self.report(kind);
            }
        }
        let (ast::Bound::Literal(lo), ast::Bound::Literal(hi)) = (start, end) else {
            return None;
        };
        let text = |literal: &ast::Literal| {
            matches!(literal, ast::Literal::Str(_) | ast::Literal::Bool(_))
        };
        if !matches!(ty, Type::I64 | Type::U8 | Type::CHAR) || text(lo) || text(hi) {
            self.report(DiagnosticKind::BadRange {
                form: BadRange::Type,
                at,
            });
            return None;
        }
        let (lo, hi) = match (lo, hi, ty) {
            (
                &ast::Literal::Int {
                    value: lo,
                    text: lo_text,
                    negative: lo_negative,
                },
                &ast::Literal::Int {
                    value: hi,
                    text: hi_text,
                    negative: hi_negative,
                },
                Type::I64 | Type::U8,
            ) => {
                // Both bounds are checked, so each one that `u8` cannot hold is reported.
                let lo = self.int_value(lo, lo_text, lo_negative, ty);
                let hi = self.int_value(hi, hi_text, hi_negative, ty);
                (lo?, hi?)
            }
            (&ast::Literal::Char(lo), &ast::Literal::Char(hi), Type::CHAR) => {
                (i64::from(u32::from(lo)), i64::from(u32::from(hi)))
            }
            _ => {
                self.pattern_mismatch(ty, at);
                return None;
            }
        };
        if lo > hi || (lo == hi && !inclusive) {
            self.report(DiagnosticKind::BadRange {
                form: BadRange::Empty,
                at,
            });
            return None;
        }
        // Not empty, so `hi - 1` cannot overflow.
        let hi = match (inclusive, ty) {
            (true, _) => hi,
            // One below `'\u{E000}'` is a surrogate, which no `char` is.
            (false, Type::CHAR) if hi == 0xE000 => 0xD7FF,
            (false, _) => hi - 1,
        };
        Some(tir::PatternKind::Range { lo, hi })
    }

    /// "expected a pattern for `ty`", for the pattern at `at` as written.
    fn pattern_mismatch(&mut self, ty: Type, at: Span) {
        let want = self.type_name(ty);
        self.report(DiagnosticKind::PatternMismatch {
            want,
            found: self.source.file.text(at).to_string(),
            at,
        });
    }

    /// The alternatives of an `|` at a position of type `ty`, which takes more than names. Each
    /// binds the names of the first, with the same types, so each name is one local, which
    /// every alternative binds. A name of the first that an alternative lacks is reported there;
    /// a name only later alternatives bind is reported once, at the first.
    fn alternatives(
        &mut self,
        alternatives: &[ast::Pattern],
        ty: Type,
        keyword: &'static str,
        bound: &mut Bound<'_>,
    ) -> Option<tir::PatternKind> {
        let mark = bound.order.len();
        let first = self.pattern(&alternatives[0], Some(ty), keyword, bound);
        let named: Vec<_> = bound
            .order
            .drain(mark..)
            .map(|name| {
                let entry = bound.names.remove(&name);
                (name, entry.expect("a name in `order` should be in `names`"))
            })
            .collect();
        bound.alternatives.push(Alternatives {
            first: named
                .iter()
                .map(|&(name, (_, local))| (name, local))
                .collect(),
            extra: None,
        });
        let mut checked = vec![first];
        let mut agree = true;
        // The names only a later alternative binds, each reported once, at the first.
        let mut later_only = FxHashSet::default();
        for alternative in &alternatives[1..] {
            checked.push(self.pattern(alternative, Some(ty), keyword, bound));
            let here: FxHashSet<_> = bound.order.drain(mark..).collect();
            for name in &here {
                bound.names.remove(name);
            }
            let extra = bound
                .alternatives
                .last_mut()
                .and_then(|alternatives| alternatives.extra.take());
            let missing = named.iter().find(|(name, _)| !here.contains(name));
            let lacking = match (missing, extra) {
                (Some(&(name, _)), _) => Some((name, alternative.span)),
                (None, Some(name)) => {
                    agree = false;
                    later_only
                        .insert(name)
                        .then_some((name, alternatives[0].span))
                }
                (None, None) => None,
            };
            if let Some((name, at)) = lacking {
                agree = false;
                self.report(DiagnosticKind::AlternativeBindings {
                    name: name.to_string(),
                    at,
                });
            }
        }
        bound.alternatives.pop();
        // From here on the `|` binds what its first alternative does.
        for (name, entry) in named {
            bound.names.insert(name, entry);
            bound.order.push(name);
        }
        let checked: Option<Vec<_>> = checked.into_iter().collect();
        if !agree {
            return None;
        }
        Some(tir::PatternKind::Or(checked?))
    }

    /// For a guard a help suggests in place of what the pattern `bound` checks cannot test: the
    /// first of `candidates` that no word of the `match` arm uses, a name to bind, and the arm's
    /// own guard as the suffix ` && <guard>`, empty without one. `None` outside a `match` arm.
    fn guard_help(
        &self,
        bound: &Bound<'_>,
        mut candidates: impl Iterator<Item = String>,
    ) -> Option<(String, String)> {
        let arm = bound.arm?;
        let source = self.source.file;
        let read = iter::once(arm.pattern.span)
            .chain(arm.guard.iter().map(|guard| guard.span))
            .chain([arm.body.span]);
        let words: FxHashSet<_> = read
            .flat_map(|span| {
                source
                    .text(span)
                    .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            })
            .collect();
        let fresh = candidates
            .find(|candidate| !words.contains(candidate.as_str()))
            .expect("some numbered name should be free");
        let own = match &arm.guard {
            None => String::new(),
            // `&&` binds tighter than `||`, which the guard's own text would then lose.
            Some(
                guard @ ast::Expr {
                    kind:
                        ast::ExprKind::Binary {
                            op: ast::BinaryOp::Or,
                            ..
                        },
                    ..
                },
            ) => format!(" && ({})", source.text(guard.span)),
            Some(guard) => format!(" && {}", source.text(guard.span)),
        };
        Some((fresh, own))
    }

    /// The variant that `qualifier.name(fields)` (`parens` is `None` without parentheses)
    /// names at a position of type `ty`, an enum or a type `==` compares, and its field types;
    /// `None` when it was reported.
    fn variant_pattern(
        &mut self,
        span: Span,
        ty: Type,
        qualifier: Option<&ast::Ident>,
        name: &ast::Ident,
        fields: &[ast::Pattern],
        parens: Option<Span>,
    ) -> Option<(usize, Vec<Option<Type>>)> {
        let path = written(qualifier, name);
        // A qualifier names an enum, whatever arguments `ty` gives it, as an enum path does in
        // an expression; any other type it names is resolved for its own errors, and fits no
        // enum.
        let fits = match qualifier {
            None => true,
            Some(q) if let Some(&Declared::Known(named)) = self.enum_ids.get(&q.name) => {
                matches!(ty.kind(), TypeKind::Enum(id, _) if id == named)
            }
            Some(q) => {
                self.resolve(&ast::TypeExpr {
                    kind: ast::TypeKind::Name(q.name),
                    optional: 0,
                    span: q.span,
                })?;
                false
            }
        };
        let (id, args) = match ty.kind() {
            TypeKind::Enum(id, args) if fits => (id.0, args),
            _ => {
                let want = self.type_name(ty);
                self.report(DiagnosticKind::PatternMismatch {
                    want,
                    found: path,
                    at: span,
                });
                return None;
            }
        };
        let info = &self.enums[id];
        let Some(&variant) = info.index.get(&name.name) else {
            let variants = info
                .variants
                .iter()
                .map(|v| (v.name.name.as_str(), Definition::Variant));
            let kind = DiagnosticKind::UnknownVariant {
                enum_name: info.name.name.to_string(),
                name: name.name.to_string(),
                at: name.span,
                suggestion: suggestion(name.name.as_str(), variants),
            };
            self.report(kind);
            return None;
        };
        // Counted, not cloned: a wide variant's malformed patterns must not each copy its fields.
        let count = info.variants[variant].fields.len();
        let kind = match parens {
            None if count != 0 => DiagnosticKind::VariantForm {
                path,
                payload: true,
                held: false,
                at: name.span,
            },
            // The fix deletes the parentheses, so what they hold (even a comment) makes it unsafe.
            Some(parens) if count == 0 => DiagnosticKind::VariantForm {
                path,
                payload: false,
                held: !fields.is_empty() || self.comment_in(&[parens.range()]),
                at: parens,
            },
            _ if fields.len() != count => DiagnosticKind::SubpatternCount {
                variant: name.name.to_string(),
                fields: count,
                found: fields.len(),
                at: span,
            },
            _ => {
                let fields = &info.variants[variant].fields;
                let fields = fields.iter().map(|ty| ty.map(|ty| ty.subst(args.get())));
                return Some((variant, fields.collect()));
            }
        };
        self.report(kind);
        None
    }

    /// "a `ty` position takes only a name or `_`", at `at`.
    fn only_names(&mut self, ty: Type, at: Span) {
        let ty = self.type_name(ty);
        self.report(DiagnosticKind::OnlyNamePosition { ty, at });
    }

    /// Declares `name`, of type `ty`, as `keyword` binds it, unless this pattern binds it
    /// already, which is reported. In a later alternative of an `|`, it is the first
    /// alternative's local, which must have the same type; a name the first lacks is recorded,
    /// and declared as `Error`.
    fn bind(
        &mut self,
        name: &ast::Ident,
        ty: Option<Type>,
        keyword: &'static str,
        bound: &mut Bound<'_>,
    ) -> Option<tir::LocalId> {
        if let Some(&(first, _)) = bound.names.get(&name.name) {
            let written = name.name.as_str();
            let next = match written.as_bytes() {
                &[c @ b'a'..=b'y'] => Some(char::from(c + 1).to_string()),
                _ => None,
            };
            let candidates = next
                .into_iter()
                .chain((2..).map(|n: u32| format!("{written}{n}")));
            // Renamed in one alternative of an `|`, the name would be missing from the others.
            let guard = if bound.arm.is_some_and(|arm| has_or(&arm.pattern)) {
                Some(GuardHelp::InWords)
            } else {
                self.guard_help(bound, candidates).map(|(fresh, own)| {
                    let pattern = self.source.file.text(bound.top);
                    let start = name.span.start() - bound.top.start();
                    let end = name.span.end() - bound.top.start();
                    GuardHelp::Rewritten(format!(
                        "{}{fresh}{} if {written} == {fresh}{own}",
                        &pattern[..start],
                        &pattern[end..]
                    ))
                })
            };
            self.report(DiagnosticKind::BoundTwice {
                name: name.name.to_string(),
                at: name.span,
                first,
                guard,
            });
            return None;
        }
        let kind = super::BindingKind::Unwrapped(keyword);
        // A later alternative binds the first's local; its type must agree, but the name is
        // bound either way, so no alternative is reported as lacking it.
        let mut agrees = true;
        let local = if let Some(alternatives) = bound.alternatives.last_mut() {
            if let Some(&local) = alternatives.first.get(&name.name) {
                let earlier = self.lookup(name.name).map(|b| {
                    b.also.push(name.span);
                    (b.ty, b.span)
                });
                if let (Some((Some(want), earlier)), Some(got)) = (earlier, ty)
                    && want != got
                {
                    let (want, got) = (self.type_name(want), self.type_name(got));
                    self.report(DiagnosticKind::Mismatch {
                        want: vec![want],
                        got,
                        at: name.span,
                        source: Some(MismatchSource::Binding {
                            name: name.name.to_string(),
                            at: earlier,
                        }),
                        fix: None,
                        conversion: false,
                        layers: false,
                    });
                    agrees = false;
                }
                local
            } else {
                alternatives.extra.get_or_insert(name.name);
                self.declare(name, None, kind, None)
            }
        } else {
            let local = self.declare(name, ty, kind, None);
            bound.locals.push(local);
            local
        };
        bound.names.insert(name.name, (name.span, local));
        bound.order.push(name.name);
        agrees.then_some(local)
    }

    /// Declares the names in `p`, a pattern that failed, as `Error`, so that uses of them
    /// report nothing more. An upper-case name is taken for a variant and binds nothing.
    fn poison(&mut self, p: &ast::Pattern, keyword: &'static str, bound: &mut Bound<'_>) {
        match &p.kind {
            ast::PatternKind::Name(name)
                if !name.name.as_str().starts_with(char::is_uppercase)
                    && !bound.names.contains_key(&name.name) =>
            {
                self.bind(name, None, keyword, bound);
            }
            ast::PatternKind::Variant { fields, .. } => {
                for field in fields {
                    self.poison(field, keyword, bound);
                }
            }
            ast::PatternKind::Or(alternatives) => {
                for alternative in alternatives {
                    self.poison(alternative, keyword, bound);
                }
            }
            _ => {}
        }
    }

    /// `if let pattern = value`'s lowering on an enum, given its checked parts: a `match` of
    /// `pattern => then` and `_ => else`, or of `_ => {}` when there is no `else`.
    pub(super) fn if_let_match(
        &mut self,
        scrutinee: tir::Expr,
        (pattern, bindings): (tir::Pattern, Vec<tir::LocalId>),
        then_block: tir::Block,
        else_block: Option<tir::Block>,
    ) -> tir::ExprKind {
        let span = pattern.span;
        let other = tir::Pattern {
            kind: tir::PatternKind::Wild,
            span,
        };
        tir::ExprKind::Match {
            temp: self.fresh(scrutinee.ty),
            scrutinee: Box::new(scrutinee),
            arms: vec![
                tir::Arm {
                    pattern,
                    bindings,
                    guard: None,
                    body: then_block,
                    reachable: true,
                },
                tir::Arm {
                    pattern: other,
                    bindings: Vec::new(),
                    guard: None,
                    body: else_block.unwrap_or_else(empty_block),
                    reachable: true,
                },
            ],
        }
    }
}

/// Whether `pattern` holds an `|`.
fn has_or(pattern: &ast::Pattern) -> bool {
    match &pattern.kind {
        ast::PatternKind::Or(_) => true,
        ast::PatternKind::Variant { fields, .. } => fields.iter().any(has_or),
        _ => false,
    }
}
