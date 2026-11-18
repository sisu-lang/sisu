//! `unit` is an ordinary value: where it is accepted, and where `E0043` stays.

use super::*;
use crate::lexer::lex;
use crate::parser::parse;

/// A `main` that has `used` on line 12, after the declarations the rows share: a function `g`
/// that returns `unit`, a function `h` over `i64`, an enum `U` with a method, and four bindings.
fn with_use(used: &str) -> String {
    format!(
        "fn g() {{}}\nfn h(a: i64) -> i64 {{ a }}\nenum U {{\n    A\n    fn log(self) {{}}\n}}\nfn main() {{\n    let c = true\n    let x: i64? = None\n    let o: U? = None\n    let n = 1\n    {used}\n}}"
    )
}

/// Whether `d` is an E0043 or an E0031 that names a call of a function of the program as its
/// cause: E0043 has a secondary label at the declaration of that function, and E0031 one at the
/// tail of the body. An `if` without an `else` as the cause adds a help. The fixtures pin the
/// text.
pub(super) fn names_a_call(d: &Diagnostic) -> bool {
    matches!(d.code(), Code::NoValue | Code::BodyHasNoValue)
        && d.secondary().len() == 1
        && d.help().is_none()
}

#[test]
fn unit_use_should_report_its_row() {
    with_prelude();
    let mut red = Vec::new();
    // Each row holds the use, and where E0043 is reported in it: a column after the use's start.
    // It holds the function that the cause says returns `unit`, when it does.
    for (row, used, no_value) in [
        ("typed let", "let x: i64 = g()", Some((13, Some("g")))),
        ("argument", "h(g())", Some((2, Some("g")))),
        ("operand", "1 + g()", Some((4, Some("g")))),
        ("if with no else", "let x = if c { 1 }", Some((8, None))),
        ("untyped let", "let u = g()", None),
        ("print", "print(g())", None),
        ("interpolation", "let s = \"{g()}\"", None),
        ("dbg", "dbg(g())", None),
        ("array of unit", "let xs: [unit] = []", None),
        ("unit fallback", "x ?? g()", Some((5, Some("g")))),
        ("print of if", "print(if c { 1 })", Some((6, None))),
        ("dbg of if", "dbg(if c { 1 })", Some((4, None))),
        ("interpolated if", "\"{if c { 1 }}\"", Some((2, None))),
        ("both branches", "let u = if c { g() } else { g() }", None),
        ("no else", "let u = if c { g() }", None),
        ("no tail", "let u = { let a = 1 }", None),
        ("bare break", "let u = loop { break }", None),
        (
            "match",
            "let u = match n {\n        1 => g()\n        _ => g()\n    }",
            None,
        ),
        ("if let", "let u = if let Some(v) = x { g() }", None),
        ("never branch", "let u = if c { return }", None),
        (
            "none beside unit",
            "let u = if c { g() } else { None }",
            None,
        ),
        ("unit through ?.", "let u = o?.log()", None),
        ("unused unit?", "o?.log()", None),
    ] {
        let wanted: Vec<_> = (no_value.into_iter())
            .map(|(offset, _)| (Code::NoValue, (12, 5 + offset)))
            .collect();
        let src = with_use(used);
        let ds = diags(&src);
        let found: Vec<_> = (ds.iter())
            .map(|d| (d.code(), at(&src, d.span())))
            .collect();
        if found != wanted {
            red.push(format!("row `{row}`: found {found:?}, wanted {wanted:?}"));
        }
        // The cause names the call: its text is in the fixtures.
        if let (Some((_, call)), [d]) = (no_value, ds.as_slice())
            && names_a_call(d) != call.is_some()
        {
            red.push(format!(
                "row `{row}`: wanted the cause of {call:?}, found {d:?}"
            ));
        }
    }
    assert!(
        red.is_empty(),
        "{} red rows:\n{}",
        red.len(),
        red.join("\n")
    );
}

/// `while` is a statement, not a value: the parser stops at it, as it does today.
#[test]
fn a_while_should_stay_a_statement() {
    let src = with_use("let u = while c {}");
    let parsed = parse(lex(&src).expect("the source should lex"), false);
    let found: Vec<_> = (parsed.diagnostics.iter())
        .map(|d| (d.code(), at(&src, d.span())))
        .collect();
    assert_eq!(found, [(Code::Expected, (12, 13))]);
}

/// `None` beside a `unit` branch joins to `unit?`, as it does beside an `i64`.
#[test]
fn none_beside_a_unit_branch_should_join_to_an_optional_unit() {
    // `w` takes a `unit?` as it is: a `unit` would be wrapped. The `?.` binds a hidden local.
    for (used, printed) in [
        ("let u = if c { g() } else { None }", "(let w#5 u#4)"),
        ("let u = o?.log()", "(let w#6 u#5)"),
    ] {
        contains(
            &with_use(&format!("{used}\n    let w: unit? = u")),
            &[printed],
        );
    }
}

/// The programs of the fixtures that gave E0059, E0098 and E0138 for a `unit` check clean.
#[test]
fn a_program_that_printed_a_unit_should_check_clean() {
    for (row, src) in [
        (
            "print",
            "fn greet() {\n    print(1)\n}\n\nfn main() {\n    print(greet())\n}\n",
        ),
        (
            "dbg",
            "fn greet() {\n    print(1)\n}\n\nfn main() {\n    dbg(greet())\n}\n",
        ),
        (
            "eprint",
            "fn greet() {\n    print(1)\n}\n\nfn main() {\n    eprint(greet())\n}\n",
        ),
        (
            "interpolation",
            "fn greet() {\n    print(\"hi\")\n}\n\nfn main() {\n    print(\"{greet()}\")\n}\n",
        ),
        (
            "type argument",
            "struct Cell<T> {\n    value: T\n}\n\nfn size(c: Cell<unit>) -> i64 {\n    0\n}\n\nfn main() {}\n",
        ),
    ] {
        clean_row(row, src);
    }
}

/// A body whose last expression is a call that returns `unit`, in a function that returns another
/// type, says which call it is. The fixture `E0031-call-tail` pins the text.
#[test]
fn a_body_that_ends_in_a_unit_call_should_name_the_call() {
    let src = "fn g() {}\nfn five() -> i64 {\n    g()\n}\nfn main() {}";
    let d = diagnostic(src, Code::BodyHasNoValue, (4, 1, "}"), &[(3, 5)]);
    assert!(names_a_call(&d));
}

/// A `main` that has `used` on line 14, after the declarations that the rows share. These are a
/// function `g` that returns `unit`, a function `h` over `i64`, and a generic `id`. Then a generic
/// `k` over `[T?]`, an enum `U` with a method, and four bindings.
fn with_names(used: &str) -> String {
    format!(
        "fn g() {{}}\nfn h(a: i64) -> i64 {{ a }}\nfn id<T>(x: T) -> T {{ x }}\nfn k<T>(xs: [T?]) {{}}\nenum U {{\n    A\n    fn log(self) {{}}\n}}\nfn main() {{\n    let c = true\n    let x: i64? = None\n    let o: U? = None\n    let n = 1\n    {used}\n}}"
    )
}

