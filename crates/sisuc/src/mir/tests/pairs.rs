//! Pair removal: a copy that is only read within its block, and then released there, becomes a
//! borrow, and its release becomes `end`s. Each `_kept` test is a copy that must stay.

use super::*;

const REPEATS: &str = "fn repeats() -> i64 {\n    let xs = [\"a\", \"b\", \"a\"]\n    let mut n = 0\n    for i in 0..xs.len() {\n        for j in 0..i {\n            if xs[i] == xs[j] { n += 1 }\n        }\n    }\n    n\n}\n\
    fn main() {\n    print(repeats())\n}";

const NUDGE: &str = "class P {\n    mut x: f64\n}\n\
    fn nudge() {\n    let ps = [P(x: 1.0)]\n    for j in 0..ps.len() {\n        let p = ps[j]\n        p.x = p.x + 1.0\n        print(p.x)\n    }\n}";

const BOX: &str = "class Box {\n    mut s: string\n}\n";

/// `compare`'s inner loop: both elements are borrowed, so the comparison retains nothing.
#[test]
fn elements_compared_in_a_loop_are_borrowed() {
    let f = function(&built(REPEATS), "repeats");
    assert_eq!(f.matches("= borrow xs#0[").count(), 2, "{f}");
    assert!(!f.contains("= copy xs#0["), "{f}");
    assert_eq!(f.matches("\n    end _").count(), 2, "{f}");
    // Only the array's release is left.
    assert_eq!(f.matches("release ").count(), 1, "{f}");
}

/// A `for` element moved into a temporary is a chain: both are borrowed and end last first. The
/// key is borrowed too. The copy of the field stays: its root is an open member when it is copied.
#[test]
fn field_compared_with_a_key_is_borrowed() {
    let printed = built(
        "class Entry {\n    name: string\n}\n\
         fn count(es: [Entry]) -> i64 {\n    let key = \"k\"\n    let mut n = 0\n    for e in es { if e.name == key { n += 1 } }\n    n\n}",
    );
    has(
        &printed,
        "count",
        &[
            "e#5 = borrow es#0[_]",
            "_ = borrow e#5",
            "_ = copy _.0",
            "end _\n    end e#5",
            "_ = borrow key#1",
            "end _",
        ],
    );
    let f = function(&printed, "count");
    assert!(!f.contains("e#5 = copy"), "{f}");
    assert!(!f.contains("= copy key#1"), "{f}");
    assert_eq!(f.matches("\n    end ").count(), 3, "{f}");
    // The field copy's and the key's own releases are left.
    assert_eq!(f.matches("release ").count(), 2, "{f}");
}

/// The `f64` store through the copy `_7` is not a clobber, so the chain `p`, `_11` around it is
/// borrowed. `owners` maps both members straight to the array.
#[test]
fn chain_with_f64_stores_is_borrowed() {
    let printed = built(NUDGE);
    has(
        &printed,
        "nudge",
        &[
            "p#4 = borrow ps#0[j#3]",
            "_ = copy p#4",
            "_ = borrow p#4",
            "end _\n    end p#4",
        ],
    );
    let f = function(&printed, "nudge");
    assert!(!f.contains("= copy ps#0["), "{f}");
    assert_eq!(f.matches("\n    end ").count(), 2, "{f}");
    // The copy `_7`'s and the array's releases are left.
    assert_eq!(f.matches("release ").count(), 2, "{f}");

    let mir = build(&checked(NUDGE));
    let nudge = mir
        .functions
        .iter()
        .find(|f| f.name == "nudge")
        .expect("the program should have `nudge`");
    let (ps, p) = (LocalId(0), LocalId(4));
    let chained = nudge
        .blocks
        .iter()
        .flat_map(|block| &block.stmts)
        .find_map(|stmt| {
            if let Statement::Assign {
                dest: Some(dest),
                value: Rvalue::Borrow(place),
            } = stmt
                && place.local == p
            {
                Some(*dest)
            } else {
                None
            }
        })
        .expect("the chain step should borrow `p`");
    let mut owners: Vec<_> = nudge.owners.iter().map(|(&k, &v)| (k, v)).collect();
    owners.sort_by_key(|&(k, _)| k.0);
    assert_eq!(owners, [(p, ps), (chained, ps)]);
}

/// Review Focus 3: the index changes right after the copy; the borrow reads the element once.
#[test]
fn for_loop_element_is_borrowed() {
    let printed = built(
        "fn long_words(ws: [string]) -> i64 {\n    let mut n = 0\n    for w in ws { if w.len() > 4 { n += 1 } }\n    n\n}",
    );
    has(
        &printed,
        "long_words",
        &[
            "w#4 = borrow ws#0[_]",
            "_ = borrow w#4",
            "end _\n    end w#4",
        ],
    );
    let f = function(&printed, "long_words");
    assert!(!f.contains("w#4 = copy"), "{f}");
    assert_eq!(f.matches("\n    end ").count(), 2, "{f}");
    // The loop reads the borrowed `ws` in place, so no release is left.
    assert_eq!(f.matches("release ").count(), 0, "{f}");
}

