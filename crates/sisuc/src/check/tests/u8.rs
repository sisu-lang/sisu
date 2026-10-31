//! `u8`: literals typed by context, conversions, and no mixing with `i64`.

use super::*;

#[test]
fn literal_from_each_source() {
    contains("fn main() { let x: u8 = 200 }", &["200u8"]);
    contains(
        "fn f(b: u8) -> u8 { b }\nfn main() { print(f(7)) }",
        &["(call f 7u8)"],
    );
    contains("fn g() -> u8 { 3 }\nfn main() {}", &["3u8"]);
    contains(
        "class P {\n    b: u8\n}\nfn main() { let p = P(b: 1) }",
        &["(new P 1u8)"],
    );
    contains(
        "fn main() {\n    let mut y: u8 = 0\n    y = 5\n}",
        &["(= y#0 5u8)"],
    );
}

#[test]
fn expected_flows_into_arithmetic() {
    contains("fn main() { let x: u8 = 1 + 2 }", &["(+ 1u8 2u8)"]);
}

#[test]
fn literal_typed_by_sibling() {
    contains(
        "fn f(b: u8, m: u8?, c: bool) {\n    print(b + 1)\n    print(1 + b)\n    print(b != 0)\n    print(0 < b < 10)\n    print(m == 5)\n    print(1 + 2 + b)\n    print(if c { 1 } else { b })\n}\nfn main() {}",
        &[
            "(+ b#0 1u8)",
            "(+ 1u8 b#0)",
            "(!= b#0 0u8)",
            "(< 0u8 b#0 < 10u8)",
            "(== m#1 (wrap 5u8))",
            "(+ (+ 1u8 2u8) b#0)",
            "(block 1u8)",
        ],
    );
}

#[test]
fn optional_literal() {
    contains("fn main() { let m: u8? = 1 }", &["(wrap 1u8)"]);
}

#[test]
fn branch_order() {
    contains(
        "fn f(c: bool, m: u8?) {\n    let a = if c { 1 } else { None }\n    let b = if c { None } else { 1 }\n    print(5 == m)\n}\nfn main() {}",
        &[
            "(let a#2 (if c#0 (block (wrap 1)) (block None)))",
            "(let b#3 (if c#0 (block None) (block (wrap 1))))",
            "(== (wrap 5u8) m#1)",
        ],
    );
}

#[test]
fn compound_assign_types_literal() {
    contains(
        "fn main() {\n    let mut c: u8 = 0\n    c += 250\n}",
        &["(= c#0 (+ c#0 250u8))"],
    );
    contains(
        "class P {\n    mut b: u8\n}\nfn main() {\n    let p = P(b: 0)\n    p.b += 250\n}",
        &["(= (. p#0 b) (+ (. p#0 b) 250u8))"],
    );
}

#[test]
fn conversions() {
    contains(
        "fn f(b: u8, n: i64) -> i64 { i64(b) + i64(u8(n) ?? 0) }\nfn main() {}",
        &[
            "(i64 b#0)",
            "(i64 (if-some #2 (u8? n#1) (block #2) (block 0u8)))",
        ],
    );
}

/// A conversion that can fail is an optional of its target; one that cannot is the target.
#[test]
fn conversion_types() {
    for (conversion, ty) in [
        ("u8(n)", "u8?"),
        ("char(n)", "char?"),
        ("i64(u8(n) ?? 0)", "i64"),
        ("i64('a')", "i64"),
    ] {
        let src = format!("fn main() {{\n    let n = 1\n    let x = {conversion}\n}}");
        let (program, ds) = checked(&src);
        assert!(ds.is_empty(), "{src}: {ds:?}");
        let program = program.expect("no errors");
        let x = program.functions[0].locals.last().expect("a local");
        assert_eq!(x.ty.to_string(), ty, "{src}");
    }
}

