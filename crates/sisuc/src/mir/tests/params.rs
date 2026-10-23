//! Borrowed-parameter inference: a parameter the function does not consume is borrowed, and a
//! call passes it in place. Each test reads the signature line, `x#0: borrow T` or `x#0: T`.

use std::fmt::Write as _;

use super::super::lower::{self, COUNTERS, DEDUPED, SEEDED};
use super::super::params::FOLLOWED;
use super::*;
use crate::tir::FuncId;

const KEEP: &str = "fn keep(p: [i64]) -> [i64] {\n    let q = p\n    q\n}";

const SIZE: &str = "fn size(xs: [i64]) -> i64 {\n    xs.len()\n}";

const SLEN: &str = "fn slen(s: string) -> i64 {\n    s.len()\n}";

const EXPR: &str = "enum Expr {\n    Num(i64)\n    Add(Expr, Expr)\n    Mul(Expr, Expr)\n}";

/// The `f`/`g` pair of the tail rule: `f` hands `g` a fresh string in tail position.
const HAND_OVER: &str = "fn f(n: i64) -> i64 {\n    if n == 0 {\n        0\n    } else {\n        g(\"x{n}\", n - 1)\n    }\n}\n\
    fn g(s: string, n: i64) -> i64 {\n    if n == 0 {\n        s.len()\n    } else {\n        f(n - 1)\n    }\n}";

/// Asserts that function `name` of `printed` has the signature that starts with `head`.
fn signature(printed: &str, name: &str, head: &str) {
    let f = function(printed, name);
    assert!(f.starts_with(head), "`{head}` in\n{f}");
}

#[test]
fn read_only_parameter_is_borrowed() {
    let printed = built(
        "fn name() -> string { \"n\" }\n\
         fn len(s: string) -> i64 {\n    print(s)\n    s.len()\n}\n\
         fn main() {\n    print(len(name()))\n}",
    );
    signature(&printed, "len", "fn len(s#0: borrow string)");
    let len = function(&printed, "len");
    assert!(!len.contains("release s#0"), "{len}");
    assert!(!len.contains("release? s#0"), "{len}");
    has(&printed, "main", &["= call len(_)\n    release _"]);
}

#[test]
fn parameter_kept_in_a_local_is_owned() {
    signature(&built(KEEP), "keep", "fn keep(p#0: [i64])");
}

/// The binding is only read, so only the named-assignment rule makes `p` owned.
#[test]
fn parameter_bound_to_a_local_is_owned() {
    let printed = built("fn bound(p: [i64]) -> i64 {\n    let q = p\n    q.len()\n}");
    signature(&printed, "bound", "fn bound(p#0: [i64])");
}

#[test]
fn stored_in_a_variant_parameter_is_owned() {
    let printed = built(&format!(
        "{EXPR}\nfn wrap_e(e: Expr) -> Expr {{ Expr.Add(e, Expr.Num(1)) }}"
    ));
    signature(&printed, "wrap_e", "fn wrap_e(e#0: Expr)");
}

#[test]
fn stored_parameter_is_owned() {
    let printed = built("class B {\n    s: string\n}\nfn wrap(s: string) -> B { B(s: s) }");
    signature(&printed, "wrap", "fn wrap(s#0: string)");
}

#[test]
fn returned_parameter_is_owned() {
    signature(
        &built("fn id(s: string) -> string { s }"),
        "id",
        "fn id(s#0: string)",
    );
}

/// `pass` hands `xs` to `keep`, which owns it; `look` hands it to `size`, which borrows it, in
/// place.
#[test]
fn parameter_passed_to_an_owned_parameter_is_owned() {
    let printed = built(&format!(
        "{KEEP}\n{SIZE}\n\
         fn pass(xs: [i64]) -> [i64] {{ keep(xs) }}\n\
         fn look(xs: [i64]) -> i64 {{ size(xs) }}"
    ));
    signature(&printed, "pass", "fn pass(xs#0: [i64])");
    signature(&printed, "look", "fn look(xs#0: borrow [i64])");
    let look = function(&printed, "look");
    assert!(look.contains("call size(xs#0)"), "{look}");
    assert!(!look.contains("copy"), "{look}");
}

