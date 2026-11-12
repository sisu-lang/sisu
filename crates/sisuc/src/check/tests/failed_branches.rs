//! A value in a poisoned context: a branch of an `if` or a `match`, and a value of a `break`, adds
//! no error that follows only from the failed part. A true error stays.

use super::*;
use Code::{
    ArgumentCount, BindingNotFound, CannotCompare, CannotInfer, FunctionNotFound,
    IntegerDoesNotFit, Mismatch, NegatedU8, NoMember, NoValue, UnknownType, UnwrapNeedsOptional,
};

/// The program of a row of the branch tables: `stmt` starts at line 5, and each line break of it
/// continues at that indent.
fn branch_program(stmt: &str) -> String {
    format!(
        "fn f(a: u8, b: bool) {{}}\nfn main() {{\n    let m: u8 = 7\n    let c = true\n    {}\n}}\n",
        stmt.replace('\n', "\n    ")
    )
}

/// The rows `(name, statement, errors)` as the rows of `error_rows`.
fn branch_rows(rows: &[(&'static str, &str, Vec<Wanted>)]) -> Vec<ErrorRow<'static>> {
    rows.iter()
        .map(|(name, stmt, wanted)| (*name, branch_program(stmt), wanted.clone()))
        .collect()
}

/// GIVEN a branch of an `if` or a `match` with a value of type `u8`, and another branch with a
///   literal
/// AND the `if` or `match` is in a poisoned context: the right side of `??` after an unknown name,
///   or an argument of a call with the wrong count
/// WHEN the program is checked
/// THEN the literal takes the type `u8` from the other branch, in each order of the branches
/// AND only the failed part reports
#[test]
fn a_literal_branch_in_a_failed_context_should_take_the_type_of_the_others() {
    let call: Wanted = (ArgumentCount, (5, 5));
    let cut: Wanted = (BindingNotFound, (5, 14));
    error_rows(&branch_rows(&[
        ("if in a call", "f(if c { m } else { 8 })", vec![call]),
        (
            "else if in a call",
            "f(if c { m } else if c { 9 } else { 8 })",
            vec![call],
        ),
        (
            "match in a call",
            "f(match c {\n true => m\n false => 8\n })",
            vec![call],
        ),
        (
            "if after the name",
            "let _x = v1 ?? (if c { m } else { 8 })",
            vec![cut],
        ),
        (
            "literal first",
            "let _x = v1 ?? (if c { 8 } else { m })",
            vec![cut],
        ),
        (
            "match, literal first",
            "let _x = v1 ?? (match c {\n true => 8\n false => m\n })",
            vec![cut],
        ),
        (
            "arrays",
            "let _x = v1 ?? (if c { [m] } else { [8] })",
            vec![cut],
        ),
        ("plus", "let _x = v1 + (if c { m } else { 8 })", vec![cut]),
        (
            "call of an unknown name",
            "let _x = v1(if c { m } else { 8 })",
            vec![(FunctionNotFound, (5, 14))],
        ),
        (
            "method of an unknown name",
            "let _x = v1.foo(if c { m } else { 8 })",
            vec![cut],
        ),
        (
            "unknown method",
            "let _x = m.nope(if c { m } else { 8 })",
            vec![(NoMember, (5, 16))],
        ),
        (
            "conversion with two arguments",
            "let _x = i64(if c { m } else { 8 }, 1)",
            vec![(ArgumentCount, (5, 14))],
        ),
        (
            "unknown declared type",
            "let _x: Nope = if c { m } else { 8 }",
            vec![(UnknownType, (5, 13))],
        ),
        (
            "float",
            "let _x = v1 ?? (if c { 1.5 } else { 8 })",
            vec![cut],
        ),
        (
            "some",
            "let _x = v1 ?? (if c { Some(m) } else { 8 })",
            vec![cut],
        ),
        (
            "arrays in arrays",
            "let _x = v1 ?? (if c { [[m]] } else { [[8]] })",
            vec![cut],
        ),
        (
            "negative literal",
            "let _x = v1 ?? (if c { m } else { -8 })",
            vec![cut, (NegatedU8, (5, 39))],
        ),
        (
            "negative literal first",
            "let _x = v1 ?? (if c { -8 } else { m })",
            vec![cut, (NegatedU8, (5, 28))],
        ),
        // The literal does not fit the `u8` that it takes, and that is a true error.
        (
            "literal too large",
            "f(if c { m } else { 300 })",
            vec![call, (IntegerDoesNotFit, (5, 25))],
        ),
    ]));
}

/// A branch that must wait in a poisoned context, a true error and a valid program keep their
/// output. Each row holds before and after the #617 fix.
#[test]
fn a_failed_context_should_keep_a_true_error_holds_before_and_after() {
    let call: Wanted = (ArgumentCount, (5, 5));
    let cut: Wanted = (BindingNotFound, (5, 14));
    error_rows(&branch_rows(&[
        (
            "literal and one in an operator",
            "f(if c { m } else { 8 } + 1)",
            vec![call],
        ),
        (
            "none second",
            "let _x = v1 ?? (if c { m } else { None })",
            vec![cut],
        ),
        (
            "none first",
            "let _x = v1 ?? (if c { None } else { 8 })",
            vec![cut],
        ),
        (
            "string against u8",
            "let _x = v1 ?? (if c { m } else { \"s\" })",
            vec![cut, (Mismatch, (5, 39))],
        ),
        (
            "array of string against array of u8",
            "let _x = v1 ?? (if c { [m] } else { [\"a\"] })",
            vec![cut, (Mismatch, (5, 41))],
        ),
        (
            "arrays with no failed context",
            "let _x = if c { [m] } else { [8] }\n print(_x)",
            vec![],
        ),
        // `[8]` ranks as a value that is checked first, as with no expected type, so it fixes
        // `[i64]` and `[m]` does not fit. A later run can change this.
        (
            "array literal first",
            "let _x = v1 ?? (if c { [8] } else { [m] })",
            vec![cut, (Mismatch, (5, 41))],
        ),
        // `[]` is no literal branch, so it waits for the next run.
        (
            "empty array in a match arm beside a typed arm",
            "f(match c {\n true => m\n false => []\n })",
            vec![call],
        ),
    ]));
}

/// A statement of the program of a `break` row.
fn loop_program(stmt: &str) -> String {
    format!(
        "fn nothing() {{}}\nfn main() {{\n    let c = true\n    let o: i64? = 1\n    let m = 5\n    {stmt}\n}}\n"
    )
}

/// GIVEN a `loop` with a `break` of a value that failed
/// AND a later `break` of a value that needs a type from its context, such as `None`, `[]` or
///   `[None]`
/// AND a later value with a type of its own waits too, so the loop gets no type from it
/// WHEN the program is checked
/// THEN only the poisoned value reports
#[test]
fn a_break_after_a_failed_break_should_wait_for_the_next_run() {
    let rows: Vec<ErrorRow<'_>> = [
        (
            "none",
            "let a = loop { if c { break yy }; break None }",
            vec![(BindingNotFound, (6, 33))],
        ),
        (
            "none, then an optional",
            "let a = loop { if c { break yy }; if c { break None }; break o }",
            vec![(BindingNotFound, (6, 33))],
        ),
        (
            "failed break of an outer loop",
            "let a = outer: loop { loop { if c { break outer yy } }; break None }",
            vec![(BindingNotFound, (6, 53))],
        ),
        (
            "call that gives no value",
            "let a = loop { if c { break nothing() }; break None }",
            vec![(NoValue, (6, 33))],
        ),
        (
            "array of none",
            "let a = loop { if c { break [yy] }; break [None] }",
            vec![(BindingNotFound, (6, 34))],
        ),
        (
            "conversion",
            "let a = loop { if c { break i64(yy) }; break None }",
            vec![(BindingNotFound, (6, 37))],
        ),
        (
            "empty array",
            "let a = loop { if c { break yy }; break [] }",
            vec![(BindingNotFound, (6, 33))],
        ),
        (
            "loop of none",
            "let a = loop { if c { break yy }; break loop { break None } }",
            vec![(BindingNotFound, (6, 33))],
        ),
        // The program is valid with a typed `yy`, so an E0032 here would be a cascade.
        (
            "typed break between",
            "let a = loop { if c { break yy }; if c { break m }; break None }",
            vec![(BindingNotFound, (6, 33))],
        ),
        // A true mismatch of the later values waits too: the loop that failed has no type.
        (
            "later values of two types",
            "let a = loop { if c { break yy }; if c { break 1 }; break \"s\" }",
            vec![(BindingNotFound, (6, 33))],
        ),
    ]
    .into_iter()
    .map(|(name, stmt, wanted)| (name, loop_program(stmt), wanted))
    .collect();
    error_rows(&rows);
}

/// A `None` before a typed or poisoned value has no type to take, and a true error after a failed
/// `break` stays. Each row holds before and after the #618 fix.
#[test]
fn a_break_that_is_first_or_a_true_error_should_stay_holds_before_and_after() {
    let rows: Vec<ErrorRow<'_>> = [
        (
            "none first",
            "let a = loop { if c { break None }; break yy }",
            vec![(CannotInfer, (6, 33)), (BindingNotFound, (6, 47))],
        ),
        (
            "none first, then an optional",
            "let a = loop { if c { break None }; break o }",
            vec![(CannotInfer, (6, 33))],
        ),
        (
            "typed after a failed break",
            "let a = loop { if c { break yy }; break 1 }",
            vec![(BindingNotFound, (6, 33))],
        ),
        (
            "declared optional",
            "let a: i64? = loop { if c { break yy }; break None }",
            vec![(BindingNotFound, (6, 39))],
        ),
        (
            "conversion of none",
            "let a = loop { if c { break yy }; break i64(None) }",
            vec![(BindingNotFound, (6, 33)), (CannotInfer, (6, 49))],
        ),
    ]
    .into_iter()
    .map(|(name, stmt, wanted)| (name, loop_program(stmt), wanted))
    .collect();
    error_rows(&rows);
}

/// GIVEN a join in a poisoned context: an argument of a call with the wrong count, or the right
///   side of `??` after an unknown name
/// AND a branch with a `break` of a value, and a later branch with `break None`, to a `loop`
///   that was open before the join
/// WHEN the program is checked
/// THEN only the failed part reports
///   (the join checks its branches out of source order. So the `break None` can come before the
///   `break` that gives the loop its type. It waits for the next run.)
/// AND this holds for `if`, `else if`, `if let` and `match`, and for a `break` by a label
/// AND a first `break None` in a join in a poisoned context waits too: in a poisoned context a
///   value waits for the next run, as in #617. Before the fix such a `None` gave E0050 at once.
/// AND a waiting `break` with a value of a type fails the loop, so it gives the loop no type
#[test]
fn a_break_in_a_failed_join_should_wait_for_a_loop_outside_it() {
    let call: Wanted = (ArgumentCount, (5, 20));
    error_rows(&branch_rows(&[
        (
            "statement in a later branch",
            "let a = loop { f(if c { if c { break Some(1) } else { 8 } } else { if c { break None }; 8 }) }",
            vec![call],
        ),
        (
            "later branch of a lower rank",
            "let a = loop { f(if c { if c { break Some(m) } else { 8 } } else { if c { break None } else { m } }) }",
            vec![call],
        ),
        (
            "else if",
            "let a = loop { f(if c { if c { break Some(1) } else { 8 } } else if c { 9 } else { if c { break None }; 8 }) }",
            vec![call],
        ),
        (
            "if let",
            "let o: u8? = Some(1)\nlet a = loop { f(if let v = o { if c { break Some(v) } else { 8 } } else { if c { break None }; 8 }) }",
            vec![(ArgumentCount, (6, 20))],
        ),
        (
            "match of blocks",
            "let a = loop { f(match c {\n true => { if c { break Some(1) } else { 8 } }\n false => { if c { break None }; 8 }\n }) }",
            vec![call],
        ),
        (
            "match of values",
            "let a = loop { f(match c {\n true => if c { break Some(m) } else { 8 }\n false => if c { break None } else { m }\n }) }",
            vec![call],
        ),
        (
            "right side of coalesce",
            "let a = loop { let _x = yy ?? (if c { if c { break Some(1) } else { 8 } } else { if c { break None }; 8 }) }",
            vec![(BindingNotFound, (5, 29))],
        ),
        (
            "outer loop by label",
            "let a = outer: loop { f(if c { if c { break outer Some(1) } else { 8 } } else { loop { if c { break outer None } }; 8 }) }",
            vec![(ArgumentCount, (5, 27))],
        ),
        (
            "first break none in a failed join",
            "let a = loop { f(if c { if c { break None }; 8 } else { 9 }) }",
            vec![call],
        ),
        // `m` is a `u8` and `8` is a literal, so `join` reads `expected`, not `carried`: the
        // branch of literals gets a hint, and it is still in a join in a poisoned context.
        (
            "branch of literals",
            "let a = loop { f(if c { m } else { if c { break None } else { 8 } }) }",
            vec![call],
        ),
        // The inner join is a join in a poisoned context in a loop that opens inside the outer
        // join in a poisoned context. Its `break` still waits for the first `break` of that loop:
        // the innermost join counts.
        (
            "inner failed join in a loop of a failed join",
            "f(if c { let x = loop { f(if c { if c { break Some(1) } else { 8 } } else { if c { break None }; 8 }) }; 8 } else { 9 })",
            vec![(ArgumentCount, (5, 5)), (ArgumentCount, (5, 29))],
        ),
        (
            "typed break in a later branch",
            "let a = loop { f(if c { if c { break Some(m) } else { 8 } } else { if c { break 1 }; 8 }) }",
            vec![call],
        ),
        (
            "two later breaks after the join",
            "let a = loop { f(if c { break Some(m) } else { 8 }); if c { break m }; break None }",
            vec![call],
        ),
        (
            "later break of another type after the join",
            "let a = loop { f(if c { if c { break Some(m) } else { 8 } } else { if c { break 1 }; 8 }); if c { break \"s\" }; break 2 }",
            vec![call],
        ),
        (
            "break of a name in a later branch",
            "let a = loop { f(if c { if c { break Some(m) } else { 8 } } else { if c { break m }; 8 }) }",
            vec![call],
        ),
        (
            "typed break on the right side of coalesce",
            "let a = loop { let _x = yy ?? (if c { if c { break Some(m) } else { 8 } } else { if c { break 1 }; 8 }) }",
            vec![(BindingNotFound, (5, 29))],
        ),
        (
            "typed break in a match",
            "let a = loop { f(match c {\n true => if c { break Some(m) } else { 8 }\n false => { if c { break 1 }; 8 }\n }) }",
            vec![call],
        ),
        // The guard of an arm is checked out of source order, so its `break` waits too.
        (
            "guard of a match arm",
            "let n = 3\nlet a = loop { f(match n {\n 1 => if c { break Some(m) } else { 8 }\n x if (if c { break None } else { c }) => m\n _ => 8\n }) }",
            vec![(ArgumentCount, (6, 20))],
        ),
    ]));
}

/// A loop that opens inside a join in a poisoned context, a valid program, and a join under no
/// expectation keep their output. Each row holds before and after the fix of a `break` in a join
/// in a poisoned context.
#[test]
fn a_break_in_a_failed_join_should_keep_its_output_holds_before_and_after() {
    error_rows(&branch_rows(&[
        // The loop opens inside the join, and its first `break` has no type: a true error.
        (
            "loop opened inside the join",
            "f(if c { let x = loop { break None }; 8 } else { 9 })",
            vec![(ArgumentCount, (5, 5)), (CannotInfer, (5, 35))],
        ),
        // E0043: the outer loop has no `break`, so it gives no value. E0053: the call has one
        // argument. E0050: the inner loop opens inside the join, and its first `break` is `None`.
        (
            "loop with a typed break inside the join",
            "let a = loop { f(if c { 8 } else { let b = loop { if c { break None }; break 1 }; 9 }) }",
            vec![
                (NoValue, (5, 13)),
                (ArgumentCount, (5, 20)),
                (CannotInfer, (5, 68)),
            ],
        ),
        (
            "valid coalesce",
            "let o: i64? = None\nlet a = loop { let _x = o ?? (if c { if c { break Some(1) }; 8 } else { if c { break None }; 9 }) }\nprint(a ?? 0)",
            vec![],
        ),
        (
            "valid call with a typed break in a later branch",
            "let a = loop { f(if c { if c { break Some(m) } else { 8 } } else { if c { break 1 }; 8 }, true) }\nprint(a ?? 0)",
            vec![],
        ),
        (
            "valid call with a later break of a name",
            "let a = loop { f(if c { break Some(m) } else { 8 }, true); if c { break m }; break None }\nprint(a ?? 0)",
            vec![],
        ),
        // E0043: the loop has no value, as `panic("x")` is `never`. A `never` value does not fail
        // the loop.
        (
            "never value in a waiting position",
            "let a = loop { f(if c { if c { break panic(\"x\") } else { 8 } } else { 8 }) }",
            vec![(NoValue, (5, 13)), (ArgumentCount, (5, 20))],
        ),
        // Under no expectation the branches are ranked in a different order than the source, and
        // the `break None` comes first. #676 fixes this false error.
        (
            "no expectation",
            "let a = loop { let _x = if c { if c { break Some(1) } else { 8 } } else { if c { break None }; 8 } }",
            vec![(CannotInfer, (5, 92))],
        ),
    ]));
}

/// GIVEN `v1 ?? (if c { NEST } else { 8 })`, where `NEST` is the same `if` again, so that the
///   right side of `??` is a poisoned context
/// WHEN the checker ranks the nested branches at three depths
/// THEN the steps above those of depth 0 grow with the depth, and grow linearly
/// AND the depths are 10, 20 and 30, because 60 levels overflow the stack of a debug test thread
#[test]
fn nested_branches_in_a_failed_context_should_rank_in_linear_steps() {
    crate::on_compiler_stack(|| {
        with_prelude();
        let steps = |depth: usize| {
            let (open, close) = ("if c { ", " } else { 8 }");
            let src = format!(
                "fn main() {{\n    let c = true\n    let _x = v1 ?? ({}8{})\n}}\n",
                open.repeat(depth),
                close.repeat(depth)
            );
            generic::RANK_STEPS.set(0);
            diags(&src);
            generic::RANK_STEPS.get()
        };
        // The prelude's own steps, about 20, are the same at each depth.
        let base = steps(0);
        let (ten, twenty, thirty) = (steps(10) - base, steps(20) - base, steps(30) - base);
        assert!(
            ten > 0 && twenty > ten && thirty > twenty,
            "the steps should grow with the depth: {ten}, {twenty}, {thirty}"
        );
        assert_near_linear(ten, twenty, thirty);
    });
}

/// The program of a row of the #651 tables: `h` takes one argument, so `h()` has a failed list of
/// arguments and is still typed `i64`. `stmt` is at line 4, after `let c = true`.
fn typed_break_program(stmt: &str) -> String {
    format!("fn h(x: i64) -> i64 {{ x }}\nfn main() {{\n    let c = true\n    {stmt}\n}}\n")
}

/// GIVEN a `loop` with a `break` of a call that has the wrong count of arguments, so the value is
///   typed `i64` and poisoned
/// WHEN the program is checked
/// THEN the loop has the type of that value, and a true error that uses the loop stays
/// AND an untyped value of a `break` after it still fails the join, as `break h()` after a
///   `break true` does
#[test]
fn a_typed_poisoned_break_value_should_give_its_loop_the_type() {
    let rows: Vec<ErrorRow<'_>> = vec![
        (
            "alone",
            "fn h(x: i64) -> i64 { x }\nfn main() {\n    let a = loop { break h() }\n    print(a + true)\n}\n"
                .into(),
            vec![(ArgumentCount, (3, 26)), (Mismatch, (4, 15))],
        ),
        (
            "first of two, then a number",
            typed_break_program("let a = loop { if c { break 1 }; break h() }; print(a + true)"),
            vec![(ArgumentCount, (4, 44)), (Mismatch, (4, 61))],
        ),
        (
            "with a declared type",
            typed_break_program("let a: bool = loop { break h() }; print(a)"),
            vec![(ArgumentCount, (4, 32)), (Mismatch, (4, 32))],
        ),
        (
            "with two wrong arguments",
            typed_break_program("let a = loop { break h(1, 2) }; print(a + true)"),
            vec![(ArgumentCount, (4, 26)), (Mismatch, (4, 47))],
        ),
        (
            "by a label",
            "fn h(x: i64) -> i64 { x }\nfn main() {\n    let a = out: loop {\n        loop { break out h() }\n    }\n    print(a + true)\n}\n"
                .into(),
            vec![(ArgumentCount, (4, 26)), (Mismatch, (6, 15))],
        ),
    ];
    error_rows(&rows);
}