/// A row of the `with_names` tables: its name, the use, and each report wanted in it. A report
/// is its code and the text whose last start in the use is the report's start. It also says if
/// the report has the note that names a call of a `unit` function.
type UseRow<'a> = (&'a str, String, Vec<(Code, &'a str, bool)>);

/// The reports of `src`: code, line and column, and whether the report names a call as its cause
/// (see `names_a_call`).
fn reports(src: &str) -> Vec<(Code, (usize, usize), bool)> {
    (diags(src).iter())
        .map(|d| (d.code(), at(src, d.span()), names_a_call(d)))
        .collect()
}

/// Checks each row of `rows` in `with_names`, and lists every row that is red.
fn use_rows(rows: &[UseRow<'_>]) {
    rows_in(rows, with_names, 14);
}

/// Checks each row of `rows` in `template`, which puts the use on `line`, and lists every row
/// that is red.
fn rows_in(rows: &[UseRow<'_>], template: fn(&str) -> String, line: usize) {
    with_prelude();
    let mut red = Vec::new();
    for (row, used, wanted) in rows {
        let wanted: Vec<_> = (wanted.iter())
            .map(|&(code, text, note)| {
                let offset = used.rfind(text).expect("each wanted text is in its use");
                (code, (line, 5 + offset), note)
            })
            .collect();
        let src = template(used);
        // A row that makes the checker panic names itself too.
        let Ok(found) = std::panic::catch_unwind(|| reports(&src)) else {
            red.push(format!("row `{row}`: the checker panicked"));
            continue;
        };
        if found != wanted {
            red.push(format!("row `{row}`: found {found:?}, wanted {wanted:?}"));
        }
    }
    assert!(
        red.is_empty(),
        "{} red rows:\n{}",
        red.len(),
        red.join("\n")
    );
}

/// An `if` with no `else` whose branch has a value gives no value. Each use of it as a value is
/// E0043 at the `if`, also through a block, a join, a `match` arm or a generic argument. The
/// same holds for `if let`.
#[test]
fn a_valueless_if_should_report_no_value_wherever_its_value_is_used() {
    let shapes = [
        ("let", "let u = if c { 1 }"),
        ("block", "let u = { if c { 1 } }"),
        ("join", "let u = if n == 1 { if c { 1 } } else { g() }"),
        (
            "join with None",
            "let u = if n == 1 { if c { 1 } } else { None }",
        ),
        ("match arm", "let a = match n { 1 => if c { 1 }; _ => g() }"),
        ("generic argument", "let a = id(if c { 1 })"),
        ("nested generic argument", "let a = id(id(if c { 1 }))"),
        ("Some", "let a = Some(if c { 1 })"),
        ("print of a block", "print({ if c { 1 } })"),
        ("dbg of a block", "let a = dbg({ if c { 1 } })"),
        ("interpolated block", "let s = \"{ { if c { 1 } } }\""),
    ];
    let mut rows = Vec::new();
    for (row, used) in shapes {
        rows.push((row.to_string(), used.to_string(), "if c"));
        rows.push((
            format!("{row}, if let"),
            used.replace("if c", "if let Some(v) = x"),
            "if let",
        ));
    }
    let rows: Vec<UseRow<'_>> = (rows.iter())
        .map(|(row, used, at)| {
            (
                row.as_str(),
                used.clone(),
                vec![(Code::NoValue, *at, false)],
            )
        })
        .collect();
    use_rows(&rows);
}

/// A `unit` where a value of another type is wanted is E0043 at the `unit` operand, with the note
/// that names the call. Two `unit` operands, and a `unit` for a call's own type parameter, check
/// clean.
#[test]
fn a_unit_where_another_type_is_wanted_should_report_no_value() {
    let at_g = || vec![(Code::NoValue, "g()", true)];
    let rows: Vec<UseRow<'_>> = vec![
        ("negation", "let a = -g()".into(), at_g()),
        ("conversion", "let a = i64(g())".into(), at_g()),
        ("array, last", "let a = [1, g()]".into(), at_g()),
        ("array, first", "let a = [g(), 1]".into(), at_g()),
        (
            "array beside a bool",
            "let a = [g(), 1 == 1]".into(),
            at_g(),
        ),
        ("less, left", "let a = g() < 1".into(), at_g()),
        ("less, right", "let a = 1 < g()".into(), at_g()),
        ("equal, left", "let a = g() == 1".into(), at_g()),
        ("equal, right", "let a = 1 == g()".into(), at_g()),
        // The next four rows pass before and after the fix of their commit: they pin the report.
        ("typed let", "let y: i64 = g()".into(), at_g()),
        ("argument", "h(g())".into(), at_g()),
        ("arithmetic", "let a = 1 + g()".into(), at_g()),
        ("fallback", "let a = x ?? g()".into(), at_g()),
        ("equal units", "let a = g() == g()".into(), vec![]),
        ("less units", "let a = g() < g()".into(), vec![]),
        ("array of units", "let a = [g(), g()]".into(), vec![]),
        ("generic argument", "let a = id(g())".into(), vec![]),
    ];
    use_rows(&rows);
}

/// A type parameter of the body that is checked is not a hole of a call: a `unit` for it is
/// E0043, with the note.
#[test]
fn a_unit_for_a_type_parameter_of_the_body_should_report_no_value() {
    with_prelude();
    for (row, body, column) in [
        ("typed let", "let y: T = g()\n    x", 16),
        ("optional let", "let y: T? = g()\n    x", 17),
        ("return", "return g()", 12),
    ] {
        let src = format!("fn g() {{}}\nfn f<T>(x: T) -> T {{\n    {body}\n}}\nfn main() {{}}");
        assert_eq!(
            reports(&src),
            [(Code::NoValue, (3, column), true)],
            "row `{row}`"
        );
    }
}

/// `o?.log()`, where `log` returns `unit`, is a `unit?` in every position: a `let unit` of it is
/// E0032, `print` and an interpolation warn as for an `i64?`, and an `i64` gets the mismatch
/// with no note.
#[test]
fn a_safe_call_of_a_unit_method_should_be_an_optional_unit() {
    let unit_of_u = |first: &str| format!("{first}; let v: unit = u");
    let mismatch = || vec![(Code::Mismatch, "u", false)];
    let at_call = || vec![(Code::PrintsOptional, "o?.log()", false)];
    let rows: Vec<UseRow<'_>> = vec![
        ("let", unit_of_u("let u = o?.log()"), mismatch()),
        ("block", unit_of_u("let u = { o?.log() }"), mismatch()),
        (
            "generic argument",
            unit_of_u("let u = id(o?.log())"),
            mismatch(),
        ),
        (
            "join with a unit",
            unit_of_u("let u = if c { o?.log() } else { g() }"),
            mismatch(),
        ),
        ("print", "print(o?.log())".into(), at_call()),
        ("eprint", "eprint(o?.log())".into(), at_call()),
        ("interpolation", "let s = \"{o?.log()}\"".into(), at_call()),
        ("dbg", "let d = dbg(o?.log())".into(), vec![]),
        (
            "i64",
            "let z: i64 = o?.log()".into(),
            vec![(Code::Mismatch, "o?.log()", false)],
        ),
        ("statement", "o?.log()".into(), vec![]),
    ];
    use_rows(&rows);
}

/// A `unit?` binding takes each of these as it is, with no wrap: each is a `unit?` already.
#[test]
fn a_safe_call_of_a_unit_method_should_bind_an_optional_unit() {
    for used in [
        "let u = { o?.log() }",
        "let u = id(o?.log())",
        "let u = if c { o?.log() } else { g() }",
    ] {
        contains(
            &with_names(&format!("{used}\n    let w: unit? = u")),
            &["(let w#6 u#5)"],
        );
    }
}

/// A `unit` element binds `T` of a `[T?]` parameter and lifts to `T?`, as an `i64` element does.
/// A valueless `if` and a `unit` beside an `i64` stay E0043.
#[test]
fn a_unit_element_should_bind_the_type_of_an_optional_array() {
    let rows: Vec<UseRow<'_>> = vec![
        ("two units", "k([g(), g()])".into(), vec![]),
        ("one unit", "k([g()])".into(), vec![]),
        ("Some beside a unit", "k([Some(g()), g()])".into(), vec![]),
        (
            "valueless if",
            "k([if c { 1 }])".into(),
            vec![(Code::NoValue, "if c", false)],
        ),
        (
            "unit beside an i64",
            "k([1, g()])".into(),
            vec![(Code::NoValue, "g()", true)],
        ),
    ];
    use_rows(&rows);
}