/// Review Focus 4: `a` stores `p`, and `b` only passes it to `a`, so both own it. The named
/// assignment after `a`'s call keeps it out of tail position: only the call edge carries
/// ownership from `b` to `a`. In the control group, nobody consumes `p`.
#[test]
fn ownership_flows_through_a_recursive_group() {
    let printed = built(
        "class B {\n    s: string\n}\n\
         fn a(p: string, n: i64) -> B {\n    if n == 0 {\n        B(s: p)\n    } else {\n        let r = b(p, n - 1)\n        r\n    }\n}\n\
         fn b(p: string, n: i64) -> B {\n    a(p, n)\n}\n\
         fn c(p: string, n: i64) -> i64 {\n    if n == 0 {\n        p.len()\n    } else {\n        let r = d(p, n - 1)\n        r\n    }\n}\n\
         fn d(p: string, n: i64) -> i64 {\n    c(p, n)\n}",
    );
    signature(&printed, "a", "fn a(p#0: string");
    signature(&printed, "b", "fn b(p#0: string");
    signature(&printed, "c", "fn c(p#0: borrow string");
    signature(&printed, "d", "fn d(p#0: borrow string");
}

/// The tail rule: `walk` hands its own parameter a fresh string, so it owns it. `walk2` adds to
/// the result, so its call is not in tail position.
#[test]
fn tail_call_with_a_fresh_argument_makes_the_parameter_owned() {
    let printed = built(
        "fn walk(s: string, n: i64) -> i64 {\n    if n == 0 {\n        s.len()\n    } else {\n        walk(\"x{n}\", n - 1)\n    }\n}\n\
         fn walk2(s: string, n: i64) -> i64 {\n    if n == 0 {\n        s.len()\n    } else {\n        walk2(\"x{n}\", n - 1) + 1\n    }\n}",
    );
    signature(&printed, "walk", "fn walk(s#0: string");
    signature(&printed, "walk2", "fn walk2(s#0: borrow string");
}

/// A `loop {}` beside a tail call lowers to one self-looping block with no exit; the
/// tail rule must still see through it.
#[test]
fn tail_call_beside_a_closed_loop_keeps_the_tail_rule() {
    let printed = built(
        "fn walk(s: string, n: i64) -> i64 {\n    if n < 0 {\n        loop {}\n    }\n    if n == 0 {\n        s.len()\n    } else {\n        walk(\"x{n}\", n - 1)\n    }\n}",
    );
    signature(&printed, "walk", "fn walk(s#0: string");
}

/// The tail rule for a borrowed binding: `leftmost` passes `a`, which borrows from `e`, to its
/// own borrowed parameter in tail position, so `e` stays borrowed.
#[test]
fn tail_call_with_a_borrowed_binding_keeps_the_parameter_borrowed() {
    let printed = built(&format!(
        "{EXPR}\nfn leftmost(e: Expr) -> i64 {{\n    match e {{\n        Add(a, b) => leftmost(a)\n        Mul(a, b) => leftmost(a)\n        Num(n) => n\n    }}\n}}"
    ));
    signature(&printed, "leftmost", "fn leftmost(e#0: borrow Expr");
}

