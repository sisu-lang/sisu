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

/// Whether `d`, an E0043 for a value that is not `never`, is the report of the kind with
/// `unit_call`. A diagnostic is only `Clone` and `Debug`, so the test compares what the kind
/// renders, and the fixtures pin the text.
pub(super) fn names_unit_call(d: &Diagnostic, unit_call: Option<&str>) -> bool {
    let kind = DiagnosticKind::NoValue {
        at: d.span(),
        never: false,
        unit_call: unit_call.map(str::to_string),
    };
    renders_as(d, kind)
}

/// Whether `d` is the report that `kind` renders. A diagnostic is only `Clone` and `Debug`.
fn renders_as(d: &Diagnostic, kind: DiagnosticKind) -> bool {
    format!("{d:?}") == format!("{:?}", Diagnostic::from(kind))
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
        // The kind says which function returns `unit`: the note's text is in the fixtures.
        if let (Some((_, call)), [d]) = (no_value, ds.as_slice())
            && !names_unit_call(d, call)
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
            "class Cell<T> {\n    value: T\n}\n\nfn size(c: Cell<unit>) -> i64 {\n    0\n}\n\nfn main() {}\n",
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
    let kind = DiagnosticKind::BodyHasNoValue {
        function: "five".to_string(),
        want: "i64".to_string(),
        at: d.span(),
        unit_call: Some("g".to_string()),
    };
    assert!(renders_as(&d, kind));
}