/// GIVEN a `loop` with a `break` of a call with the wrong count of arguments, typed `i64`
/// AND another `break` of a value of another type, or with no value
/// WHEN the program is checked
/// THEN the second `break` is a true error against the type of the first one, in either order
#[test]
fn a_typed_poisoned_break_value_should_join_with_the_other_breaks() {
    let rows: Vec<ErrorRow<'_>> = vec![
        (
            "a bool after it",
            typed_break_program("let a = loop { if c { break h() }; break true }; print(a)"),
            vec![(ArgumentCount, (4, 33)), (Mismatch, (4, 46))],
        ),
        (
            "a bool before it",
            typed_break_program("let a = loop { if c { break true }; break h() }; print(a)"),
            vec![(ArgumentCount, (4, 47)), (Mismatch, (4, 47))],
        ),
        (
            "a bare break before it",
            "fn h(x: i64) -> i64 { x }\nfn f(c: bool) {\n    let a = loop {\n        if c { break }\n        break h()\n    }\n}\nfn main() {}\n"
                .into(),
            vec![(NoValue, (4, 16)), (ArgumentCount, (5, 15))],
        ),
        (
            "a bare break after it",
            "fn h(x: i64) -> i64 { x }\nfn main() {\n    let c = true\n    let a = loop { if c { break h() }; break }\n}\n"
                .into(),
            vec![(ArgumentCount, (4, 33)), (NoValue, (4, 40))],
        ),
        (
            "a call that gives no value",
            "fn u(x: i64) {}\nfn main() {\n    let a = loop { break u() }\n    print(a)\n}\n".into(),
            vec![(ArgumentCount, (3, 26)), (NoValue, (3, 26))],
        ),
    ];
    error_rows(&rows);
}