/// The tail rule's other side: `f` owns `e` (it hands `e` to `own`) and passes `a`, which
/// borrows from `e`, to `g` in tail position, so `g`'s `x` is owned: `f` hands `a`'s reference
/// over instead of releasing `e` after the call.
#[test]
fn tail_call_from_an_owned_root_hands_a_borrowed_binding_over() {
    let printed = built(&format!(
        "{EXPR}\nfn own(e: Expr) -> Expr {{\n    e\n}}\n\
         fn f(e: Expr, k: i64) -> i64 {{\n    if k == 0 {{\n        let o = own(e)\n        return 0\n    }}\n    match e {{\n        Add(a, _) => g(a, k - 1)\n        Mul(a, _) => g(a, k - 1)\n        Num(n) => n\n    }}\n}}\n\
         fn g(x: Expr, k: i64) -> i64 {{\n    f(Expr.Num(k), k)\n}}"
    ));
    signature(&printed, "f", "fn f(e#0: Expr");
    signature(&printed, "g", "fn g(x#0: Expr");
}

/// With `g`'s `s` owned, `f` hands its fresh string over and has nothing left to release. With
/// `+ 0`, the call is not in tail position: `s` is borrowed, and `f` releases the string.
#[test]
fn tail_rule_lets_the_caller_hand_over_its_argument() {
    let printed = built(HAND_OVER);
    signature(&printed, "g", "fn g(s#0: string");
    let f = function(&printed, "f");
    let call = f.find("call g(").expect("`f` should call `g`");
    let to_return = &f[call..call + f[call..].find("return").expect("a return after the call")];
    assert!(!to_return.contains("release"), "{f}");

    let control = built(&HAND_OVER.replace("g(\"x{n}\", n - 1)", "g(\"x{n}\", n - 1) + 0"));
    signature(&control, "g", "fn g(s#0: borrow string");
    has(&control, "f", &["= call g(", "\n    release _"]);
}

/// `concat` consumes its operand; no tail call is involved.
#[test]
fn concat_operand_is_owned() {
    let printed = built("fn grow(s: string) -> i64 {\n    let t = s + \"x\"\n    t.len()\n}");
    signature(&printed, "grow", "fn grow(s#0: string)");
}

/// Lowering pins `s` into a temporary, since the second argument reads it; the tail rule sees
/// the parameter through it.
#[test]
fn pinned_parameter_in_a_tail_call_stays_borrowed() {
    let printed = built(
        "fn walk(s: string, n: i64) -> i64 {\n    if n == 0 {\n        s.len()\n    } else {\n        walk(s, s.len() + n - 1)\n    }\n}",
    );
    signature(&printed, "walk", "fn walk(s#0: borrow string");
}

/// `f`'s committed binding is owned, not borrowed: it is a copy, so `g` must own it.
#[test]
fn tail_rule_skips_a_committed_binding() {
    let g = &HAND_OVER[HAND_OVER
        .find("fn g")
        .expect("`HAND_OVER` should define `g`")..];
    let printed = built(&format!(
        "enum E {{\n    V(string)\n}}\n\
         fn f(n: i64) -> i64 {{\n    match E.V(\"x{{n}}\") {{\n        V(s) => g(s, n)\n    }}\n}}\n{g}"
    ));
    signature(&printed, "g", "fn g(s#0: string");
}

/// Only the tail rule makes `s` owned: `walk` otherwise only prints it.
#[test]
fn unit_tail_call_makes_the_parameter_owned() {
    let printed = built(
        "fn walk(s: string, n: i64) {\n    if n > 0 {\n        walk(\"x{n}\", n - 1)\n    } else {\n        print(s)\n    }\n}",
    );
    signature(&printed, "walk", "fn walk(s#0: string");
}

/// Work after the call, in its block (`walk`) or in a block it reaches (`stroll`), means the call
/// is not in tail position, so passing a fresh string leaves `s` borrowed.
#[test]
fn call_followed_by_work_is_not_a_tail_call() {
    let printed = built(
        "fn walk(s: string, n: i64) {\n    if n > 0 {\n        walk(\"x{n}\", n - 1)\n        print(n)\n    } else {\n        print(s)\n    }\n}\n\
         fn stroll(s: string, n: i64) {\n    if n > 0 {\n        stroll(\"x{n}\", n - 1)\n    } else {\n        print(s)\n    }\n    print(n)\n}",
    );
    signature(&printed, "walk", "fn walk(s#0: borrow string");
    signature(&printed, "stroll", "fn stroll(s#0: borrow string");
}

