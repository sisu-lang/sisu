//! `f64`: literals typed by context, negation, exactness, no mixing with integers, and `f64(n)`.

use super::*;

/// A function `f` over the scalar types, whose body is `let _ = <expr>` on line 2, so `expr`
/// starts at column 13.
fn scalars(expr: &str) -> String {
    format!(
        "fn f(n: i64, x: f64, b: u8, c: char, s: string) {{\n    let _ = {expr}\n}}\nfn main() {{}}"
    )
}

#[test]
fn literal_typing() {
    let rows = [
        ("fn main() { let x: f64 = 1 }", "(let x#0 1.0)"),
        ("fn main() { let x: f64? = 1 }", "(let x#0 (wrap 1.0))"),
        ("fn main() { let x: f64 = 1 + 2 }", "(let x#0 (+ 1.0 2.0))"),
        (
            "fn main() {\n    let x = 2.5\n    let _ = 2 * x\n}",
            "(* 2.0 x#0)",
        ),
        (
            "fn main() {\n    let x = 2.5\n    let _ = x * 2\n}",
            "(* x#0 2.0)",
        ),
        ("fn main() { let _ = 1 + 2.5 }", "(+ 1.0 2.5)"),
        // With no `f64` beside it, an integer literal stays an `i64`.
        ("fn main() { let n = 1 }", "(let n#0 1)"),
    ];
    for (src, part) in rows {
        contains(src, &[part]);
    }
}

#[test]
fn negation_type() {
    // `-x` is an `f64`, so it fits an `f64` and not an `i64`.
    contains(
        "fn main() {\n    let x = 2.5\n    let y = -x\n    let w: f64 = y\n}",
        &["(let y#1 (- x#0))"],
    );
    codes(
        "fn main() {\n    let x = 2.5\n    let y: i64 = -x\n}",
        &[(Code::Mismatch, (3, 18))],
    );
    contains("fn main() { let z: f64 = -1 }", &["(let z#0 -1.0)"]);
    // `-x` with `x: f64?` names `f64` as the type it wants: only its fixture,
    // tests/diagnostics/E0032-negate-optional, can see that.
}

#[test]
fn float_never_integer() {
    let src = "fn main() {\n    let n: i64 = 1.5\n}";
    let d = diagnostic(src, Code::Mismatch, (2, 18, "1.5"), &[]);
    assert_eq!(fixes(src, &d), []);
}

#[test]
fn exactness() {
    let rows = [
        ("9007199254740993", Some("9007199254740992.0")),
        ("9223372036854775807", Some("9.223372036854776e18")),
        // Exactly -2^63.
        ("-9223372036854775808", None),
        // 2^53, the last integer before the gaps.
        ("9007199254740992", None),
    ];
    for (literal, nearest) in rows {
        let src = format!("fn main() {{\n    let x: f64 = {literal}\n}}");
        match nearest {
            Some(nearest) => {
                let d = diagnostic(&src, Code::NotExactlyF64, (2, 18, literal), &[]);
                assert_eq!(
                    fixes(&src, &d),
                    [(true, vec![(2, 18, literal, nearest.to_string())])]
                );
                fix_removes_its_code(&src, &d);
            }
            None => clean(&src),
        }
    }
}

#[test]
fn mixing() {
    let rows = [
        ("n * x", (13, "n"), "f64(n) * x"),
        ("x * n", (17, "n"), "x * f64(n)"),
        ("n < x", (13, "n"), "f64(n) < x"),
        ("n == x", (13, "n"), "f64(n) == x"),
        ("x != n", (18, "n"), "x != f64(n)"),
        ("b * x", (13, "b"), "f64(i64(b)) * x"),
    ];
    for (expr, (col, found), fixed) in rows {
        let src = scalars(expr);
        let d = diagnostic(&src, Code::Mismatch, (2, col, found), &[]);
        assert!(d.fixes().iter().all(|f| !f.safe()), "{src}");
        let fixed = scalars(fixed);
        assert_eq!(applied(&src, &d), fixed);
        clean(&fixed);
    }
    // The `string` drives `+`, as beside any other type, and is the secondary label.
    diagnostic(
        &scalars("x + \"a\""),
        Code::Mismatch,
        (2, 13, "x"),
        &[(2, 17)],
    );
}

