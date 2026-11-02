//! A range where it is not allowed (E0023), and an assignment to a range index (E0021).

use super::*;

/// `fn main() { <body> }` with `body` from line 2, at column 5.
fn main_with(body: &str) -> String {
    format!("fn main() {{\n    {body}\n}}")
}

/// GIVEN a range with both bounds in a place that takes no range
/// WHEN the program is checked
/// THEN it reports E0023 at the whole range
#[test]
fn range_outside_a_place_should_be_e0023_at_the_whole_range() {
    let rows = [
        ("let value", "let r = 0..3", (2, 13)),
        ("inclusive value", "let r = 0..=3", (2, 13)),
        ("argument", "print(0..3)", (2, 11)),
        // The range binds looser than `+`.
        ("operand", "print(1 + 0..3)", (2, 11)),
    ];
    for (row, body, at) in rows {
        codes_row(row, &main_with(body), &[(Code::RangeOutsideFor, at)]);
    }
}

/// GIVEN a range with a missing bound, or `a..=` with no end
/// WHEN the program is checked
/// THEN it reports E0023 at the whole range, in a `for` too
#[test]
fn range_with_a_missing_bound_should_be_e0023_at_the_whole_range() {
    let rows = [
        ("for source, no end", "for i in 0.. { print(i) }", (2, 14)),
        ("for source, no start", "for i in ..3 { print(i) }", (2, 14)),
        ("for source, no bounds", "for i in .. { print(i) }", (2, 14)),
        (
            "for source, inclusive no end",
            "for i in 0..= { print(i) }",
            (2, 14),
        ),
        ("value, no start", "let r = ..5", (2, 13)),
        ("value, no end", "let r = 5..", (2, 13)),
        ("value, no bounds", "let r = ..", (2, 13)),
        ("value, inclusive no end", "let r = 5..=", (2, 13)),
        ("value, inclusive no start", "let r = ..=5", (2, 13)),
    ];
    for (row, body, at) in rows {
        codes_row(row, &main_with(body), &[(Code::RangeOutsideFor, at)]);
    }
}

/// The bound of a `for` range that is there is still checked, and the loop variable is typed.
#[test]
fn for_range_with_a_missing_bound_should_check_the_bound_that_is_there() {
    codes(
        "fn f(b: u8) { for i in b.. { print(i + 1) } }\nfn main() {}",
        &[(Code::RangeOutsideFor, (1, 24)), (Code::Mismatch, (1, 24))],
    );
}

/// GIVEN `const r = 1..5`
/// WHEN the program is checked
/// THEN it reports E0023, not that the constant is not known
#[test]
fn range_as_a_constant_value_should_be_e0023() {
    for (row, value) in [("both bounds", "1..5"), ("missing end", "1..")] {
        let src = format!("const r = {value}\nfn main() {{}}");
        codes_row(row, &src, &[(Code::RangeOutsideFor, (1, 11))]);
    }
}

/// GIVEN a range index with no bounds
/// WHEN the program is checked
/// THEN it reports E0023 at the range, once
#[test]
fn range_index_with_no_bounds_should_be_e0023() {
    let rows = [
        ("read", "print(xs[..])", (3, 14)),
        ("inclusive read", "print(xs[..=])", (3, 14)),
        ("inclusive no end read", "print(xs[1..=])", (3, 14)),
        ("assignment target", "xs[..] = [1]", (3, 8)),
        ("compound target", "xs[..] += [1]", (3, 8)),
        ("inclusive target", "xs[..=] = [1]", (3, 8)),
        ("inclusive no end target", "xs[1..=] = [1]", (3, 8)),
    ];
    for (row, body, at) in rows {
        // A read leaves `xs` unassigned, which a `let` says.
        let keyword = if body.starts_with("print") {
            "let"
        } else {
            "let mut"
        };
        let src = format!("fn main() {{\n    {keyword} xs = [1, 2]\n    {body}\n}}");
        codes_row(row, &src, &[(Code::RangeOutsideFor, at)]);
    }
}

/// Until the checker takes a range index, one is E0023 whatever its base is.
#[test]
fn range_index_should_be_e0023_until_the_checker_takes_it() {
    let rows = [
        ("array", "let xs = [1, 2, 3]\n    print(xs[0..2])", (3, 14)),
        ("string", "let s = \"abc\"\n    print(s[0..2])", (3, 13)),
    ];
    for (row, body, at) in rows {
        codes_row(row, &main_with(body), &[(Code::RangeOutsideFor, at)]);
    }
}

/// GIVEN `let mut xs = [1, 2, 3]` and `let mut s = "abc"` and a statement on line 4, column 5
/// WHEN the program is checked
/// THEN it reports E0021 at the range index, and no warning follows
#[test]
fn assignment_to_a_range_index_should_be_e0021_at_the_range_index() {
    judge_unused();
    let rows = [
        ("array", "xs[0..2] = [7, 8]"),
        ("element", "xs[0..2][0] = 9"),
        ("compound element", "xs[1..][0] += 5"),
        ("no start", "xs[..2][0] = 9"),
        ("string", "s[0..2] = \"x\""),
        ("string append", "s[0..2] += \"x\""),
        ("string inclusive", "s[0..=1] += \"x\""),
        ("string no end", "s[1..] += \"x\""),
        ("string no start", "s[..2] = \"x\""),
        ("string other operator", "s[0..2] -= \"x\""),
    ];
    for (row, statement) in rows {
        // The other variable changes, and both are used, so no warning is left.
        let other = if statement.starts_with("xs") {
            "s += \"d\""
        } else {
            "xs[0] = 0"
        };
        let src = format!(
            "fn main() {{\n    let mut xs = [1, 2, 3]\n    let mut s = \"abc\"\n    {statement}\n    {other}\n    print(xs)\n    print(s)\n}}"
        );
        codes_row(row, &src, &[(Code::AssignToExpression, (4, 5))]);
    }
}