/// A valueless `if` where no value is used checks as it did before `unit` became a type: a
/// statement and the body of a `unit` function warn that the `1` is not used. A function that
/// returns `i64` reports that its body has no value. A function that returns `unit?` uses the
/// value of its body, so its valueless `if` is E0043, as in a `let` of type `unit?`.
#[test]
fn a_valueless_if_should_keep_its_reports_where_no_value_is_used() {
    with_prelude();
    for (row, src, wanted) in [
        (
            "statement",
            "fn main() {\n    let c = true\n    if c { 1 }\n}",
            (Code::UnusedValue, (3, 12)),
        ),
        (
            "body of a unit function",
            "fn f(c: bool) { if c { 1 } }\nfn main() {\n    f(true)\n}",
            (Code::UnusedValue, (1, 24)),
        ),
        (
            "body of an i64 function",
            "fn f(c: bool) -> i64 { if c { 1 } }\nfn main() {\n    let a = f(true)\n}",
            (Code::BodyHasNoValue, (1, 35)),
        ),
        (
            "body of a unit? function",
            "fn f(c: bool) -> unit? { if c { 1 } }\nfn main() {\n    let a = f(true)\n}",
            (Code::NoValue, (1, 26)),
        ),
        (
            "body of a unit? function, a string",
            "fn f(c: bool) -> unit? { if c { \"s\" } }\nfn main() {\n    let a = f(true)\n}",
            (Code::NoValue, (1, 26)),
        ),
        (
            "body of a unit? function, if let",
            "fn f() -> unit? { if let x = Some(3) { x } }\nfn main() {\n    let a = f()\n}",
            (Code::NoValue, (1, 19)),
        ),
    ] {
        assert_eq!(reports(src), [(wanted.0, wanted.1, false)], "row `{row}`");
    }
}

/// A `main` that has `used` on line 12, after the declarations that the rows share: an enum `U`
/// with two variants and a method `log`, a function `g` that returns `unit`, and four bindings.
/// `o?.log()` is a `unit?`.
fn with_safe_call(used: &str) -> String {
    format!(
        "enum U {{\n    A\n    B\n    fn log(self) {{}}\n}}\nfn g() {{}}\nfn main() {{\n    let o: U? = None\n    let c = true\n    let n = 1\n    let u = U.A\n    {used}\n}}"
    )
}

/// A statement discards a `unit?` branch or arm beside a block with no value or a `None` arm. It
/// also discards a `unit?` that is the `break` value of its `loop`. Each checks clean, as a `unit`.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one table holds a row for each place that discards the value"
)]
fn a_discarded_optional_unit_should_check_clean_beside_a_block_with_no_value() {
    with_prelude();
    let in_main = [
        ("empty else", "if c { o?.log() } else {}"),
        ("else with no tail", "if c { o?.log() } else { let z = 1 }"),
        ("empty then", "if c {} else { o?.log() }"),
        ("if let", "if let Some(v) = o { o?.log() } else {}"),
        ("else if", "if c { o?.log() } else if c {} else {}"),
        ("match on an i64", "match n { 1 => o?.log(); _ => {} }"),
        ("match on an enum", "match u { U.A => o?.log(); U.B => {} }"),
        ("block arm", "match n { 1 => { o?.log() }; _ => {} }"),
        ("while body", "while c { if c { o?.log() } else {}; break }"),
        ("for body", "for i in [1] { if c { o?.log() } else {} }"),
        ("written Some", "if c { Some(g()) } else {}"),
        ("local", "let x: unit? = None; if c { x } else {}"),
        ("None arm", "match n { 1 => o?.log(); _ => None }"),
        (
            "None arm in a for body",
            "for i in [1] { match n { 1 => o?.log(); _ => None } }",
        ),
        ("loop", "loop { break o?.log() }"),
        (
            "loop with a None break",
            "loop { if c { break o?.log() } else { break None } }",
        ),
        ("inner loop", "loop { loop { break o?.log() }; break }"),
        (
            "loop in a branch",
            "if c { loop { break o?.log() } } else {}",
        ),
        (
            "loop in an arm",
            "match n { 1 => loop { break o?.log() }; _ => {} }",
        ),
        ("while with a bare break", "while c { if c { break } }"),
        (
            "else if with no else",
            "if c { o?.log() } else if c { g() }",
        ),
        (
            "else if with an empty block",
            "if c { o?.log() } else if c {}",
        ),
        (
            "arm that is an if",
            "match n { 1 => o?.log(); _ => if c { g() } }",
        ),
        (
            "if in the then branch",
            "if c { if c { g() } } else { o?.log() }",
        ),
        (
            "if after a let in the else branch",
            "if c { o?.log() } else { let z = 1; if c { g() } }",
        ),
        (
            "chain of else ifs",
            "if c { g() } else if c { o?.log() } else if c { g() }",
        ),
        (
            "else if in a for body",
            "for i in [1] { if c { o?.log() } else if c { g() } }",
        ),
        (
            "else if in a while body",
            "while c { if c { o?.log() } else if c { g() }; break }",
        ),
        (
            "else if in a block arm",
            "match n { 1 => { if c { o?.log() } else if c { g() } }; _ => {} }",
        ),
        (
            "else if after a statement",
            "if c { o?.log() } else if c { g() }\n    g()",
        ),
        (
            "else if as the tail of a while body",
            "while c { if c { break }; if c { o?.log() } else if c { g() } }",
        ),
        (
            "else if as the tail of a while let body",
            "while let Some(v) = o { if c { break }; if c { o?.log() } else if c { g() } }",
        ),
        (
            "else if as the tail of a loop body",
            "loop { if c { break }; if c { o?.log() } else if c { g() } }",
        ),
        (
            "else if in the then branch of an if without an else",
            "if c { if c { o?.log() } else if c { g() } }",
        ),
        (
            "else if in the then branch of a dropped if",
            "if c { if c { o?.log() } else if c { g() } } else {}",
        ),
        (
            "else if as an arm that is not a block",
            "match n { 1 => o?.log(); _ => if c { o?.log() } else if c { g() } }",
        ),
    ];
    for (row, used) in in_main {
        clean_row(row, &with_safe_call(used));
    }
    let tail = "fn f(o: U?, n: i64) {\n    match n { 1 => o?.log(); _ => None }\n}";
    clean_row(
        "None arm as a tail",
        &format!("{}\n{tail}", with_safe_call("")),
    );
    for (row, tail) in [
        (
            "else if as the tail of a unit function",
            "fn f(o: U?, c: bool) -> unit {\n    if c { o?.log() } else if c { g() }\n}",
        ),
        (
            "else if as the tail of main",
            "fn f(o: U?, c: bool) {\n    if c { o?.log() } else if c { g() }\n}",
        ),
    ] {
        clean_row(row, &format!("{}\n{tail}", with_safe_call("")));
    }
}

/// A comparison of a `unit` holds the other operands to `unit`, in either order. A `unit?`, a
/// `None` or a `[]` is E0032 at that operand. Only an operand whose type takes no `unit` makes
/// each `unit` operand E0043.
#[test]
fn a_comparison_of_a_unit_should_hold_the_other_operands_to_unit() {
    let rows: Vec<UseRow<'_>> = vec![
        (
            "unit and Some",
            "let a = g() < Some(g())".into(),
            vec![(Code::Mismatch, "Some(g())", false)],
        ),
        (
            "Some and unit",
            "let a = Some(g()) < g()".into(),
            vec![(Code::Mismatch, "Some(g())", false)],
        ),
        (
            "unit and a unit? local",
            "let x = Some(g()); print(g() < x)".into(),
            vec![(Code::Mismatch, "x", false)],
        ),
        (
            "unit? local and unit",
            "let x = Some(g()); let a = x < g()".into(),
            vec![(Code::Mismatch, "x <", false)],
        ),
        (
            "unit and None",
            "let a = g() < None".into(),
            vec![(Code::Mismatch, "None", false)],
        ),
        (
            "None and unit",
            "let a = None < g()".into(),
            vec![(Code::Mismatch, "None", false)],
        ),
        (
            "unit and an empty array",
            "let a = g() < []".into(),
            vec![(Code::Mismatch, "[]", false)],
        ),
        (
            "empty array and unit",
            "let a = [] < g()".into(),
            vec![(Code::Mismatch, "[]", false)],
        ),
        (
            "unit and an i64",
            "let a = g() < 1".into(),
            vec![(Code::NoValue, "g()", true)],
        ),
        (
            "i64 and unit",
            "let a = 1 < g()".into(),
            vec![(Code::NoValue, "g()", true)],
        ),
        (
            "unit equal to an i64",
            "let a = g() == 1".into(),
            vec![(Code::NoValue, "g()", true)],
        ),
        ("two units", "let a = g() < g()".into(), vec![]),
        ("two units equal", "let a = g() == g()".into(), vec![]),
        // The context adds no second report at the comparison.
        (
            "unit and None, for an i64",
            "let a: i64 = g() < None".into(),
            vec![(Code::Mismatch, "None", false)],
        ),
        (
            "unit and Some, for an i64",
            "let a: i64 = g() < Some(g())".into(),
            vec![(Code::Mismatch, "Some(g())", false)],
        ),
        (
            "unit and a unit?, for an i64",
            "let a: i64 = g() < o?.log()".into(),
            vec![(Code::Mismatch, "o?.log()", false)],
        ),
    ];
    rows_in(&rows, with_safe_call, 12);
}

