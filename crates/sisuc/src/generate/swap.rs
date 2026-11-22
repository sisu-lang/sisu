//! The join-order check: the members of a join, in the other order. A join must not depend on the
//! order of its members. So a program with no error must still have no error when they swap. The
//! pairs come from the syntax tree of the program, which the clean check already passed.

use std::ops::Range;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::ast::{
    Arg, Arm, BinaryOp, Block, Expr, ExprKind, ForSource, Function, Ident, Item, Member, Pattern,
    PatternKind, Program, Range as Rng, Stmt, StmtKind, TypeKind,
};
use crate::diagnostic::Severity;
use crate::source::Span;
use crate::symbol::Symbol;
use crate::{lexer, parser};

/// `program` with the members of each join swapped (`swap_with`), each `??` that can swap included.
#[cfg(test)]
pub(crate) fn swap_joins(program: &str) -> Option<String> {
    swap_with(program, |_| Vec::new())
}

/// `program` with the members of each join swapped, or None when it has no such join or does not
/// parse. The forms are:
/// - the two blocks of an `if` that has an `else` block; an `if let` keeps its order, because its
///   `else` block cannot see the binding, and an `else if` swaps only the blocks of its inner `if`;
/// - the first and last elements of an array literal;
/// - the first and last arms of a `match` that has no guard. Only the arms with a variant, `None`
///   or a literal as pattern count, so a catch-all arm stays last. The arms swap whole, with
///   their patterns;
/// - the first and last `break` value of a loop. No `break` of the loop may be bare. Neither
///   value may read a name that the loop binds;
/// - the first and last argument that a generic function takes at the places of one type
///   parameter;
/// - the operands of `+`, `*`, `==` and `!=`. The other operators have an order: `-`, `/`, `%`, a
///   comparison, `&&`, `||` and `is`;
/// - the sides of `??`, when neither is a `None`, a literal, an operator or a jump. `??` reads its
///   left side first, and that side must be optional. The syntax does not show two things. Both
///   sides must be optionals of one type. The right side must have a type of its own. So `errors`
///   checks probes (see `coalesce_errors`), and a `??` swaps when its probe has no error.
///
/// A pair that overlaps another is left out. `errors` gets a program. It tells where each error of
/// that program starts.
pub(crate) fn swap_with(
    program: &str,
    mut errors: impl FnMut(&str) -> Vec<usize>,
) -> Option<String> {
    let lexed = lexer::lex(program).ok()?;
    let parsed = parser::parse(lexed, false);
    // A warning, such as the W0007 that the generator writes, does not count.
    let no_error = (parsed.diagnostics.iter()).all(|d| d.severity() != Severity::Error);
    if !no_error || !parsed.checkable {
        return None;
    }
    let mut finder = Finder {
        source: program,
        generics: FxHashMap::default(),
        pairs: Vec::new(),
    };
    finder.program(&parsed.program);
    let mut pairs = compatible(finder.pairs);
    let refused = coalesce_errors(program, &pairs, &mut errors);
    let mut index = 0;
    pairs.retain(|_| {
        index += 1;
        !refused.contains(&(index - 1))
    });
    if pairs.is_empty() {
        return None;
    }
    Some(render(program, 0..program.len(), &pairs, &mut 0))
}

/// Two parts of the program that trade places.
struct Pair {
    first: Range<usize>,
    second: Range<usize>,
    /// The whole `a ?? b`, when the pair is the two sides of a `??`.
    coalesce: Option<Range<usize>>,
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
}

/// The pairs of `pairs` that `render` can swap together, an enclosing pair before the pairs inside
/// it. A pair that overlaps one before it is left out. The `break` values of a loop do this when
/// one of them sits in an `if` block that swaps. The pairs are in order of start, an enclosing
/// pair first. So a stack of the open pairs gives the one pair that the next pair must lie in.
/// The time is that of the sort and one walk. No pair is compared with all the others.
fn compatible(mut pairs: Vec<Pair>) -> Vec<Pair> {
    pairs.sort_by_key(|p| (p.first.start, std::cmp::Reverse(p.second.end)));
    let mut kept: Vec<Pair> = Vec::new();
    // The indices into `kept` of the pairs that enclose the next pair, the innermost last.
    let mut open: Vec<usize> = Vec::new();
    for pair in pairs {
        while open
            .last()
            .is_some_and(|&top| kept[top].second.end <= pair.first.start)
        {
            open.pop();
        }
        if open.last().is_none_or(|&top| kept[top].holds(&pair)) {
            open.push(kept.len());
            kept.push(pair);
        }
    }
    kept
}