/// A string's element is E0068's string form in Task 4, and a base that is no array or string
/// its range form in Task 3: until then, E0023.
#[test]
fn assignment_to_an_element_of_a_string_range_should_be_e0023_until_task_4() {
    let src = main_with("let mut s = \"abc\"\n    s[0..2][0] = 'x'\n    print(s)");
    codes(&src, &[(Code::RangeOutsideFor, (3, 7))]);
}

/// The base of a range target reports its own mistake once, and the bounds are not checked.
#[test]
fn assignment_to_a_range_index_should_report_a_wrong_base_once() {
    codes(
        &main_with("ys[0..2] = [1]"),
        &[(Code::BindingNotFound, (2, 5))],
    );
}

/// The value of a range target is checked for its own mistakes.
#[test]
fn assignment_to_a_range_index_should_check_the_value() {
    codes(
        &main_with("let mut xs = [1, 2]\n    xs[0..1] = ys\n    print(xs)"),
        &[
            (Code::AssignToExpression, (3, 5)),
            (Code::BindingNotFound, (3, 16)),
        ],
    );
}

/// GIVEN a range argument of a method, whichever bound is missing and whether or not the method
///       exists
/// WHEN the program is checked
/// THEN it reports the first form of E0023, once, and no other error follows
#[test]
fn method_range_argument_should_be_the_first_e0023_form() {
    let rows = [
        ("contains", "xs.contains(..)", ".."),
        ("array get", "xs.get(0..1)", "0..1"),
        ("array get, no bounds", "xs.get(..)", ".."),
        ("string get", "s.get(..2)", "..2"),
        ("string contains", "s.contains(1..)", "1.."),
        ("inclusive no end", "xs.get(1..=)", "1..="),
        ("second argument", "xs.get(1, ..2)", "..2"),
        ("unknown method", "xs.nope(0..1)", "0..1"),
    ];
    for (row, call, range) in rows {
        let src = format!(
            "fn main() {{\n    let xs = [1, 2]\n    let s = \"abc\"\n    print({call})\n}}"
        );
        let ds = diags(&src);
        let [d] = ds.as_slice() else {
            panic!("row `{row}`: {ds:?}");
        };
        assert_eq!(d.code(), Code::RangeOutsideFor, "row `{row}`");
        assert_eq!(&src[d.span().range()], range, "row `{row}`");
    }
}

/// GIVEN `xs[..][i] op v`
/// WHEN the program is checked
/// THEN it reports E0021, and `xs[..] = ys` keeps E0023
#[test]
fn element_of_a_whole_range_should_be_e0021_and_a_whole_range_target_e0023() {
    let rows = [
        ("assignment", "xs[..][0] = 9", Code::AssignToExpression),
        ("compound", "xs[..][0] += 9", Code::AssignToExpression),
        ("deep", "xs[..][0][1] = 9", Code::AssignToExpression),
        ("whole assignment", "xs[..] = [3]", Code::RangeOutsideFor),
        ("whole compound", "xs[..] += [3]", Code::RangeOutsideFor),
        ("inclusive element", "xs[..=][0] = 9", Code::RangeOutsideFor),
    ];
    for (row, statement, code) in rows {
        let src =
            format!("fn main() {{\n    let mut xs = [[1, 2]]\n    {statement}\n    print(xs)\n}}");
        let found: Vec<_> = diags(&src).iter().map(Diagnostic::code).collect();
        assert_eq!(found, [code], "row `{row}`");
    }
}

/// GIVEN a range that is an error for good, on a base that is wrong
/// WHEN the program is checked
/// THEN the base reports its own mistake, and the range its own
#[test]
fn range_error_for_good_should_check_its_base() {
    for (row, body) in [
        ("whole target", "ys[..] = [1]"),
        ("whole read", "print(ys[..])"),
        ("inclusive whole", "print(ys[..=])"),
        ("no end", "print(ys[1..=])"),
    ] {
        let found = codes_of(&main_with(body));
        let codes: Vec<_> = found.iter().map(|f| f.0).collect();
        assert_eq!(
            codes,
            [Code::BindingNotFound, Code::RangeOutsideFor],
            "row `{row}`"
        );
    }
}

/// GIVEN a `push` onto a range index
/// WHEN the program is checked
/// THEN it reports E0023, and no warning follows
#[test]
fn push_onto_a_range_index_should_report_e0023_alone() {
    judge_unused();
    for (row, body) in [
        (
            "array",
            "let mut xs = [1, 2]\n    xs[1..].push(4)\n    print(xs)",
        ),
        (
            "nested",
            "let mut xs = [[1, 2]]\n    xs[0][1..].push(4)\n    print(xs)",
        ),
    ] {
        let found = codes_of(&main_with(body));
        let codes: Vec<_> = found.iter().map(|f| f.0).collect();
        assert_eq!(codes, [Code::RangeOutsideFor], "row `{row}`");
    }
}

fn codes_of(src: &str) -> Vec<(Code, (usize, usize))> {
    diags(src)
        .iter()
        .map(|d| (d.code(), at(src, d.span())))
        .collect()
}