/// A bare `break` beside a `break` whose value is a `unit` gives the loop a `unit`, so the loop is
/// a `unit`. A bare `break` beside a value of another type, a `unit?` too, gives E0043 at the bare
/// `break`.
#[test]
fn a_bare_break_beside_a_unit_break_should_give_the_loop_a_unit() {
    with_prelude();
    let in_f = |body: &str| {
        format!(
            "enum U {{\n    A\n    B\n    fn log(self) {{}}\n}}\nfn g() {{}}\n{body}\nfn main() {{}}"
        )
    };
    assert_eq!(
        loop_type(&in_f(
            "fn f(c: bool) { loop { if c { break g() }; break } }"
        )),
        Type::UNIT
    );
    assert_eq!(
        loop_type(&in_f(
            "fn f(c: bool) { loop { if c { break }; break g() } }"
        )),
        Type::UNIT
    );
    for (row, used) in [
        (
            "unit value first",
            "let x = loop { if c { break g() }; break }",
        ),
        (
            "bare break first",
            "let x = loop { if c { break }; break g() }",
        ),
        ("statement", "loop { if c { break g() }; break }"),
        (
            "labelled outer loop",
            "outer: loop { loop { if c { break outer g() }; break outer } }",
        ),
        (
            "for body",
            "for i in 0..2 { loop { if c { break g() }; break } }",
        ),
    ] {
        clean_row(row, &with_safe_call(used));
    }
    let rows: Vec<UseRow<'_>> = vec![
        (
            "i64 value first",
            "let x = loop { if c { break 5 }; break }".into(),
            vec![(Code::NoValue, "break }", false)],
        ),
        (
            "bare break before an i64",
            "loop { if c { break }; break 5 }".into(),
            vec![(Code::NoValue, "break }", false)],
        ),
        (
            "i64? value first",
            "let x = loop { if c { break Some(1) }; break }".into(),
            vec![(Code::NoValue, "break }", false)],
        ),
        (
            "unit? value",
            "let x = loop { if c { break o?.log() }; break }".into(),
            vec![(Code::NoValue, "break }", false)],
        ),
        (
            "unit? statement",
            "loop { if c { break o?.log() }; break }".into(),
            vec![(Code::NoValue, "break }", false)],
        ),
    ];
    rows_in(&rows, with_safe_call, 12);
}

/// The template of `with_safe_call`, with a second `bool` binding `d` in place of `n` and `u`.
fn with_two_conditions(used: &str) -> String {
    format!(
        "enum U {{\n    A\n    B\n    fn log(self) {{}}\n}}\nfn g() {{}}\nfn main() {{\n    let o: U? = None\n    let c = true\n    let d = true\n    {used}\n}}"
    )
}

/// A bare `break` beside a `break` whose value is a `unit` adds no report to an error. The error
/// can be in the body, in the value or in the context of the loop. A bare `break` beside a value
/// that is not a `unit` gives E0043 at the bare `break`, also when the body has an error.
#[test]
fn a_bare_break_beside_a_unit_break_should_add_no_report_to_an_error() {
    let rows: Vec<UseRow<'_>> = vec![
        (
            "error in the body",
            "loop { if c { break g() }; nope; break }".into(),
            vec![(Code::BindingNotFound, "nope", false)],
        ),
        (
            "error in the body of a let",
            "let x = loop { if c { break g() }; nope; break }".into(),
            vec![(Code::BindingNotFound, "nope", false)],
        ),
        (
            "error in the body before a unit value",
            "loop { if c { break }; nope; break g() }".into(),
            vec![(Code::BindingNotFound, "nope", false)],
        ),
        (
            "error in the value",
            "loop { if c { break g(nope) }; break }".into(),
            vec![
                (Code::ArgumentCount, "g(nope)", false),
                (Code::BindingNotFound, "nope", false),
            ],
        ),
        (
            "error in an inner loop",
            "loop { if c { break g() }; loop { nope; break }; break }".into(),
            vec![(Code::BindingNotFound, "nope", false)],
        ),
        (
            "error through a label",
            "outer: loop { loop { if c { break outer g() }; nope; break outer } }".into(),
            vec![(Code::BindingNotFound, "nope", false)],
        ),
        (
            "unit? value after a unit value",
            "loop { if c { break g() }; if d { break o?.log() }; break }".into(),
            vec![(Code::NoValue, "break }", false)],
        ),
        (
            "i64 value after a unit value",
            "loop { if c { break g() }; if d { break 1 }; break }".into(),
            vec![(Code::NoValue, "break }", false)],
        ),
        (
            "failed coalesce",
            "let _a = missing ?? loop { if c { break g() }; break }".into(),
            vec![(Code::BindingNotFound, "missing", false)],
        ),
        (
            "failed call",
            "let _d = i64(loop { if c { break g() }; break }, 1)".into(),
            vec![(Code::ArgumentCount, "i64(", false)],
        ),
        (
            "i64 value and an error in the body",
            "let x = loop { if c { break 1 }; nope; break }".into(),
            vec![
                (Code::BindingNotFound, "nope", false),
                (Code::NoValue, "break }", false),
            ],
        ),
        (
            "i64 values and an error in the body",
            "let x = loop { if c { break 1 }; nope; break 2 }".into(),
            vec![(Code::BindingNotFound, "nope", false)],
        ),
    ];
    rows_in(&rows, with_two_conditions, 11);
}

/// A `main` that has `used` on line 20, after generic declarations that take a `unit` for `T`
/// beside a `T?` or a `[T]`: functions, a struct, an enum and a method.
fn with_generics(used: &str) -> String {
    format!(
        "fn g() {{}}\nfn opt<T>(o: T?, d: T) -> T {{ o ?? d }}\nfn later<T>(d: T, o: T?) -> T {{ o ?? d }}\nfn listed<T>(o: [T], d: T) -> T {{ d }}\nfn two<T>(a: T?, b: T?, c: T) {{}}\nfn wrapped<T>(_x: T?) {{}}\nstruct P<T> {{\n    a: T?\n    b: T\n}}\nenum Bx<T> {{\n    One(T?)\n    Two(T?, T)\n}}\nstruct K {{\n    n: i64\n    fn m<T>(self, a: T?, b: T) {{}}\n}}\nfn main() {{\n    {used}\n}}"
    )
}

/// A `unit` argument binds `T` before the arguments that wait for it, as a `unit` element does.
/// So a `None` or a `[]` of the same `T` meets `unit`, in any order.
#[test]
fn a_unit_argument_should_bind_its_parameter_before_the_arguments_that_wait() {
    let rows: Vec<UseRow<'_>> = vec![
        ("None first", "print(opt(None, g()))".into(), vec![]),
        ("unit first", "print(later(g(), None))".into(), vec![]),
        ("empty array", "listed([], g())".into(), vec![]),
        (
            "array of None",
            "listed([None], g())".into(),
            vec![(Code::Mismatch, "None", false)],
        ),
        ("two None", "two(None, None, g())".into(), vec![]),
        (
            "unit local",
            "let u = g(); print(opt(None, u))".into(),
            vec![],
        ),
        ("struct", "let _p = P(a: None, b: g())".into(), vec![]),
        ("enum variant", "let _b = Bx.Two(None, g())".into(), vec![]),
        ("method", "K(n: 1).m(None, g())".into(), vec![]),
        // This row passes before and after the fix: it pins that a `unit` binds `T` of a `T?`.
        ("optional parameter", "wrapped(g())".into(), vec![]),
    ];
    rows_in(&rows, with_generics, 20);
}