/// The block that holds the `a ?? b` at `whole` for a probe, with `a` and `b` the two sides of the
/// pair `sides`:
///
/// `({ let join_right = b; let mut join_probe = a ?? b; join_probe = a; join_probe })`
///
/// The parentheses keep a block that ends a function body or a block of an `if` from being a
/// statement (E0081).
///
/// The `let` of `b` has no error only when `b` has a type of its own. So `id(None)`, `first([])`
/// and the `None` of a `Some(None)` fail it. They take their type from `a`, and no type from `a`
/// reaches them on the left. The assignment of `a` has no error only when `a ?? b` has the type
/// of `a`. Then both sides are optionals of one type.
fn probe_block(program: &str, whole: &Range<usize>, sides: &Pair) -> String {
    format!(
        "({{ let join_right = {}; let mut join_probe = {}; join_probe = {}; join_probe }})",
        &program[sides.second.clone()],
        &program[whole.clone()],
        &program[sides.first.clone()],
    )
}

/// The indices into `pairs` of the `??` pairs that the swap must leave as written. The probe of
/// each (see `probe_block`) replaces its `a ?? b`. The probes of the `??` that no other `??` holds
/// go in one program. So do those at each depth below. So `errors` runs once for each depth of
/// nested `??`, on a text of the size of the program. Each probe is a block of its own. An error
/// that starts in a block refuses that probe alone.
///
/// The result of a failed block is silent in the probes that read it. So the check goes on with
/// the blocks that are left, until a check has no error. The worst input is a chain of C probes,
/// each of which reads the result of the one before it. The checks are then C, each of a text of N
/// bytes, and the time is C times N. An error outside every block cannot be told to a probe. Then
/// each probe of that depth is checked alone, with the same worst time. The generated programs
/// give few rounds and no such error.
fn coalesce_errors(
    program: &str,
    pairs: &[Pair],
    errors: &mut impl FnMut(&str) -> Vec<usize>,
) -> FxHashSet<usize> {
    let mut found: Vec<(usize, &Range<usize>)> = (pairs.iter().enumerate())
        .filter_map(|(i, p)| p.coalesce.as_ref().map(|whole| (i, whole)))
        .collect();
    // A `??` holds the `??` after it in the list that start inside it.
    found.sort_by_key(|(_, whole)| (whole.start, std::cmp::Reverse(whole.end)));
    let mut levels: Vec<Vec<(usize, &Range<usize>)>> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    for (i, whole) in found {
        while open.last().is_some_and(|&end| end <= whole.start) {
            open.pop();
        }
        if open.len() == levels.len() {
            levels.push(Vec::new());
        }
        levels[open.len()].push((i, whole));
        open.push(whole.end);
    }
    let mut refused = FxHashSet::default();
    for level in levels {
        let mut left = level;
        // An error in a block stays silent in the blocks that read its result. So the probes that
        // pass are known only when a round has no error. The refused blocks go back to the text
        // that they replaced, and the others are checked again.
        loop {
            let mut text = String::new();
            let (mut at, mut blocks) = (0, Vec::new());
            for &(i, whole) in &left {
                text += &program[at..whole.start];
                let start = text.len();
                text += &probe_block(program, whole, &pairs[i]);
                blocks.push(start..text.len());
                at = whole.end;
            }
            text += &program[at..];
            let starts = errors(&text);
            if starts.is_empty() {
                break;
            }
            let mut here = FxHashSet::default();
            let mut told = true;
            for start in starts {
                // The blocks are in order: the block of an error is the last one that starts at
                // or before it.
                let block = blocks.partition_point(|b| b.start <= start).checked_sub(1);
                match block.filter(|&b| start < blocks[b].end) {
                    Some(b) => {
                        here.insert(left[b].0);
                    }
                    None => told = false,
                }
            }
            if told {
                left.retain(|(i, _)| !here.contains(i));
                refused.extend(here);
                continue;
            }
            for &(i, whole) in &left {
                let alone = format!(
                    "{}{}{}",
                    &program[..whole.start],
                    probe_block(program, whole, &pairs[i]),
                    &program[whole.end..]
                );
                if !errors(&alone).is_empty() {
                    refused.insert(i);
                }
            }
            break;
        }
    }
    refused
}