/// The optional a conversion gives is not its payload: the mismatch gets the help, and no fix,
/// since the default is not known.
#[test]
fn failed_conversion_as_value() {
    for (decl, found) in [("b: u8", "u8(n)"), ("c: char", "char(n)")] {
        let src = format!("fn main() {{\n    let n = 1\n    let {decl} = {found}\n}}");
        let col = 12 + decl.len();
        let d = diagnostic(&src, Code::Mismatch, (3, col, found), &[]);
        assert!(d.help().is_some(), "{d:?}");
        assert_eq!(fixes(&src, &d), []);
    }
    // Any other optional where its payload is expected has no such help: a name, a call of a
    // function, and a conversion whose optional is not of the type expected.
    for (src, found) in [
        ("fn f(m: u8?) {\n    let b: u8 = m\n}\nfn main() {}", "m"),
        (
            "fn g() -> u8? { 1 }\nfn f() {\n    let b: u8 = g()\n}\nfn main() {}",
            "g()",
        ),
        (
            "fn f(n: i64) {\n    let b: char = u8(n)\n}\nfn main() {}",
            "u8(n)",
        ),
    ] {
        let ds = diags(src);
        let [d] = ds.as_slice() else {
            panic!("{src}: {ds:?}")
        };
        assert_eq!(d.code(), Code::Mismatch, "{src}");
        assert_eq!(&src[d.span().range()], found, "{src}");
        assert!(d.help().is_none(), "{src}");
    }
}

/// A literal argument is no special case: the conversion is still an optional.
#[test]
fn literal_argument_is_optional() {
    codes(
        "fn main() {\n    let b = u8(65)\n    let c: u8 = b\n}",
        &[(Code::Mismatch, (3, 17))],
    );
}

/// `u8(...)` and `char(...)` give an optional, so they are offered only where one fits.
#[test]
fn conversion_fix_needs_optional() {
    let program =
        |line: &str| format!("fn main() {{\n    let n = 1\n    let k = 'a'\n    {line}\n}}");
    for (declared, fixed) in [
        ("let b: u8? = n", Some("let b: u8? = u8(n)")),
        ("let b: u8 = n", None),
        ("let c: char? = n", Some("let c: char? = char(n)")),
        ("let c: char = n", None),
        // Through `i64`, from a `char`.
        ("let b: u8? = k", Some("let b: u8? = u8(i64(k))")),
        ("let b: u8 = k", None),
    ] {
        let src = program(declared);
        let col = declared.len() + 4;
        let d = diagnostic(&src, Code::Mismatch, (4, col, &declared[col - 5..]), &[]);
        match fixed {
            Some(fixed) => {
                assert!(d.fixes().iter().all(|f| !f.safe()), "{src}");
                let fixed = program(fixed);
                assert_eq!(applied(&src, &d), fixed);
                clean(&fixed);
            }
            None => assert_eq!(fixes(&src, &d), [], "{src}"),
        }
    }
    // The right of `??` wants the payload, which a conversion does not give.
    let src = "fn f(o: u8?, n: i64) {\n    print(o ?? n)\n}\nfn main() {}";
    let d = diagnostic(src, Code::Mismatch, (2, 16, "n"), &[]);
    assert_eq!(fixes(src, &d), []);
}

#[test]
fn literal_does_not_fit() {
    codes(
        "fn main() { let x: u8 = 300 }",
        &[(Code::IntegerDoesNotFit, (1, 25))],
    );
    codes(
        "fn f(b: u8) { print(b + 300) }\nfn main() {}",
        &[(Code::IntegerDoesNotFit, (1, 25))],
    );
}

#[test]
fn hex_literal_follows_the_u8_rules() {
    contains("fn main() { let b: u8 = 0xFF }", &["255u8"]);
    // The error quotes the literal as written.
    codes(
        "fn main() { let b: u8 = 0x100 }",
        &[(Code::IntegerDoesNotFit, (1, 25))],
    );
}

#[test]
fn u8_cannot_be_negative() {
    codes(
        "fn main() { let x: u8 = -1 }",
        &[(Code::NegatedU8, (1, 25))],
    );
    // One diagnostic: the literal's size is not reported as well.
    codes(
        "fn main() { let x: u8 = -300 }",
        &[(Code::NegatedU8, (1, 25))],
    );
    // The smallest `i64` is one literal that is negative, not a `u8` that does not fit.
    codes(
        "fn main() { let x: u8 = -9223372036854775808 }",
        &[(Code::NegatedU8, (1, 25))],
    );
    // Parenthesized, it is reported at the whole expression, the `(`, not at the literal.
    codes(
        "fn main() { let x: u8 = (-9223372036854775808) }",
        &[(Code::NegatedU8, (1, 25))],
    );
    codes(
        "fn f(b: u8) { print(-b) }\nfn main() {}",
        &[(Code::NegatedU8, (1, 21))],
    );
}