/// A `unit` argument for a `T?` parameter that it binds, or that another `unit` bound, is wrapped,
/// as an `i64` argument is.
#[test]
fn a_unit_argument_for_an_optional_parameter_should_be_wrapped() {
    with_prelude();
    for (row, used) in [
        ("function", "wrapped(g())"),
        ("bound by an earlier unit", "print(later(g(), g()))"),
        ("struct", "let _p = P(a: g(), b: g())"),
        ("enum variant", "let _b = Bx.One(g())"),
        ("method", "K(n: 1).m(g(), g())"),
    ] {
        let printed = lowered(&with_generics(used));
        assert!(
            printed.contains("(wrap (call g))"),
            "row `{row}`: {printed}"
        );
    }
}

/// A call of `unit` is no conversion: `unit` is a type with no value to write, as `bool` is.
#[test]
fn a_call_of_unit_should_report_that_unit_is_a_type() {
    for (row, used) in [
        ("one argument", "let _u = unit(1)"),
        ("no argument", "unit()"),
        ("two arguments", "unit(1, 2)"),
    ] {
        let src = format!("fn main() {{\n    {used}\n}}");
        let column = 5 + used.find("unit").expect("each row calls `unit`");
        assert_eq!(
            reports(&src),
            [(Code::TypeAsValue, (2, column), false)],
            "row `{row}`"
        );
    }
}

/// A function that returns `unit?` and ends with no value is E0031, as one that returns `i64?` is.
/// A `unit` tail lifts to `unit?`, and a `-> unit` body takes any end.
#[test]
fn an_optional_unit_body_with_no_value_should_report_that_it_has_no_value() {
    with_prelude();
    for (row, f, no_value) in [
        ("empty body", "fn f() -> unit? {}", true),
        (
            "if without else",
            "fn f(c: bool) -> unit? { if c { return None } }",
            true,
        ),
        // This row passes before and after the fix: it pins the rule that `unit?` follows.
        ("empty i64? body", "fn f() -> i64? {}", true),
        ("unit tail", "fn f() -> unit? { g() }", false),
        ("Some tail", "fn f() -> unit? { Some(g()) }", false),
        ("None tail", "fn f() -> unit? { None }", false),
        ("empty unit body", "fn f() -> unit {}", false),
        ("unit tail of a unit body", "fn f() -> unit { g() }", false),
    ] {
        let src = format!("fn g() {{}}\n{f}\nfn main() {{}}");
        // E0031 is at the `}` that ends the body.
        let wanted: Vec<_> = (no_value.then_some((Code::BodyHasNoValue, (2, f.len()), false)))
            .into_iter()
            .collect();
        assert_eq!(reports(&src), wanted, "row `{row}`");
    }
}

/// Where `unit` or `unit?` is expected, the `unit` branch fits: the branch of another type is the
/// mistake, E0032 against `unit`.
#[test]
fn a_branch_beside_a_unit_branch_should_be_the_mistake_where_unit_is_expected() {
    let rows: Vec<UseRow<'_>> = vec![
        (
            "if, unit",
            "let a: unit = if c { g() } else { 2 }".into(),
            vec![(Code::Mismatch, "2", false)],
        ),
        (
            "if, unit?",
            "let b: unit? = if c { g() } else { 2 }".into(),
            vec![(Code::Mismatch, "2", false)],
        ),
        (
            "match, unit",
            "let m: unit = match c { true => g(); false => 3 }".into(),
            vec![(Code::Mismatch, "3", false)],
        ),
        // The next three rows pass before and after the fix: they pin the reports beside it.
        (
            "no expected type",
            "let a = if c { g() } else { 2 }".into(),
            vec![(Code::NoValue, "g()", true)],
        ),
        (
            "array",
            "let xs: [unit] = [g(), 2]".into(),
            vec![(Code::Mismatch, "2", false)],
        ),
        (
            "loop",
            "let l: unit = loop { if c { break g() }; break 4 }".into(),
            vec![(Code::Mismatch, "4", false)],
        ),
    ];
    use_rows(&rows);
    with_prelude();
    let tail = "fn f(c: bool) -> unit? { if c { g() } else { 7 } }";
    let src = format!("fn g() {{}}\n{tail}\nfn main() {{}}");
    let column = 1 + tail.find('7').expect("the tail holds a 7");
    assert_eq!(
        reports(&src),
        [(Code::Mismatch, (2, column), false)],
        "function tail"
    );
}

/// The walk that fills the bare `break`s of a `unit` loop leaves a bare `break` of an inner loop
/// alone: that `break` ends the inner loop, which has no value.
#[test]
fn a_bare_break_of_an_inner_loop_should_keep_no_value() {
    with_prelude();
    contains(
        &with_safe_call("loop { if c { break g() }; loop { break }; break }"),
        &["(loop (block (break)))"],
    );
}

/// The cause of E0043 names the call that gives the `unit`, with a secondary label at its
/// declaration. `dbg` returns its argument, so the cause is the call in it.
#[test]
fn the_unit_cause_should_name_the_call_in_dbg() {
    with_prelude();
    for (row, src, declared) in [
        ("call", "g()", vec![(1, 4)]),
        ("dbg of a call", "dbg(g())", vec![(1, 4)]),
        ("dbg of dbg", "dbg(dbg(g()))", vec![(1, 4)]),
        ("dbg of a local", "dbg(u)", vec![]),
    ] {
        let program = format!(
            "fn g() {{}}\nfn main() {{\n    let u = g()\n    let y: i64 = {src}\n    print(u)\n    print(y)\n}}"
        );
        assert_eq!(
            found(&program),
            [(Code::NoValue, (4, 18), declared)],
            "row `{row}`"
        );
    }
}

/// A program with `used` in `main` and `decl` after it, after the declarations that the rows
/// share: a function `g` that returns `unit`, a function `h` over `unit?`, a generic `w` over
/// `T?`, an enum `U` with a method, and four bindings. `decl` is last, so a row finds the end of
/// its body as the last `}`.
fn with_lift(used: &str, decl: &str) -> String {
    format!(
        "fn g() {{}}\nfn h(x: unit?) {{}}\nfn w<T>(x: T?) {{}}\nenum U {{\n    A\n    fn log(self) {{}}\n}}\nfn main() {{\n    let c = false\n    let d = true\n    let o: i64? = None\n    let p: U? = None\n    {used}\n}}\n{decl}"
    )
}