/// A `for` loop reads `es` into a temporary that the loop's blocks use and its exit releases.
/// Pairs cannot drop a copy whose release is in another block, so the read consumes `es`;
/// `len_of`'s temporary lives in one block and stays an alias.
#[test]
fn temporary_used_in_another_block_consumes_its_local() {
    let printed = built(
        "fn count(es: [i64]) -> i64 {\n    let mut n = 0\n    for e in es {\n        n += e\n    }\n    n\n}\n\
         fn len_of(es: [i64]) -> i64 { es.len() }",
    );
    signature(&printed, "count", "fn count(es#0: [i64])");
    signature(&printed, "len_of", "fn len_of(es#0: borrow [i64])");
}

/// An `if` expression's result is assigned on each path, so each assignment consumes.
#[test]
fn join_result_makes_the_parameter_owned() {
    let printed = built(
        "fn pick_s(p: string, q: string, c: bool) {\n    print(if c { p } else { q })\n}\n\
         fn len_of(p: string) -> i64 { p.len() }",
    );
    signature(&printed, "pick_s", "fn pick_s(p#0: string, q#1: string");
    signature(&printed, "len_of", "fn len_of(p#0: borrow string)");
}

/// Review Focus 1: the C entry point passes `main` an owned array and releases nothing after.
#[test]
fn main_keeps_its_parameter_owned() {
    let printed = built("fn main(args: [string]) {\n    print(args.len())\n}");
    signature(&printed, "main", "fn main(args#0: [string])");
}

/// `xs`'s last use is the call: it moves into a temporary that the caller releases after.
#[test]
fn last_use_passed_to_a_borrowed_parameter_is_released_after_the_call() {
    let printed = built(&format!(
        "{SIZE}\nfn f() -> i64 {{\n    let xs = [1]\n    size(xs)\n}}"
    ));
    has(
        &printed,
        "f",
        &["_ = move xs#0", "\n    _ = call size(_)\n    release _"],
    );
}

/// A field that is not `mut` is stable: the copy and its release pair, so `f` borrows it.
#[test]
fn fixed_field_passed_to_a_borrowed_parameter_is_borrowed() {
    let printed = built(&format!(
        "class W {{\n    s: string\n}}\n{SLEN}\nfn f(w: W) -> i64 {{ slen(w.s) }}"
    ));
    has(
        &printed,
        "f",
        &["_ = borrow w#0.0", "call slen(_)", "\n    end _"],
    );
    let f = function(&printed, "f");
    assert!(!f.contains("copy"), "{f}");
}

/// Review Focus 3: a call may replace a `mut` field, so its value is copied before the call.
#[test]
fn mut_field_passed_to_a_borrowed_parameter_is_copied() {
    let printed = built(&format!(
        "class W {{\n    mut s: string\n}}\n{SLEN}\nfn f(w: W) -> i64 {{ slen(w.s) }}"
    ));
    has(
        &printed,
        "f",
        &["_ = copy w#0.0", "call slen(_)", "\n    release _"],
    );
}

/// Review Focus 3: a call may replace an element, so it is copied before the call.
#[test]
fn element_passed_to_a_borrowed_parameter_is_copied() {
    let printed = built(&format!(
        "{SLEN}\nfn f(ws: [string]) -> i64 {{ slen(ws[0]) }}"
    ));
    has(
        &printed,
        "f",
        &["copy ws#0[0]", "call slen(_)", "\n    release _"],
    );
}

