//! `string`: literals, `+`, comparison, `len()`, and the operators and tests it does not take.

use super::*;

/// `src` with an empty `main` after it.
fn with_main(src: &str) -> String {
    format!("{src}\nfn main() {{}}")
}

/// A function `f` over `params`, whose body is the one statement `body`, on line 2.
fn body(params: &str, body: &str) -> String {
    with_main(&format!("fn f({params}) {{\n    {body}\n}}"))
}

#[test]
fn string_values() {
    contains(
        "fn f(s: string, m: string?) {\n    print(s.len())\n    print(s == \"x\")\n    print(s < \"b\" <= s)\n    print(m?.len() ?? 0)\n    let xs: [string] = [s, \"y\"]\n    let ys = [\"a\"; 3]\n}\nfn main() {\n    let s = \"ab\" + \"c\"\n}",
        &["(concat \"ab\" \"c\")", "(len s#0)"],
    );
}

/// A chain over `string`s binds each operand once, and stops at the first pair that fails.
#[test]
fn chain_binds_each_operand() {
    contains(
        "fn f(a: string, b: string, c: string) {\n    print(a < b < c)\n}\nfn main() {}",
        &[
            "(print (block (let #3 a#0) (let #4 b#1) (&& (< #3 #4) (block (let #5 c#2) (< #4 #5)))))",
        ],
    );
}

/// A `+` that an `i64` context wants and that has a `string` operand reports the nested `f64`
/// sum against `i64`, at the sum's own operands, not the `string` at the sum.
#[test]
fn plus_wanting_i64_blames_the_nested_float() {
    codes(
        "fn main() {\n    let n: i64 = (1.0 + 2.0) + \"a\"\n    print(n)\n}",
        &[(Code::Mismatch, (2, 19))],
    );
    codes(
        "fn main() {\n    let n: i64 = if true { 2.5 } else { 1 } + \"x\"\n    print(n)\n}",
        &[(Code::Mismatch, (2, 28))],
    );
}

/// Asserts that `src`, whose `f` takes `c: bool, a: string`, binds `a` to one temporary and
/// then the `if` over `c` to another, and applies `op` to the two in that order.
fn spills(src: &str, op: &str) {
    let printed = lowered(src);
    let temporaries = printed
        .split_once("(block (let #")
        .and_then(|(_, rest)| rest.split_once(" a#1) (let #"))
        .and_then(|(a, rest)| Some((a, rest.split_once(" (if c#0")?.0)));
    let Some((a, b)) = temporaries else {
        panic!("no spill in {printed}");
    };
    for id in [a, b] {
        assert!(id.parse::<usize>().is_ok(), "{id} in {printed}");
    }
    assert_ne!(a, b, "{printed}");
    let applied = format!("({op} #{a} #{b})");
    assert!(printed.contains(&applied), "{applied} in {printed}");
}

/// An operand of `+` that exits after a `string` one binds the `string` first.
#[test]
fn concat_spills() {
    spills(
        "fn f(c: bool, a: string, b: string) -> string {\n    a + (if c { return \"\" } else { b })\n}\nfn main() {}",
        "concat",
    );
}

/// A comparison of two `string`s whose right operand exits binds the left one first.
#[test]
fn compare_spills() {
    spills(
        "fn f(c: bool, a: string, b: string) -> bool {\n    a < (if c { return false } else { b })\n}\nfn main() {}",
        "<",
    );
}

/// A part of an interpolation that exits after a `string` part binds the `string` first.
#[test]
fn interpolation_spills() {
    spills(
        "fn f(c: bool, a: string) -> string {\n    \"{a}{ if c { return \"\" } else { 1 } }\"\n}\nfn main() {}",
        "interp",
    );
}

/// Each part that is not an `i64`, `u8`, `f64`, `bool`, `char` or `string` is reported at the part; one
/// whose type is unknown reports nothing more.
#[test]
fn cannot_interpolate() {
    let params = "xs: [i64], m: [i64]?, n: i64?, p: P";
    for part in ["xs", "m", "n", "p", "print(1)"] {
        let src = with_items(&body(params, &format!("print(\"a{{{part}}}\")")));
        codes(&src, &[(Code::CannotInterpolate, (2, 14))]);
    }
    codes(
        &with_items(&body(params, "print(\"a{nope}\")")),
        &[(Code::BindingNotFound, (2, 14))],
    );
    // A part that never finishes is accepted, and makes the interpolation never finish.
    codes(
        &with_items(&body(params, "print(\"a{ { return } }\")")),
        &[(Code::PrintNever, (2, 11))],
    );
}

