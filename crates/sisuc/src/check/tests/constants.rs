//! Top-level constants: folded at compile time in source order, and used as their literal.

use super::*;

/// `src`, with an empty `main` after it unless it has one.
fn with_main(src: &str) -> String {
    if src.contains("fn main") {
        src.to_string()
    } else {
        format!("{src}\nfn main() {{}}")
    }
}

#[test]
fn folds() {
    for (consts, value) in [
        ("const L = 100\nconst A = L * L", "10000"),
        ("const A: u8 = 0xF0", "240u8"),
        ("const M: u8 = 0xF0\nconst A = M + 1", "241u8"),
        ("const A = 7 - 9", "-2"),
        ("const A = -7 / 2", "-3"),
        ("const A = -7 % 2", "-1"),
        ("const A: u8 = 200 / 3", "66u8"),
        ("const A: u8 = 200 % 3", "2u8"),
        ("const A = 1 < 3 < 2", "false"),
        ("const A = 3 > 2 > 1", "true"),
        ("const A = 1 <= 1", "true"),
        ("const A = 2 >= 3", "false"),
        ("const A = 1 < 2", "true"),
        ("const A = 1 < 1", "false"),
        ("const A = 2 > 2", "false"),
        ("const A = 2 <= 1", "false"),
        ("const A = 2 >= 2", "true"),
        ("const A = 1 != 2", "true"),
        ("const A = 1 == 1", "true"),
        ("const A = true == false", "false"),
        ("const A = !false && 1 < 2", "true"),
        ("const A = true && false", "false"),
        ("const A = false || true", "true"),
        ("const A = false || false", "false"),
        ("const A = false && 1 / 0 == 1", "false"),
        ("const A = true || 1 / 0 == 1", "true"),
        ("const A = 2 < 1 < 9223372036854775807 + 1", "false"),
        ("const M: u8 = 7\nconst A = i64(M) - 8", "-1"),
        ("const A = -9223372036854775807 - 1", "-9223372036854775808"),
        ("const A: u8 = 255", "255u8"),
        ("const A = 3 - 2 - 1", "0"),
        ("const B = true\nconst A = !B", "false"),
    ] {
        let src = format!("{consts}\nfn main() {{ print(A) }}");
        contains(&src, &[&format!("(print {value})")]);
    }
}

/// A use lowers to the literal at the use, and its type takes part in the checks around it.
#[test]
fn uses_lower_to_literals() {
    contains(
        "const A = 2\nfn main() { print(A * 3) }",
        &["(print (* 2 3))"],
    );
    contains(
        "const A: u8 = 1\nfn main() {\n    let b: u8 = A + 1\n    print(b)\n}",
        &["(let b#0 (+ 1u8 1u8))"],
    );
    clean("const A = true\nfn f() -> bool { A }\nfn main() { print(f()) }");
}

#[test]
fn local_shadows_const() {
    contains(
        "const A = 1\nfn main() { let A = 2\n print(A) }",
        &["(let A#0 2)", "(print A#0)"],
    );
    contains(
        "const A = 1\nfn f(A: bool) { print(A) }\nfn main() { f(true) }",
        &["(print A#0)"],
    );
    codes(
        "const A = 1\nfn main() { let A: Foo = 1\n print(A) }",
        &[(Code::UnknownType, (2, 20))],
    );
    clean("const A = 1\nfn main() { let mut A = 2\n A = 3\n print(A) }");
}

/// A source, with an empty `main` unless it has one, and its exact codes with positions.
type Row<'a> = (&'a str, &'a [(Code, (usize, usize))]);