/// The caller's releases after each call to a borrowed parameter still elaborate: none is left
/// conditional.
#[test]
fn calls_to_borrowed_parameters_keep_elaborated_releases() {
    let printed = built(&format!(
        "{SIZE}\nfn twice(c: bool) -> i64 {{\n    let xs = [1]\n    if c {{\n        return size(xs)\n    }}\n    size(xs) + size(xs)\n}}"
    ));
    signature(&printed, "size", "fn size(xs#0: borrow [i64])");
    let twice = function(&printed, "twice");
    assert!(!twice.contains("release?"), "{twice}");
}

/// One search covers the whole program: around a cycle of `n` functions, each passing its
/// parameter to the next, the last's named local makes every parameter owned, and the edges
/// the search follows grow with `n`, not with `n` times the cycle's length.
#[test]
fn infer_visits_each_edge_once() {
    let followed = |n: usize| {
        let mut chain = String::new();
        for i in 0..n - 1 {
            write!(
                chain,
                "fn h{i}(p: string, k: i64) -> i64 {{\n    if k == 0 {{\n        0\n    }} else {{\n        h{}(p, k - 1) + 1\n    }}\n}}\n",
                i + 1
            )
            .expect("writing to a `String` should not fail");
        }
        let src = format!(
            "{chain}fn h{}(p: string, k: i64) -> i64 {{\n    let q = p\n    h0(q, k) + 1\n}}",
            n - 1
        );
        let program = checked(&src);
        FOLLOWED.set(0);
        let printed = print(&build(&program), &program);
        let count = FOLLOWED.get();
        for i in 0..n {
            signature(&printed, &format!("h{i}"), &format!("fn h{i}(p#0: string"));
        }
        count
    };
    let (fifty, two_hundred) = (followed(50), followed(200));
    assert!(
        two_hundred <= 5 * fifty,
        "{fifty} edges for 50, {two_hundred} for 200"
    );
}

const TREE_REWRITE: &str = include_str!("../../../../../bench/features/tree_rewrite/main.sisu");

/// Part 4 alone infers `simplify`'s `e` borrowed: `e` reaches only a tail call to itself and
/// calls whose results go into new boxes. Its `Add(l, r)` arm takes two boxes apart and builds
/// an `Add`, so the seed keeps `e` owned, and the `Add` is built into the arm's two tokens.
#[test]
fn parameter_whose_box_is_reused_stays_owned() {
    let printed = built(TREE_REWRITE);
    signature(&printed, "simplify", "fn simplify(e#0: Expr)");
    has(
        &printed,
        "simplify",
        &["= variant Expr.Add(move _, move _) into [_, _]"],
    );
    let printed = built(include_str!("../../../tests/run/enum_alloc_rewrite.sisu"));
    signature(&printed, "swap", "fn swap(e#0: Expr)");
    // `Cons` has one boxed field, after an inline one, and `xs` is the second parameter.
    let printed = built(
        "enum L {\n    Nil\n    Cons(i64, L)\n}\n\
         fn bump(k: i64, xs: L) -> L {\n    match xs {\n        Cons(h, t) => L.Cons(h + k, L.Nil)\n        Nil => L.Nil\n    }\n}",
    );
    signature(&printed, "bump", "fn bump(k#0: i64, xs#1: L)");
    has(
        &printed,
        "bump",
        &["= variant L.Cons(_, move _) into [-, _]"],
    );
}