/// Finds the pairs of one program.
struct Finder<'a> {
    source: &'a str,
    /// The generic functions that the program declares, by name.
    generics: FxHashMap<Symbol, &'a Function>,
    pairs: Vec<Pair>,
}

impl<'a> Finder<'a> {
    fn program(&mut self, program: &'a Program) {
        for item in &program.items {
            if let Item::Function(f) = item
                && !f.type_params.is_empty()
            {
                self.generics.insert(f.name.name, f);
            }
        }
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
            coalesce: None,
        });
    }

    /// The first and last arm of `arms` that a value of the pattern's own shape reaches.
    fn arms(&mut self, arms: &'a [Arm]) {
        if arms.iter().any(|a| a.guard.is_some()) {
            return;
        }
        let mut plain = arms.iter().filter(|a| {
            matches!(
                a.pattern.kind,
                PatternKind::Variant { .. } | PatternKind::None | PatternKind::Literal(_)
            )
        });
        if let (Some(first), Some(last)) = (plain.next(), plain.next_back()) {
            let whole = |arm: &Arm| Span::new(arm.pattern.span.start(), arm.body.span.end());
            self.add(whole(first), whole(last));
        }
    }

    /// The first and last `break` value of the loop with `body`.
    fn breaks(&mut self, body: &'a Block) {
        let mut breaks = Breaks { found: Vec::new() };
        breaks.block(body);
        // A bare `break` gives `unit`, which no other value joins with.
        let Some(values) = breaks.found.into_iter().collect::<Option<Vec<&Expr>>>() else {
            return;
        };
        let (Some(first), Some(last)) = (values.first(), values.last()) else {
            return;
        };
        if values.len() < 2 {
            return;
        }
        // A value moves out of the block that binds a name it reads. The walk of a value costs its
        // size. A `break` value can hold a loop whose `break` value holds a loop, down d levels.
        // That costs d times the size of the innermost.
        let mut bound = Bound(FxHashSet::default());
        bound.block(body);
        for value in [first, last] {
            let mut reads = Reads {
                names: &bound.0,
                found: false,
            };
            reads.expr(value);
            if reads.found {
                return;
            }
        }
        self.add(first.span, last.span);
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
            ExprKind::Match { arms, .. } => self.arms(arms),
            ExprKind::Loop { body, .. } => self.breaks(body),
            ExprKind::Call { callee, args, .. } => {
                if let Some(f) = self.generics.get(&callee.name)
                    && let Some((i, j)) = one_type_parameter(f, args)
                {
                    self.add(args[i].value.span, args[j].value.span);
                }
            }
            ExprKind::Binary {
                op: op @ (BinaryOp::Add | BinaryOp::Mul | BinaryOp::Eq | BinaryOp::Ne),
                lhs,
                rhs,
                ..
            } if Self::moves(lhs, rhs, |side| same_level(side, *op)) => {
                self.add(lhs.span, rhs.span);
            }
            ExprKind::Coalesce { lhs, rhs }
                if Self::moves(lhs, rhs, |side| {
                    matches!(side.kind, ExprKind::Coalesce { .. }) || not_optional(side)
                }) =>
            {
                self.add(lhs.span, rhs.span);
                if let Some(last) = self.pairs.last_mut() {
                    last.coalesce = Some(e.span.range());
                }
            }
            _ => {}
        }
    }

    /// Whether the operands `lhs` and `rhs` of an operator can trade places without a change of
    /// how the text groups: neither is a jump, which takes all that follows it, and neither is one
    /// that `not_swapped` names. The right side may not start with a block, which would start a
    /// statement.
    fn moves(lhs: &Expr, rhs: &Expr, not_swapped: impl Fn(&Expr) -> bool) -> bool {
        let block = |e: &Expr| {
            matches!(
                e.kind,
                ExprKind::If { .. }
                    | ExprKind::IfLet { .. }
                    | ExprKind::Match { .. }
                    | ExprKind::Loop { .. }
                    | ExprKind::Block(_)
            )
        };
        ![lhs, rhs]
            .into_iter()
            .any(|e| e.is_jump() || not_swapped(e))
            && !block(rhs)
    }
}