/// The cause of #651 on its own: a typed poisoned first `break` sets the type of the loop.
/// So the use of the loop as a `bool` is the error against `i64`.
#[test]
fn a_typed_poisoned_first_break_should_set_the_type_of_the_loop() {
    error_rows(&[(
        "first break typed and poisoned",
        typed_break_program("let a = loop { if c { break h() }; break 1 }; print(a + true)"),
        vec![(ArgumentCount, (4, 33)), (Mismatch, (4, 61))],
    )]);
}

/// GIVEN a `loop` with a `break` of a value that is untyped
/// AND other rows: a typed value outside a loop, a block that never ends, a value after a `None`
/// WHEN the program is checked
/// THEN the errors are as they were before the fix of #651
#[test]
fn an_untyped_break_value_should_fail_the_loop_holds_before_and_after() {
    let rows: Vec<ErrorRow<'_>> = vec![
        (
            "an unknown name",
            typed_break_program("let a = loop { break missing }; print(a + true)"),
            vec![(BindingNotFound, (4, 26))],
        ),
        (
            "a typed value, outside a loop",
            typed_break_program("let a = h(); print(a + true)"),
            vec![(ArgumentCount, (4, 13)), (Mismatch, (4, 28))],
        ),
        (
            "a block that never ends",
            "fn h(x: i64) -> i64 { x }\nfn main() {\n    let a = loop { break { return; h() } }\n    print(a + true)\n}\n"
                .into(),
            vec![(ArgumentCount, (3, 36))],
        ),
        (
            "a none first",
            "fn h(x: i64) -> i64 { x }\nfn f(c: bool) {\n    let a = loop {\n        if c { break None }\n        break h()\n    }\n    print(a + true)\n}\nfn main() {}\n"
                .into(),
            vec![(CannotInfer, (4, 22)), (ArgumentCount, (5, 15))],
        ),
    ];
    error_rows(&rows);
}

