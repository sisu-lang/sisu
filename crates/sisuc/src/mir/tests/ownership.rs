//! The ownership pass: each `use`, borrow site and guard mark becomes a move, a copy or an
//! in-place read by liveness. The ported tests make the decision `last_use.rs` makes on `tir`.

use super::*;
use crate::mir::liveness::CONSULTED;

/// How often `part` occurs in function `name` of `printed`, temporaries normalized.
fn count(printed: &str, name: &str, part: &str) -> usize {
    function(printed, name).matches(part).count()
}

/// The first block of `f`, a function as `function` gives it, that holds every one of `parts`.
fn block_with<'f>(f: &'f str, parts: &[&str]) -> &'f str {
    f.split("  bb")
        .find(|block| parts.iter().all(|part| block.contains(part)))
        .unwrap_or_else(|| panic!("no block holds {parts:?} in\n{f}"))
}

#[test]
fn assign_from_own_call() {
    let printed = built(
        "fn f(x: [i64]) -> [i64] { x }\nfn main() {\n    let mut x = [1]\n    x = f(x)\n    print(x[0])\n}",
    );
    has(
        &printed,
        "main",
        &[
            "call f(move x#0)",
            "release? x#0",
            "x#0 = move _",
            "_ = move x#0",
            "= _[0]",
        ],
    );
    has(&printed, "f", &["= move x#0"]);
}

#[test]
fn two_reads_in_one_call() {
    let printed = built("fn g(a: [i64], b: [i64]) {}\nfn f(x: [i64]) { g(x, x) }");
    has(&printed, "f", &["_ = copy x#0", "call g(move _, move x#0)"]);
}

/// `Contains` evaluates its base, then its value: only the value's read may move.
#[test]
fn contains_base_then_value() {
    let printed = built("fn f(xs: [[i64]]) -> bool { xs.contains(xs[0]) }");
    has(
        &printed,
        "f",
        &[
            "_ = copy xs#0",
            "_ = move xs#0",
            "= copy _[0]",
            "contains(move _, move _)",
        ],
    );
}

/// A variant's arguments are evaluated left to right: only the last one's read may move.
#[test]
fn variant_args_left_to_right() {
    let printed = built("enum E { V(string, string) }\nfn f(s: string) -> E { E.V(s, s) }");
    has(
        &printed,
        "f",
        &["_ = copy s#0", "variant E.V(move _, move s#0)"],
    );
}

/// `Interpolate` evaluates its parts left to right: only the last one's read may move.
#[test]
fn interpolate_parts_left_to_right() {
    let printed = built("fn f(s: string) -> string { \"{s}{s}\" }");
    has(&printed, "f", &["_ = copy s#0", "interp(move _, move s#0)"]);
}

/// `Concat` evaluates its left operand, then its right: only the right one's read may move.
#[test]
fn concat_lhs_then_rhs() {
    let printed = built("fn f(s: string) -> string { s + s }");
    has(&printed, "f", &["_ = copy s#0", "concat(move _, move s#0)"]);
}

/// A comparison chain evaluates its operands in order, so only the last read moves.
#[test]
fn comparison_chain_reads() {
    let printed = built("fn f(xs: [i64]) -> bool { 0 < xs.len() }");
    has(&printed, "f", &["_ = move xs#0", "_ = len _", "0 < _"]);
    let printed = built("fn f(xs: [i64]) -> bool { 0 < xs.len() < xs.len() }");
    has(
        &printed,
        "f",
        &["_ = len xs#0", "_ = move xs#0", "_ = len _"],
    );
    assert_eq!(count(&printed, "f", "move xs#0"), 1, "{printed}");
}

#[test]
fn read_in_loop_body() {
    let printed =
        built("fn f(xs: [i64], c: bool) {\n    while c {\n        print(xs.len())\n    }\n}");
    has(&printed, "f", &["_ = len xs#0"]);
    assert!(!printed.contains("move xs"), "{printed}");
}