/// How tightly `op` binds: operators of one level group with their neighbour on the left.
fn level(op: BinaryOp) -> u8 {
    match op {
        BinaryOp::Or => 1,
        BinaryOp::And => 2,
        BinaryOp::Eq | BinaryOp::Ne => 3,
        BinaryOp::Add | BinaryOp::Sub => 4,
        BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => 5,
    }
}

/// Whether `side` is an operator of the level of `op`, which would group in another way beside
/// the other side. A parenthesized one is too.
fn same_level(side: &Expr, op: BinaryOp) -> bool {
    matches!(&side.kind, ExprKind::Binary { op: other, .. } if level(*other) == level(op))
}

/// Whether `e` has a form that is never an optional, or that no type comes from: a `None` takes
/// the type of its other side, and `??` reads its left side first.
fn not_optional(e: &Expr) -> bool {
    matches!(
        e.kind,
        ExprKind::NoneLit
            | ExprKind::Int { .. }
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Char(_)
            | ExprKind::Str(_)
            | ExprKind::Interpolate(_)
            | ExprKind::Unary { .. }
            | ExprKind::Binary { .. }
            | ExprKind::Compare { .. }
            | ExprKind::Is { .. }
            | ExprKind::ArrayLit(_)
            | ExprKind::ArrayFill { .. }
            | ExprKind::Range(_)
    )
}

/// The first and last place of `f` that a call with `args` fills with a value of one type
/// parameter of `f`. Each place has the type `T` or `T?`. None when no type parameter has two
/// places.
fn one_type_parameter(f: &Function, args: &[Arg]) -> Option<(usize, usize)> {
    if f.params.len() != args.len() {
        return None;
    }
    let place = |at: usize| match &f.params[at].ty.kind {
        TypeKind::Name(name)
            if !f.params[at].mutable
                && args[at].mut_keyword.is_none()
                && f.type_params.iter().any(|t| t.name.name == *name) =>
        {
            Some((*name, f.params[at].ty.optional))
        }
        _ => None,
    };
    (0..args.len()).find_map(|first| {
        let own = place(first)?;
        let last = (first + 1..args.len())
            .rev()
            .find(|&at| place(at) == Some(own))?;
        Some((first, last))
    })
}

/// The `break` values of one loop, in order, `None` for a bare `break`.
struct Breaks<'a> {
    found: Vec<Option<&'a Expr>>,
}

impl<'a> Visit<'a> for Breaks<'a> {
    fn expr(&mut self, e: &'a Expr) {
        match &e.kind {
            // A nested loop owns the `break`s of its body.
            ExprKind::Loop { .. } => {}
            ExprKind::Break {
                label: None, value, ..
            } => self.found.push(value.as_deref()),
            _ => walk_expr(self, e),
        }
    }

    fn stmt(&mut self, s: &'a Stmt) {
        if !matches!(
            s.kind,
            StmtKind::While { .. } | StmtKind::WhileLet { .. } | StmtKind::For { .. }
        ) {
            walk_stmt(self, s);
        }
    }
}

/// The names that a part of the program binds: a `let`, a `for` and each pattern. It leaves out a
/// nested loop, as `Breaks` does: the `break` values of the outer loop lie outside it, and it has
/// its own walk. So the walks of nested loops cost the size of the outer loop once in all.
struct Bound(FxHashSet<Symbol>);

impl<'a> Visit<'a> for Bound {
    fn expr(&mut self, e: &'a Expr) {
        if !matches!(e.kind, ExprKind::Loop { .. }) {
            walk_expr(self, e);
        }
    }

    fn stmt(&mut self, s: &'a Stmt) {
        if !matches!(
            s.kind,
            StmtKind::While { .. } | StmtKind::WhileLet { .. } | StmtKind::For { .. }
        ) {
            walk_stmt(self, s);
        }
    }

    fn binding(&mut self, name: &'a Ident) {
        self.0.insert(name.name);
    }

