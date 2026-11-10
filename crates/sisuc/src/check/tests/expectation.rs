//! The checker's expected type: a failed array context waits for the next run, and a string's
//! `get` with no range gives E0032.

use super::*;
use crate::front_end::checked_with;
use Code::{BindingNotFound, Mismatch, UnknownType};

const IF: &str = r#"if c { "a" } else { [] }"#;
const IFLET: &str = r#"if let Some(v) = o { "a" } else { [] }"#;
const LS: &str = r#"loop { if c { break "a" }; break [] }"#;
const LM: &str = r#"loop { if c { break 1 }; break "x" }"#;
const LN: &str = "loop { if c { break 1 }; break None }";

/// The program of a row: `stmt` is line 4, indented by four spaces.
fn program(stmt: &str) -> String {
    format!(
        "fn id<T>(x: T) -> T {{ x }}\nclass P {{}}\nfn f(c: bool, d: bool, o: i64?, p: P) {{\n    {stmt}\n}}\nfn main() {{}}\n"
    )
}

/// One row of `error_rows`: its name, its program and the errors that it must give.
type ErrorRow<'a> = (&'a str, String, Vec<Wanted>);

/// Runs every row, and asserts once at the end, so that a red run lists each red row. Only
/// errors count.
fn error_rows(rows: &[ErrorRow<'_>]) {
    let mut red = Vec::new();
    for (name, src, wanted) in rows {
        let (_, ds) = checked_with(PRELUDE_SOURCE, src);
        let found: Vec<Wanted> = ds
            .iter()
            .filter(|d| d.code().severity() == Severity::Error)
            .map(|d| (d.code(), at(src, d.span())))
            .collect();
        if found != *wanted {
            red.push(format!("row `{name}`: found {found:?}, wanted {wanted:?}"));
        }
    }
    assert!(
        red.is_empty(),
        "{} red rows:\n{}",
        red.len(),
        red.join("\n")
    );
}

const UNKNOWN_LET: Wanted = (UnknownType, (4, 13));
const GET: Wanted = (Mismatch, (4, 15));

/// GIVEN an array whose context failed, and a later element or branch that does not fit
/// WHEN the program is checked
/// THEN it reports the failure only, and the later mistake waits for the next run
#[test]
fn a_later_mistake_in_a_failed_array_should_wait() {
    let rows: Vec<ErrorRow<'_>> = vec![
        ("arg if", format!("\"abc\".get([{IF}])"), vec![GET]),
        ("arg if let", format!("\"abc\".get([{IFLET}])"), vec![GET]),
        (
            "let if",
            format!("let _x: Missing = [{IF}]"),
            vec![UNKNOWN_LET],
        ),
        (
            "let if let",
            format!("let _x: Missing = [{IFLET}]"),
            vec![UNKNOWN_LET],
        ),
        (
            "let loop empty",
            format!("let _x: Missing = [{LS}]"),
            vec![UNKNOWN_LET],
        ),
        (
            "let loop mismatch",
            format!("let _x: Missing = [{LM}]"),
            vec![UNKNOWN_LET],
        ),
        (
            "let loop none",
            format!("let _x: Missing = [{LN}]"),
            vec![UNKNOWN_LET],
        ),
        (
            "second if",
            format!("let _x: Missing = [1, {IF}]"),
            vec![UNKNOWN_LET, (Mismatch, (4, 27))],
        ),
        (
            "second if let",
            format!("let _x: Missing = [1, {IFLET}]"),
            vec![UNKNOWN_LET, (Mismatch, (4, 27))],
        ),
        (
            "second loop empty",
            format!("let _x: Missing = [1, {LS}]"),
            vec![UNKNOWN_LET],
        ),
        (
            "second loop mismatch",
            format!("let _x: Missing = [1, {LM}]"),
            vec![UNKNOWN_LET],
        ),
        (
            "second loop none",
            format!("let _x: Missing = [1, {LN}]"),
            vec![UNKNOWN_LET],
        ),
        (
            "operand if",
            format!("print(missing + [{IF}])"),
            vec![(BindingNotFound, (4, 11))],
        ),
        (
            "operand if let",
            format!("print(missing + [{IFLET}])"),
            vec![(BindingNotFound, (4, 11))],
        ),
        (
            "operand loop empty",
            format!("print(missing + [{LS}])"),
            vec![(BindingNotFound, (4, 11))],
        ),
        (
            "operand loop mismatch",
            format!("print(missing + [{LM}])"),
            vec![(BindingNotFound, (4, 11))],
        ),
        (
            "operand loop none",
            format!("print(missing + [{LN}])"),
            vec![(BindingNotFound, (4, 11))],
        ),
        ("arg loop none", format!("\"abc\".get([{LN}])"), vec![GET]),
        ("arg loop empty", format!("\"abc\".get([{LS}])"), vec![GET]),
        (
            "arg loop mismatch",
            format!("\"abc\".get([{LM}])"),
            vec![GET],
        ),
    ]
    .into_iter()
    .map(|(name, stmt, wanted)| (name, program(&stmt), wanted))
    .collect();
    error_rows(&rows);
}