#[test]
fn read_before_return() {
    let printed = built(
        "fn f(xs: [i64], c: bool) -> i64 {\n    if c {\n        return xs.len()\n    }\n    xs.len()\n}",
    );
    assert_eq!(count(&printed, "f", "move xs#0"), 2, "{printed}");
}

#[test]
fn read_before_panic() {
    let printed = built(
        "fn f(xs: [i64], c: bool) -> i64 {\n    if c {\n        panic(\"{xs.len()}\")\n    }\n    xs.len()\n}",
    );
    assert_eq!(count(&printed, "f", "move xs#0"), 2, "{printed}");
}

#[test]
fn read_in_branch_then_after() {
    let printed = built(
        "fn f(xs: [i64], c: bool) -> i64 {\n    if c {\n        print(xs.len())\n    }\n    xs.len()\n}",
    );
    has(
        &printed,
        "f",
        &["_ = len xs#0", "_ = move xs#0", "_ = len _"],
    );
    assert_eq!(count(&printed, "f", "move xs#0"), 1, "{printed}");
}

#[test]
fn read_after_continue() {
    let printed = built(
        "fn f(c: bool) {\n    let mut i = 0\n    while i < 3 {\n        i += 1\n        let ys = [i]\n        if c {\n            continue\n        }\n        print(ys.len())\n    }\n}",
    );
    has(&printed, "f", &["_ = move ys#", "_ = len _"]);
}

/// An element write uses its root after the value and never moves it, so no read before
/// it is a last use.
#[test]
fn element_write_roots() {
    let printed = built(
        "fn shared() {\n    let mut a = [1, 2]\n    let b = a\n    a[0] = 9\n}\n\
         fn compound() {\n    let mut xs = [1]\n    xs[0] += 1\n}\n\
         fn during_for() {\n    let mut xs = [1]\n    for x in xs { xs[0] = 1 }\n}\n\
         fn reads_len() {\n    let mut xs = [1]\n    xs[0] = xs.len()\n}\n\
         fn g(v: [i64]) -> i64 { 0 }\n\
         fn passes() {\n    let mut xs = [1]\n    xs[0] = g(xs)\n}",
    );
    has(
        &printed,
        "shared",
        &["b#1 = copy a#0", "make_unique a#0", "a#0[0] = 9"],
    );
    has(
        &printed,
        "compound",
        &["_ = xs#0[_]", "make_unique xs#0", "xs#0[_] = _"],
    );
    has(
        &printed,
        "during_for",
        &["_ = copy xs#0", "make_unique xs#0", "xs#0[0] = 1"],
    );
    has(
        &printed,
        "reads_len",
        &["_ = len xs#0", "make_unique xs#0", "xs#0[0] = _"],
    );
    has(
        &printed,
        "passes",
        &["_ = copy xs#0", "call g(move _)", "make_unique xs#0"],
    );
    assert!(!printed.contains("move a"), "{printed}");
    assert!(!printed.contains("move xs"), "{printed}");
}

/// An assignment in the operand a short circuit skips does not end liveness on the path
/// that skips it.
#[test]
fn short_circuits() {
    let printed = built(
        "fn or() {\n    let mut xs = [1]\n    let ok = xs.len() > 0 || if true {\n        xs = [2]\n        true\n    } else {\n        xs = [3]\n        true\n    }\n    print(xs[0])\n}\n\
         fn and() {\n    let mut xs = [1]\n    let ok = xs.len() > 0 && if true {\n        xs = [2]\n        true\n    } else {\n        xs = [3]\n        true\n    }\n    print(xs[0])\n}\n\
         fn chain() {\n    let mut xs = [1]\n    let ok = 0 < xs.len() < if true {\n        xs = [2]\n        5\n    } else {\n        xs = [3]\n        5\n    }\n    print(xs[0])\n}",
    );
    for name in ["or", "and", "chain"] {
        has(&printed, name, &["_ = len xs#0"]);
    }
}

