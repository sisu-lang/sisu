//! Patterns, `match`, and the patterns of `if let` and `while let`.

use std::collections::hash_map::Entry;
use std::iter;

use rustc_hash::{FxHashMap, FxHashSet};

use super::alias::Via;
use super::branch::{Branch, failed_join};
use super::exhaust::{self, Neighbours, Report, Row};
use super::generic::{Ahead, type_meets};
use super::names::{self, Position, Resolved};
use super::reports::Reported;
use super::stmt::BlockSoFar;
use super::{Checked, CheckedBlock, Checker, Declared, Expectation, Origin, Poisoned, type_of};
use crate::ast;
use crate::diagnostic::{
    BadRange, Definition, DiagnosticKind, Expected, GuardHelp, LowerName, MismatchOrigin, Plain,
    Unwrap, suggestion,
};
use crate::lexer::{self, TokenKind};
use crate::source::{Span, removal_end};
use crate::symbol::Symbol;
use crate::tir::{self, Type, TypeKind};

/// The span that the fix of W0006 removes for the unreached alternative `at`. It removes the `|`
/// before the alternative, unless a comment lies between them and another alternative follows: then
/// it removes the alternative and the `|` after it, up to a comment, so that the comment stays.
/// That cut works only when the `|` comes before the comment: `after_gap` is the text from the end
/// of `at` to the start of the alternative after it. Else the span runs to the next alternative,
/// or starts at the end of the alternative before. A span that holds a comment is one text: the
/// caller splits it at the comments (`Source::removal_parts`), where a kept comment is safe.
fn alternative_removal(
    comments: &[usize],
    at: Span,
    neighbours: Neighbours,
    after_gap: &str,
) -> Span {
    match (neighbours.before, neighbours.after) {
        (Some(before), after)
            if after.is_none() || removal_end(comments, before, at.start()) == at.start() =>
        {
            Span::new(before, at.end())
        }
        (before, Some(after)) => {
            let end = removal_end(comments, at.end(), after);
            if after_gap[..end - at.end()].contains('|') {
                Span::new(at.start(), end)
            } else {
                match before {
                    Some(before) => Span::new(before, at.end()),
                    None => Span::new(at.start(), after),
                }
            }
        }
        // An `|` joins two alternatives or more, so each one has a neighbour.
        (_, None) => at,
    }
}

/// Whether `at` is an alternative of the `|` that makes the pattern of `arm`, not one inside
/// brackets.
fn top_alternative(arm: &ast::Arm, at: Span) -> bool {
    matches!(
        &arm.pattern.kind,
        ast::PatternKind::Or(alternatives) if alternatives.iter().any(|alt| alt.span == at)
    )
}

/// The names one pattern binds, at `top`, as its check goes; `arm` is the `match` arm it
/// starts, if any.
struct Bound<'a> {
    top: Span,
    arm: Option<&'a ast::Arm>,
    /// The `mut` of a `let mut` whose one name this pattern binds: it is a variable, not a
    /// name `keyword` unwrapped.
    mutable: Option<Span>,
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

impl Bound<'_> {
    /// How the pattern's names are declared: as variables of the `let mut` that `mutable` holds,
    /// else as names that `keyword` unwrapped. The second is the keyword of a `let mut`.
    fn binding_kind(&self, keyword: &'static str) -> (super::BindingKind, Option<Span>) {
        match self.mutable {
            Some(_) => (super::BindingKind::Var, self.mutable),
            None => (super::BindingKind::Unwrapped(keyword), None),
        }
    }
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
    /// An enum tested against `pattern`, which binds `bindings`. When it failed, the token of
    /// its own error, or `None` at an `Error` position, whose token is the value's.
    Match {
        pattern: Result<(tir::Pattern, Vec<tir::LocalId>), Option<Reported>>,
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
            | TypeKind::Optional(_)
    )
}

