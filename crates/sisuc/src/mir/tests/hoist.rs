//! The hoist: which locals each loop makes unique once, at its pre-header (ADR 0021). A
//! borrowed read never bars a local; a copy, a take, a release or an assignment does.

use super::*;
use crate::mir::hoist::{PROBES, STEPS, hoist, unique_roots};
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

/// The `push` row: a push neither writes `a` nor bars it, so nothing is hoisted.
const PUSH: &str = "fn f(xs: [i64]) -> [i64] {\n    let mut a = xs\n    for i in 0..2 {\n        a.push(i)\n    }\n    a\n}";

/// The `push_and_write` row: the loop also writes an element of `a`, which the push does not bar.
const PUSH_AND_WRITE: &str = "fn f(xs: [i64]) -> [i64] {\n    let mut a = xs\n    for i in 0..2 {\n        a.push(i)\n        a[0] = i\n    }\n    a\n}";

/// The `push_of_the_array` row: `a` holds `b`, which the next write to `b` must not change, so
/// the pushed `b` bars its hoist.
const PUSH_OF_THE_ARRAY: &str = "fn f(xs: [i64]) -> [[i64]] {\n    let mut b = xs\n    let mut a: [[i64]] = []\n    for j in 0..3 {\n        a.push(b)\n        b[j] = j\n    }\n    a\n}";

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
        "fn f(a: [i64]) -> i64 {\n    let mut b = a\n    let c = b\n    loop {\n        return c.len()\n        b[0] = 1\n    }\n    return 0\n}",
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
    // borrow hoists it.
    ("b6_borrow_hoists", B6, &[&["a"]]),
    // A copy of `a` in a nested branch of the same loop still bars it.
    (
        "copy_in_nested_branch_bars",
        "fn f(xs: [char]) -> [char] {\n    let mut a = xs\n    let mut keep = ['k']\n    for j in 0..2 {\n        print(string(a))\n        if j == 1 {\n            keep = a\n        }\n        a[j] = 'x'\n    }\n    print(string(keep))\n    a\n}",
        &[&[]],
    ),
    ("push", PUSH, &[&[]]),
    ("push_and_write", PUSH_AND_WRITE, &[&["a"]]),
    ("push_of_the_array", PUSH_OF_THE_ARRAY, &[&[]]),
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
    has(&built(row("moved")), "f", &["_ = move a#", "return move _"]);
}

/// The printed text of block `id` in `f`, a function as `function` gives it: from its label
/// to the next label, or to the end for the last block.
fn block_text(f: &str, id: BlockId) -> &str {
    let label = format!("\n  bb{}:\n", id.0);
    let start = f
        .find(&label)
        .unwrap_or_else(|| panic!("no {label:?} in\n{f}"));
    let rest = &f[start + label.len()..];
    let end = rest.find("\n  bb").unwrap_or(rest.len());
    &rest[..end]
}

/// The printed pre-header of the only loop of `f` in `src` after `build`, and its loop blocks.
fn pre_header_and_loop(src: &str) -> (String, String) {
    let program = checked(src);
    let mir = build(&program);
    let f = mir
        .functions
        .iter()
        .find(|function| function.name == "f")
        .expect("each case should define `f`");
    let printed = function(&print(&mir, &program), "f");
    let [lp] = f.loops.as_slice() else {
        panic!("the case should have one loop:\n{printed}");
    };
    let in_loop = lp
        .blocks
        .iter()
        .map(|&block| block_text(&printed, block))
        .collect();
    (block_text(&printed, lp.pre_header).to_owned(), in_loop)
}

/// The last statement of a printed block, before its terminator, trimmed.
fn last_statement(block: &str) -> &str {
    block.lines().rev().nth(1).map_or("", str::trim)
}

/// The source of the row `name` of `CASES`.
fn row(name: &str) -> &'static str {
    CASES
        .iter()
        .find(|(row, ..)| *row == name)
        .map_or_else(|| panic!("no row {name}"), |(_, src, _)| *src)
}