#[test]
fn objects_self_and_if_let() {
    let printed = built(
        "class N {\n    v: i64\n    fn get(self) -> i64 { self.v }\n}\nfn f(m: N?) -> i64 {\n    if let n = m { n.v } else { 0 }\n}",
    );
    has(&printed, "N.get", &["_ = move self#0", "= _.0"]);
    has(&printed, "f", &["_ = move n#1", "= _.0"]);
}

/// A field assignment's base, a field root's base and an element write's index are reads
/// like any other: the last of them moves.
#[test]
fn place_operands() {
    let printed = built(
        "class O {\n    mut n: i64\n}\nclass B {\n    mut xs: [i64]\n}\n\
         fn field(o: O) {\n    print(o.n)\n    o.n = 1\n}\n\
         fn root(b: B) {\n    print(b.xs.len())\n    b.xs[0] = 1\n}\n\
         fn index(xs: [i64]) {\n    let mut ys = [1, 2]\n    print(xs.len())\n    ys[xs.len() - 1] = 5\n}",
    );
    has(
        &printed,
        "field",
        &["_ = o#0.0", "_ = move o#0", "_.0 = 1", "release _"],
    );
    has(
        &printed,
        "root",
        &["_ = copy b#0.0", "_ = move b#0", "check _.0[0]"],
    );
    has(
        &printed,
        "index",
        &["_ = len xs#0", "_ = move xs#0", "_ = len _"],
    );
}

/// An `Append` evaluates as the assignment to its place: a field root's base before the
/// value, and an element's root local after the value, never moved.
#[test]
fn append_order() {
    let printed = built(
        "class P {\n    mut name: string\n}\nfn field(p: P) {\n    p.name = p.name + p.name\n}\nfn element() {\n    let mut xs = [\"a\"]\n    xs[0] = xs[0] + xs[0]\n}",
    );
    has(
        &printed,
        "field",
        &["_ = copy p#0", "_ = move p#0", "= copy _.0"],
    );
    assert_eq!(count(&printed, "field", "move p#0"), 1, "{printed}");
    assert!(!printed.contains("move xs"), "{printed}");
}

/// `break` leaves the loop with the set live after it, so a read before the loop stays live.
#[test]
fn read_after_break() {
    let printed = built(
        "fn f(xs: [i64], c: bool) {\n    print(xs.len())\n    while c {\n        break\n    }\n    print(xs.len())\n}",
    );
    has(
        &printed,
        "f",
        &["_ = len xs#0", "_ = move xs#0", "_ = len _"],
    );
    assert_eq!(count(&printed, "f", "move xs#0"), 1, "{printed}");
}

/// A read in an inner loop stays live into both loops' next iterations.
#[test]
fn nested_loops() {
    let printed = built(
        "fn f(xs: [i64], c: bool) {\n    while c {\n        while c {\n            print(xs.len())\n        }\n    }\n    print(xs.len())\n}",
    );
    has(
        &printed,
        "f",
        &["_ = len xs#0", "_ = move xs#0", "_ = len _"],
    );
    assert_eq!(count(&printed, "f", "move xs#0"), 1, "{printed}");
}

/// A last use inside an inner loop moves: rewriting reaches every loop depth.
#[test]
fn move_in_inner_loop() {
    let printed = built(
        "fn f(xs: [i64], c: bool) {\n    while c {\n        while c {\n            print(xs.len())\n            return\n        }\n    }\n}",
    );
    has(&printed, "f", &["_ = move xs#0", "_ = len _"]);
}