/// The member is an object and its root a whole local, so a `take` and a store through it
/// neither block the pair nor clobber it.
#[test]
fn store_through_a_copied_object_is_borrowed() {
    let printed = built(&format!(
        "{BOX}fn set() {{\n    let b = Box(s: \"a\")\n    b.s = \"x\"\n    print(b.s)\n}}"
    ));
    has(
        &printed,
        "set",
        &[
            "_ = borrow b#0",
            "_ = take _.0",
            "_.0 = move _",
            "release _",
            "end _",
        ],
    );
    let f = function(&printed, "set");
    assert!(!f.contains("= copy b#0"), "{f}");
    assert_eq!(f.matches("\n    end ").count(), 1, "{f}");
}

/// The spec's case: the root is moved, then assigned, while the copy is open.
#[test]
fn copy_of_a_reassigned_root_is_kept() {
    let printed = built(
        "fn name(k: i64) -> string { \"n{k}\" }\n\
         fn reassigned(k: i64) {\n    let mut x = name(k)\n    let t = x\n    print(x)\n    x = name(k + 1)\n    print(t)\n    print(x)\n}",
    );
    let f = function(&printed, "reassigned");
    assert!(f.contains("t#2 = copy x#1"), "{f}");
    assert!(!f.contains("borrow x#1"), "{f}");
}

/// The spec's case, and Review Focus 2: a call may replace the field the copy came from.
#[test]
fn copy_across_a_call_that_replaces_a_mut_field_is_kept() {
    let printed = built(&format!(
        "{BOX}fn swap(b: Box, k: i64) {{ b.s = \"new{{k}}\" }}\n\
         fn across_call(b: Box, k: i64) {{\n    let t = b.s\n    swap(b, k)\n    print(t)\n    print(b.s)\n}}"
    ));
    let f = function(&printed, "across_call");
    assert!(f.contains("t#2 = copy b#0.0"), "{f}");
}

/// `Bag` has no written method, so the copy `ys` of its `mut` field is read only. Calls of the
/// prelude's `sum` and `min` are quiet: they replace no field, so the copy is a borrow across
/// them. A call of a function of the user's is no such call: `zs` stays a copy across `one()`.
/// `contains<string>` is quiet too, since the `==` of two strings calls no written method: the
/// element `s` is a borrow across it, and the element `t` stays a copy across the user's `has`.
#[test]
fn copy_should_become_a_borrow_across_a_quiet_prelude_call() {
    let printed = built(
        "class Bag {\n    mut items: [i64]\n}\n\
         fn one() -> i64 { 1 }\n\
         fn main() {\n    let xs = [1, 2]\n    let b = Bag(items: xs)\n    let ys = b.items\n\
             let n = [1].sum() + 5.min(ys[0])\n    print(n)\n\
             let zs = b.items\n    let m = one() + zs[0]\n    print(m)\n}",
    );
    has(
        &printed,
        "main",
        &["ys#2 = borrow b#1.0", "call [i64].sum(", "end ys#2"],
    );
    has(&printed, "main", &["zs#4 = copy _.0", "call one()"]);

    let printed = built(
        "fn has(xs: [string], t: string) -> bool { xs.len() > t.len() }\n\
         fn main() {\n    let names = [\"a {1}\", \"b\"]\n    let s = names[1]\n\
             let found = [\"b\"].contains(s)\n    print(found)\n    print(s)\n\
             let t = names[0]\n    let other = has(names, t)\n    print(other)\n    print(t)\n\
             print(names.len())\n}",
    );
    has(
        &printed,
        "main",
        &[
            "s#1 = borrow names#0[1]",
            "call [string].contains(",
            "end s#1",
        ],
    );
    has(&printed, "main", &["t#3 = copy names#0[0]", "call has("]);
}

/// Review Focus 1: copy-on-write must still copy, so the copy that is written stays.
#[test]
fn copy_of_an_array_that_is_written_is_kept() {
    let printed = built(
        "fn arr(xs: [i64]) -> i64 {\n    let mut ys = xs\n    ys[0] = 5\n    ys[0] + xs[0]\n}",
    );
    let f = function(&printed, "arr");
    assert!(f.contains("ys#1 = copy xs#0"), "{f}");
    assert!(f.contains("make_unique ys#1"), "{f}");
}

/// Review Focus 4: the `take` and the counted store through the element copy clobber it.
#[test]
fn string_store_through_an_element_copy_is_kept() {
    let printed = built(&format!(
        "{BOX}fn store(ps: [Box]) {{\n    ps[0].s = \"y\"\n    print(ps[0].s)\n}}"
    ));
    has(&printed, "store", &["_ = copy ps#0[0]", "_ = take _.0"]);
}

/// Review Focus 4: only the `make_unique` clobbers, and with `a` and `b` the same object it
/// decides on the count the copy holds.
#[test]
fn field_copy_across_make_unique_is_kept() {
    let printed = built(
        "class Box {\n    mut xs: [i64]\n}\n\
         fn f(a: Box, b: Box) {\n    let t = a.xs\n    b.xs[0] = 5\n    print(t[0])\n    print(a.xs[0])\n}",
    );
    let f = function(&printed, "f");
    assert!(f.contains("t#2 = copy a#0.0"), "{f}");
}

/// A `push` decides on the count that the copy holds, as a `make_unique` does, so it clobbers.
/// With `a` and `b` the same object, it may free the block that `t` borrows.
#[test]
fn field_copy_across_a_push_should_be_kept() {
    let printed = built(
        "class Box {\n    mut xs: [i64]\n}\n\
         fn f(a: Box, b: Box) {\n    let t = a.xs\n    b.xs.push(5)\n    print(t[0])\n    print(a.xs[0])\n}",
    );
    let f = function(&printed, "f");
    assert!(f.contains("t#2 = copy a#0.0"), "{f}");
}