#[test]
fn int_to_float() {
    // An `f64`, which the function returns.
    contains(
        "fn g(n: i64) -> f64 { f64(n) }\nfn main() {}",
        &["(f64 n#0)"],
    );
    let rows = [
        ("f64(b)", (17, "b"), Some("f64(i64(b))")),
        ("f64(c)", (17, "c"), Some("f64(i64(c))")),
        ("f64(s)", (17, "s"), None),
    ];
    for (expr, (col, found), fixed) in rows {
        let src = scalars(expr);
        let d = diagnostic(&src, Code::Mismatch, (2, col, found), &[]);
        match fixed {
            Some(fixed) => {
                let fixed = scalars(fixed);
                assert_eq!(applied(&src, &d), fixed);
                clean(&fixed);
            }
            None => assert_eq!(fixes(&src, &d), [], "{src}"),
        }
    }
    diagnostic(
        &scalars("f64(x)"),
        Code::AlreadyConverted,
        (2, 17, "x"),
        &[],
    );
}

/// A `char` after an `f64` that rejected the integer before it reports nothing more: that
/// integer, the one converted, was reported already.
#[test]
fn char_after_a_rejected_integer_adds_no_error() {
    codes(
        "fn main() {\n    let n = 1\n    print(n < 1.5 < 'c')\n}",
        &[(Code::Mismatch, (3, 11))],
    );
}

/// Where the context wants an `i64`, the `f64` is the operand reported, with no fix: a compound
/// assignment's target and a declared type are not what changes. The `f64` is reported in
/// either order.
#[test]
fn mixing_under_an_integer() {
    let rows = [
        (
            "fn main() {\n    let mut n = 1\n    n += 1.5\n    print(n)\n}",
            (3, 10, "1.5"),
        ),
        (
            "fn f(x: f64) {\n    let mut n = 1\n    n *= x\n    print(n)\n}\nfn main() {}",
            (3, 10, "x"),
        ),
        (
            "fn f(n: i64, x: f64) -> i64 {\n    n * x\n}\nfn main() {}",
            (2, 9, "x"),
        ),
        (
            "fn f(n: i64, x: f64) -> i64 {\n    x * n\n}\nfn main() {}",
            (2, 5, "x"),
        ),
        (
            "fn f(n: i64, x: f64) -> i64 {\n    return n * x\n}\nfn main() {}",
            (2, 16, "x"),
        ),
        (
            "fn f(n: i64, x: f64) -> i64 {\n    return x * n\n}\nfn main() {}",
            (2, 12, "x"),
        ),
        (
            "fn f(n: i64, x: f64) {\n    let y: i64 = n * x\n    print(y)\n}\nfn main() {}",
            (2, 22, "x"),
        ),
        (
            "fn f(n: i64, x: f64) {\n    let y: i64 = x * n\n    print(y)\n}\nfn main() {}",
            (2, 18, "x"),
        ),
    ];
    for (src, at) in rows {
        let d = diagnostic(src, Code::Mismatch, at, &[]);
        assert_eq!(fixes(src, &d), [], "{src}");
    }
}

/// The `i64` the context wants reaches an operator inside the first operand, so the `f64` is
/// reported however deep it sits. A `+` that a `string` makes a join is reported as before.
#[test]
fn mixing_under_an_integer_nested() {
    let function = |body: &str| {
        format!("fn f(x: f64, n: i64, s: string) -> i64 {{\n    {body}\n}}\nfn main() {{}}")
    };
    let rows = [
        ("x * n * n", (2, 5, "x")),
        ("n * x * n", (2, 9, "x")),
        ("n * x + n", (2, 9, "x")),
        ("x * n + n", (2, 5, "x")),
        ("(n + n * x) - n", (2, 14, "x")),
    ];
    for (body, at) in rows {
        let src = function(body);
        let d = diagnostic(&src, Code::Mismatch, at, &[]);
        assert_eq!(fixes(&src, &d), [], "{src}");
    }
    // An `i64?` wants an `i64` too.
    diagnostic(
        "fn f(x: f64, n: i64) {\n    let y: i64? = x * n * n\n    print(y ?? 0)\n}\nfn main() {}",
        Code::Mismatch,
        (2, 19, "x"),
        &[],
    );
    // A `string` beside an `i64` is reported against `string`, with the unsafe fix to interpolate.
    for (body, at, other) in [
        ("s + n + n", (2, 9, "n"), (2, 5)),
        ("n + s + n", (2, 5, "n"), (2, 9)),
    ] {
        let src = function(body);
        let d = diagnostic(&src, Code::Mismatch, at, &[other]);
        assert_eq!(fixes(&src, &d).len(), 1, "{src}");
    }
}