#[test]
fn endless_loop_hoists_into_its_pre_header() {
    let (pre_header, in_loop) = pre_header_and_loop(
        "fn f(xs: [i64]) -> i64 {\n    let mut b = xs\n    let mut i = 0\n    let mut s = 0\n    loop {\n        if i == b.len() {\n            return s\n        }\n        b[i] = i\n        s += b[i]\n        i += 1\n    }\n}",
    );
    assert!(pre_header.contains("make_unique b#"), "{pre_header}");
    assert_eq!(in_loop.matches("make_unique").count(), 0, "{in_loop}");
}

#[test]
fn hoisted_unique_sits_in_the_pre_header() {
    let (pre_header, in_loop) = pre_header_and_loop(PARTITION);
    // Last, after everything the pre-header computes before the loop.
    assert!(
        last_statement(&pre_header).starts_with("make_unique a#"),
        "{pre_header}"
    );
    // The writes stay in the loop, with their checks and without their `make_unique`.
    assert_eq!(in_loop.matches("check a#").count(), 2, "{in_loop}");
    assert!(!in_loop.contains("make_unique"), "{in_loop}");
    // Only the root moves: a grid's row is still made unique at the write.
    let (pre_header, in_loop) = pre_header_and_loop(GRID);
    assert!(
        last_statement(&pre_header).starts_with("make_unique g#"),
        "{pre_header}"
    );
    assert_eq!(in_loop.matches("make_unique").count(), 1, "{in_loop}");
    assert!(in_loop.contains("make_unique g#1["), "{in_loop}");
    // A barred array keeps its `make_unique` at the write, beside one that leaves.
    let (pre_header, in_loop) = pre_header_and_loop(
        "fn f(xs: [i64], ys: [i64]) -> [i64] {\n    let mut a = xs\n    let mut b = ys\n    let mut keep = [0]\n    for j in 0..2 {\n        a[j] = 1\n        keep = b\n        b[j] = 1\n    }\n    print(keep[0])\n    a\n}",
    );
    assert!(
        last_statement(&pre_header).starts_with("make_unique a#"),
        "{pre_header}"
    );
    assert!(in_loop.contains("make_unique b#"), "{in_loop}");
    assert!(!in_loop.contains("make_unique a#"), "{in_loop}");
    // `for x in a` copies `a` in the pre-header: made unique before that copy, the loop would
    // write into the array it iterates over.
    let (pre_header, _) = pre_header_and_loop(row("for_over_itself"));
    let copy = pre_header
        .find("copy a#")
        .unwrap_or_else(|| panic!("{pre_header}"));
    assert!(
        last_statement(&pre_header[copy..]).starts_with("make_unique a#"),
        "{pre_header}"
    );
}

/// `f(s, c)` with one loop, `bb1` and `bb2`: `bb1` makes `s` unique and branches on `c`, and
/// `bb2` runs `stmts` and ends with `exit`.
fn hand_built(stmts: Vec<Statement>, exit: Terminator) -> Function {
    let mut b = Builder::new("f");
    let s = b.param("s", Type::STRING);
    let c = b.param("c", Type::BOOL);
    let whole = |local| Place {
        local,
        projections: Vec::new(),
        borrow: false,
        guard: false,
    };
    b.block(Vec::new(), Terminator::Goto(BlockId(1)));
    b.block(
        vec![Statement::MakeUnique(whole(s))],
        Terminator::Branch {
            cond: Operand::Read(whole(c)),
            then: BlockId(1),
            else_: BlockId(2),
        },
    );
    b.block(stmts, exit);
    b.add_loop(BlockId(0), BlockId(1), vec![BlockId(1), BlockId(2)]);
    b.finish()
}

/// Bars that lowering never puts on a written local, so only a hand-built function reaches
/// them: a move in a terminator (a return value moves into a temporary first), a `release`
/// (only of temporaries), and a `use` or a guarded `use` (the ownership pass replaces each).
/// Each bars its local, as a move in a statement does; with none, `s` hoists.
#[test]
fn hand_built_bars() {
    let s = LocalId(0);
    let reads = |operand| Statement::Assign {
        dest: None,
        value: Rvalue::Use(operand),
    };
    let cases = [
        ("none", Vec::new(), Terminator::Return(None), vec![s]),
        (
            "return move",
            Vec::new(),
            Terminator::Return(Some(Operand::Move(s))),
            vec![],
        ),
        (
            "release",
            vec![Statement::Release(s)],
            Terminator::Return(None),
            vec![],
        ),
        (
            "use",
            vec![reads(Operand::Use(s))],
            Terminator::Return(None),
            vec![],
        ),
        (
            "guarded",
            vec![reads(Operand::Guarded(s))],
            Terminator::Return(None),
            vec![],
        ),
    ];
    for (name, stmts, exit, expected) in cases {
        let f = hand_built(stmts, exit);
        assert_eq!(unique_roots(&f, &f.loops[0]), expected, "{name}");
    }
}