/// GIVEN an `if` whose condition is not `bool`, so the condition failed
/// WHEN the program is checked
/// THEN the `if` has the type of the join of its branches
/// AND a true error that uses its value stays
/// AND this holds for an `else if` condition and an `if let` whose subject is no optional
/// AND this holds for a condition in a `match` arm, and for a function that returns the `if`
#[test]
fn a_condition_that_is_not_bool_should_keep_the_join_of_the_branches() {
    let rows: Vec<ErrorRow<'_>> = vec![
        (
            "if condition with else",
            "fn main() {\n    let x = if 1 { 2 } else { 3 }\n    print(x == false)\n}".into(),
            vec![(Mismatch, (2, 16)), (CannotCompare, (3, 11))],
        ),
        (
            "if condition without else",
            "fn main() {\n    let x = if 1 { print(2) }\n    print(x)\n}".into(),
            vec![(NoValue, (2, 13)), (Mismatch, (2, 16))],
        ),
        (
            "a none as the condition",
            "fn main() {\n    let x = if None { 1 } else { 2 }\n    print(x && true)\n}".into(),
            vec![(Mismatch, (2, 16)), (Mismatch, (3, 11))],
        ),
        (
            "a number, then an addition",
            "fn main() {\n    let a = if 1 { 2 } else { 3 }\n    print(a + true)\n}".into(),
            vec![(Mismatch, (2, 16)), (Mismatch, (3, 15))],
        ),
        (
            "an else if condition",
            "fn main() {\n    let c = true\n    let a = if c { 1 } else if 2 { 3 } else { 4 }\n    print(a + true)\n}"
                .into(),
            vec![(Mismatch, (3, 32)), (Mismatch, (4, 15))],
        ),
        (
            "an else if condition at depth 3",
            "fn main() {\n    let c = true\n    loop {\n        let a = if c { 1 } else if c { 2 } else if 3 { 4 } else { 5 }\n        print(a + true)\n    }\n}"
                .into(),
            vec![(Mismatch, (4, 52)), (Mismatch, (5, 19))],
        ),
        (
            "a declared type that the join misses",
            "fn main() {\n    let a: bool = if 1 { 2 } else { 3 }\n    print(a)\n}".into(),
            vec![(Mismatch, (2, 19)), (Mismatch, (2, 22))],
        ),
        (
            "an if let subject that is no optional",
            "fn main() {\n    let a = if let Some(v) = 5 { 2 } else { 3 }\n    print(a + true)\n}"
                .into(),
            vec![(UnwrapNeedsOptional, (2, 30)), (Mismatch, (3, 15))],
        ),
        (
            "an else if let subject that is no optional",
            "fn main() {\n    let c = true\n    let a = if c { 1 } else if let Some(v) = 5 { 2 } else { 3 }\n    print(a + true)\n}"
                .into(),
            vec![(UnwrapNeedsOptional, (3, 46)), (Mismatch, (4, 15))],
        ),
        (
            "a bad condition in the arm of a failed match",
            "fn main() {\n    let a = match missing { _ => if 2 { 3 } else { 4 } }\n    print(a + true)\n}"
                .into(),
            vec![
                (BindingNotFound, (2, 19)),
                (Mismatch, (2, 37)),
                (Mismatch, (3, 15)),
            ],
        ),
        (
            "a bad condition before the value that the function returns",
            "fn g() -> string { if 1 { 2 } else { 3 } }\nfn main() {}\n".into(),
            vec![(Mismatch, (1, 20)), (Mismatch, (1, 23))],
        ),
    ];
    error_rows(&rows);
}

