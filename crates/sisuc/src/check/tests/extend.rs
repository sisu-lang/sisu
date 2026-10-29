//! `extend` of a built-in type: only the prelude may write one, and the methods it gives an array
//! or an `i64` answer a call.

use super::*;
use crate::front_end::checked_with;

/// A prelude with an `extend` of each kind of built-in target. `total` is on two array types.
const BUILT_INS: &str = "extend<T> [T] {\n    fn last_or(self, d: T) -> T { d }\n}\nextend [i64] {\n    fn total(self) -> i64 { 0 }\n}\nextend [f64] {\n    fn total(self) -> f64 { 0.0 }\n}\nextend i64 {\n    fn twice(self) -> i64 { self + self }\n}\n";

/// The code and `(line, col)` of each diagnostic of `src` over the prelude `prelude`. A
/// diagnostic into the prelude is an internal error, so it stops the test.
fn codes_over(prelude: &str, src: &str) -> Vec<(Code, (usize, usize))> {
    let (_, ds) = checked_with(prelude, src);
    ds.iter().map(|d| (d.code(), at(src, d.span()))).collect()
}

/// A user's `extend` of a built-in type stays E0155 when the prelude has such blocks: only the
/// prelude's target is taken.
#[test]
fn a_user_extend_of_a_built_in_type_should_be_reported_with_the_real_prelude() {
    for (row, at) in [
        ("extend i64 {\n}", (1, 8)),
        ("extend [i64] {\n}", (1, 8)),
        ("extend [f64] {\n}", (1, 8)),
        ("extend<T> [T] {\n}", (1, 11)),
    ] {
        let src = format!("{row}\nfn main() {{}}");
        assert_eq!(
            codes_over(PRELUDE_SOURCE, &src),
            [(Code::CannotExtend, at)],
            "{row}"
        );
    }
}

/// The prelude's blocks of `[T]`, `[i64]`, `[f64]` and `i64` check clean, and a call finds the
/// method of the block whose target covers the receiver.
#[test]
fn a_prelude_extend_of_a_built_in_type_should_answer_a_call_on_a_covered_receiver() {
    let src = "fn main() {\n    let a: i64 = [1, 2].total()\n    let b: f64 = [1.5].total()\n    let c: string = [\"x\"].last_or(\"y\")\n    let d: i64 = [3].last_or(4)\n    let e: i64 = 5.twice()\n    print(\"{a} {b} {c} {d} {e}\")\n}";
    assert_eq!(codes_over(BUILT_INS, src), []);
}

/// A receiver that no block's target covers finds only the methods of the blocks that do: a
/// `[string]` has `last_or` of `[T]` and not `total` of `[i64]`.
#[test]
fn a_call_on_an_array_should_find_only_the_methods_of_blocks_that_cover_it() {
    for (expr, expected) in [
        ("[\"x\"].last_or(\"y\")", vec![]),
        ("[\"x\"].total()", vec![(Code::NoMember, (2, 19))]),
        ("[true].total()", vec![(Code::NoMember, (2, 20))]),
        ("[[1]].total()", vec![(Code::NoMember, (2, 19))]),
        ("5.last_or(1)", vec![(Code::NoMember, (2, 15))]),
        ("1.5.twice()", vec![(Code::NoMember, (2, 17))]),
    ] {
        let src = format!("fn main() {{\n    let _ = {expr}\n}}");
        assert_eq!(codes_over(BUILT_INS, &src), expected, "{expr}");
    }
}

/// The same method name on `[T]` and `[i64]` is E0034, since the targets overlap: a call on an
/// `[i64]` would have two methods.
#[test]
fn two_prelude_blocks_whose_array_targets_overlap_should_not_give_one_name_twice() {
    let prelude = "extend<T> [T] {\n    fn total(self) -> i64 { 0 }\n}\nextend [i64] {\n    fn total(self) -> i64 { 1 }\n}\n";
    let src = "fn main() {}";
    let parsed = parse_ok(src);
    let (_, ds) = check(&parsed, &prelude_program(prelude), src, &[]);
    let errors: Vec<_> = (ds.iter())
        .filter(|d| d.severity() == Severity::Error)
        .map(Diagnostic::code)
        .collect();
    assert_eq!(errors, [Code::DefinedTwice]);
}

/// A wrong name on an array suggests a method of the prelude's block that covers it.
#[test]
fn an_unknown_method_of_an_array_should_suggest_a_method_of_a_covering_prelude_block() {
    let src = "fn main() {\n    let _ = [1].totl()\n}";
    let (_, ds) = checked_with(BUILT_INS, src);
    let [d] = &ds[..] else {
        panic!("one diagnostic expected: {ds:?}");
    };
    assert_eq!(d.code(), Code::NoMember);
    assert_eq!(applied(src, d), "fn main() {\n    let _ = [1].total()\n}");
}