/// `i64(x)` can fail, so it gives an `i64?`. `u8(x)` and `char(x)` take only an `i64`, and get
/// no fix: no one conversion turns an `f64` into one.
#[test]
fn float_conversions() {
    clean("fn f(x: f64) -> i64? { i64(x) }\nfn main() {}");
    clean("fn f(x: f64) -> i64 { i64(x) ?? 0 }\nfn main() {}");
    // The plain `i64` is a mismatch at the call; see tests/diagnostics/E0032-float-to-int.
    let src = "fn f(x: f64) -> i64 { i64(x) }\nfn main() {}";
    let d = diagnostic(src, Code::Mismatch, (1, 23, "i64(x)"), &[]);
    assert_eq!(fixes(src, &d), [], "{src}");
    for (expr, col) in [("u8(x)", 16), ("char(x)", 18)] {
        let src = scalars(expr);
        let d = diagnostic(&src, Code::Mismatch, (2, col, "x"), &[]);
        assert_eq!(fixes(&src, &d), [], "{src}");
    }
}

/// `f64` constants fold with Rust's `f64`, which never fails, and their uses lower to the
/// folded literal. `i64(x)` from an `f64` can fail, so it is an error in a constant; its fix
/// writes the whole number when there is one.
#[test]
fn float_constants() {
    let rows = [
        // A float literal beside an integer one makes both `f64`.
        ("const C = 2 * 3.0", "6.0"),
        ("const C: f64 = 1", "1.0"),
        ("const C = -2.5", "-2.5"),
        ("const C = 0.1 + 0.2", "0.30000000000000004"),
        ("const C = -7.5 % 2.0", "-1.5"),
        ("const C = 1.0 / 0.0", "inf"),
        ("const C = 0.0 / 0.0", "nan"),
        ("const C = 7.0 - 0.5 / 2.0", "6.75"),
        ("const N = 3\nconst C = f64(N)", "3.0"),
        // Negation flips the sign, so it is not `0.0 - x`: `-(0.0)` is `-0.0`.
        ("const C = -(1.0 + 1.5)", "-2.5"),
        ("const C = -(1.0 - 1.0)", "-0.0"),
        // Comparisons are IEEE: `nan` compares false, `-0.0` equals `0.0`.
        ("const C = 0.0 / 0.0 < 1.0", "false"),
        ("const C = 0.0 / 0.0 == 0.0 / 0.0", "false"),
        ("const C = 0.0 / 0.0 != 0.0 / 0.0", "true"),
        ("const C = -0.0 == 0.0", "true"),
        ("const C = 1.0 < 2.0 <= 2.0", "true"),
        ("const C = 2.0 > 1.0 >= 1.5", "false"),
        ("const C = 1.5 > 1.0", "true"),
    ];
    for (constant, value) in rows {
        let src = format!("{constant}\nfn main() {{\n    let x = C\n    print(x)\n}}");
        contains(&src, &[&format!("(let x#0 {value})")]);
    }
    // An integer that overflows inside an `f64` value still panics, on either side of an operator.
    for (src, column) in [
        ("const C = f64(9223372036854775807 + 1)", 15),
        ("const C = f64(9223372036854775807 + 1) * 2.0", 15),
        ("const C = 2.0 * f64(9223372036854775807 + 1)", 21),
    ] {
        codes(
            &format!("{src}\nfn main() {{}}"),
            &[(Code::ConstantPanics, (1, column))],
        );
    }
    // A declared type other than `f64` still holds an `f64` value to it.
    codes(
        "const C: i64 = 1.5\nfn main() {}",
        &[(Code::Mismatch, (1, 16))],
    );
    let error = (Code::ConversionInConstant, (1, 11));
    for (src, fixed) in [
        ("const N = i64(2.5)", None),
        ("const N = i64(2.0)", Some("const N: i64 = 2")),
        ("const N = i64(-0.0)", Some("const N: i64 = 0")),
        (
            "const N = i64(-9223372036854775808.0)",
            Some("const N: i64 = -9223372036854775808"),
        ),
        ("const N = i64(9223372036854775808.0)", None),
        ("const N = i64(1e300)", None),
        ("const N = i64(0.0 / 0.0)", None),
    ] {
        let src = format!("{src}\nfn main() {{}}");
        codes(&src, &[error]);
        let d = &diags(&src)[0];
        match fixed {
            Some(fixed) => {
                assert!(d.fixes().iter().all(Fix::safe), "{src}");
                let fixed = format!("{fixed}\nfn main() {{}}");
                assert_eq!(applied(&src, d), fixed);
                clean(&fixed);
            }
            None => assert_eq!(fixes(&src, d), [], "{src}"),
        }
    }
}