/// GIVEN a failed guard, subject or condition that the fix of #652 does not reach
/// OR the same program, repaired
/// WHEN the program is checked
/// THEN the errors are as they were before the fix of #652
#[test]
fn a_failure_that_the_join_fix_does_not_reach_should_keep_its_errors_holds_before_and_after() {
    let rows: Vec<ErrorRow<'_>> = vec![
        (
            "a branch that uses the binding of an if let",
            "fn main() {\n    let a = if let Some(v) = 5 { v } else { 3 }\n    print(a + true)\n}"
                .into(),
            vec![(UnwrapNeedsOptional, (2, 30))],
        ),
        (
            "an enum subject that is unknown",
            "enum E {\n    A(i64)\n    B\n}\nfn main() {\n    let e = E.B\n    let a = if let E.A(v) = missing { v } else { 3 }\n    print(a + true)\n}"
                .into(),
            vec![(BindingNotFound, (7, 29))],
        ),
        (
            "a while let subject that is no optional",
            "fn main() {\n    while let Some(v) = 5 { print(v + true) }\n}".into(),
            vec![(UnwrapNeedsOptional, (2, 25))],
        ),
        (
            "a guard that is not bool",
            "fn main() {\n    let k = 1\n    let a = match k { 0 if 1 => 2; _ => 3 }\n    print(a + true)\n}"
                .into(),
            vec![(Mismatch, (3, 28)), (Mismatch, (4, 15))],
        ),
        (
            "a condition that is not bool, and branches that never end",
            "fn g() -> i64 {\n    let a = if 1 { return 1 } else { return 2 }\n    a + true\n}\nfn main() { print(g()) }\n"
                .into(),
            vec![(NoValue, (2, 13)), (Mismatch, (2, 16))],
        ),
        (
            "an unknown condition",
            "fn main() {\n    let a = if missing { 2 } else { 3 }\n    print(a + true)\n}".into(),
            vec![(BindingNotFound, (2, 16)), (Mismatch, (3, 15))],
        ),
        (
            "the same, with the condition repaired",
            "fn g() -> string { if true { 2 } else { 3 } }\nfn main() {}\n".into(),
            vec![(Mismatch, (1, 20))],
        ),
    ];
    error_rows(&rows);
}