/// A push consumes its value as a store does, so a push of a local that moved bars it: with
/// the push, `s` stays unique inside the loop. Lowering moves a local into a push only at its
/// last use, so no lowered loop that writes `s` also pushes it by a move; only a hand-built
/// function reaches this.
#[test]
fn a_push_of_a_moved_local_should_bar_it() {
    let s = LocalId(0);
    let push = Statement::Push {
        place: Place {
            local: LocalId(1),
            projections: Vec::new(),
            borrow: false,
            guard: false,
        },
        value: Operand::Move(s),
        at: Span::new(0, 0),
    };
    for (name, stmts, expected) in [("none", Vec::new(), vec![s]), ("push", vec![push], vec![])] {
        let f = hand_built(stmts, Terminator::Return(None));
        assert_eq!(unique_roots(&f, &f.loops[0]), expected, "{name}");
    }
}

/// `f` with `depth` nested loops, each of which writes `a`.
fn nested(depth: usize) -> String {
    let opens = (0..depth)
        .map(|level| format!("for i{level} in 0..2 {{\n    a[0] = {level}\n"))
        .collect::<Vec<_>>()
        .concat();
    let closes = "}\n".repeat(depth);
    format!("fn f(xs: [i64]) -> [i64] {{\n    let mut a = xs\n{opens}{closes}a\n}}")
}

/// `f` with one loop that writes each of `roots` arrays.
fn many_roots(roots: usize) -> String {
    let lets = (0..roots)
        .map(|k| format!("    let mut a{k} = [0]\n"))
        .collect::<Vec<_>>()
        .concat();
    let writes = (0..roots)
        .map(|k| format!("        a{k}[0] = j\n"))
        .collect::<Vec<_>>()
        .concat();
    format!("fn f() {{\n{lets}    for j in 0..2 {{\n{writes}    }}\n}}")
}

/// The level-0 `make_unique` statements of `f`, per block.
fn level_0_uniques(f: &Function) -> Vec<usize> {
    f.blocks
        .iter()
        .map(|block| {
            block
                .stmts
                .iter()
                .filter(|stmt| {
                    matches!(stmt, Statement::MakeUnique(place) if place.projections.is_empty())
                })
                .count()
        })
        .collect()
}

/// Hoists `f` of `src` and gives `f`, with the statements and terminators `f` had, those the
/// hoist stepped through, and the keys it touched.
fn hoisted_with_work(src: &str) -> (Function, usize, usize, usize) {
    let mut f = placed_f(src);
    let size = f.blocks.iter().map(|block| block.stmts.len() + 1).sum();
    STEPS.set(0);
    PROBES.set(0);
    hoist(&mut f);
    (f, size, STEPS.get(), PROBES.get())
}

/// The statements and terminators in `lp`'s blocks.
fn loop_size(f: &Function, lp: &Loop) -> usize {
    lp.blocks
        .iter()
        .map(|block| f.blocks[block.0].stmts.len() + 1)
        .sum()
}

/// The hoist steps through each statement a bounded number of times, however deep the loops
/// nest: an outer loop does not walk its inner loops' statements again.
#[test]
fn nested_loops_step_through_each_statement_a_bounded_number_of_times() {
    for depth in [8, 16, 32] {
        let (f, size, steps, _) = hoisted_with_work(&nested(depth));
        // Every write's `make_unique` reaches the outermost pre-header as one statement.
        let uniques = level_0_uniques(&f);
        assert_eq!(uniques.iter().sum::<usize>(), 1, "depth {depth}");
        assert_eq!(uniques[f.loops[0].pre_header.0], 1, "depth {depth}");
        // Each statement left in the outer loop was walked.
        assert!(
            steps >= loop_size(&f, &f.loops[0]),
            "depth {depth}: {steps}"
        );
        assert!(
            steps <= 2 * size,
            "depth {depth}: {steps} steps through {size} statements and terminators"
        );
    }
}