/// Review Focus 4: only the `take` and the counted store through the alias `b` clobber.
#[test]
fn string_store_through_an_alias_is_kept() {
    let printed = built(&format!(
        "{BOX}fn g(a: Box, b: Box, k: i64) {{\n    let t = a.s\n    b.s = \"m{{k}}\"\n    print(t)\n    print(a.s)\n}}"
    ));
    let f = function(&printed, "g");
    assert!(f.contains("t#3 = copy a#0.0"), "{f}");
}

/// `None` is not counted, so its store clobbers nothing: only the `take` through the alias
/// `b` does, and with `a` and `b` the same object its release frees the payload `t` holds.
#[test]
fn none_store_into_an_enum_optional_through_an_alias_is_kept() {
    let printed = built(
        "enum E {\n    V(string)\n    W\n}\nclass B {\n    mut e: E?\n}\n\
         fn h(a: B, b: B, k: E?) {\n    let t = a.e\n    b.e = None\n    print(t == k)\n    print(a.e == None)\n}",
    );
    let f = function(&printed, "h");
    assert!(f.contains("t#3 = copy a#0.0"), "{f}");
}

/// Review Focus 5: a copy that a call consumes is not a pair, and neither is the result.
#[test]
fn copy_moved_into_a_call_is_kept() {
    let printed = built(
        "fn keep(s: string) -> string {\n    print(s)\n    s\n}\n\
         fn moved(s: string) {\n    let t = s\n    let u = keep(t)\n    print(u)\n    print(s)\n}",
    );
    let f = function(&printed, "moved");
    assert!(f.contains("t#1 = copy s#0"), "{f}");
}

/// ir-2: a row read through a nested index is borrowed, both for its length and for each
/// element, so the loop retains nothing.
#[test]
fn row_of_a_nested_index_is_borrowed() {
    let f = function(
        &built(
            "fn total_index(g: [[i64]]) -> i64 {\n    let mut s = 0\n    for i in 0..g.len() {\n        for j in 0..g[i].len() {\n            s += g[i][j]\n        }\n    }\n    s\n}",
        ),
        "total_index",
    );
    assert_eq!(f.matches("= borrow g#0[").count(), 2, "{f}");
    assert!(!f.contains("= copy g#0["), "{f}");
    // The grid is a borrowed parameter, so nothing is released.
    assert!(f.contains("g#0: borrow [[i64]]"), "{f}");
    assert!(!f.contains("release "), "{f}");
}

/// A class whose written `eq` stores into a `mut` field, as a call can: the copy of `h.items`
/// stays a copy across any `==` or `contains` whose helper may run that `eq`.
const HOLDER: &str = "class Holder: Eq {\n    mut items: [string]\n    fn eq(self, other: Self) -> bool {\n        self.items = [\"y\"]\n        true\n    }\n}\n\
    enum Wrap {\n    One(Holder)\n}\n\
    enum Deep {\n    One(Wrap)\n}\nenum Lone {\n    One(Holder?)\n}\nenum Gen<T> {\n    One(T)\n}\n";

/// The program of `HOLDER` with `compare` as the line that runs the hidden call. With `live`,
/// `h` is read after it, so the comparison copies `h`, and the copy of `h.items` has nothing else
/// that keeps it: without the hidden call as a clobber it would become a borrow.
fn holder_program(compare: &str, live: bool) -> String {
    let after = if live {
        "\n    print(h.items.len())"
    } else {
        ""
    };
    format!(
        "{HOLDER}fn main() {{\n    let h = Holder(items: [\"{{1}}\"])\n    let xs = h.items\n    {compare}\n    print(xs[0])\n    print(same){after}\n}}"
    )
}

#[test]
fn copy_should_stay_across_a_written_eq() {
    for compare in [
        "let same = [h] == [h]",
        "let same = Wrap.One(h) == Wrap.One(h)",
        // A written `eq` of a part runs from the helper of an enum that holds it: directly, in an
        // array, an optional, an enum payload or a generic enum, whichever way the wrapper is
        // reached.
        "let same = Deep.One(Wrap.One(h)) == Deep.One(Wrap.One(h))",
        "let same = [Deep.One(Wrap.One(h))] == [Deep.One(Wrap.One(h))]",
        "let same = Lone.One(h) == Lone.One(h)",
        "let same = Gen.One(h) == Gen.One(h)",
    ] {
        for live in [false, true] {
            let printed = built(&holder_program(compare, live));
            let f = function(&printed, "main");
            let copy = f
                .lines()
                .find(|l| l.contains("xs#") && l.contains(" = "))
                .unwrap_or_else(|| panic!("{compare}: no binding of `xs` in\n{f}"));
            assert!(
                copy.contains("= copy "),
                "{compare} (live {live}): {copy}\n{f}"
            );
            assert!(f.contains(" [calls]"), "{compare} (live {live}): {f}");
        }
    }
}