/// A guard's reads: a binding the guard reads is copied, a guard that can run once per
/// alternative is a loop body, and its `break` or `continue` targets the source loop.
#[test]
fn guard_reads() {
    // `s` is the arm's binding: the guard's `return s` copies it.
    let printed = built(
        "enum N {\n    Name(string)\n    Other\n}\nfn f(n: N) -> string {\n    match n {\n        Name(s) if (if s == \"\" { return s } else { true }) => s\n        _ => \"x\"\n    }\n}",
    );
    let f = function(&printed, "f");
    let returning = block_with(&f, &["end s#1", "return move _"]);
    assert!(returning.contains("_ = copy s#1"), "{returning}");
    assert!(!returning.contains("move s#1"), "{returning}");
    // An outer `t`, read only in a guard: it can run again with an `Or`, so it never moves;
    // without one, it moves when no later arm reads it.
    let decls = "enum E {\n    V(i64)\n    W(i64)\n    Z\n}\nfn keep(t: string) -> bool { true }\n";
    let guarded = |src: String, parts: &[&str]| has(&built(&format!("{decls}{src}")), "f", parts);
    guarded(
        "fn f(e: E, t: string) -> i64 {\n    match e {\n        V(_) | W(_) if keep(t) => 1\n        _ => 0\n    }\n}".to_owned(),
        &["_ = copy t#1", "call keep(move _)"],
    );
    guarded(
        "fn f(e: E, t: string) -> i64 {\n    match e {\n        V(_) if keep(t) => 1\n        _ => 0\n    }\n}".to_owned(),
        &["call keep(move t#1)"],
    );
    // A later arm that reads `t` keeps it live on the guard's failure edge.
    guarded(
        "fn f(e: E, t: string) -> i64 {\n    match e {\n        V(_) if keep(t) => 1\n        _ => if keep(t) { 2 } else { 0 }\n    }\n}".to_owned(),
        &["_ = copy t#1", "call keep(move _)", "call keep(move t#1)"],
    );
    // The first arm writes `u`, but the guard's failure edge reaches the last arm, which
    // leaves `u` alone for the read after the `match`; the second arm also writes it.
    guarded(
        "fn f(e: E, t: string) -> string {\n    let mut u = t\n    let _ = match e {\n        V(_) if keep(u) => {\n            u = \"y\"\n            1\n        }\n        W(_) => {\n            u = \"z\"\n            2\n        }\n        _ => 0\n    }\n    u\n}".to_owned(),
        &["_ = copy u#2", "call keep(move _)"],
    );
    // When every later arm writes `u`, the guard's read is its last.
    guarded(
        "fn f(e: E, t: string) -> string {\n    let mut u = t\n    let _ = match e {\n        V(_) if keep(u) => {\n            u = \"y\"\n            1\n        }\n        _ => {\n            u = \"z\"\n            0\n        }\n    }\n    u\n}".to_owned(),
        &["call keep(move u#2)"],
    );
    // An `|` inside a variant runs the guard again too.
    guarded(
        "fn f(e: E, t: string) -> i64 {\n    match e {\n        V(1 | 2) if keep(t) => 1\n        _ => 0\n    }\n}".to_owned(),
        &["_ = copy t#1", "call keep(move _)"],
    );
    // A `return` leaves nothing live, even in a guard that runs again.
    guarded(
        "fn f(e: E, c: bool, t: string) -> string {\n    match e {\n        V(_) | W(_) if (if c { return t } else { true }) => \"a\"\n        _ => \"b\"\n    }\n}".to_owned(),
        &["_ = move t#2", "return move _"],
    );
    // The guard's body reads `t` as well, so `t` is live at the guard already.
    guarded(
        "fn f(e: E, t: string) -> i64 {\n    match e {\n        V(_) if keep(t) => if keep(t) { 1 } else { 2 }\n        _ => if keep(t) { 3 } else { 0 }\n    }\n}".to_owned(),
        &["_ = copy t#1", "call keep(move _)"],
    );
    // Only the guarded arm writes `u`, so the failure edge, through arms that leave `u`
    // alone, keeps it live for the read after the `match`.
    guarded(
        "fn f(e: E, t: string) -> string {\n    let mut u = t\n    let _ = match e {\n        V(_) if keep(u) => {\n            u = \"y\"\n            1\n        }\n        _ => 0\n    }\n    u\n}".to_owned(),
        &["_ = copy u#2", "call keep(move _)"],
    );
    // Every arm after the guarded one writes `u`, so an arm before it changes nothing.
    guarded(
        "fn f(e: E, t: string) -> string {\n    let mut u = t\n    let _ = match e {\n        Z => 2\n        V(_) if keep(u) => {\n            u = \"y\"\n            1\n        }\n        _ => {\n            u = \"z\"\n            0\n        }\n    }\n    u\n}".to_owned(),
        &["call keep(move u#2)"],
    );
    // `break` and `continue` in a guard reach the `while`, whose exit and head read `t`;
    // every other path from the first `print(t)` returns, so only the guard's jump keeps
    // `t` live there.
    for jump in ["break", "continue"] {
        has(
            &built(&format!(
                "enum AB {{\n    A\n    B\n    C\n}}\nfn f(e: AB, c: bool, t: string) {{\n    while c {{\n        print(t)\n        let _ = match e {{\n            A | B if (if c {{ {jump} }} else {{ true }}) => 0\n            _ => 1\n        }}\n        return\n    }}\n    print(t)\n}}"
            )),
            "f",
            &["_ = copy t#2", "call print(_)"],
        );
    }
}

