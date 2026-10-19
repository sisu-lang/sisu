//! The hoist: which locals each loop makes unique once, at its pre-header (ADR 0021). The table
//! is `codegen/hoist.rs`'s, ported unchanged, with the rows that a borrowed read widens.

use super::*;
use crate::mir::hoist::unique_roots;
use crate::mir::ownership;

/// The `partition` row: quicksort's partition loop.
pub(crate) const PARTITION: &str = "fn f(xs: [i64], lo: i64, hi: i64) -> [i64] {\n    let mut a = xs\n    let pivot = a[hi]\n    let mut i = lo\n    for j in lo..hi {\n        if a[j] < pivot {\n            let t = a[i]\n            a[i] = a[j]\n            a[j] = t\n            i += 1\n        }\n    }\n    a\n}";

/// The `grid` row: a write to a row of a grid.
pub(crate) const GRID: &str = "fn f(xs: [[i64]]) -> [[i64]] {\n    let mut g = xs\n    for i in 0..2 {\n        g[i][0] = 1\n    }\n    g\n}";

/// The `append` row: an append to an element of a `[string]`.
pub(crate) const APPEND: &str = "fn f(xs: [string]) -> [string] {\n    let mut a = xs\n    for i in 0..2 {\n        a[i] += \"!\"\n    }\n    a\n}";

/// The `both_write` row: an outer loop and its inner loop that both write `a`.
pub(crate) const BOTH_WRITE: &str = "fn f(xs: [i64]) -> [i64] {\n    let mut a = xs\n    for i in 0..2 {\n        a[i] = 0\n        for j in 0..3 {\n            a[j] += 1\n        }\n    }\n    a\n}";

/// The `outer_barred_inner_hoists` row: an outer loop that shares `a` with `keep`, and an
/// inner loop that writes `a`.
pub(crate) const OUTER_BARRED_INNER_HOISTS: &str = "fn f(xs: [i64]) -> [i64] {\n    let mut a = xs\n    let mut keep = [0]\n    for i in 0..2 {\n        keep = a\n        for j in 0..3 {\n            a[j] += 1\n        }\n    }\n    print(keep[0])\n    a\n}";

/// The `b6_borrow_hoists` row: the loop's only read of `a` is `string(a)`, a B6 borrow.
pub(crate) const B6: &str = "fn f(xs: [char]) -> [char] {\n    let mut a = xs\n    for j in 0..2 {\n        print(string(a))\n        a[j] = 'x'\n    }\n    a\n}";

/// The function `f` of `src` after lowering and the ownership pass, without the hoist.
fn placed_f(src: &str) -> Function {
    let mut f = lowered_program(src)
        .functions
        .into_iter()
        .find(|function| function.name == "f")
        .expect("each case should define `f`");
    ownership::place(&mut f);
    f
}

/// For each loop of `f` in `src`, in `Function::loops` order, the names of its `unique_roots`.
fn loops(src: &str) -> Vec<Vec<String>> {
    let f = placed_f(src);
    f.loops
        .iter()
        .map(|lp| {
            unique_roots(&f, lp)
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
    // In `both_write` the outer body writes `a` itself, so these two rows are the ones that
    // fail when the outer walk skips an inner loop's blocks.
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
    // A value use of `a` inside each kind of expression that no row above reaches bars `a`,
    // in each loop.
    (
        "inside_other_kinds",
        "class E {}\nfn e(xs: [i64]) -> E {\n    E()\n}\nfn g(xs: [i64]) -> i64 {\n    xs[0]\n}\nfn s(xs: [i64]) -> string {\n    \"x\"\n}\nfn f(xs: [i64]) -> [i64] {\n    let mut a = xs\n    let o = E()\n    for j in 0..3 {\n        a[j] = -g(a)\n    }\n    for j in 0..3 {\n        if j == 9 {\n            panic(s(a))\n        }\n        a[j] = 0\n    }\n    for j in 0..3 {\n        a[j] = s(a).chars().len()\n    }\n    for j in 0..3 {\n        a[j] = s(a).bytes().len()\n    }\n    for j in 0..3 {\n        a[j] = string(s(a).chars()).len()\n    }\n    for j in 0..3 {\n        if e(a) is o {\n            a[j] = 0\n        }\n    }\n    for j in 0..3 {\n        if a.contains(j) {\n            a[j] = 0\n        }\n    }\n    a\n}",
        &[&[], &[], &[], &[], &[], &[], &[]],
    ),
    // Review Focus 5: a borrowed read never bars, so a loop whose only read of `a` is a B6
    // borrow hoists it; `codegen/hoist.rs` gives `[[]]` here.
    ("b6_borrow_hoists", B6, &[&["a"]]),
    // A copy of `a` in a nested branch of the same loop still bars it.
    (
        "copy_in_nested_branch_bars",
        "fn f(xs: [char]) -> [char] {\n    let mut a = xs\n    let mut keep = ['k']\n    for j in 0..2 {\n        print(string(a))\n        if j == 1 {\n            keep = a\n        }\n        a[j] = 'x'\n    }\n    print(string(keep))\n    a\n}",
        &[&[]],
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
    // The `moved` row bars `a` through the move that `return a` makes into a temporary.
    let (_, moved, _) = CASES
        .iter()
        .find(|(name, ..)| *name == "moved")
        .expect("the table should have a `moved` row");
    has(&built(moved), "f", &["_ = move a#", "return move _"]);
}

/// The printed text of block `id` in `f`, a function as `function` gives it: from its label
/// to the next label or the closing `}`.
fn block_text(f: &str, id: BlockId) -> &str {
    let label = format!("\n  bb{}:\n", id.0);
    let start = f
        .find(&label)
        .unwrap_or_else(|| panic!("no {label:?} in\n{f}"));
    let rest = &f[start + label.len()..];
    let end = rest
        .find("\n  bb")
        .or_else(|| rest.find("\n}"))
        .unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn hoisted_unique_sits_in_the_pre_header() {
    let program = checked(PARTITION);
    let mir = build(&program);
    let f = mir
        .functions
        .iter()
        .find(|function| function.name == "f")
        .expect("PARTITION should define `f`");
    let printed = function(&print(&mir, &program), "f");
    let [lp] = f.loops.as_slice() else {
        panic!("PARTITION should have one loop:\n{printed}");
    };
    assert!(
        block_text(&printed, lp.pre_header).contains("make_unique a#"),
        "{printed}"
    );
    // The writes stay in the loop, with their checks and without their `make_unique`.
    let in_loop: String = lp
        .blocks
        .iter()
        .map(|&block| block_text(&printed, block))
        .collect();
    assert_eq!(in_loop.matches("check a#").count(), 2, "{printed}");
    assert!(!in_loop.contains("make_unique"), "{printed}");
}
