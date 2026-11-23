//! The checker's expected type: a failed array context waits for the next run, and a string's
//! `get` with no range gives E0032.

use super::*;
use Code::{
    BindingNotFound, CannotExtend, DefinedTwice, MethodNeedsSelf, Mismatch, NoMain, NoValue,
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
/// The `[]` of `IF` is an `[Any]` member (spec, Kinds). So the `if` joins a `string` with an
/// array, which is a true conflict, also in a failed context. The `if` is a nested join of the
/// array, so the array reports it at its head (ADR 0052). A context that the array then fails
/// waits.
const ARRAY_HEAD: (usize, usize) = (4, 15);

/// GIVEN an array whose context failed, and a later element or branch that does not fit
/// WHEN the program is checked
/// THEN it reports the failure, and an element that does not fit an earlier element,
/// AND an `if` or a `loop` whose values do not join is reported at the head of its array, as a
///   nested join (ADR 0052)
#[test]
fn a_later_mistake_in_a_failed_array_should_be_reported_at_its_head() {
    let rows: Vec<ErrorRow<'_>> = vec![
        (
            "arg if",
            format!("\"abc\".get([{IF}])"),
            vec![(Mismatch, ARRAY_HEAD)],
        ),
        (
            "arg if let",
            format!("\"abc\".get([{IFLET}])"),
            vec![(Mismatch, ARRAY_HEAD)],
        ),
        (
            "let if",
            format!("let _x: Missing = [{IF}]"),
            vec![FAILED_LET, (Mismatch, (4, 23))],
        ),
        (
            "let if let",
            format!("let _x: Missing = [{IFLET}]"),
            vec![FAILED_LET, (Mismatch, (4, 23))],
        ),
        (
            "let loop empty",
            format!("let _x: Missing = [{LS}]"),
            vec![FAILED_LET, (Mismatch, (4, 23))],
        ),
        (
            "let loop mismatch",
            format!("let _x: Missing = [{LM}]"),
            vec![FAILED_LET, (Mismatch, (4, 23))],
        ),
        (
            "let loop none",
            format!("let _x: Missing = [{LN}]"),
            vec![FAILED_LET],
        ),
        (
            "second if",
            format!("let _x: Missing = [1, {IF}]"),
            vec![FAILED_LET, (Mismatch, (4, 23))],
        ),
        (
            "second if let",
            format!("let _x: Missing = [1, {IFLET}]"),
            vec![FAILED_LET, (Mismatch, (4, 23))],
        ),
        (
            "second loop empty",
            format!("let _x: Missing = [1, {LS}]"),
            vec![FAILED_LET, (Mismatch, (4, 23))],
        ),
        (
            "second loop mismatch",
            format!("let _x: Missing = [1, {LM}]"),
            vec![FAILED_LET, (Mismatch, (4, 23))],
        ),
        (
            "second loop none",
            format!("let _x: Missing = [1, {LN}]"),
            vec![FAILED_LET],
        ),
        (
            "operand if",
            format!("print(missing + [{IF}])"),
            vec![(BindingNotFound, (4, 11)), (Mismatch, (4, 21))],
        ),
        (
            "operand if let",
            format!("print(missing + [{IFLET}])"),
            vec![(BindingNotFound, (4, 11)), (Mismatch, (4, 21))],
        ),
        (
            "operand loop empty",
            format!("print(missing + [{LS}])"),
            vec![(BindingNotFound, (4, 11)), (Mismatch, (4, 21))],
        ),
        (
            "operand loop mismatch",
            format!("print(missing + [{LM}])"),
            vec![(BindingNotFound, (4, 11)), (Mismatch, (4, 21))],
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
/// AND a mismatch between its `break` values, between the loop and the elements, or between the
/// branches of an `if` that holds it
/// WHEN the program is checked
/// THEN it reports the failure and the mismatch at the head of the array, as with no loop: the
///   `break` values are members of the array's join (ADR 0052)
#[test]
fn a_mismatch_with_a_loop_in_a_failed_array_should_be_reported_as_with_no_loop() {
    let rows: Vec<ErrorRow<'_>> = vec![
        (
            "branch loop",
            "let _x: Missing = [if c { loop { break 1 } } else { true }]".to_string(),
            vec![FAILED_LET, (Mismatch, (4, 23))],
        ),
        (
            "nested loop break",
            "let _x: Missing = [loop { if c { break loop { break 1 } }; break \"a\" }]".to_string(),
            vec![FAILED_LET, (Mismatch, (4, 23))],
        ),
        (
            "loop then bool",
            "let _x: Missing = [loop { break 1 }, true]".to_string(),
            vec![FAILED_LET, (Mismatch, (4, 23))],
        ),
        (
            "bool then loop",
            "let _x: Missing = [1, true, loop { break 2 }]".to_string(),
            vec![FAILED_LET, (Mismatch, (4, 23))],
        ),
    ]
    .into_iter()
    .map(|(name, stmt, wanted)| (name, program(&stmt), wanted))
    .collect();
    error_rows(&rows);
}

/// GIVEN an array whose context failed, and an element that is a branch of arrays
/// WHEN the branches do not join
/// THEN the mismatch is at the head of the outer array: the branch is a nested join (ADR 0052)
#[test]
fn a_nested_branch_mismatch_in_a_failed_array_should_be_at_the_outer_array() {
    let rows = [(
        "nested if",
        program("let _x: Missing = [if c { [1] } else { [\"a\"] }]"),
        vec![FAILED_LET, (Mismatch, (4, 23))],
    )];
    error_rows(&rows);
}

/// GIVEN an array with no failed context
/// WHEN the program is checked
/// THEN it still reports its own mistake
#[test]
fn an_array_that_did_not_fail_should_keep_its_mistake() {
    let rows: Vec<ErrorRow<'_>> = vec![
        // At the head of the array: the `if` is a nested join (ADR 0052).
        ("no failure if", format!("let _x = [{IF}]"), (4, 14)),
        ("no failure second", format!("let _x = [1, {IF}]"), (4, 14)),
        ("no failure loop", format!("let _x = [{LM}]"), (4, 14)),
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
        // At the head of the `if` (ADR 0052).
        vec![(Mismatch, (4, 17))],
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
            vec![EXTEND_MISSING],
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
fn expectation_program(stmt: &str) -> String {
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

/// GIVEN a branch, an operand or a call whose value has an expectation from its context
/// WHEN the program is checked
/// THEN the expectation reaches exactly its own value, and no other value
#[test]
#[expect(clippy::too_many_lines, reason = "a table of rows")]
fn an_expectation_should_reach_exactly_its_value() {
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
            e((6, 14)),
        ),
        (
            "tail in match block",
            "let _a = match c {\n        true => 1\n        false => { if d { \"a\" } else { None } }\n    }",
            e((6, 14)),
        ),
        (
            "match block none",
            "let _a = match c {\n        true => 1\n        false => { None }\n    }",
            vec![],
        ),
        (
            "coalesce neg",
            "let _a = o ?? -if d { None } else { \"a\" }",
            // An operand is a member, which no expectation reaches (Task 5d-2): the `string?` of
            // the `if` is reported at the `if`.
            e((6, 20)),
        ),
        ("neg none", "let _a = if c { 1 } else { -None }", e((6, 33))),
        (
            "coalesce add",
            "let _a = o ?? 1 + if d { None } else { \"a\" }",
            // As `coalesce neg`.
            e((6, 23)),
        ),
        (
            // The payload and the default join: the report is at `??` (Task 5d-2).
            "coalesce dbg",
            "let _a = o ?? dbg(if d { None } else { \"a\" })",
            e((6, 16)),
        ),
        (
            "dbg none",
            "let _a = if c { 1 } else { dbg(None) }",
            vec![],
        ),
        ("empty", "let _a = if c { 1 } else { [] }", e((6, 14))),
        // The `break` values join to an optional, as the branches of an `if` do (ADR 0052).
        (
            "loop none",
            "let _a = loop { if c { break 1 }; break None }",
            vec![],
        ),
        (
            "loop value",
            "let _a = if c { 1 } else { loop { break \"a\" } }",
            e((6, 14)),
        ),
        ("coalesce none", "let _a = o ?? None", vec![]),
        ("coalesce empty", "let _a = o ?? []", e((6, 16))),
        // The sides join, and two other types are E0047 (Task 5d-2).
        ("eq empty", "let _a = o == []", vec![(Code::CannotCompare, (6, 14))]),
        (
            "generic none",
            "let _a = if c { 1 } else { id(None) }",
            vec![],
        ),
        (
            "generic empty",
            "let _a = if c { 1 } else { id([]) }",
            e((6, 14)),
        ),
        ("beside any none", "let _a = [a, None]", vec![]),
        // At the head of the array (ADR 0052).
        ("beside any empty", "let _a = [a, []]", e((6, 14))),
        (
            // Rule A's `?`-added candidate: `i64??`, so the first branch wraps twice.
            "some none",
            "let _a = if c { Some(1) } else { Some(None) }",
            vec![],
        ),
        ("any array", "let _a = if c { [a] } else { [] }", vec![]),
        (
            "any array class",
            "let _a = if c { [a] } else { [p] }",
            vec![],
        ),
    ]
    .into_iter()
    .map(|(name, stmt, wanted)| (name, expectation_program(stmt), wanted))
    .collect();
    rows.extend(
        [
            ("alias none", "let _a = if c { 1 } else { O.None }", vec![]),
            (
                "alias str",
                "let _a = if c { \"a\" } else { O.None }",
                e((5, 14)),
            ),
            (
                "alias some",
                "let _a = if c { 1 } else { O.Some(2) }",
                vec![],
            ),
            (
                "alias some str",
                "let _a = if c { \"a\" } else { O.Some(\"b\") }",
                vec![(Mismatch, (5, 14)), (Mismatch, (5, 41))],
            ),
        ]
        .map(|(name, stmt, wanted)| (name, alias_program(stmt), wanted)),
    );
    // The elements join as the branches of an `if` do: `O.Some(q)` takes no type from `a`, so
    // its `q` is no `P`, and the array reports its conflict at its head.
    rows.push((
        "alias takes no type from a sibling",
        "interface I {}\nclass P: I {}\nclass Q: I {}\ntype O = P?\nfn f(a: any I, q: Q) {\n    let _a: bool = [a, O.Some(q)]\n}\nfn main() {}\n".to_string(),
        vec![(Mismatch, (6, 20)), (Mismatch, (6, 31))],
    ));
    error_rows(&rows);
}

/// The program of a row of `a_join_under_an_open_element_type_should_take_its_branches_type`:
/// `stmt` is line 9.
fn open_element_program(stmt: &str) -> String {
    format!(
        "fn pick<T>(a: T, _b: T) -> T {{ a }}\nfn one<T>(x: T) -> [T] {{ [x] }}\nenum E {{ A; B }}\nfn main() {{\n    let a: u8 = 3\n    let c = [1].len() == 2\n    let e = E.B\n    let p: u8? = None\n    {stmt}\n    dbg(x)\n}}\n"
    )
}

/// GIVEN an array argument of a generic call beside `[]` or `one(None)`, which leaves its element
///   type open
/// WHEN its element is an `if`, a `match` or an `if let` of a `u8` and a literal, or a typed
///   element beside a literal
/// THEN the program is valid, as on `main`
/// AND a conflict in an element, or a failed branch, reports once at its own place
#[test]
fn a_join_under_an_open_element_type_should_take_its_branches_type() {
    let rows: Vec<(&str, &str, Vec<Wanted>)> = vec![
        (
            "an `if` element",
            "let x = pick([if c { a } else { 1 }], [])",
            vec![],
        ),
        (
            "a `match` element",
            "let x = pick([match e { E.A => a; _ => 1 }], [])",
            vec![],
        ),
        (
            "an `if let` element",
            "let x = pick([if let Some(q) = p { q } else { 1 }], [])",
            vec![],
        ),
        (
            "an `if` of arrays",
            "let x = pick([if c { [a] } else { [1] }], [])",
            vec![],
        ),
        (
            "two `if` elements",
            "let x = pick([if c { a } else { 1 }, if c { a } else { 1 }], [])",
            vec![],
        ),
        (
            "an `if` element, then a typed element",
            "let x = pick([if c { a } else { 1 }, a], [])",
            vec![],
        ),
        (
            "a typed element, then an `if` element",
            "let x = pick([a, if c { a } else { 1 }], [])",
            vec![],
        ),
        (
            "a typed element, then a literal",
            "let x = pick([a, 1], [])",
            vec![],
        ),
        (
            "an `if` element in the second argument",
            "let x = pick([], [if c { a } else { 1 }])",
            vec![],
        ),
        (
            "an `if` of `None` and a value",
            "let x = pick([if c { None } else { a }], [])",
            vec![],
        ),
        (
            "an `if` of `Some` values beside `one(None)`",
            "let x = pick([if c { Some(a) } else { Some(1) }], one(None))",
            vec![],
        ),
        (
            "a generic call whose arguments conflict",
            "let x = pick([pick(a, \"s\")], [])",
            vec![(Mismatch, (9, 19))],
        ),
        (
            "an `if` with an unknown name",
            "let x = pick([if c { nope } else { 1 }], [])",
            vec![(BindingNotFound, (9, 26))],
        ),
    ];
    let rows: Vec<ErrorRow<'_>> = (rows.into_iter())
        .map(|(name, stmt, wanted)| (name, open_element_program(stmt), wanted))
        .collect();
    error_rows(&rows);
}

/// The program of a row of `a_nested_join_of_unit_values_should_be_unit`: `stmt` is line 6.
fn unit_arm_program(stmt: &str) -> String {
    format!(
        "enum E {{ A; B }}\nfn u() {{}}\nfn main() {{\n    let e = E.B\n    let c = [1].len() == 2\n    {stmt}\n}}\n"
    )
}

/// GIVEN a `match` whose arm is a `match`, an `if` or an `if let` of `unit` values
/// AND an `if` of `unit` values in a branch of an `if`
/// WHEN it is at a `let`, in `dbg` and in an array
/// THEN the program is valid, as on `main`
/// AND an `if` with no `else` in an arm reports E0043 at its `if`
#[test]
fn a_nested_join_of_unit_values_should_be_unit() {
    let rows: Vec<(&str, &str, Vec<Wanted>)> = vec![
        (
            "an `if` of `unit` values in an arm, in an array",
            "let x = [match e { E.A => if c { u() } else { u() }; _ => u() }]",
            vec![],
        ),
        (
            "an `if` of `unit` values in an arm, in `dbg`",
            "dbg(match e { E.A => if c { u() } else { u() }; _ => u() })",
            vec![],
        ),
        (
            "an `if` of `unit` values in a branch, in an array",
            "let x = [if c { if c { u() } else { u() } } else { u() }]",
            vec![],
        ),
        (
            "an `if` of `unit` values in a branch, in `dbg`",
            "dbg(if c { if c { u() } else { u() } } else { u() })",
            vec![],
        ),
        (
            "an `if` of `unit` values in a branch, at a `let`",
            "let x = if c { if c { u() } else { u() } } else { u() }",
            vec![],
        ),
        (
            "an `if let` of `unit` values in an arm, in an array",
            "let x = [match e { E.A => if let E.A = e { u() } else { u() }; _ => u() }]",
            vec![],
        ),
        (
            "an `if let` of `unit` values in an arm, in `dbg`",
            "dbg(match e { E.A => if let E.A = e { u() } else { u() }; _ => u() })",
            vec![],
        ),
        (
            "an `if let` of `unit` values in an arm, at a `let`",
            "let x = match e { E.A => if let E.A = e { u() } else { u() }; _ => u() }",
            vec![],
        ),
        (
            "an `if` of `unit` values in an arm, at a `let`",
            "let x = match e { E.A => if c { u() } else { u() }; _ => u() }",
            vec![],
        ),
        (
            "a `match` of `unit` values in an arm, in an array",
            "let x = [match e { E.A => match e { _ => u() }; _ => u() }]",
            vec![],
        ),
        (
            "a `match` of `unit` values in an arm, in `dbg`",
            "dbg(match e { E.A => match e { _ => u() }; _ => u() })",
            vec![],
        ),
        (
            "a `match` of `unit` values in an arm, at a `let`",
            "let x = match e { E.A => match e { _ => u() }; _ => u() }",
            vec![],
        ),
        (
            "an `if` with no `else`, a true E0043, in an arm, in an array",
            "let x = [match e { E.A => if c { 1 }; _ => u() }]",
            vec![(NoValue, (6, 31))],
        ),
        (
            "an `if` with no `else`, a true E0043, in an arm, in `dbg`",
            "dbg(match e { E.A => if c { 1 }; _ => u() })",
            vec![(NoValue, (6, 26))],
        ),
        (
            "an `if` with no `else`, a true E0043, in an arm, at a `let`",
            "let x = match e { E.A => if c { 1 }; _ => u() }",
            vec![(NoValue, (6, 30))],
        ),
    ];
    let rows: Vec<ErrorRow<'_>> = (rows.into_iter())
        .map(|(name, stmt, wanted)| (name, unit_arm_program(stmt), wanted))
        .collect();
    error_rows(&rows);
}

/// The program of a row of `a_held_conflict_that_the_join_opens_should_give_way_to_its_report`:
/// `stmt` is line 15.
fn held_program(stmt: &str) -> String {
    format!(
        "interface Seen {{ fn seen(self) -> i64 }}\nstruct Eye: Seen {{\n    v: i64\n    fn seen(self) -> i64 {{ self.v }}\n}}\nstruct Lid: Seen {{\n    v: i64\n    fn seen(self) -> i64 {{ self.v + 100 }}\n}}\nfn pick<T>(a: T, _b: T) -> T {{ a }}\nfn main() {{\n    let a: u8 = 3\n    let c = [1].len() == 2\n    let s: any Seen = Eye(v: 1)\n    {stmt}\n}}\n"
    )
}

/// GIVEN a generic call that waits with a held conflict, `pick(Eye(..), Lid(..))`, in a join that
///   fails
/// WHEN a candidate of the failed join, `any Seen`, opens the call
/// THEN the join reports its own conflict at its head, and the call reports nothing
/// AND when no candidate opens the call, the call reports its held conflict
/// AND an element of an array, at any depth, is opened by the element type of a candidate
#[test]
fn a_held_conflict_that_the_join_opens_should_give_way_to_its_report() {
    let rows: Vec<(&str, &str, Vec<Wanted>)> = vec![
        (
            "a held conflict that `any Seen` opens, beside a literal",
            "let _x = pick(if c { pick(Eye(v: 1), Lid(v: 2)) } else { 1 }, s)",
            vec![(Mismatch, (15, 14))],
        ),
        (
            "a held conflict that `any Seen` opens, beside `any Seen`",
            "let _x = pick(if c { pick(Eye(v: 1), Lid(v: 2)) } else { s }, s)",
            vec![],
        ),
        (
            "a held conflict that no candidate opens, beside a literal",
            "let _x = pick(pick(Eye(v: 1), Lid(v: 2)), 1)",
            vec![(Mismatch, (15, 19))],
        ),
        (
            "a held conflict of a `u8` and a `string`, beside literals",
            "let _x = pick(if c { pick(a, \"s\") } else { 1 }, 2)",
            vec![(Mismatch, (15, 26))],
        ),
        (
            "a held conflict of a `u8` and a `string`, beside a literal",
            "let _x = pick(pick(a, \"s\"), 1)",
            vec![(Mismatch, (15, 19))],
        ),
        (
            "a held conflict in a branch, beside a literal",
            "let _x = if c { pick(Eye(v: 1), Lid(v: 2)) } else { 1 }",
            vec![(Mismatch, (15, 21))],
        ),
        // The elements of an array are a join (f6-4): `any Seen` opens the call, so the array
        // reports at `[`, in each order, also as an argument, nested, and in a branch.
        (
            "an element with a held conflict, first",
            "let _x = [pick(Eye(v: 1), Lid(v: 2)), s, 1]",
            vec![(Mismatch, (15, 14))],
        ),
        (
            "an element with a held conflict, second",
            "let _x = [s, pick(Eye(v: 1), Lid(v: 2)), 1]",
            vec![(Mismatch, (15, 14))],
        ),
        (
            "an element with a held conflict, last",
            "let _x = [1, s, pick(Eye(v: 1), Lid(v: 2))]",
            vec![(Mismatch, (15, 14))],
        ),
        (
            "an element with a held conflict, beside `any Seen` only",
            "let _x = [pick(Eye(v: 1), Lid(v: 2)), s]",
            vec![],
        ),
        (
            "an element with a held conflict, after `any Seen` only",
            "let _x = [s, pick(Eye(v: 1), Lid(v: 2))]",
            vec![],
        ),
        (
            "an element with a held conflict, beside `None` and `any Seen`",
            "let _x = [pick(Eye(v: 1), Lid(v: 2)), None, s]",
            vec![],
        ),
        (
            "an array argument with a held conflict, beside a literal",
            "let _x = pick([pick(Eye(v: 1), Lid(v: 2)), s, 1], [])",
            vec![(Mismatch, (15, 14))],
        ),
        (
            "an array argument with a held conflict, beside `[]`",
            "let _x = pick([pick(Eye(v: 1), Lid(v: 2)), s], [])",
            vec![],
        ),
        (
            "a nested element with a held conflict, beside a literal",
            "let _x = [[pick(Eye(v: 1), Lid(v: 2))], [s], [1]]",
            vec![(Mismatch, (15, 14))],
        ),
        (
            "a nested fill with a held conflict, beside `[]`",
            "let _x = [[pick(Eye(v: 1), Lid(v: 2)); 2], [s], []]",
            vec![],
        ),
        (
            "an element with a held conflict, in a branch beside a literal",
            "let _x = if c { [pick(Eye(v: 1), Lid(v: 2))] } else if c { [s] } else { [1] }",
            vec![(Mismatch, (15, 14))],
        ),
        // No candidate opens the call: it reports its held conflict.
        (
            "a fill with a held conflict",
            "let _x = [pick(Eye(v: 1), Lid(v: 2)); 2]",
            vec![(Mismatch, (15, 15))],
        ),
    ];
    let rows: Vec<ErrorRow<'_>> = (rows.into_iter())
        .map(|(name, stmt, wanted)| (name, held_program(stmt), wanted))
        .collect();
    error_rows(&rows);
}
