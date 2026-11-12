//! A value in a failed context: a branch of an `if` or a `match`, and a value of a `break`, adds
//! no error that follows only from the failed part. A true error stays.

use super::*;
use Code::{
    ArgumentCount, BindingNotFound, CannotInfer, FunctionNotFound, IntegerDoesNotFit, Mismatch,
    NegatedU8, NoMember, NoValue, UnknownType,
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
/// AND the `if` or `match` is in a failed context: the right side of `??` after an unknown name,
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

/// A branch that must wait in a failed context, a true error and a valid program keep their
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
/// WHEN the program is checked
/// THEN only the failed value reports
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
    ]
    .into_iter()
    .map(|(name, stmt, wanted)| (name, loop_program(stmt), wanted))
    .collect();
    error_rows(&rows);
}

/// A `None` before a typed or failed value has no type to take, and a true error after a failed
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
            "typed break between",
            "let a = loop { if c { break yy }; if c { break m }; break None }",
            vec![(BindingNotFound, (6, 33)), (Mismatch, (6, 63))],
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

/// GIVEN a join in a failed context: an argument of a call with the wrong count, or the right
///   side of `??` after an unknown name
/// AND a branch with a `break` of a value, and a later branch with `break None`, to a `loop`
///   that was open before the join
/// WHEN the program is checked
/// THEN only the failed part reports
///   (the join checks its branches out of source order. So the `break None` can come before the
///   `break` that gives the loop its type. It waits for the next run.)
/// AND this holds for `if`, `else if`, `if let` and `match`, and for a `break` by a label
/// AND a first `break None` in a failed join waits too: in a failed context a value waits for
///   the next run, as in #617. Before the fix such a `None` gave E0050 at once.
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
        // branch of literals gets a hint, and it is still in a failed join.
        (
            "branch of literals",
            "let a = loop { f(if c { m } else { if c { break None } else { 8 } }) }",
            vec![call],
        ),
        // The inner join is a failed join in a loop that opens inside the outer failed join. Its
        // `break` still waits for the first `break` of that loop: the innermost join counts.
        (
            "inner failed join in a loop of a failed join",
            "f(if c { let x = loop { f(if c { if c { break Some(1) } else { 8 } } else { if c { break None }; 8 }) }; 8 } else { 9 })",
            vec![(ArgumentCount, (5, 5)), (ArgumentCount, (5, 29))],
        ),
        // The guard of an arm is checked out of source order, so its `break` waits too.
        (
            "guard of a match arm",
            "let n = 3\nlet a = loop { f(match n {\n 1 => if c { break Some(m) } else { 8 }\n x if (if c { break None } else { c }) => m\n _ => 8\n }) }",
            vec![(ArgumentCount, (6, 20))],
        ),
    ]));
}

/// A loop that opens inside a failed join, a valid program, and a join under no expectation keep
/// their output. Each row holds before and after the fix of a `break` in a failed join.
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
///   right side of `??` is a failed context
/// WHEN the checker ranks the nested branches at three depths
/// THEN it ranks each expression once: the steps above those of depth 0 grow with the depth, and
///   grow linearly
/// AND the depths are 10, 20 and 30, because 60 levels overflow the stack of a debug test thread
#[test]
fn nested_branches_in_a_failed_context_should_rank_each_expression_once() {
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