/// `sum` adds an `[f64]` too, into an `f64`, and the `sum(xs)` idiom points to it.
#[test]
fn sum_of_an_f64_array_should_be_an_f64_and_the_function_form_should_offer_the_method() {
    with_prelude();
    contains(
        "fn f(xs: [f64]) {\n    let t: f64 = xs.sum()\n    print(t)\n}\nfn main() {}",
        &["(let t#1 (call [f64].sum xs#0))"],
    );
    let src = "fn f(xs: [f64]) {\n    print(sum(xs))\n}\nfn main() {}";
    let d = diagnostic(src, Code::MethodAsFunction, (2, 11, "sum"), &[]);
    assert_eq!(
        applied(src, &d),
        "fn f(xs: [f64]) {\n    print(xs.sum())\n}\nfn main() {}"
    );
}

/// Interpolation writes an `f64`; an `f64?` warns that it may print `None`.
#[test]
fn interpolation_should_write_an_f64_and_warn_on_an_optional_one() {
    contains(
        "fn f(x: f64) {\n    print(\"{x}!\")\n}\nfn main() {}",
        &["(interp x#0 \"!\")"],
    );
    codes(
        "fn f(x: f64?) {\n    print(\"{x}\")\n}\nfn main() {}",
        &[(Code::PrintsOptional, (2, 13))],
    );
}

/// One method of each result shape. The declared type proves the result: the same call is a
/// mismatch against the wrong type, which every row names.
#[test]
fn number_method_calls_should_have_the_result_type_of_their_rule() {
    with_prelude();
    let rows = [
        ("x.sqrt()", "f64", "bool"),
        ("x.pow(2.0)", "f64", "bool"),
        ("x.hypot(1.0)", "f64", "bool"),
        ("x.is_nan()", "bool", "f64"),
        ("x.is_finite()", "bool", "f64"),
        ("x.is_infinite()", "bool", "f64"),
        ("x.min(1.0)", "f64", "bool"),
        ("x.clamp(0.0, 1.0)", "f64", "bool"),
        ("n.min(1)", "i64", "bool"),
        ("n.max(1)", "i64", "f64"),
        ("n.clamp(0, 1)", "i64", "f64"),
    ];
    for (expr, ty, wrong) in rows {
        let program = |declared: &str| {
            format!(
                "fn f(n: i64, x: f64) {{\n    let y: {declared} = {expr}\n    print(y)\n}}\nfn main() {{}}"
            )
        };
        clean(&program(ty));
        let src = program(wrong);
        let found: Vec<_> = diags(&src).iter().map(Diagnostic::code).collect();
        assert_eq!(found, [Code::Mismatch], "{src}");
    }
    // Through `?.` the result is optional.
    clean("fn f(x: f64?) {\n    let y: f64? = x?.sqrt()\n    print(y ?? 0.0)\n}\nfn main() {}");
    codes(
        "fn f(x: f64?) {\n    let y: f64 = x?.sqrt()\n    print(y)\n}\nfn main() {}",
        &[(Code::Mismatch, (2, 18))],
    );
}

