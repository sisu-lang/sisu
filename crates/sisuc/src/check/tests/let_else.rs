//! `let ... else`: the names it binds, its tests and the loops its `else` block reaches.

use super::*;

/// The typed tree of `src`, which must check clean.
fn program(src: &str) -> tir::Program {
    let (program, ds) = checked(src);
    assert!(ds.is_empty(), "{ds:?}");
    program.expect("a clean program should have a typed tree")
}

fn function<'a>(program: &'a tir::Program, name: &str) -> &'a tir::Function {
    (program.functions.iter().find(|f| f.name == name))
        .expect("the program should hold the function")
}

/// The first statement of `function`, which must be a `let ... else`.
fn let_else(function: &tir::Function) -> (&tir::Expr, &tir::LetElseTest, &tir::Block) {
    let Some(tir::Stmt::LetElse {
        scrutinee,
        test,
        else_block,
    }) = function.body.stmts.first()
    else {
        panic!("the first statement should be a `let ... else`");
    };
    (scrutinee, test, else_block)
}

/// Whether `function` reads `local` anywhere.
fn reads(function: &tir::Function, local: tir::LocalId) -> bool {
    let mut all = Vec::new();
    function.body.exprs(&mut all);
    all.iter()
        .any(|e| matches!(e.kind, tir::ExprKind::Local(l) if l == local))
}

const SHAPE: &str = "enum Shape {\n    Circle(f64)\n    Rect(f64, f64)\n    Square(f64)\n}\n";

/// GIVEN a function `first(xs: [i64?]) -> i64` whose body is `let v = xs[0] else { return -1 }`
/// and then `v * 2`
/// WHEN `sisu check` runs on it
/// THEN the typed tree binds `v` with `LetElseTest::Some` and the block's value reads it
#[test]
fn let_else_names_should_be_in_scope_after_the_statement() {
    let program = program(
        "fn first(xs: [i64?]) -> i64 {\n    let v = xs[0] else { return -1 }\n    v * 2\n}\nfn main() {\n    print(first([Some(4)]))\n}",
    );
    let first = function(&program, "first");
    let (_, test, _) = let_else(first);
    let tir::LetElseTest::Some { bind } = test else {
        panic!("an optional should give `LetElseTest::Some`");
    };
    assert_eq!(first.locals[bind.0].name.as_deref(), Some("v"));
    assert!(reads(first, *bind));
}

/// GIVEN `enum Shape { Circle(f64); Rect(f64, f64) }` and a function `area(s: Shape) -> f64`
/// whose body is `let Circle(r) = s else { return 0.0 }` and then `r * r`
/// WHEN `sisu check` runs on it
/// THEN the typed tree tests the pattern with `LetElseTest::Match`, which binds `r`
#[test]
fn let_else_on_an_enum_should_test_the_pattern() {
    let src = format!(
        "{SHAPE}fn area(s: Shape) -> f64 {{\n    let Circle(r) = s else {{ return 0.0 }}\n    r * r\n}}\nfn main() {{\n    print(area(Shape.Circle(2.0)))\n    print(area(Shape.Rect(1.0, 1.0)))\n    print(area(Shape.Square(1.0)))\n}}"
    );
    let program = program(&src);
    let area = function(&program, "area");
    let (_, test, _) = let_else(area);
    let tir::LetElseTest::Match {
        pattern, bindings, ..
    } = test
    else {
        panic!("an enum should give `LetElseTest::Match`");
    };
    assert!(matches!(pattern.kind, tir::PatternKind::Variant { .. }));
    let [bind] = bindings.as_slice() else {
        panic!("the pattern should bind one name");
    };
    assert_eq!(area.locals[bind.0].name.as_deref(), Some("r"));
    assert!(reads(area, *bind));
}

