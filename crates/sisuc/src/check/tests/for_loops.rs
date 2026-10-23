//! `for` over arrays and ranges: the lowering, what the header accepts, and the loop variable.

use super::*;

#[test]
fn lowers_for_over_array() {
    contains(
        "fn f(xs: [i64]) { for x in xs { print(x) } }\nfn main() {}",
        &[
            "(block (let #1 (len xs#0)) (let mut #2 0) (loop (block (if (< #2 #1) (block (let x#3 (index xs#0 #2)) (= #2 (+unchecked #2 1)) (block (print x#3))) (block (break))))))",
        ],
    );
}

#[test]
fn for_over_a_mut_local_should_copy_the_array() {
    contains(
        "fn f() { let mut xs = [1]\n    for x in xs { xs[0] = x } }\nfn main() {}",
        &["(let #1 xs#0) (let #2 (len #1))"],
    );
}

#[test]
fn lowers_for_over_range() {
    contains(
        "fn f(n: i64) { for i in 0..n { print(i) } }\nfn main() {}",
        &[
            "(block (let mut #1 0) (let #2 n#0) (loop (block (if (< #1 #2) (block (let i#3 #1) (= #1 (+unchecked #1 1)) (block (print i#3))) (block (break))))))",
        ],
    );
}

#[test]
fn range_bounds_are_i64() {
    codes(
        "fn f(b: u8) { for i in 0..b {} }\nfn main() {}",
        &[(Code::Mismatch, (1, 27))],
    );
}

#[test]
fn range_start_is_i64() {
    codes(
        "fn f(b: u8) { for i in b..0 {} }\nfn main() {}",
        &[(Code::Mismatch, (1, 24))],
    );
}

#[test]
fn loop_variable_has_the_element_type() {
    clean("fn f(xs: [u8]) { for x in xs { let y: u8 = x } }\nfn main() {}");
    codes(
        "fn f(xs: [u8]) { for x in xs { let y: bool = x } }\nfn main() {}",
        &[(Code::Mismatch, (1, 46))],
    );
}

#[test]
fn for_needs_array() {
    diagnostic(
        "fn main() { for x in 5 {} }",
        Code::ForNeedsArray,
        (1, 22, "5"),
        &[],
    );
}

#[test]
fn for_over_an_optional_array() {
    let d = diagnostic(
        "fn f(m: [i64]?) { for x in m {} }\nfn main() {}",
        Code::MayBeNone,
        (1, 28, "m"),
        &[],
    );
    assert!(d.fixes().is_empty(), "{d:?}");
}

#[test]
fn loop_variable_is_immutable() {
    diagnostic(
        "fn f(xs: [i64]) {\n    for x in xs {\n        x = 1\n    }\n}\nfn main() {}",
        Code::AssignToImmutable,
        (3, 9, "x = 1"),
        &[(2, 9)],
    );
}

#[test]
fn loop_variable_ends_with_the_loop() {
    codes(
        "fn f(xs: [i64]) {\n    for x in xs {}\n    print(x)\n}\nfn main() {}",
        &[(Code::BindingNotFound, (3, 11))],
    );
}

/// The outer `x` is `bool`, the loop's is `i64`: each typed use clean pins which one it resolves
/// to.
#[test]
fn loop_variable_shadows_an_outer_binding() {
    clean(
        "fn f(xs: [i64]) {\n    let x = true\n    for x in xs {\n        let a: i64 = x\n    }\n    let b: bool = x\n}\nfn main() {}",
    );
}

#[test]
fn break_and_continue_work_in_for() {
    clean(
        "fn f(xs: [i64]) {\n    for x in xs {\n        if x == 1 { continue }\n        if x == 2 { break }\n    }\n}\nfn main() {}",
    );
}

#[test]
fn for_has_no_value() {
    codes(
        "fn f(xs: [i64]) -> i64 { for x in xs {} }\nfn main() {}",
        &[(Code::BodyHasNoValue, (1, 41))],
    );
}

#[test]
fn for_poison() {
    codes(
        "fn main() {\n    for x in nope { print(x + true) }\n}",
        &[(Code::BindingNotFound, (2, 14))],
    );
}

#[test]
fn bad_range_bound_still_checks_the_body() {
    // `i` is `i64` whatever the bounds are, so the body's own error is real.
    codes(
        "fn main() {\n    for i in nope..3 { print(i + true) }\n}",
        &[(Code::BindingNotFound, (2, 14)), (Code::Mismatch, (2, 34))],
    );
}

/// `for _` lowers like the same loop over `i`, with the loop variable unnamed.
#[test]
fn for_underscore() {
    for (with_name, with_underscore) in [
        (
            "fn main() { for i in 0..3 { print(1) } }",
            "fn main() { for _ in 0..3 { print(1) } }",
        ),
        (
            "fn main() { for i in [1, 2] { print(1) } }",
            "fn main() { for _ in [1, 2] { print(1) } }",
        ),
    ] {
        assert_eq!(
            lowered(with_underscore),
            lowered(with_name).replace("i#", "#")
        );
    }
}

#[test]
fn for_underscore_declares_no_name() {
    codes(
        "fn main() {\n    for _ in 0..3 {\n        let _ = 1\n        for _ in 0..3 {}\n    }\n}",
        &[],
    );
}

#[test]
fn for_underscore_still_checks_the_source() {
    diagnostic(
        "fn main() { for _ in 5 {} }",
        Code::ForNeedsArray,
        (1, 22, "5"),
        &[],
    );
}

/// `for x in a..=b` steps with a flag, so `b` may be `i64::MAX`: the counter moves on only when
/// it has not reached `b`.
#[test]
fn for_inclusive_lowers() {
    contains(
        "fn f(s: i64, e: i64) { for i in s..=e { print(i) } }\nfn main() {}",
        &[
            "(block (let mut #2 s#0) (let #3 e#1) (let mut #4 (<= #2 #3)) (loop (block (if #4 (block (let i#5 #2) (= #4 (!= #2 #3)) (if #4 (block (= #2 (+unchecked #2 1)))) (block (print i#5))) (block (break))))))",
        ],
    );
}
