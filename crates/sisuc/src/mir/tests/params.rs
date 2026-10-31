//! Borrowed-parameter inference: a parameter the function does not consume is borrowed, and a
//! call passes it in place. Each test reads the signature line, `x#0: borrow T` or `x#0: T`.

use std::fmt::Write as _;

use super::super::lower::{self, COUNTERS, DEDUPED, SEEDED};
use super::super::params::{FOLLOWED, LOOKED, VISITED, quiet};
use super::*;
use crate::tir::{ClassId, FuncId, Program as TirProgram, TypeArgs, TypeKind};

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

/// `dbg(s)` of a parameter reads it in place, and `len` reads the result in place: `s` stays
/// borrowed, and the function takes no copy.
#[test]
fn dbg_should_keep_a_parameter_borrowed() {
    let printed = built("fn show(s: string) {\n    print(dbg(s).len())\n}");
    signature(&printed, "show", "fn show(s#0: borrow string)");
    let show = function(&printed, "show");
    assert!(!show.contains("copy"), "{show}");
    assert!(!show.contains("release"), "{show}");
}

#[test]
fn parameter_kept_in_a_local_is_owned() {
    signature(&built(KEEP), "keep", "fn keep(p#0: [i64])");
}

/// A push writes the copy `a`, not the parameter: `xs` is owned through the `let mut`, as for any
/// other use of that binding.
#[test]
fn parameter_pushed_through_a_local_should_be_owned() {
    signature(
        &built("fn f(xs: [i64]) -> [i64] {\n    let mut a = xs\n    a.push(1)\n    a\n}"),
        "f",
        "fn f(xs#0: [i64])",
    );
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

/// A spill reads `es` into a temporary that the blocks around a `break` use. Pairs cannot drop a
/// copy whose release is in another block, so the read consumes `es`; `len_of`'s temporary lives
/// in one block and stays an alias.
#[test]
fn temporary_used_in_another_block_consumes_its_local() {
    let printed = built(
        "fn h(a: [i64], b: i64) -> i64 { a.len() + b }\n\
         fn count(es: [i64], c: bool) -> i64 {\n    let mut n = 0\n    for i in 0..3 {\n        n += h(es, if c { break } else { 1 })\n    }\n    n\n}\n\
         fn len_of(es: [i64]) -> i64 { es.len() }",
    );
    signature(&printed, "count", "fn count(es#0: [i64], c#1: bool)");
    signature(&printed, "len_of", "fn len_of(es#0: borrow [i64])");
}

/// The loop reads `xs` in place, so `total` does not consume it and its caller retains nothing.
#[test]
fn for_over_a_parameter_should_borrow_it() {
    let printed = built(
        "fn total(xs: [i64]) -> i64 {\n    let mut s = 0\n    for x in xs { s += x }\n    s\n}\n\
         fn main() {\n    let xs = [1, 2, 3]\n    print(total(xs) + total(xs))\n}",
    );
    signature(&printed, "total", "fn total(xs#0: borrow [i64])");
    let total = function(&printed, "total");
    assert!(!total.contains("release"), "{total}");
    let main = function(&printed, "main");
    assert!(!main.contains("copy"), "{main}");
}

/// The iterable is a field without `mut` of a parameter, which no code can change.
#[test]
fn for_over_a_fixed_field_should_borrow_the_parameter() {
    let printed = built(
        "class Bag {\n    items: [string]\n}\n\
         fn count(b: Bag) -> i64 {\n    let mut n = 0\n    for w in b.items { n += w.len() }\n    n\n}",
    );
    signature(&printed, "count", "fn count(b#0: borrow Bag)");
    let count = function(&printed, "count");
    assert!(!count.contains("copy b#0.0"), "{count}");
}

/// The body writes `ys`, so the loop must visit a copy of it. A `mut` field of a class
/// parameter can change too, and a `for` over it reads a copy.
#[test]
fn for_over_a_place_that_can_change_should_read_a_copy() {
    let printed = built(
        "class Bag {\n    mut items: [string]\n}\n\
         fn doubled(xs: [i64]) -> i64 {\n    let mut ys = xs\n    let mut s = 0\n    for y in ys {\n        ys[0] = y + 1\n        s += y\n    }\n    s\n}\n\
         fn drain(b: Bag) -> i64 {\n    let mut n = 0\n    for w in b.items {\n        b.items = []\n        n += w.len()\n    }\n    n\n}",
    );
    signature(&printed, "doubled", "fn doubled(xs#0: [i64])");
    has(&printed, "doubled", &["_ = copy ys#"]);
    signature(&printed, "drain", "fn drain(b#0: borrow Bag)");
    has(&printed, "drain", &["= copy b#0.0"]);
}

/// The inner `match` tests the binding `x` of the outer arm in place, so `e` is not taken apart.
#[test]
fn match_on_a_binding_of_a_match_on_a_parameter_should_borrow_it() {
    let printed = built(
        "enum C {\n    C1(string)\n    C2\n}\nenum E {\n    B(C)\n    A\n}\n\
         fn d(e: E) -> i64 {\n    match e {\n        B(x) => match x {\n            C1(_) => 1\n            _ => 2\n        }\n        _ => 0\n    }\n}",
    );
    signature(&printed, "d", "fn d(e#0: borrow E)");
    let d = function(&printed, "d");
    assert!(!d.contains("take_apart") && !d.contains("release"), "{d}");
}

/// A parameter that the function consumes is owned, so the binding is not borrowed and the
/// inner `match` takes its scrutinee apart as before.
#[test]
fn match_on_a_binding_of_a_match_on_an_owned_parameter_should_take_it_apart() {
    let printed = built(
        "enum C {\n    C1(string)\n    C2\n}\nenum E {\n    B(C)\n    A\n}\n\
         fn d(e: E) -> E {\n    match e {\n        B(x) => match x {\n            C1(_) => E.A\n            _ => e\n        }\n        _ => e\n    }\n}",
    );
    signature(&printed, "d", "fn d(e#0: E)");
    has(&printed, "d", &["take_apart"]);
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

const INTERP: &str = include_str!("../../../../../bench/features/interp/main.sisu");

/// `simplify` and `simplify_stmt` take an owned tree apart and rebuild the nodes that do not
/// fold, so the rebuilt `Add` and `Seq` go into the boxes their arms took apart.
#[test]
fn interp_simplify_should_reuse_the_boxes_it_takes_apart() {
    let printed = built(INTERP);
    signature(&printed, "simplify", "fn simplify(e#0: Expr)");
    has(
        &printed,
        "simplify",
        &["= variant Expr.Add(move _, move _) into [_, _]"],
    );
    signature(&printed, "simplify_stmt", "fn simplify_stmt(s#0: Stmt)");
    has(
        &printed,
        "simplify_stmt",
        &["= variant Stmt.Seq(move _, move _) into [_, _]"],
    );
}

#[test]
fn interp_evaluator_and_count_should_borrow_the_tree() {
    let printed = built(INTERP);
    signature(&printed, "eval", "fn eval(e#0: borrow Expr");
    signature(&printed, "run", "fn run(s#0: borrow Stmt");
    signature(&printed, "count", "fn count(s#0: borrow Stmt)");
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
/// parameters marked in `reuses`. The front end recurses once per nested `match`, so it runs on
/// a stack of the compiler's own size; the counters are that thread's.
fn seed_counts(src: &str) -> (usize, usize, usize, Vec<bool>) {
    let src = src.to_string();
    std::thread::Builder::new()
        .stack_size(crate::STACK_SIZE)
        .spawn(move || {
            let program = checked(&src);
            let id = (program.functions.iter())
                .position(|f| f.name == "f")
                .expect("the program should define `f`");
            let borrowed = vec![true; program.functions[id].params.len()];
            SEEDED.set(0);
            DEDUPED.set(0);
            let (_, _, reuses) =
                lower::function(&program, FuncId(id), &borrowed, &mut Written::default());
            (SEEDED.get(), DEDUPED.get(), COUNTERS.get(), reuses)
        })
        .expect("the thread should start")
        .join()
        .expect("the front end and lowering should not overflow the stack")
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

/// Each instantiation is a mir function of its own, so inference gives each its own flags: a
/// `string` that `peek` only reads is borrowed, and one that `keep` stores is owned.
#[test]
fn instantiations_should_infer_their_own_modes() {
    let printed = built(
        "fn peek<T>(x: T) -> i64 { 0 }\nfn keep<T>(x: T) -> [T] { [x] }\n\
         fn main() {\n    print(peek(\"s\"))\n    print(peek(1))\n    print(keep(\"s\").len())\n    print(keep(1).len())\n}",
    );
    signature(
        &printed,
        "peek<string>",
        "fn peek<string>(x#0: borrow string)",
    );
    signature(&printed, "peek<i64>", "fn peek<i64>(x#0: i64)");
    signature(&printed, "keep<string>", "fn keep<string>(x#0: string)");
}

/// A conformance method is a function like any other once `mono` has resolved the interface call
/// to it, so inference gives each its own mode: `Circle.area` only reads `self.r` and borrows
/// `self`, and `Square.area` binds `self` to a local first, which makes it owned.
#[test]
fn conformance_methods_should_infer_their_own_modes() {
    let printed = built(
        "interface Shape {\n    fn area(self) -> f64\n}\n\
         class Circle: Shape {\n    r: f64\n    fn area(self) -> f64 { self.r }\n}\n\
         class Square {\n    side: f64\n}\n\
         extend Square: Shape {\n    fn area(self) -> f64 {\n        let s = self\n        s.side\n    }\n}\n\
         fn total<T: Shape>(x: T) -> f64 {\n    x.area()\n}\n\
         fn main() {\n    print(total(Circle(r: 1.0)))\n    print(total(Square(side: 2.0)))\n}",
    );
    has(&printed, "total<Circle>", &["call Circle.area("]);
    has(&printed, "total<Square>", &["call Square.Shape.area("]);
    signature(
        &printed,
        "Circle.area",
        "fn Circle.area(self#0: borrow Circle)",
    );
    signature(
        &printed,
        "Square.Shape.area",
        "fn Square.Shape.area(self#0: Square)",
    );
}

/// `c1 > c2` on a written `Ord` is the call `Circle.less(c2, c1)`: `less` keeps neither operand
/// (ADR 0029), so both stay borrowed and neither is copied.
#[test]
fn written_less_should_borrow_both_operands() {
    let printed = built(
        "class Circle: Eq, Ord {\n    r: i64\n    fn eq(self, other: Self) -> bool { self.r == other.r }\n    fn less(self, other: Self) -> bool { self.r < other.r }\n}\n\
         fn main() {\n    let c1 = Circle(r: 1)\n    let c2 = Circle(r: 2)\n    let more = c1 > c2\n    print(more)\n    print(c1.r + c2.r)\n}",
    );
    signature(
        &printed,
        "Circle.less",
        "fn Circle.less(self#0: borrow Circle, other#1: borrow Circle)",
    );
    let f = function(&printed, "main");
    assert_eq!(f.matches("= call Circle.less(").count(), 1, "{f}");
    assert!(f.contains("call Circle.less(c2#1, c1#0)"), "{f}");
    assert!(!f.contains("= copy c"), "{f}");
}

/// `sum` of the prelude loops over `self`, which a `for` reads in place: `self` is borrowed, and
/// a caller that sums the same array twice copies it nowhere.
#[test]
fn the_prelude_sum_should_borrow_its_array() {
    let printed = built(
        "fn main() {\n    let xs = [1, 2]\n    print(xs.sum() + xs.sum())\n    print([1.5].sum())\n}",
    );
    signature(&printed, "[i64].sum", "fn [i64].sum(self#0: borrow [i64])");
    signature(&printed, "[f64].sum", "fn [f64].sum(self#0: borrow [f64])");
    let main = function(&printed, "main");
    assert!(!main.contains("copy"), "{main}");
}

/// A function of `name` that does `n = 1`, calls each of `callees` and then runs `extra`: a
/// prelude function when `prelude`.
fn quiet_candidate(
    name: &str,
    prelude: bool,
    callees: &[usize],
    extra: Vec<Statement>,
) -> Function {
    let mut b = Builder::new(name);
    if prelude {
        b.prelude();
    }
    let n = b.local("n", Type::I64);
    b.local("s", Type::STRING);
    let mut stmts = vec![assign(
        n,
        Rvalue::Use(Operand::Const(Const::Int(1, Type::I64))),
    )];
    stmts.extend(callees.iter().map(|&func| Statement::Assign {
        dest: None,
        value: Rvalue::Call {
            func: FuncId(func),
            args: Vec::new(),
            borrowed: Vec::new(),
            span: Span::new(0, 0),
            quiet: false,
        },
    }));
    stmts.extend(extra);
    b.block(stmts, ret());
    b.finish()
}

/// The field of a local, as a place that a `take` or a store reads.
fn slot() -> Place {
    Place {
        projections: vec![Projection::Field {
            index: 0,
            mutable: true,
        }],
        ..whole(LocalId(1))
    }
}

/// `dest = take slot`.
fn take(dest: LocalId) -> Statement {
    Statement::Take {
        dest,
        place: slot(),
    }
}

/// `slot = value`.
fn store(value: Operand) -> Statement {
    Statement::Store {
        place: slot(),
        value,
    }
}

/// The call flags of `f`, in order.
fn call_flags(f: &Function) -> Vec<bool> {
    (f.blocks.iter().flat_map(|block| &block.stmts))
        .filter_map(|stmt| match stmt {
            Statement::Assign {
                value: Rvalue::Call { quiet, .. },
                ..
            } => Some(*quiet),
            _ => None,
        })
        .collect()
}

/// A chain of prelude functions, each calling the next, is `n` components, and a ring of them is
/// one: both mark every function quiet and visit each statement once. A function's component is
/// read once to find the members, and twice for each call. The sizes grow by two, so a visit or a
/// search for the members that repeats for a component would grow faster than the functions.
#[test]
fn quiet_should_visit_each_statement_once() {
    for n in [20, 40, 80] {
        let chain = (0..n).map(|i| {
            let next = if i + 1 < n { vec![i + 1] } else { vec![] };
            quiet_candidate(&format!("f{i}"), true, &next, Vec::new())
        });
        let ring =
            (0..n).map(|i| quiet_candidate(&format!("f{i}"), true, &[(i + 1) % n], Vec::new()));
        for (shape, mut functions) in [
            ("chain", chain.collect::<Vec<_>>()),
            ("ring", ring.collect::<Vec<_>>()),
        ] {
            let statements: usize = functions.iter().map(|f| f.blocks[0].stmts.len()).sum();
            VISITED.set(0);
            LOOKED.set(0);
            quiet(&mut functions);
            assert!(functions.iter().all(|f| f.quiet), "{shape} of {n}");
            assert!(
                functions.iter().all(|f| call_flags(f).iter().all(|&q| q)),
                "{shape} of {n}: each call takes its callee's flag"
            );
            assert_eq!(VISITED.get(), statements, "{shape} of {n}");
            assert!(
                LOOKED.get() <= 3 * n,
                "{shape} of {n}: {} reads",
                LOOKED.get()
            );
        }
    }
}

/// A function is quiet when it is the prelude's and holds no clobber but calls of quiet
/// functions. A user function never is, and a function that calls one that is not, or that is
/// in a component with one, is not either.
#[test]
fn quiet_should_mark_only_prelude_functions_without_a_clobber() {
    let clobber = || vec![Statement::MakeUnique(whole(LocalId(0)))];
    // (rows, expected quiet flags of the functions, in order)
    let rows: Vec<(&str, Vec<Function>, Vec<bool>)> = vec![
        (
            "a clobber",
            vec![quiet_candidate("a", true, &[], clobber())],
            vec![false],
        ),
        (
            "a caller of a clobber",
            vec![
                quiet_candidate("a", true, &[1], Vec::new()),
                quiet_candidate("b", true, &[], clobber()),
            ],
            vec![false, false],
        ),
        (
            "a ring with one clobber",
            vec![
                quiet_candidate("a", true, &[1], Vec::new()),
                quiet_candidate("b", true, &[2], clobber()),
                quiet_candidate("c", true, &[0], Vec::new()),
            ],
            vec![false, false, false],
        ),
        (
            "a user function",
            vec![
                quiet_candidate("a", false, &[1], Vec::new()),
                quiet_candidate("b", true, &[], Vec::new()),
            ],
            vec![false, true],
        ),
        (
            "a caller of a user function",
            vec![
                quiet_candidate("a", true, &[1], Vec::new()),
                quiet_candidate("b", false, &[], Vec::new()),
            ],
            vec![false, false],
        ),
        (
            "a quiet callee beside a loud one",
            vec![
                quiet_candidate("a", true, &[1, 2], Vec::new()),
                quiet_candidate("b", true, &[], Vec::new()),
                quiet_candidate("c", true, &[], clobber()),
            ],
            vec![false, true, false],
        ),
    ];
    assert_quiet_rows(rows);
}

/// These statements are clobbers: a call that is not quiet, a take into a counted local, a store
/// of a counted value, a `make_unique` and a `push`. The other statements leave a prelude
/// function quiet.
#[test]
fn quiet_should_judge_each_kind_of_statement() {
    let rows: Vec<(&str, Vec<Function>, Vec<bool>)> = vec![
        (
            "a take into a counted local",
            vec![quiet_candidate("a", true, &[], vec![take(LocalId(1))])],
            vec![false],
        ),
        (
            "a take into an uncounted local",
            vec![quiet_candidate("a", true, &[], vec![take(LocalId(0))])],
            vec![true],
        ),
        (
            "a store of a counted value",
            vec![quiet_candidate(
                "a",
                true,
                &[],
                vec![store(Operand::Move(LocalId(1)))],
            )],
            vec![false],
        ),
        (
            "a store of an uncounted value",
            vec![quiet_candidate(
                "a",
                true,
                &[],
                vec![store(Operand::Const(Const::Int(2, Type::I64)))],
            )],
            vec![true],
        ),
        (
            "a push onto a local",
            vec![quiet_candidate(
                "a",
                true,
                &[],
                vec![Statement::Push {
                    place: whole(LocalId(1)),
                    value: Operand::Const(Const::Int(2, Type::I64)),
                    at: Span::new(0, 0),
                }],
            )],
            vec![false],
        ),
        (
            "statements that free or end nothing the caller reads",
            vec![quiet_candidate(
                "a",
                true,
                &[],
                vec![
                    Statement::Release(LocalId(1)),
                    Statement::ScopeRelease(LocalId(1)),
                    Statement::Check(whole(LocalId(1))),
                    Statement::End(LocalId(1)),
                    Statement::Free(LocalId(1)),
                    Statement::TakeApart {
                        scrutinee: LocalId(1),
                        tokens: Vec::new(),
                        bindings: Vec::new(),
                        rest: Vec::new(),
                    },
                ],
            )],
            vec![true],
        ),
    ];
    assert_quiet_rows(rows);
}

/// Runs `quiet` over each row's functions: the flags are `expected`, and each call takes the
/// flag of its callee.
fn assert_quiet_rows(rows: Vec<(&str, Vec<Function>, Vec<bool>)>) {
    for (row, mut functions, expected) in rows {
        quiet(&mut functions);
        let found: Vec<bool> = functions.iter().map(|f| f.quiet).collect();
        assert_eq!(found, expected, "row `{row}`");
        let callee = |at: usize| functions[at].quiet;
        for (at, f) in functions.iter().enumerate() {
            let calls = call_flags(f);
            let callees: Vec<usize> = (f.blocks.iter().flat_map(|block| &block.stmts))
                .filter_map(|stmt| match stmt {
                    Statement::Assign {
                        value: Rvalue::Call { func, .. },
                        ..
                    } => Some(func.0),
                    _ => None,
                })
                .collect();
            let wanted: Vec<bool> = callees.into_iter().map(callee).collect();
            assert_eq!(
                calls, wanted,
                "row `{row}`, fn {at}: each call takes its callee's flag"
            );
        }
    }
}

/// The debug check of `build` rejects a call whose flag is not the flag of its callee.
#[test]
#[should_panic(expected = "should carry its callee's quiet flag")]
fn quiet_check_should_reject_a_call_that_disagrees_with_its_callee() {
    let mut functions = [
        quiet_candidate("a", true, &[1], Vec::new()),
        quiet_candidate("b", true, &[], Vec::new()),
    ];
    // `b` is quiet, and the call in `a` still says that it is not.
    functions[1].quiet = true;
    quiet_checked(&functions);
}

/// The debug check of `build` rejects a quiet function that holds a clobber.
#[test]
#[should_panic(expected = "should hold no clobber but a call of a quiet function")]
fn quiet_check_should_reject_a_quiet_function_with_a_clobber() {
    let clobber = vec![Statement::MakeUnique(whole(LocalId(0)))];
    let mut functions = [quiet_candidate("a", true, &[], clobber)];
    functions[0].quiet = true;
    quiet_checked(&functions);
}

/// A function whose only clobber the ownership pass adds is not quiet. Lowering never makes
/// one, for it pairs a counted store with a counted `take`, so the mir is by hand: `s` is stored
/// into a `mut` field with a `use`, which is no counted store until `place` makes it a move, and
/// the `take` before it reads into an `i64` local, which is no clobber. `finish` runs `place`
/// before `quiet`; a `quiet` that ran first would mark `put`.
#[test]
fn quiet_should_see_a_clobber_that_the_ownership_pass_adds() {
    let bag = Type::intern(TypeKind::Class(ClassId(0), TypeArgs::NONE));
    let put = || {
        let mut b = Builder::new("put");
        b.prelude();
        let this = b.param("bag", bag);
        let s = b.param("s", Type::STRING);
        b.borrowed(this);
        let old = b.local("old", Type::I64);
        let field = Place {
            projections: vec![Projection::Field {
                index: 0,
                mutable: true,
            }],
            ..whole(this)
        };
        b.block(
            vec![
                Statement::Take {
                    dest: old,
                    place: field.clone(),
                },
                Statement::Store {
                    place: field,
                    value: Operand::Use(s),
                },
            ],
            ret(),
        );
        b.finish()
    };
    let mut before = [put()];
    quiet(&mut before);
    assert!(before[0].quiet, "no clobber before the ownership pass");
    let mut after = [put()];
    finish(&mut after, &TirProgram::default());
    assert!(!after[0].quiet, "the counted store is a clobber after it");
}

/// A class whose written `eq` replaces a `mut` field, and one whose `eq` stores its argument.
const EQ_CLASSES: &str = "class Holder: Eq {\n    mut items: [string]\n    fn eq(self, other: Self) -> bool {\n        self.items = [\"y\"]\n        true\n    }\n}\n\
    class Keeper: Eq {\n    mut last: Keeper?\n    name: string\n    fn eq(self, other: Self) -> bool {\n        self.last = other\n        true\n    }\n}\n";

/// `contains` of the prelude loops over `self`, which a `for` reads in place, so `self` is
/// borrowed. Its `x` is decided for each instance: a generated `Eq` reads it in place, so it is
/// borrowed for a `string`; the written `eq` of `Keeper` stores `other`, so it is owned there.
#[test]
fn the_prelude_contains_should_borrow_its_array_and_decide_its_value_for_each_instance() {
    let printed = built(&format!(
        "{EQ_CLASSES}fn main() {{\n    let ks = [Keeper(last: None, name: \"k{{1}}\")]\n    \
         print(ks.contains(ks[0]))\n    print([\"a\"].contains(\"b\"))\n    print([1].contains(1))\n}}"
    ));
    signature(
        &printed,
        "[string].contains",
        "fn [string].contains(self#0: borrow [string], x#1: borrow string)",
    );
    signature(
        &printed,
        "[i64].contains",
        "fn [i64].contains(self#0: borrow [i64], x#1: i64)",
    );
    signature(
        &printed,
        "[Keeper].contains",
        "fn [Keeper].contains(self#0: borrow [Keeper], x#1: Keeper)",
    );
}

/// A function of the user's that passes its `string` parameters to `contains<string>` keeps both
/// borrowed: the call reads them in place.
#[test]
fn a_function_that_calls_contains_should_keep_its_parameters_borrowed() {
    let printed = built(
        "fn has(xs: [string], s: string) -> bool { xs.contains(s) }\nfn main() {\n    print(has([\"a\"], \"b\"))\n}",
    );
    signature(
        &printed,
        "has",
        "fn has(xs#0: borrow [string], s#1: borrow string)",
    );
}

/// The `==` in the body of `contains<T>` is an `Equal` whose `calls` flag follows the instance:
/// it prints ` [calls]` for `[Holder]`, whose generated `Eq` reaches the written `eq` of `Holder`,
/// and not for `string`. For `Holder` itself the `==` is a `Call` of its `eq`, which has no marker.
#[test]
fn the_equal_in_contains_should_carry_the_calls_flag_of_its_instance() {
    let printed = built(&format!(
        "{EQ_CLASSES}fn main() {{\n    let h = Holder(items: [\"{{1}}\"])\n    print([[h]].contains([h]))\n    \
         print([h].contains(h))\n    print([\"a\"].contains(\"b\"))\n}}"
    ));
    let nested = function(&printed, "[[Holder]].contains");
    assert!(
        nested.contains(" == ") && nested.contains(" [calls]"),
        "{nested}"
    );
    let strings = function(&printed, "[string].contains");
    assert!(
        strings.contains(" == ") && !strings.contains("[calls]"),
        "{strings}"
    );
    let holder = function(&printed, "[Holder].contains");
    assert!(holder.contains("= call Holder.eq("), "{holder}");
    assert!(
        !holder.contains(" == ") && !holder.contains("[calls]"),
        "{holder}"
    );
}