/// `contains<Holder>` and `contains<Gen<Holder>>` run a written `eq` through the `==` in their own
/// bodies, so a call of one is no quiet call: the copy of `h.items` stays across it, and `main`
/// holds an ordinary call, which carries no `[calls]` marker (the marker is on an `Equal`).
#[test]
fn copy_should_stay_across_a_contains_that_calls_a_written_eq() {
    for (compare, instance) in [
        ("let same = [h].contains(h)", "[Holder].contains"),
        (
            "let same = [Gen.One(h)].contains(Gen.One(h))",
            "[Gen<Holder>].contains",
        ),
    ] {
        for live in [false, true] {
            let program = checked(&holder_program(compare, live));
            let mir = build(&program);
            let printed = print(&mir, &program);
            let f = function(&printed, "main");
            let copy = f
                .lines()
                .find(|l| l.contains("xs#") && l.contains(" = "))
                .unwrap_or_else(|| panic!("{compare}: no binding of `xs` in\n{f}"));
            assert!(
                copy.contains("= copy "),
                "{compare} (live {live}): {copy}\n{f}"
            );
            assert!(
                f.contains(&format!("call {instance}(")),
                "{compare} (live {live}): {f}"
            );
            assert!(!f.contains(" [calls]"), "{compare} (live {live}): {f}");
            let callee = mir
                .functions
                .iter()
                .find(|callee| callee.name == instance)
                .unwrap_or_else(|| panic!("{compare}: no function {instance}"));
            assert!(!callee.quiet, "{compare}: `{instance}` should not be quiet");
        }
    }
}

/// A program with no written method keeps pair removal, and prints no `[calls]`.
#[test]
fn equal_without_a_written_eq_should_not_be_flagged() {
    let printed = built("fn main() {\n    let same = [1] == [1]\n    print(same)\n}");
    let f = function(&printed, "main");
    assert!(f.contains("=="), "{f}");
    assert!(!f.contains("[calls]"), "{f}");
}

/// A class whose written `to_string` stores into a `mut` field, as a call can: the copy `old` of
/// `b.s` stays a copy across any `print` or interpolation whose helper may run that `to_string`,
/// the object itself or a value that holds it.
#[test]
fn copy_should_stay_across_a_written_to_string() {
    let decls = "class B {\n    mut s: string\n}\nextend B: Display {\n    fn to_string(self) -> string {\n        self.s = \"new\"\n        \"B\"\n    }\n}\nenum Wrap {\n    One(B)\n}\n";
    for printed in [
        "print(b)",
        "print([b])",
        "print(Wrap.One(b))",
        "let t = \"{b}\"",
        "let t = \"{[b]}\"",
        "let t = \"{Wrap.One(b)}\"",
    ] {
        // The text of an interpolation is printed last.
        let last = if printed.starts_with("let t") {
            "\n    print(t)"
        } else {
            ""
        };
        let src = format!(
            "{decls}fn main() {{\n    let b = B(s: \"heap {{1}}\")\n    let old = b.s\n    {printed}\n    print(old)\n    print(b.s){last}\n}}"
        );
        let f = function(&built(&src), "main");
        let copy = f
            .lines()
            .find(|l| l.contains("old#") && l.contains(" = "))
            .unwrap_or_else(|| panic!("{printed}: no binding of `old` in\n{f}"));
        assert!(copy.contains("= copy "), "{printed}: {copy}\n{f}");
        assert_eq!(f.matches(" [calls]").count(), 1, "{printed}: {f}");
    }
    // A value of an enum that holds no object with a written `to_string` prints with no flag.
    let f = function(
        &built(
            "enum Expr {\n    Num(i64)\n    Add(Expr, Expr)\n}\nfn main() {\n    print(Expr.Num(1))\n}",
        ),
        "main",
    );
    assert!(!f.contains("[calls]"), "{f}");
}

/// A `dbg` shows the fields of a class, so it reaches the written `to_string` of `Box`'s `item`.
/// A `print` of the same `Box` writes the class's name and runs none. The copy `old` of the
/// string that the `to_string` replaces stays a copy across the `dbg`, and the `dbg` is flagged.
#[test]
fn copy_should_stay_across_a_dbg_that_runs_a_written_to_string() {
    let decls = "class B {\n    mut s: string\n}\nextend B: Display {\n    fn to_string(self) -> string {\n        self.s = \"new\"\n        \"B\"\n    }\n}\nclass Box {\n    item: B\n}\n";
    let main = |shown: &str| {
        format!(
            "{decls}fn main() {{\n    let bx = Box(item: B(s: \"heap {{1}}\"))\n    let old = bx.item.s\n    {shown}\n    print(old)\n    print(bx.item.s)\n}}"
        )
    };
    let f = function(&built(&main("let _ = dbg(bx)")), "main");
    let copy = f
        .lines()
        .find(|l| l.contains("old#") && l.contains(" = "))
        .unwrap_or_else(|| panic!("no binding of `old` in\n{f}"));
    assert!(copy.contains("= copy "), "{copy}\n{f}");
    assert!(f.contains("call dbg(") && f.contains(" [calls]"), "{f}");
    let f = function(&built(&main("print(bx)")), "main");
    assert!(!f.contains("[calls]"), "{f}");
}

