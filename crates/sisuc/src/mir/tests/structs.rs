//! Structs in mir. A struct value is held inline, and a field is a projection of its root's
//! place. A write through a struct path is a write of its root local.

use super::*;

const SHOW: &str = "struct P { s: string }\nstruct In { s: string }\nstruct T { i: In }\n\
    fn show(s: string, n: i64) {\n    print(s)\n    print(n)\n}\n";

/// GIVEN a field of a struct that an argument copies, and a later argument that writes the field
/// WHEN the program is built
/// THEN the copy stays a copy, because the write of the root frees the old field
#[test]
fn struct_field_copy_should_stay_a_copy_across_a_write_of_its_root() {
    let printed = built(&format!(
        "{SHOW}fn kept() {{\n    let mut t = P(s: \"a\")\n    show(t.s, {{ t.s = \"b\"; 1 }})\n    print(t.s)\n}}"
    ));
    has(
        &printed,
        "kept",
        &[
            "_ = copy t#0.s",
            "_ = take t#0.s",
            "call show(",
            "release _",
        ],
    );
    let f = function(&printed, "kept");
    assert!(!f.contains("borrow t#0"), "{f}");
}

/// GIVEN a field two struct steps down, passed to a written function
/// WHEN the program is built
/// THEN the argument borrows the field across the call, because a call cannot write a struct
/// that a local holds
#[test]
fn struct_field_borrow_should_last_across_a_call() {
    let printed = built(&format!(
        "{SHOW}fn lent() {{\n    let a = T(i: In(s: \"a\"))\n    show(a.i.s, 1)\n    print(a.i.s)\n}}"
    ));
    has(
        &printed,
        "lent",
        &["_ = borrow a#0.i.s", "call show(_, 1)", "end _"],
    );
}

/// GIVEN a read of a field two struct steps down, and a read of a field of an uncounted struct
/// WHEN the program is built
/// THEN the counted read copies only the field, and the uncounted one reads in place with no count
#[test]
fn struct_path_read_should_make_no_extra_copy() {
    let printed = built(
        "struct In { s: string }\nstruct T { i: In }\nstruct Q { n: i64 }\n\
         fn deep() {\n    let a = T(i: In(s: \"a\"))\n    print(a.i.s)\n}\n\
         fn plain() {\n    let q = Q(n: 1)\n    print(q.n)\n}",
    );
    let deep = function(&printed, "deep");
    // The string literal's copy and the field's.
    assert_eq!(deep.matches("copy ").count(), 2, "{deep}");
    assert!(deep.contains(".i.s"), "{deep}");
    has(&printed, "plain", &["_ = q#0.n", "call print(_)"]);
    let plain = function(&printed, "plain");
    assert!(!plain.contains("release"), "{plain}");
    assert!(!plain.contains("move q#0"), "{plain}");
}

/// GIVEN a struct copied into a `let mut` and written through the copy
/// WHEN the program is built
/// THEN the copy stays a copy, so the source keeps its value
#[test]
fn struct_copy_written_through_should_stay_a_copy() {
    let printed = built(
        "struct P { s: string }\nfn copied() {\n    let a = P(s: \"a\")\n    let mut b = a\n    \
         b.s = \"b\"\n    print(a.s)\n    print(b.s)\n}",
    );
    has(&printed, "copied", &["b#1 = copy a#0", "_ = take b#1.s"]);
    let f = function(&printed, "copied");
    assert!(!f.contains("borrow a#0"), "{f}");
}
