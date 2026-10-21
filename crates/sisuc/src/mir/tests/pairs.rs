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
            "e#6 = borrow _[_]",
            "_ = borrow e#6",
            "_ = copy _.0",
            "end _\n    end e#6",
            "_ = borrow key#1",
            "end _",
        ],
    );
    let f = function(&printed, "count");
    assert!(!f.contains("e#6 = copy"), "{f}");
    assert!(!f.contains("= copy key#1"), "{f}");
    assert_eq!(f.matches("\n    end ").count(), 3, "{f}");
    // The field copy's, the array's and the key's own releases are left.
    assert_eq!(f.matches("release ").count(), 3, "{f}");
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
        &["w#5 = borrow _[_]", "_ = borrow w#5", "end _\n    end w#5"],
    );
    let f = function(&printed, "long_words");
    assert!(!f.contains("w#5 = copy"), "{f}");
    assert_eq!(f.matches("\n    end ").count(), 2, "{f}");
    // Only the array's release is left.
    assert_eq!(f.matches("release ").count(), 1, "{f}");
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