/// The receiver's type is the expected type of each argument, so an integer literal is an
/// `f64`; the receiver comes first among the operands.
#[test]
fn an_integer_literal_argument_should_take_the_type_of_the_receiver() {
    with_prelude();
    contains(
        "fn f(x: f64) {\n    print(x.pow(2))\n}\nfn main() {}",
        &["(math pow x#0 2.0)"],
    );
    contains(
        "fn f(x: f64) {\n    print(x.clamp(0, 1))\n}\nfn main() {}",
        &["(math clamp x#0 0.0 1.0)"],
    );
    contains(
        "fn f(n: i64) {\n    print(n.clamp(0, 1))\n}\nfn main() {}",
        &["(call i64.clamp n#0 0 1)"],
    );
}

/// `Math` prints as `(math <method> <receiver> <arguments>)`, and a literal receiver is the
/// first operand.
#[test]
fn math_prints() {
    contains(
        "fn f(x: f64) {\n    print(x.sqrt())\n    print((-1.0).is_nan())\n    print(x.is_infinite())\n}\nfn main() {}",
        &[
            "(math sqrt x#0)",
            "(math is_nan -1.0)",
            "(math is_infinite x#0)",
        ],
    );
}

#[test]
fn method_errors() {
    let rows = [
        // The count is the method's, as for any method.
        ("x.sqrt(1)", Code::ArgumentCount, (2, 13)),
        ("x.pow()", Code::ArgumentCount, (2, 13)),
        ("x.clamp(1.0)", Code::ArgumentCount, (2, 13)),
        ("x.sqrtt()", Code::NoMember, (2, 15)),
        // `sqrt` is not a method of an `i64`.
        ("n.sqrt()", Code::NoMember, (2, 15)),
        ("b.min(b)", Code::NoMember, (2, 15)),
    ];
    for (expr, code, at) in rows {
        let src = scalars(expr);
        codes(&src, &[(code, at)]);
    }
    // The nearest method is the unsafe fix.
    let src = scalars("x.sqrtt()");
    let d = diagnostic(&src, Code::NoMember, (2, 15, "sqrtt"), &[]);
    assert_eq!(
        fixes(&src, &d),
        [(false, vec![(2, 15, "sqrtt", "sqrt".to_owned())])]
    );
    // An argument of the wrong type is a mismatch at the argument, whatever the method.
    codes(&scalars("x.pow(n)"), &[(Code::Mismatch, (2, 19))]);
    // The call that failed is `Error`, so the `bool` it cannot be is not reported too: a bad
    // argument, or a label, is the whole of the report.
    for (arg, code) in [("n", Code::Mismatch), ("v: 1.0", Code::LabeledArgument)] {
        let src = format!(
            "fn f(n: i64, x: f64) {{\n    let y: bool = x.pow({arg})\n    print(y)\n}}\nfn main() {{}}"
        );
        codes(&src, &[(code, (2, 25))]);
    }
}

/// `f64.<name>` for each constant gives its Rust value.
#[test]
fn f64_constants() {
    use std::f64::consts::{E, PI, TAU};
    let rows = [
        ("pi", PI),
        ("tau", TAU),
        ("e", E),
        ("infinity", f64::INFINITY),
        ("nan", f64::NAN),
        ("max", f64::MAX),
        ("min_positive", f64::MIN_POSITIVE),
        ("epsilon", f64::EPSILON),
    ];
    for (name, x) in rows {
        let printed = if x.is_nan() {
            "nan".to_owned()
        } else {
            format!("{x:?}")
        };
        let src = format!("fn main() {{\n    let y: f64 = f64.{name}\n    print(y)\n}}");
        contains(&src, &[&format!("(let y#0 {printed})")]);
    }
}

