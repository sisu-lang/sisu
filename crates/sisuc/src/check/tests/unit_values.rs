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

/// Whether `d`, an E0043 or an E0031, has the note that a function returns `unit`: its kind has
/// a `unit_call`. Only that note adds a note to these kinds. The fixtures pin which function the
/// note names.
pub(super) fn has_unit_note(d: &Diagnostic) -> bool {
    d.notes().len() == 1
}

#[test]
fn unit_use_should_report_its_row() {
    with_prelude();
    let mut red = Vec::new();
    // Each row holds the use, and where E0043 is reported in it: a column after the use's start.
    // It holds the function that the note says returns `unit`, when it does.
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
        // The kind has a `unit_call` for a call: the note's text is in the fixtures.
        if let (Some((_, call)), [d]) = (no_value, ds.as_slice())
            && has_unit_note(d) != call.is_some()
        {
            red.push(format!(
                "row `{row}`: wanted the note of {call:?}, found {d:?}"
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
/// type, says which call it is. The fixture `E0031-unit-call` pins the text.
#[test]
fn a_body_that_ends_in_a_unit_call_should_name_the_call() {
    let src = "fn g() {}\nfn five() -> i64 {\n    g()\n}\nfn main() {}";
    let d = diagnostic(src, Code::BodyHasNoValue, (4, 1, "}"), &[]);
    assert!(has_unit_note(&d));
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

/// The reports of `src`: code, line and column, and whether the report has a note. Of the reports
/// that these rows find, only E0043 and E0031 have a note. They have it only when a call of a
/// `unit` function is the expression with no value.
fn reports(src: &str) -> Vec<(Code, (usize, usize), bool)> {
    (diags(src).iter())
        .map(|d| (d.code(), at(src, d.span()), !d.notes().is_empty()))
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
        let found = reports(&template(used));
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
/// returns `i64` reports that its body has no value.
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
    ];
    for (row, used) in in_main {
        clean_row(row, &with_safe_call(used));
    }
    let tail = "fn f(o: U?, n: i64) {\n    match n { 1 => o?.log(); _ => None }\n}";
    clean_row(
        "None arm as a tail",
        &format!("{}\n{tail}", with_safe_call("")),
    );
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
    ];
    rows_in(&rows, with_safe_call, 12);
}