/// `weigh` and `eval` test `e`'s boxes, but no arm builds a box, so `e` stays borrowed. In
/// `split`, the arms that test boxes build none, and the arm that builds one tests none. In
/// `other`, the arm that tests boxes of `Expr` builds a box of `G`.
#[test]
fn parameter_only_read_stays_borrowed() {
    signature(&built(TREE_REWRITE), "weigh", "fn weigh(e#0: borrow Expr)");
    let printed = built(include_str!(
        "../../../../../bench/features/enum_match/main.sisu"
    ));
    signature(&printed, "eval", "fn eval(e#0: borrow Expr)");
    let printed = built(&format!(
        "{EXPR}\nenum G {{\n    Leaf\n    Node(G)\n}}\n\
         fn split(e: Expr) -> Expr {{\n    match e {{\n        Add(l, r) => Expr.Num(1)\n        Num(n) => Expr.Add(Expr.Num(n), Expr.Num(0))\n        Mul(l, r) => Expr.Num(2)\n    }}\n}}\n\
         fn other(e: Expr) -> G {{\n    match e {{\n        Add(l, r) => G.Node(G.Leaf)\n        _ => G.Leaf\n    }}\n}}"
    ));
    signature(&printed, "split", "fn split(e#0: borrow Expr)");
    signature(&printed, "other", "fn other(e#0: borrow Expr)");
}

/// The arm of `REBUILT` takes two boxes apart and builds a box of their enum.
const REBUILT: &str = "match e {\n        Add(l, r) => Expr.Add(Expr.Num(1), Expr.Num(2))\n        _ => Expr.Num(0)\n    }";

/// `rebuilt` matches its parameter, so the seed keeps it owned. `Holder.rebuilt` matches a field
/// of `self`, not `self` itself: taking `self.e` apart would not free `self`, which stays
/// borrowed.
#[test]
fn match_on_a_field_is_not_seeded() {
    let printed = built(&format!(
        "{EXPR}\nfn rebuilt(e: Expr) -> Expr {{\n    {REBUILT}\n}}\n\
         class Holder {{\n    e: Expr\n\n    fn rebuilt(self) -> Expr {{\n        {}\n    }}\n}}",
        REBUILT.replace("match e", "match self.e")
    ));
    signature(&printed, "rebuilt", "fn rebuilt(e#0: Expr)");
    signature(
        &printed,
        "Holder.rebuilt",
        "fn Holder.rebuilt(self#0: borrow Holder)",
    );
}

/// The `match` rebuilds a box, but `rebuilt` reads `e` after it, so the `match` is not its last
/// use: nothing is reused and `e` stays borrowed, and `main` lends `t` in place.
#[test]
fn parameter_read_after_the_match_should_stay_borrowed() {
    let printed = built(&format!(
        "{EXPR}\nfn size(e: Expr) -> i64 {{\n    match e {{\n        Num(_) => 1\n        \
         Add(l, r) => size(l) + size(r) + 1\n        Mul(l, r) => size(l) + size(r) + 1\n    }}\n}}\n\
         fn rebuilt(e: Expr) -> i64 {{\n    let x = {REBUILT}\n    size(x) + size(e)\n}}\n\
         fn main() {{\n    let t = Expr.Add(Expr.Num(1), Expr.Num(2))\n    \
         print(rebuilt(t) + size(t))\n}}"
    ));
    signature(&printed, "rebuilt", "fn rebuilt(e#0: borrow Expr)");
    has(&printed, "main", &["call rebuilt(t#0)"]);
}

/// A guard and another arm read `e` after the scrutinee, so each keeps `e` borrowed.
#[test]
fn parameter_read_in_a_guard_or_another_arm_should_stay_borrowed() {
    let size = "fn size(e: Expr) -> i64 {\n    match e {\n        Num(_) => 1\n        \
                Add(l, r) => size(l) + size(r) + 1\n        Mul(l, r) => size(l) + size(r) + 1\n    }\n}";
    let guard = REBUILT.replace("Add(l, r) =>", "Add(l, r) if size(e) > 1 =>");
    let other = REBUILT.replace("_ => Expr.Num(0)", "_ => Expr.Num(size(e))");
    for arms in [guard, other] {
        let printed = built(&format!(
            "{EXPR}\n{size}\nfn rebuilt(e: Expr) -> Expr {{\n    {arms}\n}}"
        ));
        signature(&printed, "rebuilt", "fn rebuilt(e#0: borrow Expr)");
    }
}