/// `A` meets `B`'s `Holder` through `B`, so the cycle `A` - `B` reaches a written `eq` whichever
/// of the two a comparison asks first.
#[test]
fn copy_should_stay_across_a_written_eq_reached_through_a_cycle() {
    for (first, second) in [("b1 == b2", "a1 == a2"), ("a1 == a2", "b1 == b2")] {
        let src = format!(
            "{HOLDER}enum A {{\n    X(B)\n    End\n}}\nenum B {{\n    Y(A)\n    Z(Holder)\n}}\n\
             fn main() {{\n    let h = Holder(items: [\"{{1}}\"])\n    let b1 = B.Z(h)\n    let b2 = B.Z(h)\n    let a1 = A.X(b1)\n    let a2 = A.X(b2)\n    \
             let s1 = {first}\n    let xs = h.items\n    let s2 = {second}\n    print(xs[0])\n    print(s1)\n    print(s2)\n    print(h.items.len())\n}}"
        );
        let f = function(&built(&src), "main");
        let copy = f
            .lines()
            .find(|l| l.contains("xs#") && l.contains(" = "))
            .unwrap_or_else(|| panic!("{first}: no binding of `xs` in\n{f}"));
        assert!(copy.contains("= copy "), "{first} first: {copy}\n{f}");
        assert_eq!(f.matches(" [calls]").count(), 2, "{first} first: {f}");
    }
}

/// The table of an `any Display` may hold the `to_string` of any class, so showing the value may
/// run a written method. The copy `old` of the string that a `to_string` could replace stays a
/// copy across `{d}`, `dbg(d)` and `print(d)`, and the statement is flagged.
#[test]
fn copy_should_stay_across_a_show_of_an_any_value() {
    let decls = "class H {\n    mut s: string\n}\n";
    for (shown, last) in [
        ("let t = \"{d}\"", "\n    print(t)"),
        ("let _ = dbg(d)", ""),
        ("print(d)", ""),
    ] {
        let src = format!(
            "{decls}fn main() {{\n    let h = H(s: \"x {{1}}\")\n    let d: any Display = h\n    let old = h.s\n    {shown}\n    print(old){last}\n}}"
        );
        let f = function(&built(&src), "main");
        let copy = f
            .lines()
            .find(|l| l.contains("old#") && l.contains(" = "))
            .unwrap_or_else(|| panic!("{shown}: no binding of `old` in\n{f}"));
        assert!(copy.contains("= copy "), "{shown}: {copy}\n{f}");
        assert_eq!(f.matches(" [calls]").count(), 1, "{shown}: {f}");
    }
}

/// GIVEN a copy of `xs` that a slice takes, and a release of the slice
/// WHEN pair removal reads the block
/// THEN the copy becomes a borrow and the slice reads it in place: the slice holds no count, so
/// it ends and has no release
#[test]
fn a_copy_moved_into_a_slice_should_become_a_borrow() {
    let printed = built(
        "fn f(xs: [i64]) -> i64 {\n    let t = xs[1..]\n    t.len() + xs.len()\n}\nfn main() {\n    print(f([1, 2, 3]))\n}",
    );
    has(
        &printed,
        "f",
        &["= borrow xs#0", "t#1 = slice _[1..]", "end t#1"],
    );
    let f = function(&printed, "f");
    assert!(!f.contains("= copy xs#0"), "{f}");
    assert!(!f.contains("release t#1"), "{f}");
}

/// The issue's program: each slice of the borrowed `xs` took a copy and a release.
#[test]
fn slice_of_a_borrowed_array_should_keep_no_copy_and_no_release() {
    let printed = built(
        "fn q(xs: [string]) -> i64 { xs[1..].len() + xs[..1].len() }\nfn main() {\n    print(q([\"a\", \"b\"]))\n}",
    );
    let f = function(&printed, "q");
    assert_eq!(f.matches("= borrow xs#0").count(), 2, "{f}");
    assert_eq!(f.matches("slice _[1..]").count(), 1, "{f}");
    assert_eq!(f.matches("slice _[..1]").count(), 1, "{f}");
    assert_eq!(f.matches("\n    end ").count(), 4, "{f}");
    assert!(!f.contains("= copy xs#0"), "{f}");
    assert!(!f.contains("release "), "{f}");
}

/// Each bound form, with a bound that a lost bound or a lost `inclusive` would change.
#[test]
fn each_bound_form_of_a_slice_should_become_a_borrow() {
    let printed = built(
        "fn q(xs: [string], a: i64, b: i64) -> i64 {\n    xs[a..b].len() + xs[a..=b].len() + xs[..b].len() + xs[a..].len()\n}\nfn main() {\n    print(q([\"a\", \"b\", \"c\", \"d\"], 1, 2))\n}",
    );
    let f = function(&printed, "q");
    for slice in [
        "slice _[a#1..b#2]",
        "slice _[a#1..=b#2]",
        "slice _[..b#2]",
        "slice _[a#1..]",
    ] {
        assert_eq!(f.matches(slice).count(), 1, "{slice} in\n{f}");
    }
    assert_eq!(f.matches("= borrow xs#0").count(), 4, "{f}");
    assert!(!f.contains("= copy"), "{f}");
    assert!(!f.contains("release "), "{f}");
}

#[test]
fn slice_of_a_string_should_become_a_borrow() {
    let printed = built(
        "fn q(s: string) -> i64 { s[1..].len() + s[..1].len() }\nfn main() {\n    print(q(\"abc\"))\n}",
    );
    let f = function(&printed, "q");
    assert_eq!(f.matches("= borrow s#0").count(), 2, "{f}");
    assert_eq!(f.matches("slice _[1..]").count(), 1, "{f}");
    assert_eq!(f.matches("slice _[..1]").count(), 1, "{f}");
    assert!(!f.contains("= copy s#0"), "{f}");
    assert!(!f.contains("release "), "{f}");
}