/// A spec's alignment defaults by type: numbers right, the rest left. Precision needs an `f64`;
/// on an `i64` the safe fix converts it.
#[test]
fn format_spec_checks() {
    let params = "n: i64, b: u8, x: f64, s: string, c: char, t: bool";
    for (statement, part) in [
        ("print(\"{n:5}\")", "(fmt n#0 \">5\")"),
        ("print(\"{b:5}\")", "(fmt b#1 \">5\")"),
        ("print(\"{x:5}\")", "(fmt x#2 \">5\")"),
        ("print(\"{s:5}\")", "(fmt s#3 \"<5\")"),
        ("print(\"{c:5}\")", "(fmt c#4 \"<5\")"),
        ("print(\"{t:5}\")", "(fmt t#5 \"<5\")"),
        ("print(\"{n:<5}\")", "(fmt n#0 \"<5\")"),
        ("print(\"{s:^5}\")", "(fmt s#3 \"^5\")"),
        ("print(\"{x:.2}\")", "(fmt x#2 \".2\")"),
        ("print(\"{x:>8.2}\")", "(fmt x#2 \">8.2\")"),
        // `formats` indexes the parts, text pieces included.
        (
            "print(\"a{n}b{x:.2}\")",
            "(interp \"a\" n#0 \"b\" (fmt x#2 \".2\"))",
        ),
    ] {
        contains(&body(params, statement), &[part]);
    }
    let src = body(params, "print(\"{n:.2}\")");
    let d = diagnostic(&src, Code::PrecisionNeedsF64, (2, 14, ":.2"), &[]);
    assert_eq!(
        fixes(&src, &d),
        [(
            true,
            vec![
                (2, 13, "", "f64(".to_string()),
                (2, 14, "", ")".to_string())
            ]
        )]
    );
    assert_eq!(applied(&src, &d), body(params, "print(\"{f64(n):.2}\")"));
    fix_removes_its_code(&src, &d);
    for statement in [
        "print(\"{s:.2}\")",
        "print(\"{b:.2}\")",
        "print(\"{c:.2}\")",
    ] {
        let src = body(params, statement);
        let d = diagnostic(&src, Code::PrecisionNeedsF64, (2, 14, ":.2"), &[]);
        assert_eq!(fixes(&src, &d), [], "{src}");
    }
    // A part that is already reported gets no precision error.
    let params = "xs: [i64], n: i64";
    codes(
        &body(params, "print(\"{xs:.2}\")"),
        &[(Code::CannotInterpolate, (2, 13))],
    );
    codes(
        &body(params, "print(\"{nope:.2}\")"),
        &[(Code::BindingNotFound, (2, 13))],
    );
}

/// `src` with a class `P` and a function `g` after it, on lines that come after `body`'s.
fn with_items(src: &str) -> String {
    format!("{src}\nclass P {{\n    mut name: string\n}}\nfn g(x: i64) -> i64 {{\n    x\n}}")
}

/// A `string` joined to a `print`ed value that is not one: in each row, the statement on line 2,
/// the operand reported, the other operand's secondary label, and the statement the unsafe fix makes
/// of line 2, if there is one.
type ConcatRow<'a> = (
    &'a str,
    (usize, usize, &'a str),
    &'a [(usize, usize)],
    Option<&'a str>,
);

fn concat_rows(rows: &[ConcatRow]) {
    let params = "s: string, c: char, n: i64, xs: [i64], p: P";
    for &(statement, pos, other, fixed) in rows {
        let src = with_items(&body(params, statement));
        let d = diagnostic(&src, Code::Mismatch, pos, other);
        let found = fixes(&src, &d);
        match fixed {
            Some(fixed) => {
                assert_eq!(found.len(), 1, "{src}");
                assert!(!found[0].0, "{src}: the fix is unsafe");
                assert_eq!(applied(&src, &d), with_items(&body(params, fixed)));
            }
            None => assert_eq!(found, [], "{src}"),
        }
    }
}