/// The work of deciding which `make_unique` statements leave a loop grows with the number of
/// arrays it writes, not with its square.
#[test]
fn many_roots_probe_linearly() {
    for roots in [16, 32, 64] {
        let (f, size, steps, probes) = hoisted_with_work(&many_roots(roots));
        let uniques = level_0_uniques(&f);
        assert_eq!(uniques[f.loops[0].pre_header.0], roots, "{roots} roots");
        assert_eq!(uniques.iter().sum::<usize>(), roots, "{roots} roots");
        // Each root's `make_unique` is looked up at least once.
        assert!(probes >= roots, "{roots} roots: {probes} probes");
        assert!(
            probes <= 3 * size,
            "{roots} roots: {probes} probes, size {size}"
        );
        assert!(
            steps <= 2 * size,
            "{roots} roots: {steps} steps, size {size}"
        );
    }
}

/// `f` with `depth` nested loops, each of which writes its own array, and has an empty loop
/// after the loop nested in it.
fn nested_with_siblings(depth: usize) -> String {
    let lets = (0..depth)
        .map(|level| format!("    let mut a{level} = [0]\n"))
        .collect::<Vec<_>>()
        .concat();
    let opens = (0..depth)
        .map(|level| format!("for i{level} in 0..2 {{\n    a{level}[0] = 1\n"))
        .collect::<Vec<_>>()
        .concat();
    let closes = (0..depth)
        .rev()
        .map(|level| format!("for s{level} in 0..2 {{\n}}\n}}\n"))
        .collect::<Vec<_>>()
        .concat();
    format!("fn f() {{\n{lets}{opens}{closes}}}")
}

/// A loop's states start from its largest inner loop's, so the states of the deep loop are
/// not copied into each level's small empty loop: the probes grow with the depth, not with
/// its square.
#[test]
fn sibling_loops_merge_into_the_largest() {
    for depth in [8, 16, 32] {
        let (f, size, _, probes) = hoisted_with_work(&nested_with_siblings(depth));
        // Every array reaches the outermost pre-header.
        let uniques = level_0_uniques(&f);
        assert_eq!(uniques[f.loops[0].pre_header.0], depth, "depth {depth}");
        assert_eq!(uniques.iter().sum::<usize>(), depth, "depth {depth}");
        assert!(
            probes <= 3 * size,
            "depth {depth}: {probes} probes, size {size}"
        );
    }
}

/// `f` with an outer loop and two loops inside it: the first writes `a`, the second bars it
/// with `keep = a`. `pad` locals in the writing loop when `pad_writer`, else in the barring
/// one, make that loop's states the larger.
fn sibling_bars(pad_writer: bool) -> String {
    let pad = (0..8)
        .map(|k| format!("            let p{k} = {k}\n"))
        .collect::<Vec<_>>()
        .concat();
    let (writer_pad, barring_pad) = if pad_writer {
        (pad.as_str(), "")
    } else {
        ("", pad.as_str())
    };
    format!(
        "fn f(xs: [i64]) -> [i64] {{\n    let mut a = xs\n    let mut keep = [0]\n    for i in 0..2 {{\n        for j in 0..2 {{\n            a[j] = 1\n{writer_pad}        }}\n        for k in 0..2 {{\n            keep = a\n{barring_pad}        }}\n    }}\n    print(keep[0])\n    a\n}}"
    )
}

/// A sibling loop that bars `a` bars the outer loop, so the writing loop's `make_unique`
/// stops at its own pre-header, whichever of the two has the larger states.
#[test]
fn a_sibling_loop_bars_the_outer_loop() {
    for pad_writer in [false, true] {
        let (f, _, _, _) = hoisted_with_work(&sibling_bars(pad_writer));
        let uniques = level_0_uniques(&f);
        assert_eq!(
            uniques[f.loops[1].pre_header.0], 1,
            "pad_writer {pad_writer}"
        );
        assert_eq!(uniques.iter().sum::<usize>(), 1, "pad_writer {pad_writer}");
    }
}