/// The first slice moves into the second, so the chain has two slices and ends last first.
#[test]
fn slice_of_a_slice_should_become_a_chain_of_borrows() {
    let printed = built(
        "fn q(xs: [string]) -> i64 { xs[1..][..2].len() }\nfn main() {\n    print(q([\"a\", \"b\", \"c\", \"d\"]))\n}",
    );
    has(
        &printed,
        "q",
        &["= borrow xs#0", "slice _[1..]", "slice _[..2]"],
    );
    let f = function(&printed, "q");
    assert_eq!(f.matches("\n    end ").count(), 3, "{f}");
    assert!(!f.contains("= copy xs#0"), "{f}");
    assert!(!f.contains("release "), "{f}");
}

/// A place the slice reads through a field or an element is borrowed too, with or without `mut`.
#[test]
fn slice_through_a_field_or_an_element_should_become_a_borrow() {
    for (name, src, at, root, releases) in [
        (
            "non-mut field, user call",
            "fn n(ys: [string]) -> i64 { ys.len() }\nclass K {\n    cs: [string]\n    fn part(self) -> i64 { n(self.cs[1..]) }\n}",
            "K.part",
            "self#0.0",
            0,
        ),
        (
            "mut field, len",
            "class L {\n    mut cs: [string]\n    fn part(self) -> i64 { self.cs[1..].len() }\n}",
            "L.part",
            "self#0.0",
            0,
        ),
        (
            "mut field, quiet prelude call",
            "class L {\n    mut cs: [string]\n    fn has(self) -> bool { self.cs[1..].contains(\"b\") }\n}",
            "L.has",
            "self#0.0",
            // The copy of the text `"b"`.
            1,
        ),
        (
            "element",
            "fn q(rows: [[string]]) -> i64 { rows[0][1..].len() }",
            "q",
            "rows#0[",
            0,
        ),
    ] {
        let f = function(&built(src), at);
        assert!(f.contains(&format!("= borrow {root}")), "{name}: {f}");
        assert!(f.contains("slice _[1..]"), "{name}: {f}");
        assert!(!f.contains(&format!("= copy {root}")), "{name}: {f}");
        assert_eq!(f.matches("release ").count(), releases, "{name}: {f}");
    }
}

/// `y` copies the slice `t`, so `y` keeps its own count and escapes. `t` and the borrow of `xs`
/// end before `xs` moves out at its last use.
#[test]
fn copy_of_a_slice_that_borrows_should_keep_its_own_count() {
    let printed = built(
        "fn copied(xs: [string]) -> [string] {\n    let t = xs[1..]\n    let y = t\n    print(t.len())\n    print(xs.len())\n    y\n}\nfn main() {\n    print(copied([\"a\", \"b\"]))\n}",
    );
    has(
        &printed,
        "copied",
        &[
            "= borrow xs#0",
            "t#1 = slice _[1..]",
            "y#2 = copy t#1",
            "end t#1",
            "end _",
            "_ = move xs#0",
            "return move _",
        ],
    );
    let f = function(&printed, "copied");
    assert!(!f.contains("= copy xs#0"), "{f}");
    assert!(f.contains("= move y#2"), "{f}");
}

const WRITTEN_EQ: &str = "class H { mut xs: [K] }\n\
    class K: Eq {\n    v: i64\n    mut h: H?\n    fn eq(self, o: K) -> bool {\n        if let Some(h) = self.h { h.xs = [] }\n        self.v == o.v\n    }\n}\n";

/// A written `eq` can replace the `mut` field that holds the root, so a slice of that field
/// stays a move. A slice of a whole local is safe: the call cannot replace the local.
#[test]
fn slice_read_by_a_written_method_should_follow_the_root() {
    let printed = built(&format!(
        "{WRITTEN_EQ}fn a(h: H, ys: [K]) -> bool {{ h.xs[1..] == ys }}\n\
         fn b(xs: [K], ys: [K]) -> bool {{ xs[1..] == ys }}\n\
         fn c(h: H, k: K) -> bool {{ h.xs[1..].contains(k) }}\n\
         fn d(xs: [K], k: K) -> bool {{ xs[1..].contains(k) }}"
    ));
    for (at, root) in [("b", "xs#0"), ("d", "xs#0")] {
        let f = function(&printed, at);
        assert!(f.contains(&format!("= borrow {root}")), "{at}: {f}");
        assert!(f.contains("slice _[1..]"), "{at}: {f}");
        assert!(!f.contains(&format!("= copy {root}")), "{at}: {f}");
    }
    for at in ["a", "c"] {
        let f = function(&printed, at);
        assert!(f.contains("= copy h#0.0"), "{at}: {f}");
        assert!(f.contains("slice move"), "{at}: {f}");
        assert!(!f.contains("slice _["), "{at}: {f}");
    }
}

const SLICE_HOLDER: &str = "class H { mut xs: [string] }\n";