#[test]
fn f64_constant_errors() {
    let src = "fn main() {\n    let _ = f64.PI\n}";
    let d = diagnostic(src, Code::NoMember, (2, 17, "PI"), &[]);
    assert_eq!(
        fixes(src, &d),
        [(false, vec![(2, 17, "PI", "pi".to_owned())])]
    );
    // An `i64` has no constants, so there is no unsafe fix to offer.
    let src = "fn main() {\n    let _ = i64.max\n}";
    let d = diagnostic(src, Code::NoMember, (2, 17, "max"), &[]);
    assert_eq!(fixes(src, &d), []);
    // Any other type name is the same.
    codes(
        "fn main() {\n    let _ = string.len\n}",
        &[(Code::NoMember, (2, 20))],
    );
}

/// A local named `f64` wins, so `f64.pi` reads a field of an `i64`.
#[test]
fn shadowed_type_path() {
    let src = "fn main() {\n    let f64 = 1\n    let _ = f64.pi\n}";
    // The fixture `E0063-shadowed-type` pins the wording: a field of the local's `i64`, no
    // constant.
    diagnostic(src, Code::NoMember, (3, 17, "pi"), &[]);
}

#[test]
fn type_as_value() {
    let class = "class Point {\n    x: i64\n}\nfn main() {\n    let p = Point\n}";
    let rows = [
        ("fn main() {\n    let x = f64\n}", (2, 13)),
        ("fn main() {\n    print(i64)\n}", (2, 11)),
        ("fn main() {\n    f64 = 1\n}", (2, 5)),
        (class, (5, 13)),
    ];
    for (src, at) in rows {
        codes(src, &[(Code::TypeAsValue, at)]);
    }
}

/// A comparison with `f64.nan` is always false: the warning offers `is_nan` when the other side is
/// an `f64`, and nothing when it is an `f64?`, which `is_nan` would not check.
#[test]
fn nan_comparison() {
    let program =
        |expr: &str| format!("fn f(x: f64, o: f64?) {{\n    let _ = {expr}\n}}\nfn main() {{}}");
    let rows = [
        ("x == f64.nan", (13, "x == f64.nan"), "x.is_nan()"),
        ("f64.nan != x", (13, "f64.nan != x"), "!x.is_nan()"),
        (
            "x + 1.0 == f64.nan",
            (13, "x + 1.0 == f64.nan"),
            "(x + 1.0).is_nan()",
        ),
        ("f64.nan == -x", (13, "f64.nan == -x"), "(-x).is_nan()"),
        ("(x == f64.nan)", (14, "x == f64.nan"), "(x.is_nan())"),
    ];
    for (expr, (col, found), fixed) in rows {
        let src = program(expr);
        let d = diagnostic(&src, Code::NanComparison, (2, col, found), &[]);
        assert!(d.fixes().iter().all(Fix::safe), "{src}");
        let fixed = program(fixed);
        assert_eq!(applied(&src, &d), fixed);
        clean(&fixed);
    }
    for expr in ["o == f64.nan", "f64.nan != o"] {
        let src = program(expr);
        let d = diagnostic(&src, Code::NanComparison, (2, 13, expr), &[]);
        assert_eq!(fixes(&src, &d), []);
    }
    // No other constant, and no other comparison, warns.
    for expr in ["x == f64.pi", "x < f64.nan", "x == x"] {
        clean(&program(expr));
    }
    // A side that failed, or has no `f64` to compare, is reported once, by its own error.
    codes(
        "fn f(xs: [f64]) {\n    let _ = xs == f64.nan\n}\nfn main() {}",
        &[(Code::CannotCompare, (2, 13))],
    );
    codes(
        "fn f(xs: [f64]?) {\n    let _ = xs == f64.nan\n}\nfn main() {}",
        &[(Code::CannotCompare, (2, 13))],
    );
    codes(
        "fn f() {\n    let _ = y == f64.nan\n}\nfn main() {}",
        &[(Code::BindingNotFound, (2, 13))],
    );
    // A comparison that is an error is not also a warning.
    codes(
        "fn f(n: i64) {\n    let _ = n == f64.nan\n}\nfn main() {}",
        &[(Code::Mismatch, (2, 13))],
    );
}