    fn pattern(&mut self, pattern: &'a Pattern) {
        match &pattern.kind {
            PatternKind::Name(name) => {
                self.0.insert(name.name);
            }
            PatternKind::Variant { fields, .. } | PatternKind::Or(fields) => {
                for p in fields {
                    self.pattern(p);
                }
            }
            PatternKind::Wild
            | PatternKind::None
            | PatternKind::Literal(_)
            | PatternKind::Range { .. } => {}
        }
    }
}

/// Whether an expression reads one of `names`.
struct Reads<'s> {
    names: &'s FxHashSet<Symbol>,
    found: bool,
}

impl<'a> Visit<'a> for Reads<'_> {
    fn expr(&mut self, e: &'a Expr) {
        if let ExprKind::Name(name) = &e.kind
            && self.names.contains(name)
        {
            self.found = true;
        }
        walk_expr(self, e);
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
    use super::{swap_joins, swap_with};

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
            swapped("if xs[(i)] < 1 { a } else { b }\n"),
            "if xs[(i)] < 1 { b } else { a }\n"
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
    fn swap_joins_should_swap_the_arms_of_a_match() {
        assert_eq!(
            swapped("match n {\n    Some(v) => v\n    None => 0\n}\n"),
            "match n {\n    None => 0\n    Some(v) => v\n}\n"
        );
        // A catch-all arm stays last: before another arm it would hide that arm.
        assert_eq!(
            swapped("match n { 1 => a; 2 => b; _ => c }\n"),
            "match n { 2 => b; 1 => a; _ => c }\n"
        );
    }

    #[test]
    fn swap_joins_should_leave_a_guarded_match_as_written() {
        assert_eq!(
            swap("match n { 1 => a; 2 => b; v if v > 3 => c; _ => d }\n"),
            None
        );
    }

    #[test]
    fn swap_joins_should_swap_the_break_values_of_a_loop() {
        assert_eq!(
            swapped("let x = loop {\n    if c { break a }\n    break 1\n}\n"),
            "let x = loop {\n    if c { break 1 }\n    break a\n}\n"
        );
    }

    #[test]
    fn swap_joins_should_drop_a_pair_that_overlaps_an_earlier_one() {
        // The `break` values `a` and `1` would swap through the blocks of the `if`, which swap too.
        assert_eq!(
            swapped("let x = loop {\n    if c { break a } else { 0 }\n    break 1\n}\n"),
            "let x = loop {\n    if c { 0 } else { break a }\n    break 1\n}\n"
        );
    }

    #[test]
    fn swap_joins_should_leave_a_break_value_that_reads_a_binding_of_the_loop_as_written() {
        let program = "let x = loop {\n    if let Some(v) = o { break v }\n    break 1\n}\n";
        assert_eq!(swap(program), None);
    }

    #[test]
    fn swap_joins_should_swap_the_arguments_of_one_type_parameter() {
        let program = "fn pick<T, U>(a: T, b: U, c: T) -> T { a }\nfn main() {\n    let x = pick(i, 2, 1)\n}\n";
        assert_eq!(
            swap(program).as_deref(),
            Some(
                "fn pick<T, U>(a: T, b: U, c: T) -> T { a }\nfn main() {\n    let x = pick(1, 2, i)\n}\n"
            )
        );
    }

    #[test]
    fn swap_joins_should_leave_arguments_of_different_type_parameters_as_written() {
        let program =
            "fn two<T, U>(a: T, b: U) -> T { a }\nfn main() {\n    let x = two(i, 2)\n}\n";
        assert_eq!(swap(program), None);
    }

    #[test]
    fn swap_joins_should_swap_the_operands_of_a_commutative_operator() {
        assert_eq!(
            swapped("let x = a + 1\nlet y = a * 2\nlet z = a == 3\nlet w = a != 4\n"),
            "let x = 1 + a\nlet y = 2 * a\nlet z = 3 == a\nlet w = 4 != a\n"
        );
    }

    #[test]
    fn swap_joins_should_leave_the_other_operators_as_written() {
        let program = "let x = a - b\nlet y = a / b\nlet z = a % b\nlet u = a < b\nlet v = a && b\nlet w = a || b\nlet t = a is b\n";
        assert_eq!(swap(program), None);
    }

    #[test]
    fn swap_joins_should_swap_the_sides_of_a_coalesce_of_two_optionals() {
        assert_eq!(swapped("let x = a ?? b\n"), "let x = b ?? a\n");
    }

    /// `swap_with` where `errors` tells that a probe has an error at `at` of its text, or none.
    const PAIR: &str = "fn main() {\n    let x = a ?? b\n}\n";

    #[test]
    fn swap_with_should_leave_a_coalesce_that_the_probe_refuses_as_written() {
        assert_eq!(swap_with(PAIR, |_| vec![0]), None);
    }

    #[test]
    fn swap_with_should_give_the_probe_the_coalesce_in_a_block_that_checks_both_sides() {
        let mut probes = Vec::new();
        let swapped = swap_with(PAIR, |probe| {
            probes.push(probe.to_string());
            Vec::new()
        });
        assert_eq!(
            probes,
            [
                "fn main() {\n    let x = ({ let join_right = b; let mut join_probe = a ?? b; join_probe = a; join_probe })\n}\n"
            ]
        );
        assert_eq!(
            swapped.as_deref(),
            Some("fn main() {\n    let x = b ?? a\n}\n")
        );
    }

    #[test]
    fn swap_with_should_check_the_coalesces_of_one_depth_in_one_probe() {
        let program =
            "fn main() {\n    let x = a ?? b\n    let y = c ?? d\n    let z = e ?? f\n}\n";
        let mut probes = 0;
        let swapped = swap_with(program, |_| {
            probes += 1;
            Vec::new()
        });
        assert_eq!(probes, 1);
        assert_eq!(
            swapped.as_deref(),
            Some("fn main() {\n    let x = b ?? a\n    let y = d ?? c\n    let z = f ?? e\n}\n")
        );
    }

    #[test]
    fn swap_with_should_refuse_the_coalesce_whose_block_has_the_error() {
        let program =
            "fn main() {\n    let x = a ?? b\n    let y = c ?? d\n    let z = e ?? f\n}\n";
        // An error in the probe of `c ?? d`.
        let swapped = swap_with(program, |probe| {
            probe.find("let mut join_probe = c").into_iter().collect()
        });
        assert_eq!(
            swapped.as_deref(),
            Some("fn main() {\n    let x = b ?? a\n    let y = c ?? d\n    let z = f ?? e\n}\n")
        );
    }

    #[test]
    fn swap_with_should_check_each_coalesce_alone_when_an_error_is_in_no_block() {
        let program = "fn main() {\n    let x = a ?? b\n    let y = c ?? d\n}\n";
        let mut probes = Vec::new();
        let swapped = swap_with(program, |probe| {
            probes.push(probe.to_string());
            // An error at the start of the first probe, which is in no block, and in the probe
            // of `a ?? b` when it stands alone.
            if probes.len() == 1 || probe.contains("join_right = b") {
                vec![0]
            } else {
                Vec::new()
            }
        });
        assert_eq!(probes.len(), 3);
        assert_eq!(
            swapped.as_deref(),
            Some("fn main() {\n    let x = a ?? b\n    let y = d ?? c\n}\n")
        );
    }

    #[test]
    fn swap_with_should_check_each_coalesce_alone_when_an_error_starts_at_the_end_of_a_block() {
        let program = "fn main() {\n    let x = a ?? b\n    let y = c ?? d\n}\n";
        let mut probes = 0;
        let swapped = swap_with(program, |probe| {
            probes += 1;
            // The offset just after the first block: it lies in no block.
            let end = "join_probe })";
            match probes {
                1 => vec![probe.find(end).unwrap() + end.len()],
                _ => Vec::new(),
            }
        });
        assert_eq!(probes, 3);
        assert_eq!(
            swapped.as_deref(),
            Some("fn main() {\n    let x = b ?? a\n    let y = d ?? c\n}\n")
        );
    }

    #[test]
    fn swap_with_should_check_a_coalesce_in_a_coalesce_in_a_probe_of_its_own() {
        let program = "fn main() {\n    let x = f(a ?? b) ?? g(c ?? d)\n}\n";
        let mut probes = 0;
        swap_with(program, |_| {
            probes += 1;
            Vec::new()
        });
        assert_eq!(probes, 2);
    }

    #[test]
    fn swap_joins_should_leave_a_coalesce_with_a_none_as_written() {
        assert_eq!(swap("let x = a ?? None\nlet y = None ?? a\n"), None);
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
