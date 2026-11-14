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

#[test]
fn unit_use_should_report_its_row() {
    with_prelude();
    let mut red = Vec::new();
    // Each row: the use, and where E0043 is reported in it, as a column after the use's start.
    for (row, used, no_value) in [
        ("typed let", "let x: i64 = g()", Some(13)),
        ("argument", "h(g())", Some(2)),
        ("operand", "1 + g()", Some(4)),
        ("if with no else", "let x = if c { 1 }", Some(8)),
        ("untyped let", "let u = g()", None),
        ("print", "print(g())", None),
        ("interpolation", "let s = \"{g()}\"", None),
        ("dbg", "dbg(g())", None),
        ("array of unit", "let xs: [unit] = []", None),
        ("unit fallback", "x ?? g()", Some(5)),
        ("print of if", "print(if c { 1 })", Some(6)),
        ("dbg of if", "dbg(if c { 1 })", Some(4)),
        ("interpolated if", "\"{if c { 1 }}\"", Some(2)),
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
            .map(|offset| (Code::NoValue, (12, 5 + offset)))
            .collect();
        let src = with_use(used);
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
    // `w` takes a `unit?` as it is: a `unit` would be wrapped.
    for used in ["let u = if c { g() } else { None }", "let u = o?.log()"] {
        let printed = lowered(&with_use(&format!("{used}\n    let w: unit? = u")));
        assert!(!printed.contains("(wrap"), "{used}: {printed}");
    }
}