#[test]
fn f64_constant_in_const() {
    let src = "const TWO_PI = 2 * f64.pi\nfn main() {\n    let x: f64 = TWO_PI\n    print(x)\n}";
    contains(src, &[&format!("{:?}", 2.0 * std::f64::consts::PI)]);
    // The gate lets the member through, so only the unknown name is reported.
    codes(
        "const C = f64.PI\nfn main() {}",
        &[(Code::NoMember, (1, 15))],
    );
    // Only a built-in type's member passes the gate; any other is not known at compile time.
    codes(
        "class P {\n    x: i64\n}\nconst C = P.x\nfn main() {}",
        &[(Code::ConstantNotKnown, (4, 11))],
    );
    // A constant's own comparison with `nan` still warns, and holds nothing.
    codes(
        "const C = f64.nan == f64.nan\nfn main() {}",
        &[(Code::NanComparison, (1, 11))],
    );
}

/// Math the way other languages write it: a method of `f64` named as a function (E0070), or as a
/// member of `math`, `Math` or `f64` (the new code). Each row's fix is applied, and a safe one
/// leaves a program that checks.
#[test]
fn math_written_another_way_should_be_reported_with_a_fix_where_it_checks() {
    with_prelude();
    let program = |expr: &str| {
        format!(
            "fn f(n: i64, x: f64, y: f64, xs: [i64], ys: [f64]) {{\n    let _ = {expr}\n}}\nfn main() {{}}"
        )
    };
    let function = Code::MethodAsFunction;
    let math = Code::MathIdiom;
    let rows = [
        ("sqrt(x)", function, "sqrt", Some("x.sqrt()")),
        ("pow(x, 2)", function, "pow", Some("x.pow(2)")),
        ("max(n, 3)", function, "max", Some("n.max(3)")),
        ("max(n, n)", function, "max", Some("n.max(n)")),
        (
            "math.sqrt(x + 1.0)",
            math,
            "math.sqrt",
            Some("(x + 1.0).sqrt()"),
        ),
        ("Math.pow(x, 2)", math, "Math.pow", Some("x.pow(2)")),
        ("math.atan2(y, x)", math, "math.atan2", Some("y.atan2(x)")),
        ("math.max(n, 3)", math, "math.max", Some("n.max(3)")),
        (
            "math.sqrt(x).abs()",
            math,
            "math.sqrt",
            Some("x.sqrt().abs()"),
        ),
        ("Math.PI", math, "Math.PI", Some("f64.pi")),
        ("Math.E", math, "Math.E", Some("f64.e")),
        ("math.pi", math, "math.pi", Some("f64.pi")),
        ("math.inf", math, "math.inf", Some("f64.infinity")),
        ("Math.INFINITY", math, "Math.INFINITY", Some("f64.infinity")),
        ("math.NaN", math, "math.NaN", Some("f64.nan")),
        ("f64.max(x, y)", math, "f64.max", Some("x.max(y)")),
        (
            "f64.sqrt(x - 1.0)",
            math,
            "f64.sqrt",
            Some("(x - 1.0).sqrt()"),
        ),
        // No fix where the method call would not check.
        ("math.sqrt(n)", math, "math.sqrt", None),
        ("math.sqrt(x, 2.0)", math, "math.sqrt", None),
        ("math.pow(x)", math, "math.pow", None),
        ("math.pow(x, n)", math, "math.pow", None),
        ("f64.pow(n, 2.0)", math, "f64.pow", None),
        ("math.sqrt()", math, "math.sqrt", None),
        ("math.sqrt(v: x)", math, "math.sqrt", None),
    ];
    for (expr, code, found, fixed) in rows {
        let src = program(expr);
        let d = diagnostic(&src, code, (2, 13, found), &[]);
        match fixed {
            Some(fixed) => {
                if code == math {
                    assert!(d.fixes().iter().all(Fix::safe), "{src}");
                }
                let fixed = program(fixed);
                assert_eq!(applied(&src, &d), fixed);
                clean(&fixed);
            }
            None => assert_eq!(fixes(&src, &d), [], "{src}"),
        }
    }
    // A name that no method of the type has stays an unknown function, and so does a method of
    // numbers called on a `u8`.
    codes(&program("abs(n)"), &[(Code::FunctionNotFound, (2, 13))]);
    // An array has only `len`, `sum` and `contains`: a number's method on one is no idiom.
    for expr in ["sqrt(xs)", "abs(xs)", "max(xs, 3)", "sqrt(ys)"] {
        codes(&program(expr), &[(Code::FunctionNotFound, (2, 13))]);
    }
    // A member that is a method or constant, written with no call, has no fix.
    for expr in ["math.sqrt", "Math.max", "math.max"] {
        let src = program(expr);
        let d = diagnostic(&src, math, (2, 13, expr), &[]);
        assert_eq!(fixes(&src, &d), [], "{src}");
    }
    let src = "fn f(b: u8) {\n    let _ = max(b, 3)\n}\nfn main() {}";
    codes(src, &[(Code::FunctionNotFound, (2, 13))]);
    // A call on another type's name is still a type read as a value.
    codes(&program("i64.max(n, 3)"), &[(Code::TypeAsValue, (2, 13))]);
    // The rewrite would delete the comment.
    let src = "fn f(x: f64) {\n    let _ = math.sqrt( // root\n        x)\n}\nfn main() {}";
    let d = diagnostic(src, math, (2, 13, "math.sqrt"), &[]);
    assert_eq!(fixes(src, &d), []);
    // A bad argument is reported once, beside the idiom.
    codes(
        &program("math.sqrt(y_)"),
        &[(Code::MathIdiom, (2, 13)), (Code::BindingNotFound, (2, 23))],
    );
    codes(
        &program("math.pow(x, z_)"),
        &[(Code::MathIdiom, (2, 13)), (Code::BindingNotFound, (2, 25))],
    );
}

