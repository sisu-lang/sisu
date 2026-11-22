//! The join-order check: the members of a join, in the other order. A join must not depend on the
//! order of its members. So a program with no error must still have no error when they swap. The
//! pairs come from the syntax tree of the program, which the clean check already passed.

use std::ops::Range;

use crate::ast::{
    Arm, Block, Expr, ExprKind, ForSource, Function, Ident, Item, Member, Pattern, Program,
    Range as Rng, Stmt, StmtKind,
};
use crate::diagnostic::Severity;
use crate::source::Span;
use crate::{lexer, parser};

/// `program` with the two blocks of each `if` that has an `else` block swapped. It also swaps the
/// first and last elements of each array literal of two or more elements. None when it has no such
/// join, or does not parse. An `if let` keeps its order: its `else` block cannot see the binding.
/// An `else if` swaps only the blocks of its inner `if`.
pub(crate) fn swap_joins(program: &str) -> Option<String> {
    let lexed = lexer::lex(program).ok()?;
    let parsed = parser::parse(lexed, false);
    // A warning, such as the W0007 that the generator writes, does not count.
    let no_error = (parsed.diagnostics.iter()).all(|d| d.severity() != Severity::Error);
    if !no_error || !parsed.checkable {
        return None;
    }
    let mut finder = Finder {
        source: program,
        pairs: Vec::new(),
    };
    finder.program(&parsed.program);
    let pairs = compatible(finder.pairs);
    if pairs.is_empty() {
        return None;
    }
    Some(render(program, 0..program.len(), &pairs, &mut 0))
}

/// Two parts of the program that trade places.
struct Pair {
    first: Range<usize>,
    second: Range<usize>,
}

impl Pair {
    /// The three texts that the pair splits its range into.
    fn slots(&self) -> [Range<usize>; 3] {
        [
            self.first.clone(),
            self.first.end..self.second.start,
            self.second.clone(),
        ]
    }

    /// Whether the whole of `inner` lies in one part of `self`.
    fn holds(&self, inner: &Pair) -> bool {
        let hull = inner.first.start..inner.second.end;
        (self.slots().iter()).any(|slot| slot.start <= hull.start && hull.end <= slot.end)
    }

    /// Whether `render` can swap both: each pair holds the other or the two do not overlap.
    fn fits(&self, other: &Pair) -> bool {
        self.second.end <= other.first.start
            || other.second.end <= self.first.start
            || self.holds(other)
            || other.holds(self)
    }
}

/// The pairs of `pairs` that `render` can swap together, an enclosing pair before the pairs inside
/// it. A pair that overlaps one before it is left out. The `break` values of a loop overlap
/// when one sits in an `if` block that swaps.
fn compatible(mut pairs: Vec<Pair>) -> Vec<Pair> {
    pairs.sort_by_key(|p| (p.first.start, std::cmp::Reverse(p.second.end)));
    let mut kept: Vec<Pair> = Vec::new();
    for pair in pairs {
        if kept.iter().all(|k| k.fits(&pair)) {
            kept.push(pair);
        }
    }
    kept
}

/// Finds the pairs of one program.
struct Finder<'a> {
    source: &'a str,
    pairs: Vec<Pair>,
}

impl<'a> Finder<'a> {
    fn program(&mut self, program: &'a Program) {
        for item in &program.items {
            match item {
                Item::Function(f) => self.block(&f.body),
                Item::Class(c) => self.members(&c.members),
                Item::Struct(s) => self.members(&s.members),
                Item::Enum(e) => self.methods(&e.methods),
                Item::Extend(e) => self.methods(&e.methods),
                Item::Const(c) => self.expr(&c.value),
                Item::Alias(_) | Item::Interface(_) => {}
            }
        }
    }

    fn methods(&mut self, methods: &'a [Function]) {
        for f in methods {
            self.block(&f.body);
        }
    }

    fn members(&mut self, members: &'a [Member]) {
        for member in members {
            if let Member::Method(f) = member {
                self.block(&f.body);
            }
        }
    }