/// Loops four deep, with a sibling at the second and third levels. Each of `a` to `h` is
/// written in the innermost loop, and all but `a` and `e` are barred at one level:
///
/// ```text
/// L0  bars d          (loops[0])
///   L1  bars c        (loops[1])
///     L2  bars b      (loops[2])
///       L3  writes a, b, c, d, g, h   (loops[3])
///     L2' bars h      (loops[4])
///   L1' bars g, writes e   (loops[5])
/// ```
const TREE: &str = "fn f(xs: [i64]) {
    let mut a = xs
    let mut b = [0]
    let mut c = [0]
    let mut d = [0]
    let mut e = [0]
    let mut g = [0]
    let mut h = [0]
    let mut kb = [0]
    let mut kc = [0]
    let mut kd = [0]
    let mut kg = [0]
    let mut kh = [0]
    for i in 0..2 {
        kd = d
        for j in 0..2 {
            kc = c
            for k in 0..2 {
                kb = b
                for m in 0..2 {
                    a[m] = 1
                    b[m] = 1
                    c[m] = 1
                    d[m] = 1
                    g[m] = 1
                    h[m] = 1
                }
            }
            for k2 in 0..2 {
                kh = h
            }
        }
        for j2 in 0..2 {
            kg = g
            e[j2] = 1
        }
    }
    print(a[0] + kb[0] + kc[0] + kd[0] + kg[0] + kh[0])
}";

/// Each root stops at the pre-header of the outermost loop around its write that does not bar
/// it, whether the bar is in a loop around the write or in a sibling's subtree.
#[test]
fn roots_stop_at_different_ancestors_of_a_tree() {
    let (f, _, _, _) = hoisted_with_work(TREE);
    // Every `make_unique` in the pre-header, as its local's name and its projection count.
    let at = |lp: usize| -> Vec<(Option<String>, usize)> {
        let mut uniques: Vec<_> = f.blocks[f.loops[lp].pre_header.0]
            .stmts
            .iter()
            .filter_map(|stmt| {
                let Statement::MakeUnique(place) = stmt else {
                    return None;
                };
                Some((
                    f.locals[place.local.0].name.clone(),
                    place.projections.len(),
                ))
            })
            .collect();
        uniques.sort();
        uniques
    };
    let expected: [&[&str]; 6] = [&["a", "e"], &["d", "g"], &["c", "h"], &["b"], &[], &[]];
    for (lp, names) in expected.iter().enumerate() {
        let names: Vec<_> = names
            .iter()
            .map(|name| (Some((*name).to_owned()), 0))
            .collect();
        assert_eq!(at(lp), names, "loops[{lp}]");
    }
    // Each `make_unique` has left the loops.
    assert_eq!(level_0_uniques(&f).iter().sum::<usize>(), 7);
}

/// The `break outer` in the inner loop makes the outer loop's exit, which holds the write after
/// the outer loop. `tests/run/labels.sisu` runs this `f`: were that exit one of the inner
/// loop's blocks, the inner loop would hoist the write's `make_unique` too, and the write would
/// reach `keep`, which shares `a` from the outer body.
const LABELED_BREAK: &str = "fn f(xs: [i64]) -> i64 {\n    let mut a = xs\n    let mut keep = [0]\n    outer: for i in 0..2 {\n        for j in 0..2 {\n            a[j] = i + 1\n            if j == 5 { break outer }\n        }\n        keep = a\n    }\n    a[0] = 9\n    if keep[1] > 0 {\n        return keep[0]\n    }\n    0\n}";

#[test]
fn labeled_break_keeps_outer_exit_out_of_inner_loop() {
    let f = placed_f(LABELED_BREAK);
    let [outer, inner] = f.loops.as_slice() else {
        panic!("the case should have two loops: {:?}", f.loops.len());
    };
    let outside: Vec<_> = inner
        .blocks
        .iter()
        .filter(|block| !outer.blocks.contains(block))
        .collect();
    assert_eq!(outside, Vec::<&BlockId>::new());
    // The inner loop still hoists `a`, so the case reaches the miscompile it guards.
    assert_eq!(
        loops(LABELED_BREAK),
        [Vec::<String>::new(), vec!["a".to_owned()]]
    );
}