#[test]
fn concat_mismatch() {
    concat_rows(&[
        (
            "print(s + c)",
            (2, 15, "c"),
            &[(2, 11)],
            Some("print(\"{s}{c}\")"),
        ),
        (
            "print(1 + s)",
            (2, 11, "1"),
            &[(2, 15)],
            Some("print(\"{1}{s}\")"),
        ),
        // A `char` before a `string` is joined, not arithmetic.
        (
            "print(c + s)",
            (2, 11, "c"),
            &[(2, 15)],
            Some("print(\"{c}{s}\")"),
        ),
        (
            "print(\"count: \" + n)",
            (2, 23, "n"),
            &[(2, 11)],
            Some("print(\"count: {n}\")"),
        ),
        (
            "print(n + \" items\")",
            (2, 11, "n"),
            &[(2, 15)],
            Some("print(\"{n} items\")"),
        ),
        ("print(s + xs)", (2, 15, "xs"), &[(2, 11)], None),
        // A `string` after an operand that is no number still drives.
        (
            "print(true + s)",
            (2, 11, "true"),
            &[(2, 18)],
            Some("print(\"{true}{s}\")"),
        ),
        ("print(xs + s)", (2, 11, "xs"), &[(2, 16)], None),
        ("print(p + s)", (2, 11, "p"), &[(2, 15)], None),
        // So does one under an expected `u8`, in either place.
        (
            "let b: u8 = true + s",
            (2, 17, "true"),
            &[(2, 24)],
            Some("let b: u8 = \"{true}{s}\""),
        ),
        (
            "let b: u8 = s + n",
            (2, 21, "n"),
            &[(2, 17)],
            Some("let b: u8 = \"{s}{n}\""),
        ),
        // A `string` decides the join before an expected `u8` reaches the other operand.
        (
            "let b: u8 = (1 + n) + s",
            (2, 17, "(1 + n)"),
            &[(2, 27)],
            Some("let b: u8 = \"{(1 + n)}{s}\""),
        ),
        // A `[]` or a `None` has no type of its own: it is the literal that cannot be a
        // `string`, beside the `string` it would join, in either place.
        ("let b: u8 = [] + s", (2, 17, "[]"), &[(2, 22)], None),
        ("let b: u8 = None + s", (2, 17, "None"), &[(2, 24)], None),
        ("print(s + [])", (2, 15, "[]"), &[(2, 11)], None),
        ("print(s + None)", (2, 15, "None"), &[(2, 11)], None),
        (
            "let b: u8 = 300 + s",
            (2, 17, "300"),
            &[(2, 23)],
            Some("let b: u8 = \"{300}{s}\""),
        ),
    ]);
}

/// A `None` beside a `string` that holds a mismatch of its own gets the label, and that
/// mismatch is left as it is.
#[test]
fn join_literal_after_another_mismatch() {
    let src = with_items(&body(
        "s: string",
        "print({ let x: i64 = true\n s } + None)",
    ));
    assert_eq!(
        found(&src),
        [
            (Code::Mismatch, (2, 26), vec![]),
            (Code::Mismatch, (3, 8), vec![(2, 11)]),
        ]
    );
}

/// A compound `+=` on an element joins as `+` does: the `None` or `[]` points at the element.
#[test]
fn join_literal_in_element_compound() {
    let src = with_items(&body(
        "",
        "let mut xs = [\"a\"]\n    xs[0] += None\n    xs[0] += []",
    ));
    assert_eq!(
        found(&src),
        [
            (Code::Mismatch, (3, 14), vec![(3, 5)]),
            (Code::Mismatch, (4, 14), vec![(4, 5)]),
        ]
    );
}

