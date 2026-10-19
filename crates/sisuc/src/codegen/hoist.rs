//! Which local arrays a loop can make unique once, before it starts, so that its element
//! writes skip the count check at the root (ADR 0021).

use rustc_hash::FxHashSet;

use super::array::{borrowed_index_base, borrowed_local};
use crate::tir::{Block, Expr, ExprKind, LocalId, Place, Stmt, Type};

/// The locals that the loop whose body is `body` can make unique before it starts, sorted and
/// without repeats. A local `L` qualifies when the body, its nested loops included, both:
///
/// - writes `L`: an `Assign` or `Append` to an `Index` place rooted at `Place::Local(L)`. Only a
///   write that codegen emits counts: the walk of a block stops after its first statement of
///   type `never`, and skips the block's value then, as `Codegen::block` does. `last_use` counts
///   no use after a `return`, so a dead write there may find `L` already moved.
/// - never shares `L`. Every node that could raise the count of `L`'s block, or replace it,
///   bars `L`: a `Local(L)` read, which retains it; a `Move(L)` read, which retains nothing
///   (`move_local` transfers the reference and clears the slot) but is barred anyway, since
///   the slot no longer holds the block; an `Assign` or `Append` to `Place::Local(L)`; and a
///   node that binds `L` (`Let`, `IfSome`'s `bind`, `Match`'s `temp` and its arms' bindings), since
///   `L` has no value before the loop then. Two positions borrow `L` without retaining it, and do
///   not bar, as codegen's `borrowed_index_base` and `borrowed_local` decide: the base of an
///   `Index` whose index passes `leaves_locals` (otherwise `index` retains the base while the index
///   runs, and a write inside it must copy), and the operand of `Len`. Every other read bars: `Sum`
///   and `Contains`, which retain their base, and `Field`, `Chars`, `Bytes`, `Words` and
///   `FromChars`, which codegen borrows too.
///
/// The matches below have no `_` arm, so a node kind added later gets its arm from this rule.
pub(super) fn unique_roots(body: &Block) -> Vec<LocalId> {
    let mut walk = Walk::default();
    walk.block(body);
    let mut roots: Vec<_> = walk.written.difference(&walk.barred).copied().collect();
    roots.sort_unstable_by_key(|local| local.0);
    roots
}

/// The locals a loop body writes, and those it bars, as `unique_roots` defines them.
#[derive(Default)]
struct Walk {
    /// The locals whose elements the body writes.
    written: FxHashSet<LocalId>,
    /// The locals that the body could share, replace or bind.
    barred: FxHashSet<LocalId>,
}

impl Walk {
    /// Walks `block`'s statements through its first of type `never`, then its value if none was.
    fn block(&mut self, block: &Block) {
        for stmt in &block.stmts {
            self.stmt(stmt);
            // The checker gives a value of type `never` only to an expression statement.
            if matches!(stmt, Stmt::Expr(e) if e.ty == Type::NEVER) {
                return;
            }
        }
        if let Some(value) = &block.value {
            self.expr(value);
        }
    }