/// GIVEN the `Shape` with a third variant `Square(f64)`
/// AND a function whose body is `let Circle(r) | Square(r) = s else { return 0.0 }` and then `r`
/// WHEN `sisu check` runs on it
/// THEN the pattern binds one `r` in both alternatives
#[test]
fn let_else_alternatives_should_bind_in_each() {
    let src = format!(
        "{SHAPE}fn side(s: Shape) -> f64 {{\n    let Circle(r) | Square(r) = s else {{ return 0.0 }}\n    r\n}}\nfn main() {{\n    print(side(Shape.Square(3.0)))\n    print(side(Shape.Rect(1.0, 1.0)))\n    print(side(Shape.Circle(1.0)))\n}}"
    );
    let program = program(&src);
    let side = function(&program, "side");
    let (_, test, _) = let_else(side);
    let tir::LetElseTest::Match {
        pattern, bindings, ..
    } = test
    else {
        panic!("an enum should give `LetElseTest::Match`");
    };
    let tir::PatternKind::Or(alternatives) = &pattern.kind else {
        panic!("the pattern should keep its alternatives");
    };
    // Each alternative binds the local that the statement binds, which the lines after it read.
    let [bind] = bindings.as_slice() else {
        panic!("the pattern should bind one name");
    };
    let bound: Vec<_> = alternatives
        .iter()
        .map(|alternative| match &alternative.kind {
            tir::PatternKind::Variant { fields, .. } => match fields.as_slice() {
                [
                    tir::Pattern {
                        kind: tir::PatternKind::Bind(local),
                        ..
                    },
                ] => Some(*local),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(bound, [Some(*bind), Some(*bind)]);
}

/// GIVEN `outer: for i in 0..3` with an inner `for o in [Some(i), None]` whose body is
/// `let v = o else { continue outer }` and then `print(v)`
/// WHEN `sisu check` runs on it
/// THEN the program checks clean and the `continue` names the outer loop
#[test]
fn let_else_block_should_reach_the_loops_around_it() {
    let program = program(
        "fn main() {\n    outer: for i in 0..3 {\n        for o in [Some(i), None] {\n            let v = o else { continue outer }\n            print(v)\n        }\n    }\n}",
    );
    let mut all = Vec::new();
    function(&program, "main").body.exprs(&mut all);
    let depths: Vec<usize> = all
        .iter()
        .filter_map(|e| match e.kind {
            tir::ExprKind::Continue { depth } => Some(depth),
            _ => None,
        })
        .collect();
    // The inner `for` is the loop nearest to the `continue`; its outer loop is one out.
    assert_eq!(depths, [1]);
}

/// GIVEN a `main` with `let o: i64? = Some(1)`, then `let mut n = o else { return }`, then
/// `n = n + 1` and `print(n)`
/// WHEN `sisu check` runs on it
/// THEN `n` is a `let mut` local and the program checks clean
#[test]
fn let_mut_else_should_declare_a_variable() {
    let program = program(
        "fn main() {\n    let o: i64? = Some(1)\n    let mut n = o else { return }\n    n = n + 1\n    print(n)\n}",
    );
    let main = function(&program, "main");
    let tir::Stmt::LetElse { test, .. } = &main.body.stmts[1] else {
        panic!("the second statement should be a `let ... else`");
    };
    let tir::LetElseTest::Some { bind } = test else {
        panic!("an optional should give `LetElseTest::Some`");
    };
    assert!(main.locals[bind.0].mutable);
}

/// `let mut` keeps its keyword, so W0002 judges the name, and a name that nothing changes gets it.
#[test]
fn let_mut_else_should_warn_when_nothing_changes_the_name() {
    judge_unused();
    let ds = diags(
        "fn main() {\n    let o: i64? = Some(1)\n    let mut n = o else { return }\n    print(n)\n}",
    );
    let codes: Vec<_> = ds.iter().map(Diagnostic::code).collect();
    assert_eq!(codes, [Code::NeverReassigned]);
}

/// A name that the pattern binds and nothing reads gets W0014, as a `let` name does.
#[test]
fn let_else_should_warn_when_nothing_reads_the_name() {
    judge_unused();
    let ds = diags("fn main() {\n    let o: i64? = Some(1)\n    let v = o else { return }\n}");
    let codes: Vec<_> = ds.iter().map(Diagnostic::code).collect();
    assert_eq!(codes, [Code::UnusedVariable]);
}

/// Each row is a value, a line that tests it with a pattern, and whether E0051 on that line gives
/// a help. Only a literal of the type of the value is a value to compare with `==`: not a name, a
/// call, a range, an `|`, a variant, a literal of another type or a literal that does not fit
/// (`300` and `-1` on a `u8`).
#[test]
fn plain_value_patterns_should_offer_a_comparison_only_for_a_literal_of_its_type() {
    const ROWS: [(&str, &str, bool); 36] = [
        ("let v = 5", "let 1 = v else { return }", true),
        ("let v = 5", "let -1 = v else { return }", true),
        ("let v = 5", "let 1..3 = v else { return }", false),
        ("let v = 5", "let 1 | 2 = v else { return }", false),
        ("let v = 5", "let g(x) = v else { return }", false),
        ("let v = 5", "let None = v else { return }", false),
        ("let v = 5", "let Some(x) = v else { return }", false),
        ("let v = 5", "let \"a\" = v else { return }", false),
        ("let v = 5", "let true = v else { return }", false),
        ("let v = 5", "let x = v else { return }", false),
        ("let v = 5", "let _ = v else { return }", false),
        ("let v = 5", "let x..y = v else { return }", false),
        ("let v = 5", "if let 1 = v {}", true),
        ("let v = 5", "if let 1..3 = v {}", false),
        ("let v = 5", "if let g(x) = v {}", false),
        ("let v = 5", "while let 1 = v {}", true),
        ("let v = 5", "while let 1..3 = v {}", false),
        ("let v = 5", "let 1 = v", true),
        ("let v = 5", "let g(x) = v", false),
        ("let v: u8 = 5", "let 3 = v else { return }", true),
        ("let v: u8 = 5", "let 0 = v else { return }", true),
        ("let v: u8 = 5", "let 255 = v else { return }", true),
        ("let v: u8 = 5", "let 256 = v else { return }", false),
        ("let v: u8 = 5", "let 300 = v else { return }", false),
        ("let v: u8 = 5", "let -1 = v else { return }", false),
        ("let v: u8 = 5", "let -0 = v else { return }", false),
        ("let v: u8 = 5", "let 'a' = v else { return }", false),
        ("let v: N = 5", "let 1 = v else { return }", true),
        ("let v = true", "let false = v else { return }", true),
        ("let v = true", "let 1 = v else { return }", false),
        ("let v = \"s\"", "let \"t\" = v else { return }", true),
        ("let v = \"s\"", "let 't' = v else { return }", false),
        ("let v = 'c'", "let 'd' = v else { return }", true),
        ("let v = 'c'", "let 'a'..'z' = v else { return }", false),
        ("let v = C(n: 1)", "let 1 = v else { return }", false),
        ("let v = [1]", "let 1 = v else { return }", false),
    ];
    for (value, line, help) in ROWS {
        let src = format!(
            "class C {{\n    n: i64\n}}\ntype N = i64\nfn main() {{\n    {value}\n    {line}\n}}"
        );
        let ds = diags(&src);
        let d = (ds.iter().find(|d| d.code() == Code::UnwrapNeedsOptional))
            .unwrap_or_else(|| panic!("row `{value}`, `{line}`: {ds:?}"));
        assert_eq!(d.help().is_some(), help, "row `{value}`, `{line}`");
    }
}

/// The enums of `always_matching_names_should_offer_a_plain_let`: `E` has two variants and
/// `Only` has one, so a name or `_` on either always matches.
const ALWAYS: &str = "enum E {\n    A(n: i64)\n    B\n}\nenum Only {\n    One\n}\n";

/// A function header, its body, whether E0131 gives a help, the fix of E0131 as whether it is safe
/// and the body after the edit, and the codes of the other diagnostics of the program.
type AlwaysRow = (
    &'static str,
    &'static str,
    bool,
    Option<(bool, &'static str)>,
    &'static [Code],
);

/// The rows of `always_matching_names_should_offer_a_plain_let`.
const SIG: &str = "fn f(w: E, o: Only) {";
const INT: &str = "fn f(w: E) -> i64 {";
const BOOL: &str = "fn f(w: E) -> bool {";
const CHAR: &str = "fn f(w: E) -> char {";
const ALWAYS_ROWS: [AlwaysRow; 27] = [
    (
        SIG,
        "let v = w else { return }",
        false,
        Some((true, "let v = w")),
        &[],
    ),
    (
        SIG,
        "let _ = w else { return }",
        false,
        Some((true, "let _ = w")),
        &[],
    ),
    (
        SIG,
        "let mut v = w else { return }",
        false,
        Some((false, "let mut v = w")),
        &[Code::NeverReassigned],
    ),
    (
        SIG,
        "let v = (w) else { return }",
        false,
        Some((true, "let v = (w)")),
        &[],
    ),
    (
        SIG,
        "let v = o else { return }",
        false,
        Some((true, "let v = o")),
        &[],
    ),
    (
        SIG,
        "let v = w else {\n        return\n    }",
        false,
        Some((true, "let v = w")),
        &[],
    ),
    (
        INT,
        "let v = w else { return 0 }\n    1",
        false,
        Some((true, "let v = w\n    1")),
        &[],
    ),
    (
        SIG,
        "loop { let v = w else { break } }",
        false,
        Some((false, "loop { let v = w }")),
        &[],
    ),
    (
        INT,
        "loop { let v = w else { break } }",
        false,
        Some((false, "loop { let v = w }")),
        &[Code::BodyHasNoValue],
    ),
    (
        SIG,
        "loop { let v = w else { continue } }",
        false,
        Some((true, "loop { let v = w }")),
        &[],
    ),
    (
        SIG,
        "outer: loop {\n        loop { let v = w else { break outer } }\n    }",
        false,
        Some((false, "outer: loop {\n        loop { let v = w }\n    }")),
        &[],
    ),
    (
        SIG,
        "outer: loop {\n        loop { let v = w else { continue outer } }\n    }",
        false,
        Some((false, "outer: loop {\n        loop { let v = w }\n    }")),
        &[],
    ),
    (
        BOOL,
        "let v = w else { return true }\n    true",
        false,
        Some((true, "let v = w\n    true")),
        &[],
    ),
    (
        CHAR,
        "let v = w else { return 'c' }\n    'c'",
        false,
        Some((true, "let v = w\n    'c'")),
        &[],
    ),
    (
        SIG,
        "let v = w else { panic(\"no\") }",
        false,
        Some((false, "let v = w")),
        &[],
    ),
    (
        SIG,
        "let v = w else { missing(); return }",
        false,
        Some((false, "let v = w")),
        &[Code::FunctionNotFound],
    ),
    (
        INT,
        "let v = w else { return }\n    1",
        false,
        Some((false, "let v = w\n    1")),
        &[Code::ReturnNeedsValue],
    ),
    (
        SIG,
        "let v = w else { }",
        false,
        Some((false, "let v = w")),
        &[Code::LetElseCanFinish],
    ),
    (
        SIG,
        "let v = w else {\n        // why\n        return\n    }",
        false,
        Some((false, "let v = w")),
        &[],
    ),
    // One statement that is no expression.
    (
        SIG,
        "let v = w else { let x = 1 }",
        false,
        Some((false, "let v = w")),
        &[Code::LetElseCanFinish],
    ),
    (SIG, "let One = o else { return }", true, None, &[]),
    (SIG, "if let v = w { print(1) }", true, None, &[]),
    (
        SIG,
        "if let _ = w { print(1) } else { print(2) }",
        true,
        None,
        &[],
    ),
    (SIG, "if let A(_) | B = w { print(1) }", true, None, &[]),
    (SIG, "while let v = w { return }", false, None, &[]),
    (SIG, "while let _ = w { return }", false, None, &[]),
    (SIG, "let A(_) | B = w else { return }", true, None, &[]),
];

/// A name or `_` always matches an enum value, and a plain `let` does the same: a `let ... else`
/// offers to remove its `else` block, and an `if let` has a help. The fix is safe only when it
/// removes nothing but the `else` block and a diagnostic of the block. A pattern that takes the
/// value apart keeps the help to use `match`, and `while let` has none. The text of the help and
/// the fix is in the fixtures `E0131-if-let-name` and `E0131-let-else-name`. A `;` in the block
/// is a warning of the lexer, which this table does not see: `E0131-let-else-semicolon` has it.
#[test]
fn always_matching_names_should_offer_a_plain_let() {
    for (head, body, help, fix, others) in ALWAYS_ROWS {
        let src = format!("{ALWAYS}{head}\n    {body}\n}}\nfn main() {{}}");
        let ds = diags(&src);
        let row = format!("row `{body}` in `{head}`: {ds:?}");
        let (always, rest): (Vec<_>, Vec<_>) =
            ds.iter().partition(|d| d.code() == Code::AlwaysMatches);
        let [d] = always.as_slice() else {
            panic!("{row}");
        };
        assert_eq!(d.help().is_some(), help, "{row}");
        let rest: Vec<_> = rest.iter().map(|d| d.code()).collect();
        assert_eq!(rest, others, "{row}");
        let Some((safe, after)) = fix else {
            assert!(d.fixes().is_empty(), "{row}");
            continue;
        };
        let [offered] = d.fixes() else {
            panic!("{row}");
        };
        assert_eq!(offered.safe(), safe, "{row}");
        let want = src.replacen(body, after, 1);
        let fixed = offered.apply(&src);
        assert_eq!(fixed, want, "{row}");
        if safe {
            let codes: Vec<_> = diags(&fixed).iter().map(Diagnostic::code).collect();
            assert_eq!(codes, others, "fixed: {row}");
        }
    }
}

/// The program of `let_else_comment_checks_should_read_near_linearly`: `n` rows of a comment, a
/// `let ... else` whose block is one plain exit, and a read of the name.
fn comment_rows(n: usize) -> String {
    let rows = (0..n)
        .map(|i| format!("    // c{i}\n    let v{i} = o else {{ return 0 }}\n    print(v{i})\n"))
        .collect::<Vec<_>>()
        .concat();
    format!("fn f(o: i64?) -> i64 {{\n{rows}    0\n}}\nfn main() {{}}")
}

/// GIVEN a function of `n` rows, each a comment, a `let ... else` with one plain exit and a read
/// of the name, for `n` = 100 and 200
/// WHEN `sisu check` runs on each
/// THEN `comment_in` reads at least one comment, and the reads for 200 rows are at most three
/// times those for 100
///
/// A scan of the comment list for each statement reads n^2 comments: 4 times as many for 2n rows,
/// and a scan that ticks nothing reads 0. `assert_near_linear` does not fit a binary search: it
/// reads about n * (ceil(log2 n) + 2) comments, so the step from 2n to 3n jumps when a power of
/// two falls between them, and `assert_near_linear` (at most 1.1 times) fails for about half of
/// all n. With c = ceil(log2 n), the ratio of a search for 2n rows to n rows is 2(c + 3)/(c + 2),
/// about 2.2 at n = 100.
#[test]
fn let_else_comment_checks_should_read_near_linearly() {
    let reads = |n: usize| {
        COMMENT_READS.set(0);
        assert!(checked(&comment_rows(n)).0.is_some());
        COMMENT_READS.get()
    };
    let (one, two) = (reads(100), reads(200));
    assert!(one > 0, "the first count is 0");
    assert!(two <= 3 * one, "{one} reads for 100 rows, {two} for 200");
}

/// `let mut` keeps its name judged: a name that nothing changes gets W0002, which the fix of
/// E0131 would remove (the plain `let` would get W0014 in its place), so the fix is unsafe.
#[test]
fn let_mut_else_on_an_always_matching_name_should_keep_its_unused_warning() {
    judge_unused();
    let src = format!(
        "{ALWAYS}fn f(w: E) {{\n    let mut v = w else {{ return }}\n    match v {{\n        A(n) => print(n)\n        B => print(0)\n    }}\n}}\nfn main() {{\n    f(E.B)\n}}"
    );
    let ds = diags(&src);
    let codes: Vec<_> = ds.iter().map(Diagnostic::code).collect();
    assert_eq!(codes, [Code::AlwaysMatches, Code::NeverReassigned]);
    let [fix] = ds[0].fixes() else {
        panic!("{ds:?}");
    };
    assert!(!fix.safe(), "{ds:?}");
}

/// A program whose `main` ends with `line` at line 17: the enum `E` has `A` and `B`, `Only` has
/// one variant, `P` is a class, and `o` is an optional.
fn let_program(line: &str) -> String {
    format!(
        "enum E {{\n    A(i64)\n    B(i64)\n}}\nenum Only {{\n    One(i64)\n}}\nclass P {{\n    x: i64\n    y: i64\n}}\nfn main() {{\n    let v = E.A(1)\n    let w = Only.One(1)\n    let p = P(x: 1, y: 2)\n    let o: i64? = 1\n    {line}\n}}"
    )
}

/// Diagnostics, each as a code and a `(line, column)`.
type Found = &'static [(Code, (usize, usize))];

/// A line, the diagnostics it gives, and those of its `else` form.
type LetRow = (&'static str, Found, Found);

/// The wanted diagnostics of `let_pattern_without_else_should_report_by_the_value_type` and of
/// `let_pattern_without_else_should_agree_with_its_else_form`. The column of E0051 is the value's.
/// The `else` form of a line is the line with `else { return }` after its first line. Where the
/// pattern always matches, the plain form gives E0174 and the `else` form E0131. Where the
/// pattern can fail, the `else` form gives nothing. A failed value keeps its error in both.
const LET_ROWS: [LetRow; 16] = [
    // One variant: the pattern always matches.
    (
        "let One(n) = w",
        &[(Code::RefutablePattern, (17, 9))],
        &[(Code::AlwaysMatches, (17, 9))],
    ),
    // All variants of the enum.
    (
        "let A(n) | B(n) = v",
        &[(Code::RefutablePattern, (17, 9))],
        &[(Code::AlwaysMatches, (17, 9))],
    ),
    // A class has no variant to match: E0051 at the value.
    (
        "let P(a, b) = p",
        &[(Code::UnwrapNeedsOptional, (17, 19))],
        &[(Code::UnwrapNeedsOptional, (17, 19))],
    ),
    (
        "let Q(a) = p",
        &[(Code::UnwrapNeedsOptional, (17, 16))],
        &[(Code::UnwrapNeedsOptional, (17, 16))],
    ),
    (
        "let f(x) = 5",
        &[(Code::UnwrapNeedsOptional, (17, 16))],
        &[(Code::UnwrapNeedsOptional, (17, 16))],
    ),
    (
        "let 1 = 5",
        &[(Code::UnwrapNeedsOptional, (17, 13))],
        &[(Code::UnwrapNeedsOptional, (17, 13))],
    ),
    (
        "let \"a\" = \"b\"",
        &[(Code::UnwrapNeedsOptional, (17, 15))],
        &[(Code::UnwrapNeedsOptional, (17, 15))],
    ),
    (
        "let x..y = 5",
        &[(Code::UnwrapNeedsOptional, (17, 16))],
        &[(Code::UnwrapNeedsOptional, (17, 16))],
    ),
    // A name that is no variant of the enum.
    (
        "let f(x) = v",
        &[(Code::UnknownVariant, (17, 9))],
        &[(Code::UnknownVariant, (17, 9))],
    ),
    (
        "let A(n, m) = v",
        &[(Code::SubpatternCount, (17, 9))],
        &[(Code::SubpatternCount, (17, 9))],
    ),
    // The pattern can fail: the error stays, with the help of `if let` and `match`.
    ("let Some(n) = o", &[(Code::RefutablePattern, (17, 9))], &[]),
    ("let A(n) = v", &[(Code::RefutablePattern, (17, 9))], &[]),
    ("let None = o", &[(Code::RefutablePattern, (17, 9))], &[]),
    // The value fails: its error stays, and so does the pattern's.
    (
        "let One(n) = zz",
        &[
            (Code::RefutablePattern, (17, 9)),
            (Code::BindingNotFound, (17, 18)),
        ],
        &[(Code::BindingNotFound, (17, 18))],
    ),
    // The value has a type, though it fails: the pattern is judged.
    (
        "let One(n) = Only.One(zz)",
        &[
            (Code::RefutablePattern, (17, 9)),
            (Code::BindingNotFound, (17, 27)),
        ],
        &[
            (Code::AlwaysMatches, (17, 9)),
            (Code::BindingNotFound, (17, 27)),
        ],
    ),
    // A failed statement still binds its name, so a later write reports the name only.
    (
        "let One(n) = w\n    n = 2",
        &[
            (Code::RefutablePattern, (17, 9)),
            (Code::AssignToImmutable, (18, 5)),
        ],
        &[
            (Code::AlwaysMatches, (17, 9)),
            (Code::AssignToImmutable, (18, 5)),
        ],
    ),
];

/// The checker reports a `let` with a pattern and no `else` by the type of the value. A pattern
/// that is right for an enum or an optional gives E0174: it always matches, or it can fail. Any
/// other pattern gives the error of the `else` form. The table pins the codes. The fixtures
/// `E0174`, `E0174-one-variant`, `E0174-failed-value` and `E0174-unknown-value` pin the texts.
#[test]
fn let_pattern_without_else_should_report_by_the_value_type() {
    for (line, expected, _) in LET_ROWS {
        codes_row(line, &let_program(line), expected);
    }
}

/// GIVEN a `let` with a pattern and no `else`
/// WHEN `sisu check` runs on it with an `else { return }` after its first line
/// THEN it gives the diagnostics of the table's `else` column: E0131 in place of E0174 "always
/// matches", none for a pattern that can fail, and the error of a failed value alone
#[test]
fn let_pattern_without_else_should_agree_with_its_else_form() {
    let with_else = |line: &str| match line.split_once('\n') {
        Some((first, rest)) => format!("{first} else {{ return }}\n{rest}"),
        None => format!("{line} else {{ return }}"),
    };
    for (line, _, expected) in LET_ROWS {
        codes_row(line, &let_program(&with_else(line)), expected);
    }
}

/// GIVEN a `fn f` whose body is `let = 1` and a `main` with `let One(n) = Only.One(1)` on line 3
/// WHEN `sisu check` runs on it
/// THEN it reports E0011 at 1:14 and E0174 at 3:9
/// AND the checker still runs on the item that has no parse error
#[test]
fn let_pattern_without_else_should_show_with_a_parse_error_in_another_item() {
    let src = "fn f() { let = 1 }\nfn main() {\n    let One(n) = Only.One(1)\n}\nenum Only {\n    One(i64)\n}";
    let ds = crate::front_end::front_end(src).1;
    let found: Vec<_> = ds.iter().map(|d| (d.code(), at(src, d.span()))).collect();
    assert_eq!(
        found,
        [(Code::Expected, (1, 14)), (Code::RefutablePattern, (3, 9))]
    );
}
