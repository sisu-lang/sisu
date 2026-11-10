//! The checker's expected type: a failed array context waits for the next run, and a string's
//! `get` with no range gives E0032.

use super::*;
use Code::{
    AssignToSelf, BindingNotFound, CannotExtend, DefinedTwice, MethodNeedsSelf, Mismatch, NoMain,
    SelfOutside, SelfOutsideMethod, UnknownType,
};

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

const FAILED_LET: Wanted = (UnknownType, (4, 13));
const GET: Wanted = (Mismatch, (4, 15));

/// GIVEN an array whose context failed, and a later element or branch that does not fit
/// WHEN the program is checked
/// THEN it reports the failure, and an element that does not fit an earlier element,
/// AND a mistake inside the later element waits for the next run
#[test]
fn a_later_mistake_in_a_failed_array_should_wait() {
    let rows: Vec<ErrorRow<'_>> = vec![
        ("arg if", format!("\"abc\".get([{IF}])"), vec![GET]),
        ("arg if let", format!("\"abc\".get([{IFLET}])"), vec![GET]),
        (
            "let if",
            format!("let _x: Missing = [{IF}]"),
            vec![FAILED_LET],
        ),
        (
            "let if let",
            format!("let _x: Missing = [{IFLET}]"),
            vec![FAILED_LET],
        ),
        (
            "let loop empty",
            format!("let _x: Missing = [{LS}]"),
            vec![FAILED_LET],
        ),
        (
            "let loop mismatch",
            format!("let _x: Missing = [{LM}]"),
            vec![FAILED_LET],
        ),
        (
            "let loop none",
            format!("let _x: Missing = [{LN}]"),
            vec![FAILED_LET],
        ),
        (
            "second if",
            format!("let _x: Missing = [1, {IF}]"),
            vec![FAILED_LET, (Mismatch, (4, 27))],
        ),
        (
            "second if let",
            format!("let _x: Missing = [1, {IFLET}]"),
            vec![FAILED_LET, (Mismatch, (4, 27))],
        ),
        (
            "second loop empty",
            format!("let _x: Missing = [1, {LS}]"),
            vec![FAILED_LET],
        ),
        (
            "second loop mismatch",
            format!("let _x: Missing = [1, {LM}]"),
            vec![FAILED_LET],
        ),
        (
            "second loop none",
            format!("let _x: Missing = [1, {LN}]"),
            vec![FAILED_LET],
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

/// GIVEN an array whose context failed, and a `loop` element
/// AND a mismatch between its `break` values, between the elements, or between the branches of
/// an `if` that holds it
/// WHEN the program is checked
/// THEN it reports the failure only, and the mismatch waits for the next run
#[test]
fn a_mismatch_with_a_loop_in_a_failed_array_should_wait() {
    let rows: Vec<ErrorRow<'_>> = vec![
        (
            "branch loop",
            "let _x: Missing = [if c { loop { break 1 } } else { true }]".to_string(),
            vec![FAILED_LET],
        ),
        (
            "nested loop break",
            "let _x: Missing = [loop { if c { break loop { break 1 } }; break \"a\" }]".to_string(),
            vec![FAILED_LET],
        ),
        (
            "loop then bool",
            "let _x: Missing = [loop { break 1 }, true]".to_string(),
            vec![FAILED_LET],
        ),
        (
            "bool then loop",
            "let _x: Missing = [1, true, loop { break 2 }]".to_string(),
            vec![FAILED_LET],
        ),
    ]
    .into_iter()
    .map(|(name, stmt, wanted)| (name, program(&stmt), wanted))
    .collect();
    error_rows(&rows);
}

/// GIVEN an array whose context failed, and an element that is a branch of arrays
/// WHEN the branches do not join
/// THEN the mismatch is at the whole branch
#[test]
fn a_nested_branch_mismatch_in_a_failed_array_should_be_at_the_whole_branch() {
    let rows = [(
        "nested if",
        program("let _x: Missing = [if c { [1] } else { [\"a\"] }]"),
        vec![FAILED_LET, (Mismatch, (4, 44))],
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

const EXTEND_MISSING: Wanted = (UnknownType, (1, 8));

/// The program of a row: `line` is line 2 of an `extend` block of a type that does not exist.
fn failed_extend(line: &str) -> String {
    format!("extend Missing {{\n    {line}\n}}\nfn main() {{}}\n")
}

/// GIVEN a block that failed (its target, its interface or its parameters), or `self` and `Self`
/// outside a method
/// WHEN the program is checked
/// THEN it gives the errors that the target and the plain forms give, and no more
#[test]
fn a_failed_self_should_give_no_error_of_its_own() {
    let rows: Vec<ErrorRow<'_>> = vec![
        (
            "extend i64",
            "extend Missing {\n    fn m(self) -> i64 { self }\n}\nfn main() {}\n".into(),
            vec![EXTEND_MISSING],
        ),
        (
            "extend Self",
            "extend Missing {\n    fn m(self) -> Self { self }\n}\nfn main() {}\n".into(),
            vec![EXTEND_MISSING],
        ),
        (
            "extend param",
            "extend Missing {\n    fn m(self, o: Self) {}\n}\nfn main() {}\n".into(),
            vec![EXTEND_MISSING],
        ),
        (
            "extend field",
            "extend Missing {\n    fn m(self) -> i64 { self.x + true }\n}\nfn main() {}\n".into(),
            vec![EXTEND_MISSING],
        ),
        (
            "extend mut",
            "extend Missing {\n    fn m(mut self) { self = 1 }\n}\nfn main() {}\n".into(),
            vec![EXTEND_MISSING, (AssignToSelf, (2, 22))],
        ),
        (
            "extend unused",
            "extend<T> i64 {\n    fn m(self) -> i64 { self }\n}\nfn main() {}\n".into(),
            vec![(CannotExtend, (1, 11))],
        ),
        (
            "extend interface",
            "extend i64: Missing {\n    fn m(self) -> i64 { self }\n}\nfn main() {}\n".into(),
            vec![(CannotExtend, (1, 8)), (UnknownType, (1, 13))],
        ),
        (
            "self outside",
            "fn f() -> i64 { self }\nfn main() {}\n".into(),
            vec![(SelfOutsideMethod, (1, 17))],
        ),
        (
            "Self outside",
            "fn f(x: Self) {}\nfn main() {}\n".into(),
            vec![(SelfOutside, (1, 9))],
        ),
        (
            "main in failed extend",
            "extend Missing {\n    fn main() {}\n}\n".into(),
            vec![(NoMain, (1, 1)), EXTEND_MISSING, (MethodNeedsSelf, (2, 8))],
        ),
    ];
    error_rows(&rows);
}

/// GIVEN a program whose `main` is defined twice, or that has a `main` method but no `main`
/// WHEN the program is checked
/// THEN a rejected `main` is not reported missing, and a method does not count as the `main`
#[test]
fn a_rejected_main_should_not_be_reported_missing() {
    let rows: Vec<ErrorRow<'_>> = vec![
        (
            "main twice",
            "fn main() {}\nfn main() {}\n".into(),
            vec![(DefinedTwice, (2, 4))],
        ),
        (
            "main method of a failed block",
            "extend i64 {\n    fn main(self) {}\n}\n".into(),
            vec![(NoMain, (1, 1)), (CannotExtend, (1, 8))],
        ),
        (
            "main and a main method of a failed block",
            "fn main() {}\nextend i64 {\n    fn main(self) {}\n}\n".into(),
            vec![(CannotExtend, (2, 8))],
        ),
        (
            "main method of an enum",
            "enum E { A }\nextend E {\n    fn main(self) {}\n}\n".into(),
            vec![(NoMain, (1, 1))],
        ),
    ];
    error_rows(&rows);
}

/// GIVEN a signature that names `Self` as the type of a failed block
/// WHEN the program is checked
/// THEN it reports the failed target only, because every other error follows from it
#[test]
fn a_failed_self_in_a_signature_should_give_no_error_of_its_own() {
    let rows: Vec<ErrorRow<'_>> = [
        ("member param", "fn m(self, x: Self.Item) {}"),
        ("member return", "fn m(self) -> Self.Item { self }"),
        ("member array", "fn m(self, x: [Self.Item]) {}"),
        ("member optional", "fn m(self, x: Self.Item?) {}"),
        ("member nested", "fn m(self, x: Self.Item.Other) {}"),
        ("bound", "fn m<T: Self>(self, x: T) {}"),
        ("bound optional", "fn m<T: Self?>(self, x: T) {}"),
        ("bound array", "fn m<T: [Self]>(self, x: T) {}"),
        ("bound member", "fn m<T: Self.Item>(self, x: T) {}"),
    ]
    .into_iter()
    .map(|(name, line)| (name, failed_extend(line), vec![EXTEND_MISSING]))
    .chain([
        (
            "optional target",
            "extend Missing? {\n    fn m(self, o: Self.Item) -> Self { self }\n}\nfn main() {}\n".to_string(),
            vec![EXTEND_MISSING],
        ),
        (
            "failed type argument",
            "class Bq<T> { v: T }\nextend Bq<Missing> {\n    fn m(self, o: Self) -> Self.Item { o }\n}\nfn main() {}\n".to_string(),
            vec![(UnknownType, (2, 11))],
        ),
    ])
    .collect();
    error_rows(&rows);
}

/// The program of a row: `stmt` is line 6, indented by four spaces.
fn hint_program(stmt: &str) -> String {
    format!(
        "interface I {{}}\nclass P: I {{}}\nclass Q: I {{}}\nfn id<T>(x: T) -> T {{ x }}\nfn f(c: bool, d: bool, o: i64?, p: P, a: any I) {{\n    {stmt}\n}}\nfn main() {{}}\n"
    )
}

/// The program of a row with an alias: `stmt` is line 5.
fn alias_program(stmt: &str) -> String {
    format!(
        "type O = i64?\nfn id<T>(x: T) -> T {{ x }}\nclass P {{}}\nfn f(c: bool, d: bool, o: i64?, p: P) {{\n    {stmt}\n}}\nfn main() {{}}\n"
    )
}

/// GIVEN a branch, an operand or a call whose value has a hint from its context
/// WHEN the program is checked
/// THEN the hint reaches exactly its own value, and no other value
#[test]
#[expect(clippy::too_many_lines, reason = "a table of rows")]
fn a_hint_should_reach_exactly_its_value() {
    let e = |pos| vec![(Mismatch, pos)];
    let mut rows: Vec<ErrorRow<'_>> = vec![
        ("none", "let _a = if c { 1 } else { None }", vec![]),
        (
            "nested none",
            "let _a = if c { 1 } else { if d { 2 } else { None } }",
            vec![],
        ),
        (
            "tail",
            "let _a = if c { 1 } else { if d { \"a\" } else { None } }",
            e((6, 32)),
        ),
        (
            "tail in match block",
            "let _a = match c {\n        true => 1\n        false => { if d { \"a\" } else { None } }\n    }",
            e((8, 20)),
        ),
        (
            "match block none",
            "let _a = match c {\n        true => 1\n        false => { None }\n    }",
            vec![],
        ),
        (
            "coalesce neg",
            "let _a = o ?? -if d { None } else { \"a\" }",
            e((6, 41)),
        ),
        ("neg none", "let _a = if c { 1 } else { -None }", e((6, 33))),
        (
            "coalesce add",
            "let _a = o ?? 1 + if d { None } else { \"a\" }",
            e((6, 44)),
        ),
        (
            "coalesce dbg",
            "let _a = o ?? dbg(if d { None } else { \"a\" })",
            e((6, 44)),
        ),
        (
            "dbg none",
            "let _a = if c { 1 } else { dbg(None) }",
            vec![],
        ),
        ("empty", "let _a = if c { 1 } else { [] }", e((6, 32))),
        (
            "loop none",
            "let _a = loop { if c { break 1 }; break None }",
            e((6, 45)),
        ),
        (
            "loop value",
            "let _a = if c { 1 } else { loop { break \"a\" } }",
            e((6, 32)),
        ),
        ("coalesce none", "let _a = o ?? None", vec![]),
        ("coalesce empty", "let _a = o ?? []", e((6, 19))),
        ("eq empty", "let _a = o == []", e((6, 19))),
        (
            "generic none",
            "let _a = if c { 1 } else { id(None) }",
            vec![],
        ),
        (
            "generic empty",
            "let _a = if c { 1 } else { id([]) }",
            e((6, 35)),
        ),
        ("beside any none", "let _a = [a, None]", vec![]),
        ("beside any empty", "let _a = [a, []]", e((6, 18))),
        (
            "some none",
            "let _a = if c { Some(1) } else { Some(None) }",
            e((6, 43)),
        ),
        ("any array", "let _a = if c { [a] } else { [] }", vec![]),
        (
            "any array class",
            "let _a = if c { [a] } else { [p] }",
            vec![],
        ),
    ]
    .into_iter()
    .map(|(name, stmt, wanted)| (name, hint_program(stmt), wanted))
    .collect();
    rows.extend(
        [
            ("alias none", "let _a = if c { 1 } else { O.None }", vec![]),
            (
                "alias str",
                "let _a = if c { \"a\" } else { O.None }",
                e((5, 34)),
            ),
            (
                "alias some",
                "let _a = if c { 1 } else { O.Some(2) }",
                vec![],
            ),
            (
                "alias some str",
                "let _a = if c { \"a\" } else { O.Some(\"b\") }",
                vec![(Mismatch, (5, 34)), (Mismatch, (5, 41))],
            ),
        ]
        .map(|(name, stmt, wanted)| (name, alias_program(stmt), wanted)),
    );
    rows.push((
        "alias keeps hint",
        "interface I {}\nclass P: I {}\nclass Q: I {}\ntype O = P?\nfn f(a: any I, q: Q) {\n    let _a: bool = [a, O.Some(q)]\n}\nfn main() {}\n".to_string(),
        e((6, 20)),
    ));
    error_rows(&rows);
}