    fn add(&mut self, first: Span, second: Span) {
        self.pairs.push(Pair {
            first: first.range(),
            second: second.range(),
        });
    }

    /// The pair that `e` makes, when it is a join with a swap form.
    fn join(&mut self, e: &'a Expr) {
        match &e.kind {
            // `else if` has an `else` block that holds the inner `if`, and no `{` of its own.
            ExprKind::If {
                then_block,
                else_block: Some(else_block),
                ..
            } if self.source[else_block.span.range()].starts_with('{') => {
                self.add(then_block.span, else_block.span);
            }
            ExprKind::ArrayLit(items) if items.len() >= 2 => {
                if let (Some(first), Some(last)) = (items.first(), items.last()) {
                    self.add(first.span, last.span);
                }
            }
            _ => {}
        }
    }
}

impl<'a> Visit<'a> for Finder<'a> {
    fn expr(&mut self, e: &'a Expr) {
        self.join(e);
        walk_expr(self, e);
    }
}

/// A walk over the syntax tree. A method does what the default does, and a visitor calls
/// `walk_expr` or `walk_stmt` to go on.
trait Visit<'a> {
    fn expr(&mut self, e: &'a Expr) {
        walk_expr(self, e);
    }

    fn stmt(&mut self, s: &'a Stmt) {
        walk_stmt(self, s);
    }

    /// A name that a `let` or a `for` binds.
    fn binding(&mut self, _name: &'a Ident) {}

    /// A pattern, whole.
    fn pattern(&mut self, _pattern: &'a Pattern) {}

    fn exprs(&mut self, es: impl IntoIterator<Item = &'a Expr>) {
        for e in es {
            self.expr(e);
        }
    }

    fn blocks(&mut self, bs: impl IntoIterator<Item = &'a Block>) {
        for b in bs {
            self.block(b);
        }
    }

    fn block(&mut self, b: &'a Block) {
        for s in &b.stmts {
            self.stmt(s);
        }
    }
}

fn walk_stmt<'a, V: Visit<'a> + ?Sized>(v: &mut V, s: &'a Stmt) {
    match &s.kind {
        StmtKind::Let { name, init, .. } => {
            v.binding(name);
            v.expr(init);
        }
        StmtKind::LetElse {
            pattern,
            value,
            else_block,
            ..
        } => {
            v.pattern(pattern);
            v.expr(value);
            v.blocks(else_block);
        }
        StmtKind::While { cond, body, .. } => {
            v.expr(cond);
            v.block(body);
        }
        StmtKind::WhileLet {
            pattern,
            value,
            body,
            ..
        } => {
            v.pattern(pattern);
            v.expr(value);
            v.block(body);
        }
        StmtKind::For {
            name, source, body, ..
        } => {
            v.binding(name);
            match source {
                ForSource::Array(e) => v.expr(e),
                ForSource::Range(r) => walk_range(v, r),
            }
            v.block(body);
        }
        StmtKind::Assign { target, value } | StmtKind::CompoundAssign { target, value, .. } => {
            v.expr(target);
            v.expr(value);
        }
        StmtKind::Expr(e) => v.expr(e),
    }
}

fn walk_range<'a, V: Visit<'a> + ?Sized>(v: &mut V, r: &'a Rng) {
    v.exprs(r.start.iter().chain(&r.end).map(|e| &**e));
}

fn walk_arm<'a, V: Visit<'a> + ?Sized>(v: &mut V, arm: &'a Arm) {
    v.pattern(&arm.pattern);
    v.exprs(&arm.guard);
    v.expr(&arm.body);
}

fn walk_expr<'a, V: Visit<'a> + ?Sized>(v: &mut V, e: &'a Expr) {
    match &e.kind {
        ExprKind::Int { .. }
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Char(_)
        | ExprKind::Str(_)
        | ExprKind::Name(_)
        | ExprKind::Applied { .. }
        | ExprKind::SelfValue
        | ExprKind::NoneLit
        | ExprKind::Continue { .. } => {}
        ExprKind::Interpolate(parts) => v.exprs(parts.iter().map(|p| &p.value)),
        ExprKind::Call { args, .. } | ExprKind::Todo { args, .. } => {
            v.exprs(args.iter().map(|a| &a.value));
        }
        ExprKind::MethodCall { receiver, args, .. } => {
            v.expr(receiver);
            v.exprs(args.iter().map(|a| &a.value));
        }
        ExprKind::Field { base: operand, .. } | ExprKind::Unary { operand, .. } => v.expr(operand),
        ExprKind::Binary { lhs, rhs, .. }
        | ExprKind::Coalesce { lhs, rhs }
        | ExprKind::Is { lhs, rhs } => {
            v.expr(lhs);
            v.expr(rhs);
        }
        ExprKind::Compare { operands, .. } | ExprKind::ArrayLit(operands) => {
            v.exprs(operands);
        }
        ExprKind::ArrayFill { value, len } => {
            v.expr(value);
            v.expr(len);
        }
        ExprKind::Index { base, index } => {
            v.expr(base);
            v.expr(index);
        }
        ExprKind::Range(r) => walk_range(v, r),
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => {
            v.expr(cond);
            v.block(then_block);
            v.blocks(else_block);
        }
        ExprKind::IfLet {
            pattern,
            value,
            then_block,
            else_block,
        } => {
            v.pattern(pattern);
            v.expr(value);
            v.block(then_block);
            v.blocks(else_block);
        }
        ExprKind::Match {
            scrutinee, arms, ..
        } => {
            v.expr(scrutinee);
            for arm in arms {
                walk_arm(v, arm);
            }
        }
        ExprKind::Loop { body, .. } | ExprKind::Block(body) => v.block(body),
        ExprKind::Return { value, .. } | ExprKind::Break { value, .. } => {
            v.exprs(value.iter().map(|e| &**e));
        }
    }
}

/// The text of `range` of `program` with each pair from `pairs[*next]` on that starts in it
/// swapped, the pairs inside each part swapped too. `pairs` is in order of start, an enclosing
/// pair first, and two pairs either nest or do not overlap.
fn render(program: &str, range: Range<usize>, pairs: &[Pair], next: &mut usize) -> String {
    let mut out = String::new();
    let mut at = range.start;
    while let Some(pair) = pairs.get(*next).filter(|p| p.first.start < range.end) {
        let (a, b) = (pair.first.clone(), pair.second.clone());
        *next += 1;
        out += &program[at..a.start];
        let first = render(program, a.clone(), pairs, next);
        let between = render(program, a.end..b.start, pairs, next);
        let second = render(program, b.clone(), pairs, next);
        out += &second;
        out += &between;
        out += &first;
        at = b.end;
    }
    out += &program[at..range.end];
    out
}

#[cfg(test)]
mod tests {
    use super::swap_joins;

    /// `swap_joins` of `program`, which holds the statements of a `main` unless it starts with an
    /// item. The text that comes back is that of `program`.
    fn swap(program: &str) -> Option<String> {
        if program.starts_with("fn ") {
            return swap_joins(program);
        }
        let (head, tail) = ("fn main() {\n", "}\n");
        let swapped = swap_joins(&format!("{head}{program}{tail}"))?;
        let body = swapped.strip_prefix(head)?;
        Some(body.strip_suffix(tail)?.to_string())
    }

    fn swapped(program: &str) -> String {
        swap(program).unwrap_or_else(|| program.to_string())
    }

    #[test]
    fn swap_joins_should_swap_the_blocks_of_an_if() {
        assert_eq!(
            swapped("let x = if c { 1 } else { None }\n"),
            "let x = if c { None } else { 1 }\n"
        );
    }

    #[test]
    fn swap_joins_should_swap_the_first_and_last_elements_of_an_array() {
        assert_eq!(swapped("f([a, g(b, c), d])\n"), "f([d, g(b, c), a])\n");
    }

    #[test]
    fn swap_joins_should_swap_inside_each_swapped_part() {
        assert_eq!(
            swapped("if c { [1, None] } else { if d { [x, y] } else { z } }\n"),
            "if c { if d { z } else { [y, x] } } else { [None, 1] }\n"
        );
    }

    #[test]
    fn swap_joins_should_swap_only_the_inner_if_of_an_else_if() {
        assert_eq!(
            swapped("if a { 1 } else if b { 2 } else { 3 }\n"),
            "if a { 1 } else if b { 3 } else { 2 }\n"
        );
    }

    #[test]
    fn swap_joins_should_leave_other_forms_as_written() {
        let program =
            "if let Some(v) = o { v } else { 0 }\nlet xs: [i64] = [0; 3]\nxs[0]\nif c { 1 }\n[1]\n";
        assert_eq!(swap(program), None);
    }

    #[test]
    fn swap_joins_should_leave_an_array_type_as_written() {
        let program = "fn f(x: [Pair<i64, u8>]) {}\n";
        assert_eq!(swap(program), None);
    }

    #[test]
    fn swap_joins_should_leave_a_match_guard_as_written() {
        let program = "match n { v if v > 0 => 1; _ => 2 }\n";
        assert_eq!(swap(program), None);
    }

    #[test]
    fn swap_joins_should_read_a_bracket_in_a_condition() {
        assert_eq!(
            swapped("if xs[(i)] == 1 { a } else { b }\n"),
            "if xs[(i)] == 1 { b } else { a }\n"
        );
    }

    #[test]
    fn swap_joins_should_swap_an_array_literal_that_holds_a_less_than() {
        assert_eq!(
            swapped("fn main() {\n    let xs = [1 < 2, false]\n}\n"),
            "fn main() {\n    let xs = [false, 1 < 2]\n}\n"
        );
    }

    #[test]
    fn swap_joins_should_ignore_a_trailing_comma() {
        assert_eq!(swapped("[1, 2,]\n"), "[2, 1,]\n");
    }

    #[test]
    fn swap_joins_should_keep_a_comma_inside_a_call_in_its_element() {
        assert_eq!(swapped("[f(a, b), c]\n"), "[c, f(a, b)]\n");
    }

    #[test]
    fn swap_joins_should_swap_the_fills_of_an_array_literal() {
        assert_eq!(swapped("[[0; 2], [1; 3]]\n"), "[[1; 3], [0; 2]]\n");
    }

    #[test]
    fn swap_joins_should_swap_an_if_whose_condition_is_an_if() {
        assert_eq!(
            swapped("if (if c { true } else { false }) { a } else { b }\n"),
            "if (if c { false } else { true }) { b } else { a }\n"
        );
    }

    #[test]
    fn swap_joins_should_swap_an_if_in_a_condition() {
        assert_eq!(
            swapped("if if c { true } else { false } { 1 } else { 2 }\n"),
            "if if c { false } else { true } { 2 } else { 1 }\n"
        );
    }

    #[test]
    fn swap_joins_should_swap_an_if_after_a_match_guard() {
        assert_eq!(
            swapped("match n { v if v > 0 => if c { 1 } else { 2 }; _ => 3 }\n"),
            "match n { v if v > 0 => if c { 2 } else { 1 }; _ => 3 }\n"
        );
    }

    #[test]
    fn swap_joins_should_swap_an_if_whose_condition_is_a_match() {
        assert_eq!(
            swapped("if (match n { _ => true }) { a } else { b }\n"),
            "if (match n { _ => true }) { b } else { a }\n"
        );
    }

    #[test]
    fn swap_joins_should_swap_an_array_literal_that_holds_a_less_than_in_a_program_with_a_warning()
    {
        assert_eq!(
            swapped("fn main() {\n    let xs = [1 < 2, false];\n}\n"),
            "fn main() {\n    let xs = [false, 1 < 2];\n}\n"
        );
    }
}