/// GIVEN an array whose context failed, and an element with a branch or a `loop` whose values
/// do not join
/// WHEN the program is checked
/// THEN it reports the failure only, and the mismatch waits for the next run
#[test]
fn a_mismatch_in_a_branch_or_loop_of_a_failed_array_should_wait() {
    let rows: Vec<ErrorRow<'_>> = vec![
        (
            "branch loop",
            "let _x: Missing = [if c { loop { break 1 } } else { true }]".to_string(),
            vec![UNKNOWN_LET],
        ),
        (
            "nested loop break",
            "let _x: Missing = [loop { if c { break loop { break 1 } }; break \"a\" }]".to_string(),
            vec![UNKNOWN_LET],
        ),
        (
            "loop then bool",
            "let _x: Missing = [loop { break 1 }, true]".to_string(),
            vec![UNKNOWN_LET],
        ),
        (
            "loop break bool",
            "let _x: Missing = [loop { if c { break 1 }; break true }]".to_string(),
            vec![UNKNOWN_LET],
        ),
    ]
    .into_iter()
    .map(|(name, stmt, wanted)| (name, program(&stmt), wanted))
    .collect();
    error_rows(&rows);
}

/// GIVEN an array whose context failed, and an element that is a branch of arrays
/// WHEN the branches do not join
/// THEN the mismatch is at the whole branch and names the array types
#[test]
fn a_nested_branch_mismatch_in_a_failed_array_should_be_at_the_whole_branch() {
    let rows = [(
        "nested if",
        program("let _x: Missing = [if c { [1] } else { [\"a\"] }]"),
        vec![UNKNOWN_LET, (Mismatch, (4, 44))],
    )];
    error_rows(&rows);
}

/// GIVEN an array with no failed context
/// WHEN the program is checked
/// THEN it still reports its own mistake
#[test]
fn an_array_that_did_not_fail_should_keep_its_mistake() {
    let rows: Vec<ErrorRow<'_>> = vec![
        ("no failure if", format!("let _x = [{IF}]"), (4, 35)),
        ("no failure second", format!("let _x = [1, {IF}]"), (4, 38)),
        ("no failure loop", format!("let _x = [{LN}]"), (4, 46)),
        ("typed", format!("let _x: [string] = [{IF}]"), (4, 45)),
    ]
    .into_iter()
    .map(|(name, stmt, pos)| (name, program(&stmt), vec![(Mismatch, pos)]))
    .collect();
    error_rows(&rows);
}

/// GIVEN a string's `get` with an argument that has no range type
/// WHEN the program is checked
/// THEN it reports E0032 at the argument, and does not fail a debug assertion
#[test]
fn a_string_get_with_no_range_should_report_it() {
    let rows: Vec<ErrorRow<'_>> = [
        ("none", "\"abc\".get(None)"),
        ("empty", "\"abc\".get([])"),
        ("array none", "\"abc\".get([None])"),
        ("some none", "\"abc\".get(Some(None))"),
        ("nested empty", "\"abc\".get([[]])"),
        ("id none", "\"abc\".get(id(None))"),
        ("block empty", "\"abc\".get({ [] })"),
        ("if empty", "\"abc\".get(if c { [] } else { [] })"),
        ("block none", "\"abc\".get({ None })"),
        ("if none", "\"abc\".get(if true { None } else { None })"),
    ]
    .into_iter()
    .map(|(name, stmt)| (name, program(stmt), vec![GET]))
    .collect();
    error_rows(&rows);
}

/// GIVEN a string's `get` whose argument has a mistake of its own
/// WHEN the program is checked
/// THEN it reports that mistake only
#[test]
fn a_string_get_with_a_failed_argument_should_wait() {
    let rows = [(
        "negated if",
        program("\"abc\".get(-(if c { 1 } else { \"x\" }))"),
        vec![(Mismatch, (4, 35))],
    )];
    error_rows(&rows);
}

/// GIVEN an error earlier in the function
/// WHEN a string's `get` has no range after it
/// THEN only the first error shows
#[test]
fn a_string_get_after_an_error_should_wait() {
    codes(
        "fn f() {\n    print(missing)\n    let _a = \"abc\".get(None)\n}\nfn main() {}\n",
        &[(BindingNotFound, (2, 11))],
    );
}