#[test]
fn scalars_never_move() {
    let printed = built("fn f(n: i64) -> i64 { n }");
    has(&printed, "f", &["= n#0", "return _"]);
    assert!(!function(&printed, "f").contains("move"), "{printed}");
}

#[test]
fn borrow_site_at_last_use_moves_and_releases() {
    let printed = built(
        "fn f(a: [i64]) -> i64 { a.len() }\n\
         fn g(a: [i64]) -> i64 {\n    let n = a.len()\n    a[0] + n\n}",
    );
    has(&printed, "f", &["_ = move a#0", "_ = len _", "release _"]);
    has(&printed, "g", &["n#1 = len a#0"]);
}

/// Review Focus 1: a guard binding read at a borrow site on a path that returns from the
/// guard is read in place, though nothing reads it after the `return`.
#[test]
fn guard_borrow_site_on_a_returning_path() {
    let printed = built(
        "enum E {\n    S(string)\n    T(string)\n}\n\
         fn f(e: E, c: bool) -> i64 {\n    match e {\n        S(x) | T(x) if if c { return x.len() } else { false } => 1\n        _ => 0\n    }\n}",
    );
    let f = function(&printed, "f");
    let returning = block_with(&f, &["end x#2", "return _"]);
    let (len, ret) = (returning.find("_ = len x#2"), returning.find("return _"));
    assert!(
        matches!((len, ret), (Some(len), Some(ret)) if len < ret),
        "{returning}"
    );
    assert!(!returning.contains("copy x#2"), "{returning}");
    assert!(!f.contains("move x#2"), "{f}");
    assert_eq!(f.matches("copy x#2").count(), 1, "{f}");
    assert!(f.contains("x#2 = copy x#2"), "{f}");
}

/// Review Focus 4: a guard that passes its binding to an owned parameter copies it on every
/// alternative, though the body never reads the binding.
#[test]
fn guard_use_of_binding_never_moves() {
    let printed = built(
        "enum E {\n    S(string)\n    T(string)\n    U\n}\n\
         fn keep(s: string) -> bool { true }\n\
         fn f(e: E) -> i64 {\n    match e {\n        S(x) | T(x) if keep(x) => 0\n        _ => 1\n    }\n}",
    );
    let f = function(&printed, "f");
    let lines: Vec<&str> = f.lines().collect();
    let calls: Vec<usize> = (1..lines.len())
        .filter(|&i| lines[i].contains("call keep("))
        .collect();
    assert!(!calls.is_empty(), "{f}");
    for i in calls {
        assert!(lines[i - 1].ends_with("_ = copy x#1"), "{f}");
    }
    assert!(!f.contains("move x#"), "{f}");
}

/// The guard mark on a `use`: the guard's `return x` copies the borrowed binding, though
/// nothing reads it after the `return`.
#[test]
fn guard_return_of_binding_copies() {
    let printed = built(
        "enum E {\n    S(string)\n    T(string)\n    U\n}\n\
         fn f(e: E, c: bool) -> string {\n    match e {\n        S(x) | T(x) if if c { return x } else { false } => \"\"\n        _ => \"u\"\n    }\n}",
    );
    has(&printed, "f", &["_ = copy x#2", "return move _"]);
    assert!(!function(&printed, "f").contains("move x#"), "{printed}");
}