/// A row of the `with_lift` tables: its name, the use, the declaration, and each report wanted
/// in it. A report is its code and the text whose last start in the program is the report's start.
type LiftRow<'a> = (&'a str, &'a str, &'a str, Vec<(Code, &'a str)>);

/// Checks each row of `rows` in `with_lift`, and lists every row that is red.
fn lift_rows(rows: &[LiftRow<'_>]) {
    with_prelude();
    let mut red = Vec::new();
    for (row, used, decl, wanted) in rows {
        let src = with_lift(used, decl);
        let wanted: Vec<_> = (wanted.iter())
            .map(|&(code, text)| {
                let offset = src.rfind(text).expect("each wanted text is in the program");
                (code, line_col(&src, offset))
            })
            .collect();
        let found: Vec<_> = (diags(&src).iter())
            .map(|d| (d.code(), at(&src, d.span())))
            .collect();
        if found != wanted {
            red.push(format!("row `{row}`: found {found:?}, wanted {wanted:?}"));
        }
    }
    assert!(
        red.is_empty(),
        "{} red rows:\n{}",
        red.len(),
        red.join("\n")
    );
}

/// An `if` or an `if let` without an `else` gives no `unit?` value, as it gives no `i64?` value.
/// Where its `unit` lifts to `unit?`, it is E0043 at that `if`. The value can come through a
/// block, a branch, an arm, a `return`, an argument, an element, a `break`, an `==` or a `??`.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one table holds a row for each place that lifts the value"
)]
fn an_if_without_else_should_give_no_optional_unit_value() {
    let at = |text| vec![(Code::NoValue, text)];
    let rows = [
        (
            "else if in a body",
            "",
            "fn b(c: bool, d: bool) -> unit? { if c { g() } else if d { g() } }",
            at("if d"),
        ),
        (
            "branch beside None in a body",
            "",
            "fn f(c: bool, d: bool) -> unit? { if c { if d { g() } } else { None } }",
            at("if d"),
        ),
        (
            "return",
            "",
            "fn r(c: bool) -> unit? { return if c { g() } }",
            at("if c"),
        ),
        (
            "match arm beside None",
            "",
            "fn m(c: bool, n: i64) -> unit? { match n { 1 => if c { g() }; _ => None } }",
            at("if c"),
        ),
        ("let", "let y: unit? = if c { g() }", "", at("if c")),
        ("block", "let y: unit? = { if c { g() } }", "", at("if c")),
        (
            "else of a let",
            "let y: unit? = if c { g() } else { if d { g() } }",
            "",
            at("if d"),
        ),
        (
            "branch beside None",
            "let y = if c { if d { g() } } else { None }",
            "",
            at("if d"),
        ),
        ("argument", "h(if c { g() })", "", at("if c")),
        ("generic argument", "w(if c { g() })", "", at("if c")),
        (
            "element",
            "let xs: [unit?] = [if c { g() }]",
            "",
            at("if c"),
        ),
        (
            "break value",
            "let y: unit? = loop { break if c { g() } }",
            "",
            at("if c"),
        ),
        (
            "if let",
            "let y: unit? = if let x = o { g() }",
            "",
            at("if let"),
        ),
        (
            "equal to a unit?",
            "let e = if c { g() } == p?.log()",
            "",
            at("if c"),
        ),
        (
            "a unit? equal to",
            "let e = p?.log() == if c { g() }",
            "",
            at("if c"),
        ),
        (
            "fallback of a unit??",
            "let a: unit?? = None; let z = a ?? if c { g() }",
            "",
            at("if c"),
        ),
        (
            "used else if beside a unit?",
            "let y = if c { p?.log() } else if d { g() }",
            "",
            at("if d"),
        ),
        (
            "if let on an enum",
            "let v = V.B; let y: unit? = if let V.A = v { g() }",
            ENUM_V,
            at("if let"),
        ),
        (
            "if let on an enum as an argument",
            "let v = V.B; h(if let V.A = v { g() })",
            ENUM_V,
            at("if let"),
        ),
        (
            "if let on an enum in a return",
            "",
            "enum V {\n    A\n    B\n}\nfn r(v: V) -> unit? { return if let V.A = v { g() } }",
            at("if let"),
        ),
        (
            "if let on an enum in an else of a body",
            "",
            "enum V {\n    A\n    B\n}\nfn b(c: bool, v: V) -> unit? { if c { g() } else if let V.A = v { g() } }",
            at("if let"),
        ),
        (
            "if let on an enum in an else of a let",
            "let v = V.B; let y: unit? = if let V.A = v { g() } else if let V.B = v { g() }",
            ENUM_V,
            at("if let V.B"),
        ),
        (
            "untyped array beside a unit?",
            "let z = [p?.log(), if c { g() }]",
            "",
            at("if c"),
        ),
    ];
    lift_rows(&rows);
}

/// An enum `V` with two variants, for the `if let` rows of the `with_lift` tables.
const ENUM_V: &str = "enum V {\n    A\n    B\n}";

/// A `unit` value lifts to `unit?` with no report. A body that ends in an `if` without an `else`
/// is E0031, as an `i64?` body is. These rows pass before and after the fix of their
/// commit: they pin the reports beside it.
#[test]
fn a_unit_value_should_lift_and_a_body_if_should_keep_its_report() {
    let rows = [
        (
            "body if",
            "",
            "fn a(c: bool) -> unit? { if c { g() } }",
            vec![(Code::BodyHasNoValue, "}")],
        ),
        (
            "body if that returns",
            "",
            "fn n(c: bool) -> unit? { if c { return None } }",
            vec![(Code::BodyHasNoValue, "}")],
        ),
        (
            "empty body",
            "",
            "fn e() -> unit? {}",
            vec![(Code::BodyHasNoValue, "}")],
        ),
        ("unit tail", "", "fn f() -> unit? { g() }", vec![]),
        ("unit let", "let y: unit? = g()", "", vec![]),
        ("unit block", "let y: unit? = { g() }", "", vec![]),
        ("Some", "let y: unit? = Some(g())", "", vec![]),
        (
            "both branches",
            "let y: unit? = if c { g() } else { g() }",
            "",
            vec![],
        ),
        ("statement", "if c { g() }", "", vec![]),
        (
            "unit body",
            "",
            "fn t(c: bool) -> unit { if c { g() } }",
            vec![],
        ),
        ("safe call in a statement", "if c { p?.log() }", "", vec![]),
        (
            "body if let on an enum",
            "",
            "enum V {\n    A\n    B\n}\nfn a(v: V) -> unit? { if let V.A = v { g() } }",
            vec![(Code::BodyHasNoValue, "}")],
        ),
        (
            "if let on an enum in a statement",
            "let v = V.B; if let V.A = v { g() }",
            ENUM_V,
            vec![],
        ),
        (
            "if let on an enum beside a unit? in a statement",
            "let v = V.B; if c { p?.log() } else if let V.A = v { g() }",
            ENUM_V,
            vec![],
        ),
        (
            "if let on an enum with an else",
            "let v = V.B; let y: unit? = if let V.A = v { g() } else { g() }",
            ENUM_V,
            vec![],
        ),
    ];
    lift_rows(&rows);
}

/// A call of a `unit` function with one argument, where an `i64` is wanted, is E0043 whose cause
/// names that function, with a secondary label at it. Only `dbg` names the call in its argument.
#[test]
fn a_unit_call_with_one_argument_should_name_its_function_in_the_cause() {
    with_prelude();
    let src = "fn k(x: i64) {}\nfn main() {\n    let y: i64 = k(1)\n}";
    assert_eq!(found(src), [(Code::NoValue, (3, 18), vec![(1, 4)])]);
}

/// A program whose `f` holds `used` on line 9, in a `loop`, after the declarations that the rows
/// share: a `unit` function `g`, functions `h` over `ty?` and `hu` over `ty`, a generic `id` and
/// a struct `S` over `ty?`. `items`, on line 6, holds more declarations.
fn with_jump(ty: &str, items: &str, used: &str) -> String {
    format!(
        "fn g() {{}}\nfn h(x: {ty}?) {{}}\nfn hu(x: {ty}) {{}}\nfn id<T>(x: T) -> T {{ x }}\nstruct S {{ x: {ty}? }}\n{items}\nfn f(c: bool, d: bool, n: i64) {{\n    loop {{\n        {used}\n        break\n    }}\n}}\nfn main() {{ f(false, false, 1) }}"
    )
}