    /// Walks `stmt`: a `Let` bars the local it binds.
    fn stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let { local, init } => {
                self.barred.insert(*local);
                self.expr(init);
            }
            Stmt::Assign { place, value } | Stmt::Append { place, value } => {
                self.place(place);
                self.expr(value);
            }
            Stmt::Expr(e) => self.expr(e),
        }
    }

    /// A written place: its root local is written when the place is an element, and barred when
    /// the place is the local itself.
    fn place(&mut self, place: &Place) {
        match place {
            Place::Local(local) => {
                self.barred.insert(*local);
            }
            Place::Field { base, .. } => self.expr(base),
            Place::Index { base, index } => {
                self.expr(index);
                if let Place::Local(local) = **base {
                    self.written.insert(local);
                } else {
                    self.place(base);
                }
            }
        }
    }

    /// Walks `e`: a read of a local bars it, unless it is an `Index` base or a `Len` operand that
    /// codegen borrows.
    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Char(_)
            | ExprKind::Str(_)
            | ExprKind::None
            | ExprKind::ReadLine
            | ExprKind::Break
            | ExprKind::Continue
            | ExprKind::Return(None) => {}
            ExprKind::Local(local) | ExprKind::Move(local) => {
                self.barred.insert(*local);
            }
            ExprKind::Index { base, index } => {
                if borrowed_index_base(base, index).is_none() {
                    self.expr(base);
                }
                self.expr(index);
            }
            ExprKind::Len(operand) => {
                if borrowed_local(operand).is_none() {
                    self.expr(operand);
                }
            }
            ExprKind::Call { args: operands, .. }
            | ExprKind::New { args: operands, .. }
            | ExprKind::Variant { args: operands, .. }
            | ExprKind::ArrayLit(operands)
            | ExprKind::Math { operands, .. }
            | ExprKind::Compare { operands, .. }
            | ExprKind::Interpolate {
                parts: operands, ..
            } => {
                for operand in operands {
                    self.expr(operand);
                }
            }
            ExprKind::Field { base: operand, .. }
            | ExprKind::Wrap(operand)
            | ExprKind::Convert(operand)
            | ExprKind::Print { operand, .. }
            | ExprKind::Panic(operand)
            | ExprKind::Chars(operand)
            | ExprKind::Bytes(operand)
            | ExprKind::Words(operand)
            | ExprKind::FromChars(operand)
            | ExprKind::Sum(operand)
            | ExprKind::Unary { operand, .. }
            | ExprKind::Return(Some(operand)) => self.expr(operand),
            ExprKind::Binary { lhs, rhs, .. }
            | ExprKind::Concat { lhs, rhs }
            | ExprKind::Equal { lhs, rhs, .. }
            | ExprKind::Is { lhs, rhs }
            | ExprKind::ArrayFill {
                value: lhs,
                len: rhs,
            }
            | ExprKind::Contains {
                base: lhs,
                value: rhs,
            } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                self.expr(cond);
                self.branches(then_block, else_block.as_ref());
            }
            ExprKind::IfSome {
                bind,
                scrutinee,
                then_block,
                else_block,
            } => {
                self.barred.insert(*bind);
                self.expr(scrutinee);
                self.branches(then_block, else_block.as_ref());
            }
            ExprKind::Match {
                scrutinee,
                temp,
                arms,
            } => {
                self.barred.insert(*temp);
                self.expr(scrutinee);
                for arm in arms {
                    self.barred.extend(arm.bindings.iter().copied());
                    if let Some(guard) = &arm.guard {
                        self.expr(guard);
                    }
                    self.block(&arm.body);
                }
            }
            ExprKind::Loop(block) | ExprKind::Block(block) => self.block(block),
        }
    }

    /// Walks both branches of an `If` or an `IfSome`.
    fn branches(&mut self, then_block: &Block, else_block: Option<&Block>) {
        self.block(then_block);
        if let Some(block) = else_block {
            self.block(block);
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    // The rows that `codegen::tests` compiles as well are constants.
    use super::*;
    use crate::check::check;
    use crate::parser::parse_ok;
    use crate::tir::{Expr, ExprKind, Function, Program, Stmt};

    /// The `partition` row: quicksort's partition loop.
    pub(in crate::codegen) const PARTITION: &str = "fn f(xs: [i64], lo: i64, hi: i64) -> [i64] {\n    let mut a = xs\n    let pivot = a[hi]\n    let mut i = lo\n    for j in lo..hi {\n        if a[j] < pivot {\n            let t = a[i]\n            a[i] = a[j]\n            a[j] = t\n            i += 1\n        }\n    }\n    a\n}";

    /// The `grid` row: a write to a row of a grid.
    pub(in crate::codegen) const GRID: &str = "fn f(xs: [[i64]]) -> [[i64]] {\n    let mut g = xs\n    for i in 0..2 {\n        g[i][0] = 1\n    }\n    g\n}";

    /// The `append` row: an append to an element of a `[string]`.
    pub(in crate::codegen) const APPEND: &str = "fn f(xs: [string]) -> [string] {\n    let mut a = xs\n    for i in 0..2 {\n        a[i] += \"!\"\n    }\n    a\n}";

    /// The `both_write` row: an outer loop and its inner loop that both write `a`.
    pub(in crate::codegen) const BOTH_WRITE: &str = "fn f(xs: [i64]) -> [i64] {\n    let mut a = xs\n    for i in 0..2 {\n        a[i] = 0\n        for j in 0..3 {\n            a[j] += 1\n        }\n    }\n    a\n}";

    /// The `outer_barred_inner_hoists` row: an outer loop that shares `a` with `keep`, and an
    /// inner loop that writes `a`.
    pub(in crate::codegen) const OUTER_BARRED_INNER_HOISTS: &str = "fn f(xs: [i64]) -> [i64] {\n    let mut a = xs\n    let mut keep = [0]\n    for i in 0..2 {\n        keep = a\n        for j in 0..3 {\n            a[j] += 1\n        }\n    }\n    print(keep[0])\n    a\n}";

    /// Checks `src`, with an empty `main` added, and marks its last uses.
    fn program(src: &str) -> Program {
        let src = format!("{src}\nfn main() {{}}");
        let (program, diagnostics) = check(&parse_ok(&src), &src);
        assert!(program.is_some(), "{diagnostics:?}");
        let mut program = program.expect("the assert above should have stopped a failed check");
        crate::last_use::mark(&mut program);
        program
    }

    /// The function `f` of `program`.
    fn f(program: &Program) -> &Function {
        program
            .functions
            .iter()
            .find(|function| function.name == "f")
            .expect("each case should define `f`")
    }

    /// The bodies of the loops in `block`, in pre-order.
    fn loop_bodies<'a>(block: &'a Block, found: &mut Vec<&'a Block>) {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let { init: e, .. } | Stmt::Expr(e) => expr_loops(e, found),
                Stmt::Assign { .. } | Stmt::Append { .. } => {}
            }
        }
        if let Some(value) = &block.value {
            expr_loops(value, found);
        }
    }

    fn expr_loops<'a>(e: &'a Expr, found: &mut Vec<&'a Block>) {
        match &e.kind {
            ExprKind::Loop(body) => {
                found.push(body);
                loop_bodies(body, found);
            }
            ExprKind::Block(block) => loop_bodies(block, found),
            ExprKind::If {
                then_block,
                else_block,
                ..
            } => {
                loop_bodies(then_block, found);
                if let Some(block) = else_block {
                    loop_bodies(block, found);
                }
            }
            _ => {}
        }
    }

    /// For each loop of `f` in `src`, in pre-order, the names of its `unique_roots`.
    fn loops(src: &str) -> Vec<Vec<String>> {
        let program = program(src);
        let f = f(&program);
        let mut bodies = Vec::new();
        loop_bodies(&f.body, &mut bodies);
        bodies
            .into_iter()
            .map(|body| {
                unique_roots(body)
                    .into_iter()
                    .map(|local| f.locals[local.0].name.clone().unwrap_or_default())
                    .collect()
            })
            .collect()
    }

    /// Each row: its name, a program with a function `f`, and the names of `unique_roots` for
    /// each loop of `f`, in pre-order.
    const CASES: &[(&str, &str, &[&[&str]])] = &[
        ("partition", PARTITION, &[&["a"]]),
        (
            "read_only",
            "fn f(xs: [i64]) -> i64 {\n    let mut a = xs\n    let mut s = 0\n    for j in 0..3 {\n        s += a[j]\n    }\n    a[0] = s\n    a[0]\n}",
            &[&[]],
        ),
        (
            "call_arg",
            "fn g(xs: [i64]) -> i64 {\n    xs[0]\n}\nfn f(xs: [i64]) -> [i64] {\n    let mut a = xs\n    for j in 0..3 {\n        a[j] = g(a)\n    }\n    a\n}",
            &[&[]],
        ),
        (
            "variant_arg",
            "enum H {\n    Arr([i64])\n}\nfn f(xs: [i64]) -> [i64] {\n    let mut a = xs\n    let mut keep = H.Arr([0])\n    for j in 0..3 {\n        keep = H.Arr(a)\n        a[j] = 1\n    }\n    print(keep == H.Arr([0]))\n    a\n}",
            &[&[]],
        ),
        (
            "field_store",
            "class K {\n    mut xs: [i64]\n}\nfn f(xs: [i64], keep: K) -> [i64] {\n    let mut a = xs\n    for j in 0..3 {\n        keep.xs = a\n        a[j] = j\n    }\n    a\n}",
            &[&[]],
        ),
        (
            "let_copy",
            "fn f(xs: [i64]) -> [i64] {\n    let mut a = xs\n    for j in 0..3 {\n        let b = a\n        a[j] = b[0]\n    }\n    a\n}",
            &[&[]],
        ),
        (
            "reassign",
            "fn f(xs: [i64], c: [i64]) -> [i64] {\n    let mut a = xs\n    for j in 0..3 {\n        a = c\n        a[j] = 1\n    }\n    a\n}",
            &[&[]],
        ),
        (
            "declared_in_loop",
            "fn f() {\n    for j in 0..3 {\n        let mut c = [0; 3]\n        c[j] = 1\n    }\n}",
            &[&[]],
        ),
        (
            "moved",
            "fn f(xs: [i64]) -> [i64] {\n    let mut a = xs\n    for j in 0..3 {\n        if a[j] < 0 {\n            return a\n        }\n        a[j] = 0\n    }\n    a\n}",
            &[&[]],
        ),
        (
            "write_after_return",
            "fn f(a: [i64]) -> i64 {\n    let mut b = a\n    let c = b\n    while true {\n        return c.len()\n        b[0] = 1\n    }\n    return 0\n}",
            &[&[]],
        ),
        (
            "early_exits",
            "fn f(xs: [i64], stop: i64) -> i64 {\n    let mut b = xs\n    for j in 0..6 {\n        if j == stop {\n            break\n        }\n        if j % 2 == 1 {\n            continue\n        }\n        if j == 4 {\n            return j\n        }\n        b[j] = 0\n    }\n    b[0]\n}",
            &[&["b"]],
        ),
        (
            "len",
            "fn f(xs: [i64]) -> [i64] {\n    let mut a = xs\n    for j in 0..3 {\n        a[a.len() - 1 - j] = j\n    }\n    a\n}",
            &[&["a"]],
        ),
        (
            "call_in_write_index",
            "fn h(j: i64) -> i64 {\n    j\n}\nfn f(xs: [i64]) -> [i64] {\n    let mut a = xs\n    for j in 0..3 {\n        a[h(j)] = 1\n    }\n    a\n}",
            &[&["a"]],
        ),
        (
            "call_in_read_index",
            "fn h(j: i64) -> i64 {\n    j\n}\nfn f(xs: [i64]) -> [i64] {\n    let mut a = xs\n    for j in 0..3 {\n        a[j] = a[h(j)]\n    }\n    a\n}",
            &[&[]],
        ),
        (
            "index_with_a_write",
            "fn f(xs: [i64]) -> i64 {\n    let mut a = xs\n    let mut s = 0\n    for j in 0..2 {\n        s += a[if j == 0 {\n            a[0] = 10\n            0\n        } else { 1 }]\n    }\n    s\n}",
            &[&[]],
        ),
        ("grid", GRID, &[&["g"]]),
        ("append", APPEND, &[&["a"]]),
        (
            "for_over_itself",
            "fn f(xs: [i64]) -> [i64] {\n    let mut a = xs\n    for x in a {\n        a[0] = x\n    }\n    a\n}",
            &[&["a"]],
        ),
        (
            "field_root",
            "class O {\n    mut xs: [i64]\n}\nfn f(o: O) {\n    for j in 0..3 {\n        o.xs[j] = 1\n    }\n}",
            &[&[]],
        ),
        (
            "outer_barred_inner_hoists",
            OUTER_BARRED_INNER_HOISTS,
            &[&[], &["a"]],
        ),
        ("both_write", BOTH_WRITE, &[&["a"], &["a"]]),
        // Not in the plan: in `both_write` the outer body writes `a` itself, so these two
        // rows are the ones that fail when the outer walk skips an inner loop's body.
        (
            "inner_write_only",
            "fn f(xs: [i64]) -> [i64] {\n    let mut a = xs\n    for i in 0..2 {\n        for j in 0..3 {\n            a[j] += i\n        }\n    }\n    a\n}",
            &[&["a"], &["a"]],
        ),
        (
            "inner_bars_outer",
            "fn f(xs: [i64]) -> [i64] {\n    let mut a = xs\n    let mut keep = [0]\n    for i in 0..2 {\n        a[i] = 0\n        for j in 0..3 {\n            keep = a\n        }\n    }\n    print(keep[0])\n    a\n}",
            &[&[], &[]],
        ),
        (
            "declared_in_outer_body",
            "fn f() -> i64 {\n    let mut s = 0\n    for i in 0..2 {\n        let mut row = [0; 3]\n        for j in 0..3 {\n            row[j] = 1\n        }\n        s += row[0]\n    }\n    s\n}",
            &[&[], &["row"]],
        ),
        // A write in a `match` arm hoists; the scrutinee and an arm's body read `a`, and bar it.
        (
            "match_arms",
            "enum H {\n    Arr([i64])\n    Empty\n}\nfn f(xs: [i64], h: H) -> [i64] {\n    let mut a = xs\n    for j in 0..3 {\n        match h {\n            Arr(b) => a[j] = b[0]\n            Empty => a[j] = 0\n        }\n    }\n    for j in 0..3 {\n        match H.Arr(a) {\n            Arr(b) => print(b[0])\n            Empty => {}\n        }\n        a[j] = 0\n    }\n    for j in 0..3 {\n        match h {\n            Arr(_) => print(a.sum())\n            Empty => {}\n        }\n        a[j] = 0\n    }\n    a\n}",
            &[&["a"], &[], &[]],
        ),
        // A guard reads `a` and bars it.
        (
            "match_guards",
            "enum H {\n    Arr([i64])\n    Empty\n}\nfn f(xs: [i64], h: H) -> [i64] {\n    let mut a = xs\n    for j in 0..3 {\n        match h {\n            Arr(_) if a.sum() > 0 => {}\n            _ => {}\n        }\n        a[j] = 0\n    }\n    a\n}",
            &[&[]],
        ),
        // Not in the plan: a value use of `a` inside each kind of expression that no row above
        // reaches bars `a`, in each loop.
        (
            "inside_other_kinds",
            "class E {}\nfn e(xs: [i64]) -> E {\n    E()\n}\nfn g(xs: [i64]) -> i64 {\n    xs[0]\n}\nfn s(xs: [i64]) -> string {\n    \"x\"\n}\nfn f(xs: [i64]) -> [i64] {\n    let mut a = xs\n    let o = E()\n    for j in 0..3 {\n        a[j] = -g(a)\n    }\n    for j in 0..3 {\n        if j == 9 {\n            panic(s(a))\n        }\n        a[j] = 0\n    }\n    for j in 0..3 {\n        a[j] = s(a).chars().len()\n    }\n    for j in 0..3 {\n        a[j] = s(a).bytes().len()\n    }\n    for j in 0..3 {\n        a[j] = string(s(a).chars()).len()\n    }\n    for j in 0..3 {\n        if e(a) is o {\n            a[j] = 0\n        }\n    }\n    for j in 0..3 {\n        if a.contains(j) {\n            a[j] = 0\n        }\n    }\n    a\n}",
            &[&[], &[], &[], &[], &[], &[], &[]],
        ),
    ];

    #[test]
    fn unique_roots_of_each_loop() {
        let found: Vec<_> = CASES
            .iter()
            .map(|(name, src, _)| (*name, loops(src)))
            .collect();
        let expected: Vec<_> = CASES
            .iter()
            .map(|(name, _, expected)| {
                let names = expected
                    .iter()
                    .map(|roots| roots.iter().map(ToString::to_string).collect())
                    .collect();
                (*name, names)
            })
            .collect();
        assert_eq!(found, expected);
        // The `moved` row bars `a` through the `Move` that `return a` became.
        let (_, moved, _) = CASES
            .iter()
            .find(|(name, ..)| *name == "moved")
            .expect("the table should have a `moved` row");
        let moved = program(moved).to_string();
        assert!(moved.contains("(return (move a#"), "{moved}");
    }
}