#[test]
fn no_mixing() {
    let src = "fn f(b: u8, x: i64) {\n    print(b + x)\n    print(x + b)\n    print(b == x)\n    print(b < x)\n}\nfn main() {}";
    codes(
        src,
        &[
            (Code::Mismatch, (2, 15)),
            (Code::Mismatch, (3, 15)),
            (Code::Mismatch, (4, 16)),
            (Code::Mismatch, (5, 15)),
        ],
    );
}

/// Each row: the program, the expression found, and its conversion. The rows cover each
/// `mismatch` call that can offer one: an expected type, a comparison, `??`, a branch with
/// none expected, an array element.
#[test]
fn mismatch_fix_converts_the_expression_found() {
    let rows = [
        // `u8` found where `i64` is wanted: an argument, a return value, a field.
        (
            "fn g(n: i64) {}\nfn f(b: u8) {\n    g(b)\n}\nfn main() {}",
            "b",
            "i64",
        ),
        (
            "fn f(b: u8) -> i64 {\n    return b\n}\nfn main() {}",
            "b",
            "i64",
        ),
        (
            "class P {\n    v: i64\n}\nfn f(b: u8) -> P {\n    P(v: b)\n}\nfn main() {}",
            "b",
            "i64",
        ),
        // An optional `u8` wants the conversion to `u8`, which gives one.
        (
            "fn f(n: i64) {\n    let b: u8? = n\n}\nfn main() {}",
            "n",
            "u8",
        ),
        // A longer expression is wrapped whole.
        (
            "fn n() -> i64 { 1 }\nfn f() {\n    let b: u8? = n()\n}\nfn main() {}",
            "n()",
            "u8",
        ),
        // Offsets are bytes, and an earlier line holds a multibyte char.
        (
            "// ä\nfn f(n: i64) {\n    let b: u8? = n\n}\nfn main() {}",
            "n",
            "u8",
        ),
        // A branch with a type expected, a range bound.
        (
            "fn f(c: bool, x: i64) -> u8? {\n    if c { x } else { 1 }\n}\nfn main() {}",
            "x",
            "u8",
        ),
        (
            "fn f(b: u8) {\n    for i in 0..b {}\n}\nfn main() {}",
            "b",
            "i64",
        ),
        // No type is expected: the second operand, the second branch, the second element.
        (
            "fn f(n: i64, b: u8) {\n    print(n == b)\n}\nfn main() {}",
            "b",
            "i64",
        ),
        (
            "fn f(c: bool, b: u8?, n: i64) {\n    let x = if c { b } else { n }\n}\nfn main() {}",
            "n",
            "u8",
        ),
        (
            "fn f(b: u8?, n: i64) {\n    let xs = [b, n]\n}\nfn main() {}",
            "n",
            "u8",
        ),
    ];
    for (src, found, to) in rows {
        let ds = diags(src);
        let [d] = ds.as_slice() else {
            panic!("{src}: {ds:?}")
        };
        assert_eq!(d.code(), Code::Mismatch, "{src}");
        let [fix] = d.fixes() else {
            panic!("{src}: one fix")
        };
        assert!(!fix.safe(), "{src}");
        let [open, close] = fix.edits() else {
            panic!("{src}: {:?}", fix.edits())
        };
        assert_eq!(open.replacement(), format!("{to}("), "{src}");
        assert_eq!(close.replacement(), ")", "{src}");
        assert!(open.span().start() == open.span().end(), "{src}");
        assert!(close.span().start() == close.span().end(), "{src}");
        assert_eq!(
            &src[open.span().start()..close.span().start()],
            found,
            "{src}"
        );
        // The conversion is what the types need.
        clean(&applied(src, d));
    }
}

#[test]
fn mismatch_has_no_fix_between_other_types() {
    for (src, pos) in [
        ("fn main() { let b: bool = 1 }", (1, 27, "1")),
        (
            "fn f(n: i64) {\n    let b: bool = n\n}\nfn main() {}",
            (2, 19, "n"),
        ),
        (
            "fn f(b: bool) {\n    let n: u8 = b\n}\nfn main() {}",
            (2, 17, "b"),
        ),
    ] {
        let d = diagnostic(src, Code::Mismatch, pos, &[]);
        assert!(d.fixes().is_empty(), "{src}: {:?}", d.fixes());
    }
}