/// A row of the `with_jump` tables: its name, its `items`, its use, and each report wanted in it.
/// A report is its code and a text. The last start of that text in the program is its start.
type JumpRow<'a> = (&'a str, &'a str, &'a str, Vec<(Code, &'a str)>);

/// Checks each row of `rows` in `with_jump` over `unit`, and also over `i64` when `twin` is set.
/// The `i64` twin has `2` in place of each `g()`. Lists every row that is red.
fn jump_rows(rows: &[JumpRow<'_>], twin: bool) {
    with_prelude();
    let mut red = Vec::new();
    for (row, items, used, wanted) in rows {
        let mut forms = vec![("unit", with_jump("unit", items, used))];
        if twin {
            let swap = |text: &str| text.replace("unit", "i64").replace("g()", "2");
            forms.push(("i64 twin", with_jump("i64", &swap(items), &swap(used))));
        }
        for (form, src) in forms {
            let wanted: Vec<_> = (wanted.iter())
                .map(|&(code, text)| {
                    let offset = src.rfind(text).expect("each wanted text is in its program");
                    (code, line_col(&src, offset), false)
                })
                .collect();
            let Ok(found) = std::panic::catch_unwind(|| reports(&src)) else {
                red.push(format!("row `{row}`, {form}: the checker panicked"));
                continue;
            };
            if found != wanted {
                red.push(format!(
                    "row `{row}`, {form}: found {found:?}, wanted {wanted:?}"
                ));
            }
        }
    }
    assert!(
        red.is_empty(),
        "{} red rows:\n{}",
        red.len(),
        red.join("\n")
    );
}

/// A block that ends with `never` keeps its unreachable tail. No code reads that tail's value,
/// so an `if` without an `else` there is not E0043, and a value there is not W0004. Each row
/// reports only the unreachable code, as its `i64` twin does.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one table holds a row for each place that reads the value"
)]
fn a_valueless_if_after_a_jump_should_report_only_unreachable_code() {
    let at_if = || vec![(Code::UnreachableCode, "if d")];
    let rows: Vec<JumpRow<'_>> = vec![
        (
            "jump on its own line",
            "",
            "let u = if c { return\n            if d { 1 } } else { g() }\n        print(u)",
            at_if(),
        ),
        (
            "jump in the else branch",
            "",
            "let u = if c { g() } else { return; if d { 1 } }\n        print(u)",
            at_if(),
        ),
        (
            "jump in a match arm",
            "",
            "let u = match n { 1 => { return; if d { 1 } }; _ => g() }\n        print(u)",
            at_if(),
        ),
        (
            "break",
            "",
            "let u = if c { break; if d { 1 } } else { g() }\n        print(u)",
            at_if(),
        ),
        (
            "continue",
            "",
            "let u = if c { continue; if d { 1 } } else { g() }\n        print(u)",
            at_if(),
        ),
        (
            "panic",
            "",
            "let u = if c { panic(\"x\"); if d { 1 } } else { g() }\n        print(u)",
            at_if(),
        ),
        (
            "let of type unit?",
            "",
            "let _y: unit? = if c { return; if d { g() } } else { g() }",
            at_if(),
        ),
        (
            "argument of type unit?",
            "",
            "h(if c { return; if d { 1 } } else { g() })",
            at_if(),
        ),
        (
            "argument of type unit",
            "",
            "hu(if c { return; if d { 1 } } else { g() })",
            at_if(),
        ),
        (
            "untyped array element",
            "",
            "let a = [if c { return; if d { 1 } } else { g() }, g()]\n        print(a)",
            at_if(),
        ),
        (
            "struct field of type unit?",
            "",
            "let _s = S(x: if c { return; if d { 1 } } else { g() })",
            at_if(),
        ),
        (
            "interpolation",
            "",
            "let u = \"{if c { return; if d { 1 } } else { g() }}\"\n        print(u)",
            at_if(),
        ),
        (
            "if let on a Some",
            "",
            "if let Some(v) = Some(if c { return; if d { 1 } } else { g() }) { print(v) }",
            at_if(),
        ),
        (
            "array fill",
            "",
            "let u = [if c { return; if d { 1 } } else { g() }; 2]\n        print(u)",
            at_if(),
        ),
        (
            "body of a unit? function",
            "fn t(c: bool, d: bool) -> unit? { if c { return None; if d { 1 } } else { g() } }",
            "let _v = t(c, d)",
            at_if(),
        ),
        (
            "return in a unit? function",
            "fn t(c: bool, d: bool) -> unit? { return if c { return None; if d { 1 } } else { g() } }",
            "let _v = t(c, d)",
            at_if(),
        ),
        (
            "left operand of ==",
            "",
            "let u = (if c { return; if d { 1 } } else { g() }) == g()\n        print(u)",
            at_if(),
        ),
        (
            "right operand of ==",
            "",
            "let u = g() == (if c { return; if d { 1 } } else { g() })\n        print(u)",
            at_if(),
        ),
        (
            "generic argument",
            "",
            "let u = id(if c { return; if d { 1 } } else { g() })\n        print(u)",
            at_if(),
        ),
        (
            "if let after the jump",
            "",
            "let u = if c { return; if let Some(v) = Some(1) { v } } else { g() }\n        print(u)",
            vec![(Code::UnreachableCode, "if let")],
        ),
        (
            "body of a unit function",
            "fn t(c: bool, d: bool) -> unit { if c { panic(\"x\"); if d { 1 } } else { g() } }",
            "let _v = t(c, d)",
            at_if(),
        ),
    ];
    jump_rows(&rows, true);
}

/// The reports of an `if` without an `else` that no jump makes unreachable stay. So do those of
/// a `never` value that is read, and of a jump inside a value that the branch reads. This test
/// passes before and after the fix of its commit: it pins the reports.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one table holds a row for each place that reads the value"
)]
fn a_valueless_if_that_no_jump_skips_should_keep_its_reports() {
    let no_value = || vec![(Code::NoValue, "if d")];
    let rows: Vec<JumpRow<'_>> = vec![
        (
            "never block as the value",
            "",
            "let u = { return; if d { 1 } }\n        print(u)",
            vec![(Code::NoValue, "{ return"), (Code::UnreachableCode, "if d")],
        ),
        (
            "never block in parentheses",
            "",
            "let u = if c { ({ return; if d { 1 } }) } else { g() }\n        print(u)",
            vec![(Code::UnreachableCode, "if d")],
        ),
        (
            "never match in a branch",
            "",
            "let u = if c { match n { _ => { return; if d { 1 } } } } else { g() }\n        print(u)",
            vec![(Code::UnreachableCode, "if d")],
        ),
        (
            "reachable, else branch",
            "",
            "let u = if c { g() } else { if d { 1 } }\n        print(u)",
            no_value(),
        ),
        (
            "reachable, match arm",
            "",
            "let u = match n { 1 => { if d { 1 } }; _ => g() }\n        print(u)",
            no_value(),
        ),
        (
            "reachable, let of type unit?",
            "",
            "let _y: unit? = if c { if d { g() } } else { g() }",
            no_value(),
        ),
        (
            "reachable, argument of type unit?",
            "",
            "h(if c { if d { 1 } } else { g() })",
            no_value(),
        ),
        (
            "reachable, argument of type unit",
            "",
            "hu(if c { if d { 1 } } else { g() })",
            no_value(),
        ),
        (
            "reachable, untyped array element",
            "",
            "let a = [if c { if d { 1 } } else { g() }, g()]\n        print(a)",
            no_value(),
        ),
        (
            "reachable, struct field",
            "",
            "let _s = S(x: if c { if d { 1 } } else { g() })",
            no_value(),
        ),
        (
            "reachable, interpolation",
            "",
            "let u = \"{if c { if d { 1 } } else { g() }}\"\n        print(u)",
            no_value(),
        ),
        (
            "reachable, if let on a Some",
            "",
            "if let Some(v) = Some(if c { if d { 1 } } else { g() }) { print(v) }",
            no_value(),
        ),
        (
            "reachable, array fill",
            "",
            "let u = [if c { if d { 1 } } else { g() }; 2]\n        print(u)",
            no_value(),
        ),
        (
            "reachable, body of a unit? function",
            "fn t(c: bool, d: bool) -> unit? { if c { if d { 1 } } else { g() } }",
            "let _v = t(c, d)",
            no_value(),
        ),
        (
            "reachable, return in a unit? function",
            "fn t(c: bool, d: bool) -> unit? { return if c { if d { 1 } } else { g() } }",
            "let _v = t(c, d)",
            no_value(),
        ),
        (
            "reachable, left operand of ==",
            "",
            "let u = (if c { if d { 1 } } else { g() }) == g()\n        print(u)",
            no_value(),
        ),
        (
            "reachable, right operand of ==",
            "",
            "let u = g() == (if c { if d { 1 } } else { g() })\n        print(u)",
            no_value(),
        ),
        (
            "reachable, generic argument",
            "",
            "let u = id(if c { if d { 1 } } else { g() })\n        print(u)",
            no_value(),
        ),
        (
            "reachable, if let",
            "",
            "let u = if c { if let Some(v) = Some(1) { v } } else { g() }\n        print(u)",
            vec![(Code::NoValue, "if let")],
        ),
        (
            "reachable, body of a unit function",
            "fn t(c: bool, d: bool) -> unit { if c { if d { 1 } } else { g() } }",
            "let _v = t(c, d)",
            vec![(Code::UnusedValue, "1 }")],
        ),
    ];
    jump_rows(&rows, false);
}