/// Asserts each row.
fn rows(rows: &[Row<'_>]) {
    for (src, expected) in rows {
        codes(&with_main(src), expected);
    }
}

/// The gate reports each part of a value that is not known, and a constant used before it
/// is declared.
#[test]
fn gate_errors() {
    rows(&[
        ("const A = f()", &[(Code::ConstantNotKnown, (1, 11))]),
        ("const A = [1]", &[(Code::ConstantNotKnown, (1, 11))]),
        (
            "const A = true || f()",
            &[(Code::ConstantNotKnown, (1, 19))],
        ),
        (
            "const A = f() + g()",
            &[
                (Code::ConstantNotKnown, (1, 11)),
                (Code::ConstantNotKnown, (1, 17)),
            ],
        ),
        (
            "const A = if true { 1 } else { 2 }",
            &[(Code::ConstantNotKnown, (1, 11))],
        ),
        ("const A = { 1 }", &[(Code::ConstantNotKnown, (1, 11))]),
        ("const A = len([1])", &[(Code::ConstantNotKnown, (1, 11))]),
        ("const A = 1 < A", &[(Code::ConstantOrder, (1, 15))]),
        (
            "const A = i64(B)\nconst B = 1",
            &[(Code::ConstantOrder, (1, 15))],
        ),
        // `B` uses `A`, which failed, so `B` is poisoned and says nothing.
        (
            "const A = B\nconst B = A",
            &[(Code::ConstantOrder, (1, 11))],
        ),
    ]);
}

/// A declared type outside the type list is the only error; an unknown one is `resolve`'s.
#[test]
fn type_list_errors() {
    rows(&[
        ("const A: [i64] = [1]", &[(Code::ConstantType, (1, 10))]),
        ("const A: i64? = 1", &[(Code::ConstantType, (1, 10))]),
        ("const A: Foo = 1", &[(Code::UnknownType, (1, 10))]),
    ]);
}

/// The value is typed as a `let` initializer is.
#[test]
fn typing_errors() {
    rows(&[
        ("const A = true || 5", &[(Code::Mismatch, (1, 19))]),
        ("const A: bool = 1", &[(Code::Mismatch, (1, 17))]),
        ("const A = true + 1", &[(Code::Mismatch, (1, 11))]),
        ("const A: u8 = 300", &[(Code::IntegerDoesNotFit, (1, 15))]),
        ("const A: u8 = -1", &[(Code::NegatedU8, (1, 15))]),
        ("const A = 1 == true", &[(Code::CannotCompare, (1, 11))]),
        ("const A = x", &[(Code::BindingNotFound, (1, 11))]),
    ]);
}

/// What would panic at run time is an error at the failing operation.
#[test]
fn fold_errors() {
    rows(&[
        (
            "const A = 9223372036854775807 + 1",
            &[(Code::ConstantPanics, (1, 11))],
        ),
        (
            "const A = 2 * 4611686018427387904",
            &[(Code::ConstantPanics, (1, 11))],
        ),
        (
            "const A = -9223372036854775807 - 2",
            &[(Code::ConstantPanics, (1, 11))],
        ),
        (
            "const A: u8 = 250 + 10 - 10",
            &[(Code::ConstantPanics, (1, 15))],
        ),
        ("const A: u8 = 1 - 2", &[(Code::ConstantPanics, (1, 15))]),
        ("const A: u8 = 16 * 16", &[(Code::ConstantPanics, (1, 15))]),
        ("const A: u8 = 1 / 0", &[(Code::ConstantPanics, (1, 15))]),
        ("const A = 1 / 0", &[(Code::ConstantPanics, (1, 11))]),
        ("const A = 1 % 0", &[(Code::ConstantPanics, (1, 11))]),
        (
            "const A = 1 < 2 < 1 / 0",
            &[(Code::ConstantPanics, (1, 19))],
        ),
        (
            "const A = 1 < 2 && 1 / 0 == 1",
            &[(Code::ConstantPanics, (1, 20))],
        ),
        // Run time evaluates the left operand first, so its panic is the one reported.
        (
            "const A = (1 / 0) + (9223372036854775807 + 1)",
            &[(Code::ConstantPanics, (1, 11))],
        ),
        (
            "const A = 1 / 0 == 9223372036854775807 + 1",
            &[(Code::ConstantPanics, (1, 11))],
        ),
        (
            "const A = 1 / 0 < 9223372036854775807 + 1",
            &[(Code::ConstantPanics, (1, 11))],
        ),
        (
            "const A = 1 < 1 / 0 < 9223372036854775807 + 1",
            &[(Code::ConstantPanics, (1, 15))],
        ),
    ]);
    let min = "const MIN = -9223372036854775807 - 1";
    for value in ["MIN / -1", "MIN % -1", "-MIN", "MIN - 1"] {
        codes(
            &with_main(&format!("{min}\nconst A = {value}")),
            &[(Code::ConstantPanics, (2, 11))],
        );
    }
}

/// A `char` constant folds and compares by code point as run time does.
#[test]
fn const_char() {
    // Each initializer folds on its own, and the prints show them in order: a comparison in
    // `main` would not be folded, and a value that folded wrongly would show in its place.
    let consts = [
        ("C", "'ä'", "'ä'"),
        ("LESS", "'a' < 'b'", "true"),
        ("MORE", "'a' > 'b'", "false"),
        ("SAME", "C == 'ä'", "true"),
        ("OTHER", "C == 'a'", "false"),
        ("DIFFERS", "C != 'a'", "true"),
        ("EQUAL", "C != 'ä'", "false"),
        ("BELOW", "'\\u{D7FF}' < '\\u{E000}'", "true"),
        ("ABOVE", "'\\u{E000}' < '\\u{D7FF}'", "false"),
        ("TOP", "'\\u{10FFFF}' >= '\\u{E000}'", "true"),
        ("CHAIN", "'a' < 'b' <= 'b'", "true"),
        ("NEXT", "i64(C) + 1", "229"),
        ("N", "'x'", "'x'"),
        ("CODE", "i64(C)", "228"),
    ];
    let lines = |line: &dyn Fn(&str, &str) -> String| -> String {
        let lines: Vec<_> = consts
            .iter()
            .map(|(name, value, _)| line(name, value))
            .collect();
        lines.join("")
    };
    let decls = lines(&|name, value| format!("const {name} = {value}\n"));
    let prints = lines(&|name, _| format!("    print({name})\n"));
    let folded: Vec<_> = consts
        .iter()
        .map(|(.., value)| format!("(print {value})"))
        .collect();
    contains(
        &format!("{decls}fn main() {{\n{prints}}}"),
        &[&format!("(block {})", folded.join(" "))],
    );
    // No arithmetic, even in a constant.
    codes(
        &with_main("const C = 'ä'\nconst D = C + 1"),
        &[(Code::ArithmeticOnChar, (2, 13))],
    );
}

/// A `string` constant folds `+` and chains as run time does, and each use is its literal.
#[test]
fn const_string() {
    contains(
        "const G = \"good\" + \"bye\"\nconst S: string = \"x\"\nconst L = \"a\" < \"b\" < \"c\"\nconst K = \"b\" < \"a\" < \"c\"\nfn main() {\n    let n = 1\n    print(G)\n    print(G)\n    print(S)\n    print(L)\n    print(K)\n}",
        // A chain's locals stay with the constant: `main`'s first local is still 0.
        &[
            "(let n#0 1) (print \"goodbye\") (print \"goodbye\") (print \"x\") (print true) (print false)",
        ],
    );
    rows(&[
        (
            "const G = \"a\"\nconst N = G.len()",
            &[(Code::ConstantNotKnown, (2, 11))],
        ),
        ("const P = \"a\" + 1", &[(Code::Mismatch, (1, 17))]),
    ]);
    // Interpolation is not known at compile time, so it is not offered.
    let src = with_main("const P = \"a\" + 1");
    let d = diagnostic(&src, Code::Mismatch, (1, 17, "1"), &[(1, 11)]);
    assert_eq!(fixes(&src, &d), []);
}

/// Constants share the namespace of functions and classes.
#[test]
fn name_errors() {
    rows(&[
        // A duplicate goes through every step for its own errors.
        (
            "const A = 1\nconst A = 1 / 0",
            &[
                (Code::DefinedTwice, (2, 7)),
                (Code::ConstantPanics, (2, 11)),
            ],
        ),
        // The first of two constants of one name is the one its uses read.
        (
            "const A = 1\nconst A = true\nfn main() { let b: i64 = A }",
            &[(Code::DefinedTwice, (2, 7))],
        ),
        ("class A {}\nconst A = 1", &[(Code::DefinedTwice, (2, 7))]),
        ("const A = 1\nfn A() {}", &[(Code::DefinedTwice, (2, 4))]),
        ("const i64 = 1", &[(Code::BuiltInName, (1, 7))]),
    ]);
}

/// A use is checked as a literal of the constant's type; a failed constant is poisoned.
#[test]
fn use_errors() {
    rows(&[
        (
            "const A = 1\nfn main() { A[0] = 1 }",
            &[
                (Code::AssignToImmutable, (2, 13)),
                (Code::CannotIndex, (2, 13)),
            ],
        ),
        (
            "const A = 1\nfn main() { A() }",
            &[(Code::FunctionNotFound, (2, 13))],
        ),
        (
            "const A = f()\nconst B = A + 1\nfn main() {\n    print(A)\n    print(B)\n}",
            &[(Code::ConstantNotKnown, (1, 11))],
        ),
        (
            "const A: u8 = 1 / 0\nfn main() { let b: bool = A }",
            &[(Code::ConstantPanics, (1, 15)), (Code::Mismatch, (2, 27))],
        ),
    ]);
}

#[test]
fn const_diagnostics() {
    diagnostic(
        "const A = B\nconst B = 1\nfn main() {}",
        Code::ConstantOrder,
        (1, 11, "B"),
        &[(2, 7)],
    );
    diagnostic(
        "const A = A + 1\nfn main() {}",
        Code::ConstantOrder,
        (1, 11, "A"),
        &[(1, 7)],
    );
    diagnostic(
        "const A = 1\nconst A = 2\nfn main() {}",
        Code::DefinedTwice,
        (2, 7, "A"),
        &[(1, 7)],
    );
    for assign in ["A = 2", "A += 2"] {
        diagnostic(
            &format!("const A = 1\nfn main() {{ {assign} }}"),
            Code::AssignToImmutable,
            (2, 13, assign),
            &[(1, 7)],
        );
    }
    diagnostic(
        "const A: u8 = 250 + 10 - 10\nfn main() {}",
        Code::ConstantPanics,
        (1, 15, "250 + 10"),
        &[],
    );
}

/// E0032's unsafe fix wraps the constant in the conversion, as it does a local.
#[test]
fn mismatch_unsafe_fix_wraps_the_constant() {
    for (src, wrapped) in [
        ("const A: u8 = 1\nfn main() { let b: i64 = A }", "i64(A)"),
        ("const A = 1\nfn main() { let b: u8? = A }", "u8(A)"),
    ] {
        let ds = diags(src);
        let [d] = ds.as_slice() else {
            panic!("{src}: {ds:?}");
        };
        assert_eq!(d.code(), Code::Mismatch);
        assert!(applied(src, d).contains(wrapped), "{src}");
        assert!(d.fixes().iter().all(|f| !f.safe()), "{src}");
    }
}

/// A conversion that can fail is an error in a constant, at the call, and nothing else is: the
/// call is typed as its payload. The safe fix writes the literal, when the call is the whole
/// value and the argument converts to a value the literal can show without an escape.
#[test]
fn conversion_in_constant() {
    // The source, the codes it reports, and the program the fix gives, if there is one.
    type Case<'a> = (&'a str, &'a [(Code, (usize, usize))], Option<&'a str>);
    let error = |col| (Code::ConversionInConstant, (1, col));
    let rows: [Case<'_>; 19] = [
        ("const B = u8(200)", &[error(11)], Some("const B: u8 = 200")),
        (
            "const C = char(65)",
            &[error(11)],
            Some("const C: char = 'A'"),
        ),
        (
            "const B: u8 = u8(200)",
            &[error(15)],
            Some("const B: u8 = 200"),
        ),
        // `'` and `\` need an escape, and a value that is not ASCII graphic is not offered.
        ("const C = char(39)", &[error(11)], None),
        ("const C = char(92)", &[error(11)], None),
        ("const C = char(32)", &[error(11)], None),
        ("const C = char(0xD7FF)", &[error(11)], None),
        // A value that does not convert.
        ("const B = u8(300)", &[error(11)], None),
        // An argument that does not fold, or has its own error, gives no fix.
        ("const B = u8(1 / 0)", &[error(11)], None),
        (
            "const B = u8(1 + true)",
            &[error(11), (Code::Mismatch, (1, 18))],
            None,
        ),
        ("const B = u8(-1)", &[error(11)], None),
        ("const C = char(0xD800)", &[error(11)], None),
        ("const C = char(0x110000)", &[error(11)], None),
        // A declared type other than the target, or a call that is not the whole value.
        (
            "const C: char = u8(200)",
            &[error(17), (Code::Mismatch, (1, 17))],
            None,
        ),
        ("const B = u8(200) + 1", &[error(11)], None),
        // Each call is reported, even where run time would not reach it.
        (
            "const B = false && u8(1) == u8(2)",
            &[error(20), error(29)],
            None,
        ),
        // A use of the constant says nothing more.
        (
            "const B = u8(200)\nfn main() { let x: u8 = B }",
            &[error(11)],
            Some("const B: u8 = 200\nfn main() { let x: u8 = B }"),
        ),
        // A conversion that cannot fail is allowed, and its own errors stay.
        (
            "const N = i64(2)",
            &[(Code::AlreadyConverted, (1, 15))],
            None,
        ),
        (
            "const M: u8 = 200\nconst N = i64(M + M)",
            &[(Code::ConstantPanics, (2, 15))],
            None,
        ),
    ];
    for (src, expected, fixed) in rows {
        let src = with_main(src);
        codes(&src, expected);
        let ds = diags(&src);
        match fixed {
            Some(fixed) => {
                let fixed = with_main(fixed);
                assert!(ds[0].fixes().iter().all(Fix::safe), "{src}");
                assert_eq!(applied(&src, &ds[0]), fixed);
                clean(&fixed);
            }
            None => assert!(
                ds.iter()
                    .filter(|d| d.code() == Code::ConversionInConstant)
                    .all(|d| d.fixes().is_empty()),
                "{src}"
            ),
        }
    }
}

/// The `main` rejected as a duplicate still exists, whichever comes first.
#[test]
fn main_beside_a_constant_named_main_is_not_missing() {
    diagnostic(
        "const main = 1\nfn main() {}",
        Code::DefinedTwice,
        (2, 4, "main"),
        &[(1, 7)],
    );
    diagnostic(
        "fn main() {}\nconst main = 1",
        Code::DefinedTwice,
        (2, 7, "main"),
        &[(1, 4)],
    );
}
