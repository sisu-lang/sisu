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
/// a `unit`. Beside a `unit?` value, the loop is a `unit?`. A bare `break` beside a value of
/// another type stays E0043.
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
    assert_eq!(
        loop_type(&in_f(
            "fn f(c: bool, o: U?) -> unit? { loop { if c { break o?.log() }; break } }"
        )),
        Type::optional(Type::UNIT)
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
        (
            "unit? value",
            "let x = loop { if c { break o?.log() }; break }",
        ),
        ("statement", "loop { if c { break g() }; break }"),
        ("unit? statement", "loop { if c { break o?.log() }; break }"),
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
/// can be in the body, in the value or in the context of the loop. A bare `break` beside an `i64`
/// value stays E0043, also when the body has an error.
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
            vec![(Code::Mismatch, "o?.log()", false)],
        ),
        (
            "i64 value after a unit value",
            "loop { if c { break g() }; if d { break 1 }; break }".into(),
            vec![(Code::Mismatch, "1 }", false)],
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

/// A bare `break` beside a `break` whose value is a `unit?` gives the loop a wrapped `unit`, a
/// `Some`. This test passes before and after the fix of its commit: it pins the value.
#[test]
fn a_bare_break_beside_an_optional_unit_break_should_hold_a_wrapped_unit() {
    with_prelude();
    contains(
        &with_safe_call("let x = loop { if c { break o?.log() }; break }"),
        &["(break (wrap ()))"],
    );
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

/// The note of E0043 and E0031 names the call that gives the `unit`. `dbg` returns its argument,
/// so the note names the call in it.
#[test]
fn the_unit_note_should_name_the_call_in_dbg() {
    for (row, src, called) in [
        ("call", "g()", Some("g")),
        ("dbg of a call", "dbg(g())", Some("g")),
        ("dbg of dbg", "dbg(dbg(g()))", Some("g")),
        ("dbg of a local", "dbg(u)", None),
    ] {
        let program = parse_ok(&format!("fn main() {{ {src} }}"));
        let [ast::Item::Function(f)] = program.items.as_slice() else {
            panic!("row `{row}`: the program should hold one function");
        };
        let tail = tail(&f.body).expect("the body should end in the expression");
        assert_eq!(expr::unit_call(tail).as_deref(), called, "row `{row}`");
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
    ];
    lift_rows(&rows);
}

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
    ];
    lift_rows(&rows);
}

/// A call of a `unit` function with one argument, where an `i64` is wanted, is E0043 with the
/// note that names that function. Only `dbg` names the call in its argument.
#[test]
fn a_unit_call_with_one_argument_should_name_its_function_in_the_note() {
    with_prelude();
    let src = "fn k(x: i64) {}\nfn main() {\n    let y: i64 = k(1)\n}";
    assert_eq!(reports(src), [(Code::NoValue, (3, 18), true)]);
    let program = parse_ok("fn main() { k(1) }");
    let [ast::Item::Function(f)] = program.items.as_slice() else {
        panic!("the program should hold one function");
    };
    let tail = tail(&f.body).expect("the body should end in the call");
    assert_eq!(expr::unit_call(tail).as_deref(), Some("k"));
}