/// Whether `name` is a variant of the enum inside `ty`, under any layers of optional.
fn variant_inside(enums: &[super::EnumInfo], ty: Type, name: Symbol) -> bool {
    match ty.kind() {
        TypeKind::Optional(payload) => variant_inside(enums, payload, name),
        TypeKind::Enum(id, _) => enums[id.0].index.contains_key(&name),
        _ => false,
    }
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

/// Whether `literal` is a value that `==` compares with a value of type `kind`: a literal of that
/// type, and for a `u8` one that is 0 through 255 with no `-`, as `-0` is no `u8` literal.
fn literal_fits(literal: &ast::Literal, kind: TypeKind) -> bool {
    match (literal, kind) {
        (ast::Literal::Int { .. }, TypeKind::I64)
        | (ast::Literal::Bool(_), TypeKind::Bool)
        | (ast::Literal::Str(_), TypeKind::String)
        | (ast::Literal::Char(_), TypeKind::Char) => true,
        (
            ast::Literal::Int {
                value, negative, ..
            },
            TypeKind::U8,
        ) => negative.is_none() && u8::try_from(*value).is_ok(),
        _ => false,
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
    /// The branch for each of `arms`, ranked when no type is `expected` (see `block_branch`). The
    /// patterns bind for the ranking in `form`.
    fn arm_branches<'a>(
        &mut self,
        arms: &'a [ast::Arm],
        form: &LetValue,
        expected: Expectation,
    ) -> Vec<Branch<'a>> {
        arms.iter()
            .map(|arm| {
                let bind = |c: &mut Self| Some(c.bind_pattern(None, form, &arm.pattern, Some(arm)));
                match &arm.body.kind {
                    ast::ExprKind::Block(block) => self.block_branch(block, expected, bind),
                    _ => self.expr_branch(arm, &arm.body, expected, bind),
                }
            })
            .collect()
    }

    /// `match scrutinee { arms }`, whose keyword is at `keyword` and `}` at `close`. Each arm is
    /// checked in a scope that holds its bindings, and the arms join as an `if`'s branches do
    /// (`join`). When every pattern checked, the arms are checked for coverage and reachability.
    #[expect(
        clippy::too_many_lines,
        reason = "the scrutinee, the arms, the coverage and the result read in order in one place"
    )]
    pub(super) fn match_expr(
        &mut self,
        span: Span,
        keyword: Span,
        close: Span,
        scrutinee: &ast::Expr,
        arms: &[ast::Arm],
        expected: Expectation,
    ) -> Checked {
        let (expected, siblings) = expected.split_hint();
        let checked = self.value_kept(scrutinee);
        let mut position = type_of(&checked);
        let cannot = position.take_if(|&mut ty| !matchable(ty)).map(|ty| {
            let ty = self.type_name(ty);
            self.error(DiagnosticKind::CannotMatch {
                ty,
                at: scrutinee.span,
            })
        });
        // The token of an `Error` position: the scrutinee's, or the error that it cannot match.
        let unmatched = checked.as_ref().err().map(Poisoned::reported).or(cannot);
        let mut patterns: Vec<_> = arms.iter().map(|_| None).collect();
        let mut guards: Vec<_> = arms.iter().map(|_| None).collect();
        let form = LetValue::Match { ty: position };
        let (blocks, ty) = self.ranked(|c| {
            let sources = c.arm_branches(arms, &form, expected);
            c.join(&sources, (expected, siblings), |c, i, expected| {
                let arm = &arms[i];
                let (bound, guard, so_far) = c.bound_branch(arm_key(arm), |c| {
                    c.bind_pattern(None, &form, &arm.pattern, Some(arm))
                });
                let LetPattern::Match { pattern } = bound else {
                    panic!(
                        "a `match` arm should test its value, which `bind_pattern` never unwraps"
                    );
                };
                patterns[i] = Some(pattern);
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
            })
        });
        // A scrutinee `match` cannot take fails the `match`, as a failed condition fails an `if`.
        // With no arms, only a scrutinee of a known type makes it `never`.
        let ty = match (ty, cannot, unmatched) {
            (Ok(Type::NEVER), Some(cannot), _) if !arms.is_empty() => {
                Err(Poisoned::typed(Type::NEVER, cannot))
            }
            (Ok(_), Some(cannot), _) => Err(Poisoned::untyped(cannot)),
            (Ok(_), None, Some(scrutinee)) if arms.is_empty() && position.is_none() => {
                Err(Poisoned::untyped(scrutinee))
            }
            (ty, ..) => ty,
        };
        let mut reachable = vec![true; arms.len()];
        if let Some(ty) = position {
            let rows: Option<Vec<_>> = patterns
                .iter()
                .zip(arms)
                .map(|(p, arm)| {
                    let (pattern, _) = p.as_ref()?.as_ref().ok()?;
                    Some(Row {
                        pattern,
                        guarded: arm.guard.is_some(),
                    })
                })
                .collect();
            if let Some(rows) = rows {
                let report = exhaust::check(&self.lowered_enums, &self.conformances, ty, &rows);
                self.report_coverage(keyword, close, arms, report, &mut reachable);
            }
        }
        let arms: Result<Vec<_>, Reported> = patterns
            .into_iter()
            .zip(guards)
            .zip(blocks)
            .zip(reachable)
            .map(|(((pattern, guard), body), reachable)| {
                let pattern = pattern.expect("`join` should bind the pattern of every arm");
                let (pattern, bindings) = pattern.map_err(|own| {
                    own.or(unmatched).expect(
                        "a pattern with no error of its own should be at an `Error` position",
                    )
                })?;
                Ok(tir::Arm {
                    pattern,
                    bindings,
                    guard: guard.transpose().map_err(|p| p.reported())?,
                    body: body.map_err(|p| p.reported())?,
                    reachable,
                })
            })
            .collect();
        match (checked, arms, ty) {
            (Ok(scrutinee), Ok(arms), Ok(ty)) => Ok(tir::Expr::new(
                tir::ExprKind::Match {
                    temp: self.fresh(scrutinee.ty),
                    scrutinee: Box::new(scrutinee),
                    arms,
                },
                ty,
                span,
            )),
            (checked, arms, ty) => failed_join(
                ty,
                checked.err().map(|p| p.reported()).or_else(|| arms.err()),
            ),
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
        let source = self.source.holding(close.start());
        // The offsets are absolute: the prelude's text starts at `PRELUDE_BASE`, not at 0.
        let base = source.start();
        let line_start = |at: usize| {
            source
                .slice(base..at)
                .rfind('\n')
                .map_or(base, |n| base + n + 1)
        };
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
            let whole_arm = || {
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
                    .map_or_else(|| source.end(), |n| end + n);
                if close.start() < line_end {
                    Span::new(line_start(start) - 1, end)
                } else {
                    Span::new(line_start(start), (line_end + 1).min(source.end()))
                }
            };
            let end = found.at.end();
            let remove = match found.alternative {
                Some(neighbours) => {
                    let span = alternative_removal(
                        self.source.comments,
                        found.at,
                        neighbours,
                        source.slice(end..neighbours.after.unwrap_or(end)),
                    );
                    // At the top of an arm, a kept comment would put its line break before the
                    // `=>`, which ends the pattern there. So the span goes whole.
                    if top_alternative(arm, found.at) {
                        vec![span]
                    } else {
                        self.source.removal_parts(span)
                    }
                }
                None => vec![whole_arm()],
            };
            self.warn(DiagnosticKind::UnreachableArm {
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
        self.error(DiagnosticKind::NonExhaustive {
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
    /// optional when `pattern` is `_` or a name that is no variant of the enum inside, else
    /// matches it. A value that is neither an enum nor optional is reported, as the `bool`
    /// says, and becomes `Error`; the pattern is then checked at an `Error` position, so its
    /// names are poisoned.
    pub(super) fn let_value(
        &mut self,
        keyword: Unwrap,
        pattern: &ast::Pattern,
        value: &ast::Expr,
    ) -> (Checked, LetValue, bool) {
        let checked = self.value(value, Expectation::None);
        let ty = type_of(&checked);
        let name = match &pattern.kind {
            // A variant of the enum inside matches that value, as in `match`.
            ast::PatternKind::Name(name)
                if ty.is_some_and(|ty| variant_inside(&self.enums, ty, name.name)) =>
            {
                None
            }
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
            (Some((_, TypeKind::Optional(_) | TypeKind::Enum(..))) | None, _) => {
                (checked, LetValue::Match { ty }, false)
            }
            (Some((ty, kind)), _) => {
                let compare = matches!(
                    &pattern.kind,
                    ast::PatternKind::Literal(literal) if literal_fits(literal, kind)
                );
                let found = self.type_name(ty);
                let reported = self.error(DiagnosticKind::UnwrapNeedsOptional {
                    keyword,
                    found,
                    at: value.span,
                    compare,
                });
                (
                    Err(Poisoned::untyped(reported)),
                    LetValue::Match { ty: None },
                    true,
                )
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
        expected: Expectation,
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
        self.bind_pattern_as(keyword, None, None, value, pattern, arm)
    }

    /// `bind_pattern`, where `mutable`, the `mut` of a `let ... else`, makes the one name a
    /// `let mut` variable. `else_removal` is what E0131 removes from a `let ... else`: the span
    /// of its `else` block and whether the removal is safe.
    pub(super) fn bind_pattern_as(
        &mut self,
        keyword: Option<Unwrap>,
        mutable: Option<Span>,
        else_removal: Option<(Span, bool)>,
        value: &LetValue,
        pattern: &ast::Pattern,
        arm: Option<&ast::Arm>,
    ) -> (LetPattern, Option<Checked>) {
        let word = keyword.map_or("match", Unwrap::word);
        let bound = match value {
            LetValue::Unwrap { name, bound } => LetPattern::Some {
                bind: match mutable {
                    Some(_) => self.declare(name, *bound, super::BindingKind::Var, mutable),
                    None => self.bind_unwrapped(word, name, *bound),
                },
            },
            &LetValue::Match { ty } => {
                let pattern = self.pattern_in_scope(pattern, ty, word, arm, mutable);
                // A `let` with no `else` reports its own error, which needs the same test.
                if let (Some(keyword), Some(ty), Ok((pattern, _))) = (keyword, ty, &pattern)
                    && !matches!(keyword, Unwrap::Let)
                    && self.always_matches(ty, pattern)
                {
                    // A name or `_` takes nothing apart, so a plain `let` does the same. A bare
                    // variant is a name in the `ast` and a variant here.
                    let plain = match (&pattern.kind, else_removal) {
                        (
                            tir::PatternKind::Bind(_) | tir::PatternKind::Wild,
                            Some((span, safe)),
                        ) => Plain::RemoveElse { span, safe },
                        (tir::PatternKind::Bind(_) | tir::PatternKind::Wild, None) => Plain::Let,
                        _ => Plain::No,
                    };
                    self.error(DiagnosticKind::AlwaysMatches {
                        keyword,
                        at: pattern.span,
                        plain,
                    });
                }
                LetPattern::Match { pattern }
            }
        };
        let guard = arm
            .and_then(|arm| arm.guard.as_ref())
            .map(|guard| self.expect(guard, Expectation::Required(Type::BOOL, Origin::None)));
        (bound, guard)
    }

    /// Whether `pattern`, checked at a position of the enum or optional type `ty`, matches every
    /// value of it. `_`, or a name that is no variant inside, unwraps an optional instead, never
    /// here.
    pub(super) fn always_matches(&self, ty: Type, pattern: &tir::Pattern) -> bool {
        let rows = [Row {
            pattern,
            guarded: false,
        }];
        exhaust::covers(&self.lowered_enums, &self.conformances, ty, &rows)
    }

    /// `pattern` at a position of type `ty` (`None` is `Error`), its names declared in the
    /// innermost scope as `keyword` binds them; `arm` is the `match` arm it starts, if any.
    /// When it failed, the token of its own error, or `None` at an `Error` position.
    fn pattern_in_scope(
        &mut self,
        pattern: &ast::Pattern,
        ty: Option<Type>,
        keyword: &'static str,
        arm: Option<&ast::Arm>,
        mutable: Option<Span>,
    ) -> Result<(tir::Pattern, Vec<tir::LocalId>), Option<Reported>> {
        let mut bound = Bound {
            top: pattern.span,
            arm,
            mutable,
            names: FxHashMap::default(),
            order: Vec::new(),
            locals: Vec::new(),
            alternatives: Vec::new(),
        };
        // An `Error` position matches anything; its own error is reported.
        let Some(ty) = ty else {
            self.poison(pattern, keyword, &mut bound);
            return Err(None);
        };
        let pattern = self.pattern(pattern, ty, keyword, &mut bound)?;
        Ok((pattern, bound.locals))
    }

    /// `p` at a position of type `ty`; see `pattern_in_scope`. A failed pattern still declares
    /// its names, poisoned, so the arm's body does not report them.
    fn pattern(
        &mut self,
        p: &ast::Pattern,
        ty: Type,
        keyword: &'static str,
        bound: &mut Bound<'_>,
    ) -> Result<tir::Pattern, Reported> {
        let pattern = |kind| tir::Pattern { kind, span: p.span };
        if let TypeKind::Optional(payload) = ty.kind() {
            return self.optional_pattern(p, ty, payload, keyword, bound);
        }
        let names_only = !matchable(ty);
        match &p.kind {
            ast::PatternKind::None
            | ast::PatternKind::Literal(_)
            | ast::PatternKind::Range { .. }
                if names_only =>
            {
                Err(self.only_names(ty, p.span))
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
                let reported = self.only_names(ty, p.span);
                for alternative in alternatives {
                    self.poison(alternative, keyword, bound);
                }
                Err(reported)
            }
            ast::PatternKind::Or(alternatives) => self
                .alternatives(alternatives, ty, keyword, bound)
                .map(pattern),
            ast::PatternKind::Wild => Ok(pattern(tir::PatternKind::Wild)),
            ast::PatternKind::Name(name) => {
                self.name_pattern(name, ty, keyword, bound).map(pattern)
            }
            ast::PatternKind::None => {
                let want = self.type_name(ty);
                Err(self.error(DiagnosticKind::PatternMismatch {
                    want,
                    found: "None".to_string(),
                    at: p.span,
                }))
            }
            ast::PatternKind::Variant {
                qualifier,
                name,
                fields,
                parens,
            } => {
                let kind = if names_only {
                    // A poisoned name, or an alias that is `Error`, gives no error beside the
                    // first one.
                    let poison = match qualifier {
                        None => self.use_poison(name.name, name.span),
                        Some(q) => matches!(self.through(q.name), Via::Error)
                            .then(|| self.failed_alias_token(q.span)),
                    };
                    Err(poison.unwrap_or_else(|| self.only_names(ty, p.span)))
                } else {
                    self.variant_pattern(p.span, ty, qualifier.as_ref(), name, fields, *parens)
                };
                let (variant, types) = match kind {
                    Ok(kind) => kind,
                    Err(reported) => {
                        for field in fields {
                            self.poison(field, keyword, bound);
                        }
                        return Err(reported);
                    }
                };
                // Every field is checked, so the names after a failed one are still bound.
                let fields: Vec<_> = fields
                    .iter()
                    .zip(&types)
                    .map(|(field, &ty)| {
                        if let Some(ty) = ty {
                            self.pattern(field, ty, keyword, bound)
                        } else {
                            // An `Error` field matches anything; its declaration's error is
                            // reported.
                            self.poison(field, keyword, bound);
                            Err(self.proof(field.span)) // proof: declaration
                        }
                    })
                    .collect();
                Ok(pattern(tir::PatternKind::Variant {
                    variant,
                    fields: fields.into_iter().collect::<Result<_, _>>()?,
                }))
            }
        }
    }

    /// `p` at a position of the optional type `ty`, whose payload is `payload`. `Some(p)` and
    /// `None` match the optional itself, a variant, literal or range pattern matches the value
    /// inside, each alternative of an `|` takes this rule, and a name or `_` covers the whole
    /// optional. See `pattern`.
    #[expect(
        clippy::too_many_lines,
        reason = "one match over the pattern kinds, each arm short"
    )]
    fn optional_pattern(
        &mut self,
        p: &ast::Pattern,
        ty: Type,
        payload: Type,
        keyword: &'static str,
        bound: &mut Bound<'_>,
    ) -> Result<tir::Pattern, Reported> {
        let pattern = |kind| tir::Pattern { kind, span: p.span };
        let some = |inner: Result<_, _>| Ok(pattern(tir::PatternKind::Some(Box::new(inner?))));
        match &p.kind {
            ast::PatternKind::Wild => Ok(pattern(tir::PatternKind::Wild)),
            ast::PatternKind::None => Ok(pattern(tir::PatternKind::None)),
            ast::PatternKind::Name(name) if variant_inside(&self.enums, payload, name.name) => {
                some(self.pattern(p, payload, keyword, bound))
            }
            ast::PatternKind::Name(name) if name.name == Symbol::SOME => {
                Err(self.error(DiagnosticKind::VariantForm {
                    path: "Some".to_string(),
                    payload: true,
                    held: false,
                    at: name.span,
                }))
            }
            ast::PatternKind::Name(name) => {
                self.name_pattern(name, ty, keyword, bound).map(pattern)
            }
            ast::PatternKind::Or(alternatives) => self
                .alternatives(alternatives, ty, keyword, bound)
                .map(pattern),
            ast::PatternKind::Variant {
                qualifier,
                name,
                fields,
                parens,
            } if name.name == Symbol::SOME
                && qualifier
                    .as_ref()
                    .is_none_or(|q| self.names_option_at(q.name, ty)) =>
            {
                let kind = match (parens, &fields[..]) {
                    (Some(_), [field]) => {
                        return some(self.pattern(field, payload, keyword, bound));
                    }
                    (None, _) => DiagnosticKind::VariantForm {
                        path: written(qualifier.as_ref(), name),
                        payload: true,
                        held: false,
                        at: name.span,
                    },
                    (Some(_), _) => DiagnosticKind::SubpatternCount {
                        variant: name.name.to_string(),
                        fields: 1,
                        found: fields.len(),
                        at: p.span,
                    },
                };
                let reported = self.error(kind);
                self.poison(p, keyword, bound);
                Err(reported)
            }
            ast::PatternKind::Variant {
                qualifier: Some(q),
                name,
                fields,
                parens,
            } if name.name == Symbol::NONE && self.names_option_at(q.name, ty) => {
                let Some(parens) = parens else {
                    return Ok(pattern(tir::PatternKind::None));
                };
                let reported = self.error(DiagnosticKind::VariantForm {
                    path: written(Some(q), name),
                    payload: false,
                    held: !fields.is_empty() || self.comment_in(&[parens.range()]),
                    at: *parens,
                });
                self.poison(p, keyword, bound);
                Err(reported)
            }
            ast::PatternKind::Variant {
                qualifier: Some(q),
                name,
                ..
            } if self.names_option_at(q.name, ty) => {
                let variants = [("None", Definition::Variant), ("Some", Definition::Variant)];
                let reported = self.error(DiagnosticKind::UnknownVariant {
                    enum_name: q.name.to_string(),
                    name: name.name.to_string(),
                    at: name.span,
                    suggestion: suggestion(name.name.as_str(), variants),
                });
                self.poison(p, keyword, bound);
                Err(reported)
            }
            // An alias of an optional type that is not this one.
            ast::PatternKind::Variant {
                qualifier: Some(q),
                name,
                ..
            } if self.names_option(q.name) => {
                let want = self.type_name(ty);
                let reported = self.error(DiagnosticKind::PatternMismatch {
                    want,
                    found: written(Some(q), name),
                    at: p.span,
                });
                self.poison(p, keyword, bound);
                Err(reported)
            }
            ast::PatternKind::Variant { .. }
            | ast::PatternKind::Literal(_)
            | ast::PatternKind::Range { .. } => some(self.pattern(p, payload, keyword, bound)),
        }
    }

    /// The name pattern `name` at a position of type `ty`: a variant of `ty`'s enum, else a
    /// binding, unless it is a constant or an upper-case name, which `ty` has no variant of. Such
    /// a name that is poisoned gives no error and hides any outer local of that name.
    fn name_pattern(
        &mut self,
        name: &ast::Ident,
        ty: Type,
        keyword: &'static str,
        bound: &mut Bound<'_>,
    ) -> Result<tir::PatternKind, Reported> {
        let text = name.name.as_str();
        let enum_id = match ty.kind() {
            TypeKind::Enum(id, _) => Some(id.0),
            _ => None,
        };
        if let Some(id) = enum_id
            && let Some(&variant) = self.enums[id].index.get(&name.name)
        {
            if self.enums[id].variants[variant].fields.is_empty() {
                return Ok(tir::PatternKind::Variant {
                    variant,
                    fields: Vec::new(),
                });
            }
            return Err(self.error(DiagnosticKind::VariantForm {
                path: text.to_string(),
                payload: true,
                held: false,
                at: name.span,
            }));
        }
        let answer = self.meaning(name.name, names::Site::new(Position::Pattern, false));
        // A name the prelude takes is a binding, which `declare` reports.
        if !matches!(answer, Resolved::PreludeVariant) {
            // A poisoned name gives no error.
            if let Resolved::Poisoned(_, reported) = answer {
                self.hide_local(name);
                return Err(self.poison_token(reported, name.span));
            }
            if matches!(answer, Resolved::Constant(_)) {
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
                return Err(self.error(DiagnosticKind::ConstantInPattern {
                    name: text.to_string(),
                    at: name.span,
                    guard,
                }));
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
                    let (kind, mutable) = bound.binding_kind(keyword);
                    if let Entry::Vacant(entry) = bound.names.entry(name.name) {
                        let local = self.insert_local(name, None, kind, mutable);
                        entry.insert((name.span, local));
                        bound.order.push(name.name);
                        bound.locals.push(local);
                    }
                    // A name that is not a variant here starts with an upper-case letter.
                    let mut lower = text.to_string();
                    if let Some(first) = text.chars().next() {
                        lower.replace_range(..first.len_utf8(), &first.to_lowercase().to_string());
                    }
                    // Another keyword is no binding, so the fix would not parse. `true` and `false`
                    // are patterns: `True` on a `bool` is likely one of them.
                    let lower = match lexer::keyword(&lower) {
                        None | Some(TokenKind::True | TokenKind::False) => {
                            LowerName::Binding(lower)
                        }
                        Some(TokenKind::Reserved(_)) => LowerName::Reserved(lower),
                        Some(_) => LowerName::Keyword(lower),
                    };
                    DiagnosticKind::NotVariantHere {
                        name: text.to_string(),
                        lower,
                        at: name.span,
                    }
                };
                return Err(self.error(kind));
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
    ) -> Result<tir::PatternKind, Reported> {
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
                Ok(tir::PatternKind::Range {
                    lo: value,
                    hi: value,
                })
            }
            (&ast::Literal::Char(c), Type::CHAR) => {
                let value = i64::from(u32::from(c));
                Ok(tir::PatternKind::Range {
                    lo: value,
                    hi: value,
                })
            }
            (&ast::Literal::Bool(value), Type::BOOL) => Ok(tir::PatternKind::Bool(value)),
            (ast::Literal::Str(text), Type::STRING) => Ok(tir::PatternKind::Str(text.clone())),
            _ => Err(self.pattern_mismatch(ty, at)),
        }
    }

    /// The integer `value`, its digits at `text` after the `-` at `negative`, at a position of
    /// type `ty`, `i64` or `u8`; the token of the error when `u8` cannot hold it.
    fn int_value(
        &mut self,
        value: i64,
        text: Span,
        negative: Option<Span>,
        ty: Type,
    ) -> Result<i64, Reported> {
        if ty == Type::U8 {
            if let Some(minus) = negative {
                return Err(self.error(DiagnosticKind::NegatedU8 { at: minus.to(text) }));
            }
            if value > i64::from(u8::MAX) {
                return Err(self.error(DiagnosticKind::IntegerDoesNotFit {
                    text: self.source.text(text).to_string(),
                    at: text,
                }));
            }
        }
        Ok(value)
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
    ) -> Result<tir::PatternKind, Reported> {
        let mut named = None;
        for bound in [start, end] {
            let ast::Bound::Name(name) = bound else {
                continue;
            };
            if let Some(reported) = self.use_poison(name.name, name.span) {
                named.get_or_insert(reported);
            } else {
                // `n == LIMIT` would test one value, not the range, so the help would mislead.
                let kind = if self.names.constant(name.name).is_some() {
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
                let reported = self.error(kind);
                named.get_or_insert(reported);
            }
        }
        let (ast::Bound::Literal(lo), ast::Bound::Literal(hi)) = (start, end) else {
            return Err(named.expect("a bound that is a name should be reported or poisoned"));
        };
        let text = |literal: &ast::Literal| {
            matches!(literal, ast::Literal::Str(_) | ast::Literal::Bool(_))
        };
        if !matches!(ty, Type::I64 | Type::U8 | Type::CHAR) || text(lo) || text(hi) {
            return Err(self.error(DiagnosticKind::BadRange {
                form: BadRange::Type,
                at,
            }));
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
            _ => return Err(self.pattern_mismatch(ty, at)),
        };
        if lo > hi || (lo == hi && !inclusive) {
            return Err(self.error(DiagnosticKind::BadRange {
                form: BadRange::Empty,
                at,
            }));
        }
        // Not empty, so `hi - 1` cannot overflow.
        let hi = match (inclusive, ty) {
            (true, _) => hi,
            // One below `'\u{E000}'` is a surrogate, which no `char` is.
            (false, Type::CHAR) if hi == 0xE000 => 0xD7FF,
            (false, _) => hi - 1,
        };
        Ok(tir::PatternKind::Range { lo, hi })
    }

    /// "expected a pattern for `ty`", for the pattern at `at` as written.
    fn pattern_mismatch(&mut self, ty: Type, at: Span) -> Reported {
        let want = self.type_name(ty);
        self.error(DiagnosticKind::PatternMismatch {
            want,
            found: self.source.text(at).to_string(),
            at,
        })
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
    ) -> Result<tir::PatternKind, Reported> {
        let mark = bound.order.len();
        let first = self.pattern(&alternatives[0], ty, keyword, bound);
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
        // The token of the first name that the alternatives do not agree on.
        let mut lacks = None;
        // The names only a later alternative binds, each reported once, at the first.
        let mut later_only = FxHashSet::default();
        for alternative in &alternatives[1..] {
            checked.push(self.pattern(alternative, ty, keyword, bound));
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
                // A name reported at the first already gave its token.
                (None, Some(name)) => later_only
                    .insert(name)
                    .then_some((name, alternatives[0].span)),
                (None, None) => None,
            };
            if let Some((name, at)) = lacking {
                let reported = self.error(DiagnosticKind::AlternativeBindings {
                    name: name.to_string(),
                    at,
                });
                lacks.get_or_insert(reported);
            }
        }
        bound.alternatives.pop();
        // From here on the `|` binds what its first alternative does.
        for (name, entry) in named {
            bound.names.insert(name, entry);
            bound.order.push(name);
        }
        let checked: Result<Vec<_>, _> = checked.into_iter().collect();
        if let Some(reported) = lacks {
            return Err(reported);
        }
        Ok(tir::PatternKind::Or(checked?))
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
        let source = self.source;
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
    /// the token of its error when it was reported, or of the poison when the name is poisoned.
    fn variant_pattern(
        &mut self,
        span: Span,
        ty: Type,
        qualifier: Option<&ast::Ident>,
        name: &ast::Ident,
        fields: &[ast::Pattern],
        parens: Option<Span>,
    ) -> Result<(usize, Vec<Option<Type>>), Reported> {
        // A poisoned name gives no error, unless `ty` has a variant of that name.
        if qualifier.is_none()
            && !variant_inside(&self.enums, ty, name.name)
            && let Some(reported) = self.use_poison(name.name, name.span)
        {
            return Err(reported);
        }
        let path = written(qualifier, name);
        // A qualifier names an enum, whatever arguments `ty` gives it, as an enum path does in
        // an expression; any other type it names is resolved for its own errors, and fits no
        // enum.
        let fits = match qualifier {
            None => true,
            Some(q) if let Some(Declared::Known(named)) = self.names.enum_id(q.name) => {
                matches!(ty.kind(), TypeKind::Enum(id, _) if id == named)
            }
            Some(q) => match self.through(q.name) {
                // An alias of an enum is the type that it names, whose fixed arguments hold.
                Via::To(through) if matches!(through.ty.kind(), TypeKind::Enum(..)) => {
                    type_meets(through.params.len(), through.ty, ty)
                }
                Via::Error => return Err(self.failed_alias_token(q.span)),
                _ => {
                    let resolved = self.resolve(&ast::TypeExpr {
                        kind: ast::TypeKind::Name(q.name),
                        optional: 0,
                        span: q.span,
                        bare: q.span,
                    });
                    if resolved.is_none() {
                        // `resolve` reported the error of the written type, or its declaration
                        // did, but it gives no token.
                        return Err(self.proof(q.span)); // proof: declaration
                    }
                    false
                }
            },
        };
        let (id, args) = match ty.kind() {
            TypeKind::Enum(id, args) if fits => (id.0, args),
            _ => {
                let want = self.type_name(ty);
                return Err(self.error(DiagnosticKind::PatternMismatch {
                    want,
                    found: path,
                    at: span,
                }));
            }
        };
        let info = &self.enums[id];
        let Some(&variant) = info.index.get(&name.name) else {
            let variants = info
                .variants
                .iter()
                .map(|v| (v.name.name.as_str(), Definition::Variant));
            let kind = DiagnosticKind::UnknownVariant {
                // As written: an alias of the enum names itself.
                enum_name: qualifier.map_or(info.name.name, |q| q.name).to_string(),
                name: name.name.to_string(),
                at: name.span,
                suggestion: suggestion(name.name.as_str(), variants),
            };
            return Err(self.error(kind));
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
                let fields = fields
                    .iter()
                    .map(|ty| ty.and_then(|ty| self.subst(ty, args.get())));
                return Ok((variant, fields.collect()));
            }
        };
        Err(self.error(kind))
    }

    /// "a `ty` position takes only a name or `_`", at `at`.
    fn only_names(&mut self, ty: Type, at: Span) -> Reported {
        let ty = self.type_name(ty);
        self.error(DiagnosticKind::OnlyNamePosition { ty, at })
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
    ) -> Result<tir::LocalId, Reported> {
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
                    let pattern = self.source.text(bound.top);
                    let start = name.span.start() - bound.top.start();
                    let end = name.span.end() - bound.top.start();
                    GuardHelp::Rewritten(format!(
                        "{}{fresh}{} if {written} == {fresh}{own}",
                        &pattern[..start],
                        &pattern[end..]
                    ))
                })
            };
            return Err(self.error(DiagnosticKind::BoundTwice {
                name: name.name.to_string(),
                at: name.span,
                first,
                guard,
            }));
        }
        let (kind, mutable) = bound.binding_kind(keyword);
        // A later alternative binds the first's local; its type must agree, but the name is
        // bound either way, so no alternative is reported as lacking it.
        let mut misfit = None;
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
                    misfit = Some(self.error(DiagnosticKind::Mismatch {
                        want: Expected::Types(vec![want]),
                        got,
                        at: name.span,
                        origin: Some(MismatchOrigin::Alternative {
                            name: name.name.to_string(),
                            at: earlier,
                        }),
                        fix: None,
                        conversion: false,
                        layers: false,
                    }));
                }
                local
            } else {
                alternatives.extra.get_or_insert(name.name);
                self.declare(name, None, kind, mutable)
            }
        } else {
            let local = self.declare(name, ty, kind, mutable);
            bound.locals.push(local);
            local
        };
        bound.names.insert(name.name, (name.span, local));
        bound.order.push(name.name);
        misfit.map_or(Ok(local), Err)
    }

    /// Hides any outer local of the poisoned name `name` from the reads and writes in its scope.
    /// `bound` does not hold it, so an alternative of an `|` that lacks the name is good.
    fn hide_local(&mut self, name: &ast::Ident) {
        self.insert_local(name, None, super::BindingKind::Var, None);
        if let Some(b) = self.lookup(name.name) {
            b.hides_name = true;
        }
    }

    /// Declares the names in `p`, a pattern that failed, as `Error`, so that uses of them
    /// report nothing more. An upper-case name is taken for a variant and binds nothing. A
    /// poisoned one, or a poisoned constant, hides any outer local of that name.
    fn poison(&mut self, p: &ast::Pattern, keyword: &'static str, bound: &mut Bound<'_>) {
        match &p.kind {
            ast::PatternKind::Name(name)
                if (self.names.constant(name.name).is_some()
                    || name.name.as_str().starts_with(char::is_uppercase))
                    && self.poisoned_use(name.name) =>
            {
                self.hide_local(name);
            }
            ast::PatternKind::Name(name)
                if !name.name.as_str().starts_with(char::is_uppercase)
                    && !bound.names.contains_key(&name.name) =>
            {
                // The pattern's own failure is the error: a name with no type binds as `Error`.
                let _ = self.bind(name, None, keyword, bound);
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