/// An `else if` or `else if let` is no expression to wrap: `else u8(if ...)` would not parse.
#[test]
fn mismatch_has_no_fix_for_an_else_if() {
    for (src, found) in [
        (
            "fn f(c: bool, d: bool, b: u8) -> i64 {\n    if c { 1 } else if d { b } else { b }\n}\nfn main() {}",
            "if d { b } else { b }",
        ),
        (
            "fn f(c: bool, d: u8?, b: u8) -> i64 {\n    if c { 1 } else if let x = d { b } else { b }\n}\nfn main() {}",
            "if let x = d { b } else { b }",
        ),
    ] {
        let d = diagnostic(src, Code::Mismatch, (2, 21, found), &[]);
        assert!(d.fixes().is_empty(), "{src}: {:?}", d.fixes());
    }
}

#[test]
fn already_that_type() {
    let src = "fn f(x: i64, b: u8) {\n    print(i64(x))\n    print(u8(b))\n    print(i64(5))\n}\nfn main() {}";
    codes(
        src,
        &[
            (Code::AlreadyConverted, (2, 15)),
            (Code::AlreadyConverted, (3, 14)),
            (Code::AlreadyConverted, (4, 15)),
        ],
    );
    // Each gains the safe fix that removes the conversion.
    for (conversion, fixed) in [("i64(x)", "x"), ("u8(b)", "b"), ("i64(5)", "5")] {
        let src = format!("fn f(x: i64, b: u8) {{\n    print({conversion})\n}}\nfn main() {{}}");
        let d = &diags(&src)[0];
        let edits: Vec<_> = d.fixes().iter().flat_map(Fix::edits).collect();
        assert!(d.fixes().iter().all(Fix::safe));
        assert!(apply(&src, &edits).contains(&format!("print({fixed})")));
        fix_removes_its_code(&src, d);
    }
}

#[test]
fn conversion_argument_type() {
    codes(
        "fn main() { print(u8(true)) }",
        &[(Code::Mismatch, (1, 22))],
    );
}

#[test]
fn conversion_arity() {
    codes(
        "fn main() { print(u8(1, 2)) }",
        &[(Code::ArgumentCount, (1, 19))],
    );
}

#[test]
fn built_in_names() {
    for (src, col) in [
        ("fn u8() {}", 4),
        ("fn i64() {}", 4),
        ("fn bool() {}", 4),
        ("class u8 {}", 7),
    ] {
        codes(
            &format!("{src}\nfn main() {{}}"),
            &[(Code::BuiltInName, (1, col))],
        );
    }
}

#[test]
fn print_takes_u8() {
    clean("fn f(b: u8) { print(b) }\nfn main() {}");
}

/// An operand that holds only an integer literal takes the `u8` of the other, whether it is a
/// block, a call, an `if` or a `dbg`, and on either side of `+`, `<` and `==`. Before, such an
/// operand counted as concrete, was checked first as an `i64`, and was reported against the `u8`
/// (#289).
#[test]
fn an_operand_that_holds_a_literal_should_take_the_type_of_the_other_operand() {
    with_prelude();
    let decls = "fn id<T>(x: T) -> T { x }\n";
    for statement in [
        "let _a = { 1 } + x",
        "let _a = x + { 1 }",
        "let _a = id(1) + x",
        "let _a = x + id(1)",
        "let _a = (if c { 1 } else { 2 }) + x",
        "let _a = x + (if c { 1 } else { 2 })",
        "let _a = { 1 } < x",
        "let _a = x < { 1 }",
        "let _a = { 1 } == x",
        "let _a = x == { 1 }",
        "let _a = id(1) == x",
        "let _a = dbg({ 1 }) == x",
        "let _a = ({ ({ 1 }) }) + x",
        "let _a = (match c {\n        true => 1\n        false => 2\n    }) + x",
    ] {
        let src = format!(
            "{decls}fn main() {{\n    let c = true\n    let x: u8 = 3\n    {statement}\n}}"
        );
        clean_row(statement, &src);
    }
}