/// Literals, comments, line breaks and statements around the operands decide the unsafe fix.
#[test]
fn concat_mismatch_layout() {
    concat_rows(&[
        // An interpolated literal joins as a plain one does.
        (
            "print(\"a{s}\" + n)",
            (2, 20, "n"),
            &[(2, 11)],
            Some("print(\"a{s}{n}\")"),
        ),
        // A literal in parentheses is wrapped whole, as any other operand is.
        (
            "print((\"a\") + n)",
            (2, 19, "n"),
            &[(2, 11)],
            Some("print(\"{(\"a\")}{n}\")"),
        ),
        (
            "print(n + (\"a\"))",
            (2, 11, "n"),
            &[(2, 15)],
            Some("print(\"{n}{(\"a\")}\")"),
        ),
        // The unsafe fix would delete the comment between the operands.
        (
            "print(s + // why\n        n)",
            (3, 9, "n"),
            &[(2, 11)],
            None,
        ),
        // A line break between the operands goes with the text the unsafe fix removes.
        (
            "print(s +\n        n)",
            (3, 9, "n"),
            &[(2, 11)],
            Some("print(\"{s}{n}\")"),
        ),
        // Wrapped, an operand that spans lines would break the literal.
        (
            "print(g(1\n    ) + s)",
            (2, 11, "g(1\n    )"),
            &[(3, 9)],
            None,
        ),
        (
            "print(s + (n // c\n    ))",
            (2, 15, "(n // c\n    )"),
            &[(2, 11)],
            None,
        ),
        // A compound assignment is a statement, which a rewrite would break; its place, a local
        // or a field, is the `string`.
        ("let mut t = s\n    t += n", (3, 10, "n"), &[(3, 5)], None),
        ("p.name += n", (2, 15, "n"), &[(2, 5)], None),
        ("print(s - s)", (2, 11, "s"), &[], None),
    ]);
}

/// A join under a `u8` is checked again with no expected type, and a join inside it only once
/// more, so each `u8` parameter around a join adds to the work instead of doubling it.
#[test]
fn nested_joins_check_in_quadratic_time() {
    const LEVELS: usize = 40;
    let mut value = "b".to_owned();
    for _ in 0..LEVELS {
        value = format!("h({value}) + s");
    }
    let src = with_main(&format!(
        "fn h(b: u8) -> u8 {{\n    b\n}}\nfn f(b: u8, s: string) {{\n    let c: u8 = {value}\n}}"
    ));
    let ds = diags(&src);
    assert_eq!(ds.len(), LEVELS, "{ds:?}");
    assert!(ds.iter().all(|d| d.code() == Code::Mismatch), "{ds:?}");
}

/// A join with an unknown or a mismatched operand, in either place, is `Error`, so neither an
/// expected type, a `char` beside it nor its uses report anything more: the operand may have
/// been meant as a number.
#[test]
fn failed_concat_type() {
    let params = "s: string, xs: [i64], n: i64, c: char";
    for (statement, expected) in [
        ("let b: u8 = nope + s", (Code::BindingNotFound, (2, 17))),
        ("let b: u8 = s + nope", (Code::BindingNotFound, (2, 21))),
        ("let b: u8 = ((1 + n) + s) + s", (Code::Mismatch, (2, 18))),
        ("print(s + 1 + c)", (Code::Mismatch, (2, 15))),
        ("print(s + nope + c)", (Code::BindingNotFound, (2, 15))),
    ] {
        codes(&body(params, statement), &[expected]);
    }
    codes(
        &with_main(&format!(
            "fn f({params}) {{\n    let t = s + nope\n    print(t - 1)\n}}"
        )),
        &[(Code::BindingNotFound, (2, 17))],
    );
    codes(
        &with_main(&format!(
            "fn f({params}) {{\n    let t = s + xs\n    print(t - 1)\n}}"
        )),
        &[(Code::Mismatch, (2, 17))],
    );
    // An operand that failed but kept its type, `string`, leaves the join a `string`, so a
    // `char` beside it is still reported.
    codes(
        &body(params, "print(s + \"{xs}\" + c)"),
        &[
            (Code::CannotInterpolate, (2, 17)),
            (Code::Mismatch, (2, 24)),
        ],
    );
    // A value appended to a field that already failed is not the join's mistake.
    codes(
        &with_items(&body("p: P", "p.name += -true")),
        &[(Code::Mismatch, (2, 16))],
    );
}