/// A class, function or constant named `math` or `Math` is declared, so `math.sqrt` is not the
/// idiom: the path that reports a class, a function or a constant's field reports it.
#[test]
fn declared_math_should_not_be_the_idiom() {
    let class = "class Math {\n    a: i64\n}\n";
    let rows = [
        (class, "Math.PI", Code::TypeAsValue, (5, 13)),
        (class, "Math.sqrt(x)", Code::TypeAsValue, (5, 13)),
        (class, "Math.sqrt", Code::TypeAsValue, (5, 13)),
        // An interface is declared too, and is a type, as a class is.
        ("interface Math {}\n", "Math.PI", Code::TypeAsValue, (3, 13)),
        (
            "interface Math {}\n",
            "Math.sqrt(x)",
            Code::TypeAsValue,
            (3, 13),
        ),
        ("fn math() {}\n", "math.sqrt(x)", Code::TypeAsValue, (3, 13)),
        ("fn math() {}\n", "math.pi", Code::TypeAsValue, (3, 13)),
        ("const Math = 1\n", "Math.PI", Code::NoMember, (3, 18)),
    ];
    for (item, expr, code, at) in rows {
        let src = format!("{item}fn f(x: f64) {{\n    let _ = {expr}\n}}\nfn main() {{}}");
        codes(&src, &[(code, at)]);
    }
}

/// A local named `math` is a value, so its members are its own; a name no constant or method has
/// is left to the unbound-name error at `math`.
#[test]
fn math_is_a_name_like_any_other() {
    let class = "class M {\n    a: i64\n}\n";
    codes(
        &format!(
            "{class}fn f(x: f64) {{\n    let math = M(a: 1)\n    let _ = math.sqrt(x)\n}}\nfn main() {{}}"
        ),
        &[(Code::NoMember, (6, 18))],
    );
    codes(
        &format!(
            "{class}fn f() {{\n    let Math = M(a: 1)\n    let _ = Math.a\n}}\nfn main() {{}}"
        ),
        &[],
    );
    for expr in ["math.foo", "Math.foo(x)", "math.PI()"] {
        codes(
            &format!("fn f(x: f64) {{\n    let _ = {expr}\n}}\nfn main() {{}}"),
            &[(Code::BindingNotFound, (2, 13))],
        );
    }
}