/// A statement or a `unit` body discards the value of each branch, but not the unreachable tail of
/// a branch that ends with `never`: W0001 reports that tail, and W0004 does not.
#[test]
fn a_value_after_a_jump_should_report_only_unreachable_code_where_it_is_discarded() {
    with_prelude();
    let mut red = Vec::new();
    for (row, src, unused) in [
        (
            "statement",
            "fn g() {}\nfn main() {\n    let c = true\n    if c { return; 5 } else { g() }\n}",
            None,
        ),
        (
            "branch in a body",
            "fn t(c: bool) { if c { return; 5 } else { print(1) } }\nfn main() {\n    t(true)\n}",
            None,
        ),
        (
            "branch beside a value",
            "fn main() {\n    let c = true\n    if c { return; 5 } else { 6 }\n}",
            Some("6 }"),
        ),
    ] {
        let mut wanted = vec![(
            Code::UnreachableCode,
            line_col(src, src.find('5').expect("a 5")),
            false,
        )];
        wanted.extend(unused.map(|text| {
            let offset = src.find(text).expect("the unused text is in the program");
            (Code::UnusedValue, line_col(src, offset), false)
        }));
        let found = reports(src);
        if found != wanted {
            red.push(format!("row `{row}`: found {found:?}, wanted {wanted:?}"));
        }
    }
    assert!(
        red.is_empty(),
        "{} red rows:\n{}",
        red.len(),
        red.join("\n")
    );
}

/// A `main` that has `used` on line 18 and writes `p` after it, so `p` is always written. Before
/// it come `items` and the declarations the rows share. The struct `P` has two `mut self`
/// methods: `bump` returns `unit` and `get` returns `i64`. The enum `U` has a method. The
/// function `g` returns `unit`, `h` takes a `unit?` and `k` returns `i64`.
fn with_mut_self(items: &str, used: &str) -> String {
    format!(
        "struct P {{\n    x: i64\n    fn bump(mut self) {{ self.x += 1 }}\n    fn get(mut self) -> i64 {{ self.x += 1; self.x }}\n}}\nenum U {{\n    A\n    fn log(self) {{}}\n}}\nfn g() {{}}\nfn h(x: unit?) {{}}\nfn k() -> i64 {{ 1 }}\n{items}\nfn main() {{\n    let mut p = P(x: 1)\n    let c = false\n    let o: U? = None\n    {used}\n    p.x = 2\n}}"
    )
}

/// A row of the `with_mut_self` table: its name, its `items`, its use, and each report wanted in
/// it. A report is its code, a text whose last start in the program is its start, and whether it
/// names the call as its cause.
type MutSelfRow<'a> = (&'a str, &'a str, &'a str, Vec<(Code, &'a str, bool)>);

/// A call of a `mut self` method gives the value of a call of a function: `p.bump()` is a `unit`
/// value as `g()` is, and `p.get()` an `i64` as `k()` is.
#[test]
fn a_mut_self_call_should_give_the_value_of_a_function_call() {
    let rows: &[MutSelfRow<'_>] = &[
        ("bound", "", "let u = p.bump()", vec![]),
        (
            "typed i64",
            "",
            "let x: i64 = p.bump()",
            vec![(Code::NoValue, "p.bump()", true)],
        ),
        ("typed unit?", "", "let y: unit? = p.bump()", vec![]),
        ("unit? argument", "", "h(p.bump())", vec![]),
        (
            "unit? tail",
            "fn f(mut p: P) -> unit? { p.x = 2; p.bump() }",
            "f(mut p)",
            vec![],
        ),
        (
            "if for unit?",
            "",
            "let y: unit? = if c { p.bump() }",
            vec![(Code::NoValue, "if c", false)],
        ),
        (
            "if for unit? argument",
            "",
            "h(if c { p.bump() })",
            vec![(Code::NoValue, "if c", false)],
        ),
        (
            "dropped join",
            "",
            "if c { o?.log() } else if c { p.bump() }",
            vec![],
        ),
        ("array of unit", "", "let a = [p.bump(), g()]", vec![]),
        (
            "array beside i64",
            "",
            "let a = [p.bump(), 1]",
            vec![(Code::NoValue, "p.bump()", true)],
        ),
        ("equal", "", "let b = p.bump() == g()", vec![]),
        ("interpolation", "", "let s = \"{p.bump()}\"", vec![]),
        ("statement", "", "p.bump()", vec![]),
        (
            "unit tail",
            "fn t(mut p: P) -> unit { p.x = 2; p.bump() }",
            "t(mut p)",
            vec![],
        ),
        ("i64 typed", "", "let x: i64 = p.get()", vec![]),
        (
            "i64 statement",
            "",
            "p.get()",
            vec![(Code::UnusedValue, "p.get()", false)],
        ),
        (
            "after a jump",
            "",
            "let u = if c { return; if c { p.bump() } } else { p.bump() }",
            vec![(Code::UnreachableCode, "if c { p.bump() } }", false)],
        ),
    ];
    mut_self_rows(rows);
}

/// Checks each row of `rows` in `with_mut_self`, as written and with function calls. The second
/// form has `g()` in place of `p.bump()` and `k()` in place of `p.get()`, in the wanted texts too.
/// Lists every row that is red.
fn mut_self_rows(rows: &[MutSelfRow<'_>]) {
    with_prelude();
    let swap = |text: &str| text.replace("p.bump()", "g()").replace("p.get()", "k()");
    let mut red = Vec::new();
    for (row, items, used, wanted) in rows {
        let forms = [
            ("as written", with_mut_self(items, used), false),
            (
                "with functions",
                with_mut_self(&swap(items), &swap(used)),
                true,
            ),
        ];
        for (form, src, swapped) in forms {
            let wanted: Vec<_> = (wanted.iter())
                .map(|&(code, text, note)| {
                    let text = if swapped { swap(text) } else { text.to_owned() };
                    let offset = src
                        .rfind(&text)
                        .expect("each wanted text is in its program");
                    (code, line_col(&src, offset), note)
                })
                .collect();
            let Ok(found) = std::panic::catch_unwind(|| reports(&src)) else {
                red.push(format!("row `{row}`, {form}: the checker panicked"));
                continue;
            };
            if found != wanted {
                red.push(format!(
                    "row `{row}`, {form}: found {found:?}, wanted {wanted:?}"
                ));
            }
        }
    }
    assert!(
        red.is_empty(),
        "{} red rows:\n{}",
        red.len(),
        red.join("\n")
    );
}

/// GIVEN `fn g1<T>(xs: [T?])` and an array literal of `unit` values as its argument
/// WHEN no element of another type is in the array
/// THEN `T` is `unit`, as on `main`; a `unit` beside a number is E0043, in each order
#[test]
fn unit_elements_should_bind_an_open_optional_element_type() {
    let decls = "fn g1<T>(xs: [T?]) {}\nfn main() {\n    ";
    let rows: [(&str, &[Wanted]); 4] = [
        ("g1([print(1), print(2)])", &[]),
        ("g1([print(1), None])", &[]),
        ("g1([print(1), 2])", &[(Code::NoValue, (3, 9))]),
        ("g1([2, print(1)])", &[(Code::NoValue, (3, 12))]),
    ];
    for (call, expected) in rows {
        codes_row(call, &format!("{decls}{call}\n}}"), expected);
    }
}
