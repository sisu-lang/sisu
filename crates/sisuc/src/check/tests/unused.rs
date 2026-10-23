//! The unused-value warning: which expression statements and block tails drop a value.

use super::*;

/// Line 10 of `in_body(body)` is `body`, indented four spaces; a `unit` call follows it.
fn in_body(body: &str) -> String {
    format!(
        "fn f() -> i64 {{ 1 }}
fn g() {{}}
fn h() {{}}
class A {{
    fn k(self) -> i64 {{ 1 }}
    fn m(self) {{}}
}}
fn main() {{}}
fn t(c: bool, d: bool, n: i64, o: i64?, xs: [i64], a: A?) {{
    {body}
    g()
}}"
    )
}

/// The diagnostics of `src` are exactly unused-value warnings at `expected`: (line, col, the
/// text the warning covers).
fn warns(src: &str, expected: &[(usize, usize, &str)]) {
    let ds = diags(src);
    let found: Vec<_> = ds
        .iter()
        .map(|d| {
            let span = d.span();
            let (line, col) = at(src, span);
            (d.code(), line, col, &src[span.range()])
        })
        .collect();
    let expected: Vec<_> = expected
        .iter()
        .map(|&(line, col, text)| (Code::UnusedValue, line, col, text))
        .collect();
    assert_eq!(found, expected, "{src}");
}

#[test]
fn unused_value_warns() {
    warns(
        "fn main() {\n    let n = 1\n    u8(n)\n}",
        &[(3, 5, "u8(n)")],
    );
    warns("fn g() { 1 }\nfn main() {}", &[(1, 10, "1")]);
    let rows: &[(&str, &[(usize, &str)])] = &[
        ("f()", &[(5, "f()")]),
        ("if c { 1 } else { 2 }", &[(12, "1"), (23, "2")]),
        // The checker accepts an `if` with no `else` whose branch has a value.
        ("if c { f() }", &[(12, "f()")]),
        ("if let v = o { v } else { 0 }", &[(20, "v"), (31, "0")]),
        // The inner `if` joins with `None` to `i64?`, so its value is wrapped.
        (
            "if c { if d { 1 } else { 2 } } else { None }",
            &[(19, "1"), (30, "2"), (43, "None")],
        ),
        ("while f() > 0 { f() }", &[(21, "f()")]),
        ("while let v = o { v }", &[(23, "v")]),
        ("for x in xs { x }", &[(19, "x")]),
        // A block is not the value either: its tail is.
        ("({ 1 })", &[(8, "1")]),
        // A `?.` call lowers to an `if let`, whose value is the call's, as an optional.
        ("a?.k()", &[(5, "a?.k()")]),
        ("\"a\" + \"b\"", &[(5, "\"a\" + \"b\"")]),
    ];
    for (body, expected) in rows {
        let expected: Vec<_> = expected
            .iter()
            .map(|&(col, text)| (10, col, text))
            .collect();
        warns(&in_body(body), &expected);
    }
}

#[test]
fn unused_value_clean() {
    for body in [
        "let _ = u8(n)",
        "a?.m()",
        "if c { g() } else { h() }",
        "if let v = o { g() }",
        "while f() > 0 { g() }",
        "while let v = o { g() }",
        "for x in xs { g() }",
        // `never` in a branch tail that nothing uses.
        "if c { panic(\"x\") }",
        "let y = if c { 1 } else { 2 }",
    ] {
        clean(&in_body(body));
    }
    clean("fn f() -> i64 { 1 }\nfn main() {}");
    clean("fn main() {\n    panic(\"x\")\n}");
    // A `panic` that is not last makes what follows unreachable, but is no unused value.
    codes(
        "fn main() {\n    panic(\"x\")\n    print(1)\n}",
        &[(Code::UnreachableCode, (3, 5))],
    );
}

#[test]
fn unused_value_skips_errors() {
    codes(
        "fn main() {\n    undefined_name\n}",
        &[(Code::BindingNotFound, (2, 5))],
    );
    codes(
        "fn main() {\n    undefined_name\n    print(1)\n}",
        &[(Code::BindingNotFound, (2, 5))],
    );
}

#[test]
fn unused_value_fix() {
    let src = "fn g() { 1 }\nfn main() {}";
    let d = diagnostic(src, Code::UnusedValue, (1, 10, "1"), &[]);
    assert_eq!(
        fixes(src, &d),
        [(false, vec![(1, 10, "", "let _ = ".into())])]
    );
    let fixed = applied(src, &d);
    assert_eq!(fixed, "fn g() { let _ = 1 }\nfn main() {}");
    clean(&fixed);
    // An arm's body takes no `let` outside a block, so the fix writes one around it.
    let src = "fn f() -> i64 { 1 }\nfn main() {\n    match 1 {\n        t => f()\n    }\n}";
    let d = diagnostic(src, Code::UnusedValue, (4, 14, "f()"), &[]);
    assert_eq!(
        fixes(src, &d),
        [(
            false,
            vec![(4, 14, "", "{ let _ = ".into()), (4, 17, "", " }".into())]
        )]
    );
    let fixed = applied(src, &d);
    assert_eq!(
        fixed,
        "fn f() -> i64 { 1 }\nfn main() {\n    match 1 {\n        t => { let _ = f() }\n    }\n}"
    );
    clean(&fixed);
}