/// Each program holds a slice of a copy that a borrow would make wrong, or that the pass cannot
/// borrow: the root changes while the slice lives, the slice is written, escapes or crosses a
/// block, or it is `safe`. The copy proves that each row opens a candidate, and the row must
/// keep it and the `slice move`.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the table is one row for each program, and the loop under it is short"
)]
fn slice_should_stay_a_move_where_a_borrow_is_wrong() {
    let src0 = format!("{WRITTEN_EQ}fn a(h: H, ys: [K]) -> bool {{ h.xs[1..] == ys }}");
    let src1 = format!("{WRITTEN_EQ}fn c(h: H, k: K) -> bool {{ h.xs[1..].contains(k) }}");
    let src2 = format!(
        "{SLICE_HOLDER}fn f(h: H) -> string {{\n    let t = h.xs[1..]\n    h.xs = [\"z\"]\n    t[0]\n}}"
    );
    let src3 = format!(
        "{SLICE_HOLDER}fn swap(h: H) {{ h.xs = [\"z\"] }}\nfn f(h: H) -> string {{\n    let t = h.xs[1..]\n    swap(h)\n    t[0]\n}}"
    );
    let src4 = format!(
        "{SLICE_HOLDER}fn stored(h: H, ys: [string]) {{\n    h.xs = ys[1..]\n    print(ys.len())\n}}"
    );
    let src5 = format!(
        "{SLICE_HOLDER}fn keep(h: H, ys: [string]) {{ h.xs = ys }}\nfn owned(h: H, xs: [string]) {{\n    keep(h, xs[1..])\n    print(xs.len())\n}}"
    );
    let rows = [
        (
            "mut field, user call",
            "fn n(ys: [string]) -> i64 { ys.len() }\nclass L {\n    mut cs: [string]\n    fn part(self) -> i64 { n(self.cs[1..]) }\n}",
            "L.part",
        ),
        (
            "mut field, written eq",
            "class K: Eq {\n    v: i64\n    fn eq(self, o: K) -> bool { self.v == o.v }\n}\nclass H { mut xs: [K] }\nfn q(h: H, ys: [K]) -> bool { h.xs[1..] == ys }",
            "q",
        ),
        ("written eq on a mut field", &src0, "a"),
        ("written contains on a mut field", &src1, "c"),
        ("field replaced while the slice lives", &src2, "f"),
        ("call replaces the field", &src3, "f"),
        (
            "root assigned",
            "fn a() -> string {\n    let mut xs = [\"a\", \"b\".to_string() + \"c\"]\n    let t = xs[1..]\n    print(xs.len())\n    xs = [\"z\"]\n    print(xs.len())\n    t[0]\n}",
            "a",
        ),
        (
            "root pushed",
            "fn f() {\n    let mut xs = [\"a\", \"b\".to_string() + \"c\"]\n    let t = xs[1..]\n    xs.push(\"z\")\n    print(t.len())\n    print(t[0])\n}",
            "f",
        ),
        (
            "root element written",
            "fn f() {\n    let mut xs = [\"a\", \"b\".to_string() + \"c\"]\n    let t = xs[1..]\n    xs[1] = \"z\"\n    print(t[0])\n    print(xs[1])\n}",
            "f",
        ),
        (
            "mut argument",
            "fn g(mut a: [string]) { a = [\"z\"] }\nfn f() {\n    let mut xs = [\"a\", \"b\".to_string() + \"c\"]\n    let t = xs[1..]\n    g(mut xs)\n    print(t[0])\n}",
            "f",
        ),
        (
            "bound with a mut argument",
            "fn g(mut a: [string]) -> i64 {\n    a = [\"z\"]\n    0\n}\nfn f() {\n    let mut xs = [\"a\", \"b\".to_string() + \"c\"]\n    print(xs[g(mut xs)..].len())\n    print(xs.len())\n}",
            "f",
        ),
        (
            "slice written",
            "fn f() {\n    let xs = [\"a\", \"b\".to_string() + \"c\"]\n    let mut t = xs[1..]\n    t[0] = \"z\"\n    print(t[0])\n    print(xs[1])\n}",
            "f",
        ),
        (
            "slice pushed",
            "fn f() {\n    let xs = [\"a\", \"b\".to_string() + \"c\", \"d\"]\n    let mut t = xs[..1]\n    t.push(\"z\")\n    print(t[1])\n    print(xs[1])\n}",
            "f",
        ),
        (
            "reassigned slice",
            "fn b(xs: [string]) -> i64 {\n    let mut t = xs[1..]\n    t = t[1..]\n    print(xs.len())\n    t.len()\n}",
            "b",
        ),
        (
            "safe slice, one block",
            "fn safe_one_block(s: string) -> bool { s.get(1..) == None }",
            "safe_one_block",
        ),
        (
            "safe slice, root live",
            "fn safeslice(s: string) -> i64 {\n    let o = s.get(1..)\n    print(s.len())\n    (o ?? \"\").len()\n}",
            "safeslice",
        ),
        (
            "returned",
            "fn returned(xs: [string]) -> [string] {\n    let t = xs[1..]\n    print(xs.len())\n    t\n}",
            "returned",
        ),
        ("stored", &src4, "stored"),
        ("owned parameter", &src5, "owned"),
        (
            "across blocks",
            "fn blocks(xs: [string], c: bool) -> i64 {\n    let t = xs[1..]\n    if c { print(t.len()) }\n    print(xs.len())\n    t.len()\n}",
            "blocks",
        ),
        (
            "for over a slice",
            "fn forloop(xs: [string]) -> i64 {\n    let mut n = 0\n    for x in xs[1..] { n += x.len() }\n    n + xs.len()\n}",
            "forloop",
        ),
        (
            "temporary root",
            "class L { cs: [string] }\nfn make() -> L { L(cs: [\"a\", \"b\".to_string() + \"c\"]) }\nfn main() { print(make().cs[1..].len()) }",
            "main",
        ),
    ];
    for (name, src, at) in rows {
        // A panic inside the certifier names no row, so the row is named first.
        eprintln!("row: {name}");
        let f = function(&built(src), at);
        let copy = f
            .find("= copy")
            .unwrap_or_else(|| panic!("{name}: no copy in\n{f}"));
        let moved = [f.find("slice move"), f.find("slice? move")]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or_else(|| panic!("{name}: no `slice move` in\n{f}"));
        assert!(copy < moved, "{name}: the copy should come first in\n{f}");
        assert!(
            !f.contains("slice _[") && !f.contains("slice? _["),
            "{name}: the slice should stay a move in\n{f}"
        );
    }
}