/// The loop runs the `match` on `e` again, so the `match` is not the last use of `e`.
#[test]
fn parameter_matched_in_a_loop_should_stay_borrowed() {
    let printed = built(&format!(
        "{EXPR}\nfn rebuilt(e: Expr) -> Expr {{\n    let mut acc = Expr.Num(0)\n    \
         for _ in 0..3 {{\n        acc = {REBUILT}\n    }}\n    acc\n}}"
    ));
    signature(&printed, "rebuilt", "fn rebuilt(e#0: borrow Expr)");
}

/// Lowers `f` of `src` with every parameter borrowed: `SEEDED`, `DEDUPED`, `COUNTERS` and the
/// parameters marked in `reuses`.
fn seed_counts(src: &str) -> (usize, usize, usize, Vec<bool>) {
    let program = checked(src);
    let id = (program.functions.iter())
        .position(|f| f.name == "f")
        .expect("the program should define `f`");
    let borrowed = vec![true; program.functions[id].params.len()];
    SEEDED.set(0);
    DEDUPED.set(0);
    let (_, _, reuses) = lower::function(&program, FuncId(id), &borrowed);
    (SEEDED.get(), DEDUPED.get(), COUNTERS.get(), reuses)
}

/// Each construction writes one counter and each arm on a parameter reads one twice, so `n`
/// nested arms, each open around all `n` constructions, cost `n + 2 × n`, not `n × n`. Unrelated
/// enum declarations add no counter. A construction and an arm over `n` distinct boxed enums
/// each ask once per enum whether they have seen it, not once per pair.
#[test]
fn seed_bookkeeping_is_linear() {
    let n = 40;
    let params: Vec<String> = (0..n).map(|i| format!("e{i}: Expr")).collect();
    let mut lets = String::new();
    for i in 0..n {
        writeln!(lets, "let b{i} = Expr.Add(Expr.Num({i}), Expr.Num(0))")
            .expect("writing to a `String` should not fail");
    }
    let mut body = format!("{{\n{lets}Expr.Num(0)\n}}");
    for i in (0..n).rev() {
        body = format!("match e{i} {{\n    Add(l{i}, r{i}) => {body}\n    _ => Expr.Num({i})\n}}");
    }
    let nested = format!("fn f({}) -> Expr {{\n{body}\n}}", params.join(", "));
    let (seeded, _, counters, reuses) = seed_counts(&format!("{EXPR}\n{nested}"));
    assert_eq!(seeded, n + 2 * n);
    assert_eq!(counters, 1);
    assert_eq!(reuses, vec![true; n]);

    let (mut unrelated, mut enums) = (String::new(), String::new());
    for i in 0..n {
        writeln!(unrelated, "enum U{i} {{\n    A\n}}")
            .expect("writing to a `String` should not fail");
        writeln!(enums, "enum F{i} {{\n    Nil\n    Back(Root)\n}}")
            .expect("writing to a `String` should not fail");
    }
    let (seeded, _, counters, _) = seed_counts(&format!("{unrelated}{EXPR}\n{nested}"));
    assert_eq!((seeded, counters), (n + 2 * n, 1));

    let fs: Vec<String> = (0..n).map(|i| format!("F{i}")).collect();
    let bindings: Vec<String> = (0..n).map(|i| format!("a{i}")).collect();
    let nils: Vec<String> = (0..n).map(|i| format!("F{i}.Nil")).collect();
    let wide = format!(
        "enum Root {{\n    Nil\n    All({})\n}}\n{enums}\
         fn f(r: Root) -> Root {{\n    match r {{\n        All({}) => Root.All({})\n        Nil => Root.Nil\n    }}\n}}",
        fs.join(", "),
        bindings.join(", "),
        nils.join(", ")
    );
    let (_, deduped, counters, reuses) = seed_counts(&wide);
    assert_eq!((deduped, counters), (2 * n, n));
    assert_eq!(reuses, vec![true]);
}