/// An ordering is held to its first operand's type; `==` reports the pair.
#[test]
fn string_comparison_mismatch() {
    let params = "s: string, n: i64";
    for (statement, code, pos) in [
        ("print(s < n)", Code::Mismatch, (2, 15, "n")),
        ("print(n < s)", Code::Mismatch, (2, 15, "s")),
        ("print(s == n)", Code::CannotCompare, (2, 11, "s == n")),
    ] {
        let src = body(params, statement);
        let d = diagnostic(&src, code, pos, &[]);
        assert_eq!(fixes(&src, &d), [], "{src}");
    }
}

/// `-=` on a string field is a mismatch: `-` takes numbers, so nothing here reaches the append
/// recognition that `places::recognises_appends` covers for `+=`.
#[test]
fn minus_assign_on_string_field() {
    codes(
        &with_main("class P {\n    mut name: string\n}\nfn f(p: P) {\n    p.name -= \"x\"\n}"),
        &[(Code::Mismatch, (5, 5))],
    );
}

#[test]
fn string_is() {
    let params = "s: string, m: string?, n: string?";
    for statement in ["print(s is s)", "print(m is n)", "print(s is m)"] {
        codes(&body(params, statement), &[(Code::IsNeedsObjects, (2, 11))]);
    }
}

/// `len()`, `chars()`, `bytes()` and `words()` are the methods; a near miss offers one of them.
#[test]
fn string_methods() {
    for (method, fix) in [
        ("push", None),
        ("lem", Some("len")),
        ("char", Some("chars")),
        ("byte", Some("bytes")),
        ("word", Some("words")),
    ] {
        let src = format!("fn main() {{\n    \"a\".{method}()\n}}");
        let d = diagnostic(&src, Code::NoMember, (2, 9, method), &[]);
        let expected: Vec<FixAt> = fix
            .map(|fix| (false, vec![(2, 9, method, fix.to_owned())]))
            .into_iter()
            .collect();
        assert_eq!(fixes(&src, &d), expected);
    }
    for method in ["len", "chars", "bytes"] {
        codes(
            &format!("fn main() {{\n    print(\"a\".{method}(1))\n}}"),
            &[(Code::ArgumentCount, (2, 11))],
        );
    }
}

/// `chars()` is a `[char]` and `bytes()` a `[u8]`, so milestone 3's array methods apply to them
/// and `?.` gives an optional array.
#[test]
fn chars_and_bytes_are_arrays() {
    contains(
        "fn f(s: string, m: string?) {\n    let cs: [char] = s.chars()\n    let bs: [u8] = s.bytes()\n    let o: [char]? = m?.chars()\n    print(s.chars().contains('a'))\n    print(s.bytes().len())\n}\nfn main() {}",
        &["(chars s#0)", "(bytes s#0)"],
    );
    // A `[char]` has no sum.
    codes(
        "fn f(s: string) { print(s.chars().sum()) }\nfn main() {}",
        &[(Code::SumNeedsNumbers, (1, 35))],
    );
}

/// `words()` is a `[string]`, so the array methods apply to it.
#[test]
fn words_lowers() {
    contains(
        "fn f(s: string) {\n    let ws: [string] = s.words()\n    print(s.words().len())\n}\nfn main() {}",
        &["(words s#0)", "(len (words s#0))"],
    );
}

/// `s.words(1)` is a wrong argument count.
#[test]
fn words_takes_no_arguments() {
    codes(
        "fn f(s: string) { print(s.words(1).len()) }\nfn main() {}",
        &[(Code::ArgumentCount, (1, 25))],
    );
}

/// `?.` gives an optional array.
#[test]
fn safe_words() {
    let (program, ds) = checked("fn f(s: string?) {\n    let ws = s?.words()\n}\nfn main() {}");
    assert!(ds.is_empty(), "{ds:?}");
    let program = program.expect("no errors");
    assert!(program.to_string().contains("(words "), "{program}");
    assert_eq!(
        program.functions[0].locals.last().expect("a local").ty,
        Type::optional(Type::array(Type::STRING))
    );
}

/// `string(cs)` takes a `[char]` and is a `string`.
#[test]
fn string_from_chars() {
    contains(
        "fn f(s: string, cs: [char]) {\n    let t: string = string(cs)\n    print(string(s.chars()) == s)\n    print(string([]))\n}\nfn main() {}",
        &["(string cs#1)", "(string (chars s#0))"],
    );
}