/// `t = copy xs[0]`, a read of `t`, and `release t` in one block of a function over `xs`, an
/// array of `ty`, with `t` of the type `ty`.
fn pair_candidate(ty: Type) -> Builder {
    let mut b = Builder::new("f");
    let xs = b.param("xs", Type::array(ty));
    let t = b.local("t", ty);
    b.block(
        vec![
            assign(t, Rvalue::Copy(first(xs))),
            Statement::Assign {
                dest: None,
                value: Rvalue::Print {
                    stream: Stream::Stdout,
                    operand: read(t),
                    calls: false,
                },
            },
            Statement::Release(t),
            Statement::Release(xs),
        ],
        ret(),
    );
    b
}

/// Runs the pass on the function of `b` and gives it back.
fn pair_removed(b: Builder) -> Function {
    let mut function = b.finish();
    crate::mir::pairs::remove_pairs(&mut function);
    function
}

/// Runs the pass on the function of `b`, and checks that it changed no statement.
fn assert_unchanged(b: Builder) {
    let mut function = b.finish();
    let before = format!("{:?}", function.blocks);
    crate::mir::pairs::remove_pairs(&mut function);
    assert_eq!(
        format!("{:?}", function.blocks),
        before,
        "no statement should change"
    );
    assert!(function.owners.is_empty(), "no copy should become a borrow");
}

/// `local[0]`.
fn first(local: LocalId) -> Place {
    Place {
        projections: vec![Projection::Index {
            index: Operand::Const(Const::Int(0, Type::I64)),
            span: Span::new(0, 0),
        }],
        ..whole(local)
    }
}

/// GIVEN a copy `t = copy xs[0]` that is read and then released, once with a counted `t` and once
/// with an uncounted one
/// WHEN the pairs are removed
/// THEN only the counted copy becomes a borrow: an uncounted copy holds no reference to pair
#[test]
fn remove_pairs_should_keep_a_copy_into_an_uncounted_local() {
    let counted = pair_removed(pair_candidate(Type::STRING));
    let stmts = &counted.blocks[0].stmts;
    assert!(
        matches!(
            stmts[0],
            Statement::Assign {
                value: Rvalue::Borrow(_),
                ..
            }
        ),
        "a counted copy should become a borrow: {stmts:?}"
    );
    assert!(
        stmts.iter().any(|stmt| matches!(stmt, Statement::End(_))),
        "the release of a counted copy should become an end: {stmts:?}"
    );
    assert_unchanged(pair_candidate(Type::I64));
}

/// GIVEN a copy into `t`, and a second copy into `t` before the release of `t`
/// WHEN the pairs are removed
/// THEN no copy becomes a borrow: an open member that is assigned again keeps its copy and its
/// release. No lowering assigns a local twice in a pair, so the mir is by hand
#[test]
fn remove_pairs_should_keep_two_copies_into_one_local() {
    let mut b = Builder::new("f");
    let xs = b.param("xs", Type::array(Type::STRING));
    let ys = b.param("ys", Type::array(Type::STRING));
    let t = b.local("t", Type::STRING);
    b.block(
        vec![
            assign(t, Rvalue::Copy(first(xs))),
            assign(t, Rvalue::Copy(first(ys))),
            Statement::Release(t),
            Statement::Release(xs),
            Statement::Release(ys),
        ],
        ret(),
    );
    assert_unchanged(b);
}

/// GIVEN a copy `t = copy xs[0]`, then `u = slice move m[t..]` that consumes `t` as a bound
/// WHEN the pairs are removed
/// THEN nothing changes: a step that consumes another member than the one it moves invalidates
/// the candidate. No lowering gives a slice a counted bound
#[test]
fn remove_pairs_should_keep_a_copy_that_a_slice_bound_consumes() {
    let mut b = Builder::new("f");
    let xs = b.param("xs", Type::array(Type::STRING));
    let m = b.local("m", Type::array(Type::STRING));
    let t = b.local("t", Type::STRING);
    let u = b.local("u", Type::array(Type::STRING));
    b.block(
        vec![
            fresh(m),
            assign(t, Rvalue::Copy(first(xs))),
            assign(
                u,
                Rvalue::Slice {
                    base: Operand::Move(m),
                    start: Some(Operand::Move(t)),
                    end: None,
                    inclusive: false,
                    safe: false,
                    span: Span::new(0, 0),
                },
            ),
            Statement::Release(t),
            Statement::Release(u),
            Statement::Release(xs),
        ],
        ret(),
    );
    assert_unchanged(b);
}