/// Every warning of one statement carries the fix that drops all of the statement's values, so
/// applying any one of them leaves no branch without a value beside one with a value.
#[test]
fn unused_value_fix_drops_every_branch() {
    type Warned<'a> = (usize, usize, &'a str);
    let rows: &[(&str, &[Warned])] = &[
        ("if c { 1 } else { 2 }", &[(10, 12, "1"), (10, 23, "2")]),
        // A loop body's tail is a root of its own, `discard_tail`.
        (
            "while f() > 0 { if d { 1 } else { 2 } }",
            &[(10, 28, "1"), (10, 39, "2")],
        ),
        (
            "if let v = o { v } else { 0 }",
            &[(10, 20, "v"), (10, 31, "0")],
        ),
        (
            "if c { if d { 1 } else { 2 } } else { 3 }",
            &[(10, 19, "1"), (10, 30, "2"), (10, 43, "3")],
        ),
        (
            "match n {\n        0 => {\n            1\n        }\n        _ => 2\n    }",
            &[(12, 13, "1"), (14, 14, "2")],
        ),
    ];
    for (body, expected) in rows {
        let src = in_body(body);
        warns(&src, expected);
        for d in &diags(&src) {
            clean(&applied(&src, d));
        }
    }
}

/// The fix's edits: one per value, or two around an arm's whole body, the same on each warning.
#[test]
fn unused_value_fix_edits_every_branch() {
    type Edit = (usize, usize, &'static str, String);
    let rows: &[(&str, Vec<Edit>)] = &[
        (
            "if c { 1 } else { 2 }",
            vec![
                (10, 12, "", "let _ = ".into()),
                (10, 23, "", "let _ = ".into()),
            ],
        ),
        (
            "match n {\n        0 => {\n            1\n        }\n        _ => 2\n    }",
            vec![
                (12, 13, "", "let _ = ".into()),
                (14, 14, "", "{ let _ = ".into()),
                (14, 15, "", " }".into()),
            ],
        ),
    ];
    for (body, edits) in rows {
        let src = in_body(body);
        let ds = diags(&src);
        assert_eq!(ds.len(), 2, "{ds:?}");
        for d in &ds {
            assert_eq!(fixes(&src, d), [(false, edits.clone())], "{src}");
        }
    }
}

/// A statement with up to `SHARED_FIX_SITES` values shares one fix among its warnings; with more,
/// each warning keeps its span and offers no fix, so the output stays linear.
#[test]
fn unused_value_fix_is_shared_up_to_a_cap() {
    let arms = |n: usize| {
        let numbered: Vec<_> = (0..n - 1)
            .map(|i| format!("\n        {i} => {i}"))
            .collect();
        let numbered = numbered.concat();
        in_body(&format!("match n {{{numbered}\n        _ => 0\n    }}"))
    };
    let src = arms(8);
    let ds = diags(&src);
    assert_eq!(ds.len(), 8, "{ds:?}");
    for d in &ds {
        assert_eq!(d.code(), Code::UnusedValue);
        assert_eq!(d.fixes().len(), 1);
        assert_eq!(d.fixes()[0].edits().len(), 8 * 2, "{src}");
        clean(&applied(&src, d));
    }
    let src = arms(9);
    let ds = diags(&src);
    assert_eq!(ds.len(), 9, "{ds:?}");
    for d in &ds {
        assert_eq!(d.code(), Code::UnusedValue);
        assert!(d.fixes().is_empty(), "{d:?}");
    }
}

/// An `if let` on an enum lowers to a `match`: its warnings share one fix too.
#[test]
fn unused_value_fix_drops_every_enum_if_let_branch() {
    let src = "enum E {\n    A\n    B\n}\nfn main() {\n    let e = E.A\n    if let A = e { 1 } else { 2 }\n    print(1)\n}";
    warns(src, &[(7, 20, "1"), (7, 31, "2")]);
    let ds = diags(src);
    let edits = vec![
        (7, 20, "", "let _ = ".into()),
        (7, 31, "", "let _ = ".into()),
    ];
    for d in &ds {
        assert_eq!(fixes(src, d), [(false, edits.clone())], "{src}");
        clean(&applied(src, d));
    }
}

/// Two statements never share a fix: each fix drops only its own statement's values.
#[test]
fn unused_value_fix_stays_within_its_statement() {
    let src = in_body("if c { 1 } else { 2 }\n    if d { 3 } else { 4 }");
    warns(
        &src,
        &[(10, 12, "1"), (10, 23, "2"), (11, 12, "3"), (11, 23, "4")],
    );
    let ds = diags(&src);
    for (d, line) in ds.iter().zip([10, 10, 11, 11]) {
        let edits = vec![
            (line, 12, "", "let _ = ".into()),
            (line, 23, "", "let _ = ".into()),
        ];
        assert_eq!(fixes(&src, d), [(false, edits)], "{src}");
    }
}

/// A `match` statement is not itself the value: each arm's is.
#[test]
fn match_unused_value() {
    warns(
        "enum E {\n    A\n    B\n}\nfn main() {\n    let e = E.A\n    match e {\n        A => 1\n        B => {\n            2\n        }\n    }\n    print(1)\n}",
        &[(8, 14, "1"), (10, 13, "2")],
    );
    // `if let` on an enum lowers to a `match`, and each branch's value is unused too.
    warns(
        "enum E {\n    A\n    B\n}\nfn main() {\n    let e = E.A\n    if let A = e { 1 } else { 2 }\n    print(1)\n}",
        &[(7, 20, "1"), (7, 31, "2")],
    );
}