/// A write through a projection uses its root, so the call before it copies `a`.
#[test]
fn projection_write_is_a_use() {
    let printed = built(
        "fn h(xs: [i64]) -> i64 {\n    xs[0]\n}\n\
         fn k(xs: [i64]) {\n    let mut a = xs\n    print(h(a))\n    a[0] = 1\n}",
    );
    has(&printed, "k", &["_ = copy a#1", "call h(move _)"]);
    assert!(!function(&printed, "k").contains("move a#1"), "{printed}");
}

/// A counted field store at its base's last use moves the base once, before the `take` of the
/// old value: the `take` and the store read the one moved root.
#[test]
fn field_store_take_and_store_share_one_root() {
    let printed =
        built("class O {\n    mut s: string\n}\nfn f(o: O, name: string) {\n    o.s = name\n}");
    has(
        &printed,
        "f",
        &[
            "_ = move o#0",
            "_ = take _.0",
            "_.0 = move name#1",
            "release _",
        ],
    );
    assert_eq!(count(&printed, "f", "o#0."), 0, "{printed}");
}

/// A panic reads its message in the terminator, so an earlier read copies it.
#[test]
fn panic_reads_its_message() {
    let printed = built("fn die(m: string) {\n    print(m)\n    panic(m)\n}");
    has(
        &printed,
        "die",
        &["_ = copy m#0", "call print(_)", "panic move m#0"],
    );
}

/// After the pass no `use`, guard mark or borrow-site flag is left, in a function with every
/// kind of node and in a guard that reads a binding both ways; lowering left each of them.
#[test]
fn pass_leaves_no_use_or_mark() {
    let guard = "enum E {\n    S(string)\n    T(string)\n}\nfn keep(s: string) -> bool { true }\n\
                 fn f(e: E) -> i64 {\n    match e {\n        S(x) | T(x) if keep(x) && x.len() > 0 => 0\n        _ => 1\n    }\n}";
    let marks = ["Use(LocalId(", "Guarded(", "borrow: true", "guard: true"];
    let mut before = Vec::new();
    for src in [EVERY_KIND, guard] {
        let program = checked(src);
        before.push(format!("{:?}", lower(&program).functions));
        let after = format!("{:?}", build(&program).functions);
        for mark in marks {
            assert!(!after.contains(mark), "`{mark}` left in\n{after}");
        }
    }
    for mark in marks {
        assert!(
            before.iter().any(|lowered| lowered.contains(mark)),
            "lowering left no `{mark}`"
        );
    }
}

/// Liveness reads its entries (mentions, block stamps, answers) a bounded number of times per
/// statement and per edge, never a search of a local's mentions per question: for `n` borrow
/// sites of `a` in a row, and for `n` under `if`s, the reads grow by the same amount from `n`
/// to `2n` as from `2n` to `3n`.
#[test]
fn liveness_reads_grow_linearly() {
    let line = |n: usize| format!("fn f(a: [i64]) {{\n{}}}", "    print(a.len())\n".repeat(n));
    let branch = |n: usize| {
        format!(
            "fn g(a: [i64], c: bool) {{\n{}}}",
            "    if c { print(a.len()) }\n".repeat(n)
        )
    };
    for shape in [&line as &dyn Fn(usize) -> String, &branch] {
        let reads = |n: usize| {
            let program = checked(&shape(n));
            CONSULTED.set(0);
            let printed = print(&build(&program), &program);
            assert_eq!(printed.matches("len a#").count(), n - 1, "{printed}");
            CONSULTED.get()
        };
        let n = 100;
        let (one, two, three) = (reads(n), reads(2 * n), reads(3 * n));
        assert_eq!(
            three - two,
            two - one,
            "{one}, {two}, {three} reads for {n}, {}, {}",
            2 * n,
            3 * n
        );
    }
}