#[test]
fn string_built_in_names() {
    codes(
        "fn string() {}\nfn main() {}",
        &[(Code::BuiltInName, (1, 4))],
    );
    codes(
        "class string {}\nfn main() {}",
        &[(Code::BuiltInName, (1, 7))],
    );
}

/// `string(x)` with a `string` is already one, and removing the call is safe. Any other type
/// but `[char]` is a mismatch at the argument: a value that interpolation prints is offered
/// interpolation, as an unsafe fix; any other has no fix.
#[test]
fn string_call() {
    let params = "s: string, t: string, xs: [u8], n: i64, b: u8, c: char, ok: bool, p: P";
    let rows = [
        // (statement, code, the argument, the statement the fix makes if there is one, safe)
        (
            "print(string(s))",
            Code::AlreadyConverted,
            (2, 18, "s"),
            Some(("print(s)", true)),
        ),
        (
            "print(string(\"hi!\"))",
            Code::AlreadyConverted,
            (2, 18, "\"hi!\""),
            Some(("print(\"hi!\")", true)),
        ),
        (
            "print(string(s + t))",
            Code::AlreadyConverted,
            (2, 18, "s + t"),
            Some(("print((s + t))", true)),
        ),
        (
            "print(string(n))",
            Code::Mismatch,
            (2, 18, "n"),
            Some(("print(\"{n}\")", false)),
        ),
        (
            "print(string(1))",
            Code::Mismatch,
            (2, 18, "1"),
            Some(("print(\"{1}\")", false)),
        ),
        (
            "print(string(b))",
            Code::Mismatch,
            (2, 18, "b"),
            Some(("print(\"{b}\")", false)),
        ),
        (
            "print(string(c))",
            Code::Mismatch,
            (2, 18, "c"),
            Some(("print(\"{c}\")", false)),
        ),
        (
            "print(string(ok))",
            Code::Mismatch,
            (2, 18, "ok"),
            Some(("print(\"{ok}\")", false)),
        ),
        (
            "print(string(n + 1))",
            Code::Mismatch,
            (2, 18, "n + 1"),
            Some(("print(\"{n + 1}\")", false)),
        ),
        // A line break after the argument is deleted with the `)`, and leaves a literal on one line.
        (
            "print(string(n\n    ))",
            Code::Mismatch,
            (2, 18, "n"),
            Some(("print(\"{n}\")", false)),
        ),
        // An array of anything but `char`, and a class, have no text to interpolate.
        ("print(string(xs))", Code::Mismatch, (2, 18, "xs"), None),
        ("print(string(p))", Code::Mismatch, (2, 18, "p"), None),
        // The unsafe fix would delete the comment, or put a line break in a literal.
        (
            "print(string(\n        // why\n        n))",
            Code::Mismatch,
            (4, 9, "n"),
            None,
        ),
        (
            "print(string(g(1\n    )))",
            Code::Mismatch,
            (2, 18, "g(1\n    )"),
            None,
        ),
    ];
    for (statement, code, pos, fixed) in rows {
        let src = with_items(&body(params, statement));
        let d = diagnostic(&src, code, pos, &[]);
        let found = fixes(&src, &d);
        match fixed {
            Some((fixed, safe)) => {
                assert_eq!(found.len(), 1, "{src}");
                assert_eq!(found[0].0, safe, "{src}");
                assert_eq!(applied(&src, &d), with_items(&body(params, fixed)));
                // Either fix leaves a program that checks.
                clean(&with_items(&body(params, fixed)));
            }
            None => assert_eq!(found, [], "{src}"),
        }
    }
}

/// `string` takes exactly one argument, and a label on it is E0055 as for `u8(x)`.
#[test]
fn string_call_arguments() {
    codes(
        "fn main() {\n    print(string())\n    print(string([], []))\n}",
        &[
            (Code::ArgumentCount, (2, 11)),
            (Code::ArgumentCount, (3, 11)),
        ],
    );
    codes(
        "fn main() {\n    print(string(cs: []))\n}",
        &[(Code::LabeledArgument, (2, 18))],
    );
    // A broken argument is reported once, and nothing follows from it.
    codes(
        "fn main() {\n    print(string(nope))\n}",
        &[(Code::BindingNotFound, (2, 18))],
    );
}
