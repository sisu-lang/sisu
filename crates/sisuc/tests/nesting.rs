//! The parser bounds how deep a program nests (`MAX_DEPTH` in `parser.rs`), so no later stage
//! overflows its stack on a deep tree.
//!
//! Each construct below has a generator for a program that nests it `n` times and the byte offset
//! of the token where the parser runs out of depth. `limit` is the deepest `n` that compiles: the
//! depth counts the function body and the statement around the construct too, and an `if` or a
//! `while` adds a block, so `limit` differs between constructs. The programs are too large to
//! keep as files.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

/// Classes and functions that the generated programs call. Each program uses only some of them,
/// so the root `_prelude` uses each one, and no program warns that it is never used.
const PRELUDE: &str = "\
class N {
    next: N?
    v: i64

    fn me(self) -> N {
        self
    }
}

fn id(x: i64) -> i64 {
    x
}

fn some(x: i64) -> i64? {
    x
}

enum R {
    A(R)
    Z
}

fn _prelude() {
    print(id(some(N(next: None, v: 1).me().v) ?? 0))
    print(N(next: None, v: 2).next == None)
    let _ = R.A(R.Z)
}

";

/// A generated program and the byte offset of the token where the depth runs out.
struct Generated {
    source: String,
    fail: usize,
}

/// `PRELUDE`, `fn main() {`, then `lines`, then `}`. `fail` is a byte offset into `lines`.
fn in_main(lines: &str, fail: usize) -> Generated {
    let head = format!("{PRELUDE}fn main() {{\n");
    Generated {
        fail: head.len() + fail,
        source: format!("{head}{lines}}}\n"),
    }
}

/// The line `    <before><open × n><leaf><close × n><after>`.
fn wrapped(before: &str, open: &str, leaf: &str, close: &str, after: &str, n: usize) -> String {
    format!(
        "    {before}{}{leaf}{}{after}\n",
        open.repeat(n),
        close.repeat(n)
    )
}

/// `print(` nests `n` times, then `1`.
fn paren(n: usize) -> Generated {
    in_main(&wrapped("print(", "(", "1", ")", ")", n), 10 + n)
}

fn call(n: usize) -> Generated {
    in_main(&wrapped("print(", "id(", "1", ")", ")", n), 10 + 3 * n)
}

fn array(n: usize) -> Generated {
    in_main(&wrapped("let _ = ", "[", "1", "]", "", n), 12 + n)
}

/// The last `-`: a unary operator fails at itself, before it is consumed.
fn neg(n: usize) -> Generated {
    in_main(&wrapped("print(", "-", "1", "", ")", n), 10 + n - 1)
}

fn not(n: usize) -> Generated {
    in_main(&wrapped("print(", "!", "true", "", ")", n), 10 + n - 1)
}

/// The `n`th `+`: a left-associative chain is as deep as it is long.
fn plus(n: usize) -> Generated {
    let line = format!("    print(1{})\n", "+1".repeat(n));
    in_main(&line, 11 + 2 * (n - 1))
}

fn and(n: usize) -> Generated {
    let line = format!("    print(true{})\n", "&&true".repeat(n));
    in_main(&line, 14 + 6 * (n - 1))
}

/// The `n`th `??`, which is right-associative.
fn coalesce(n: usize) -> Generated {
    let line = format!("    let x: i64? = None\n    print({}1)\n", "x??".repeat(n));
    in_main(
        &line,
        "    let x: i64? = None\n".len() + 10 + 1 + 3 * (n - 1),
    )
}

/// The `n`th `[`.
fn index(n: usize) -> Generated {
    let first = wrapped("let a = ", "[", "1", "]", "", n);
    let line = format!("{first}    let _ = a{}\n", "[0]".repeat(n));
    in_main(&line, first.len() + 13 + 3 * (n - 1))
}

/// The `n`th `.`.
fn method_chain(n: usize) -> Generated {
    let first = "    let n = N(next: None, v: 1)\n";
    let line = format!("{first}    let _ = n{}\n", ".me()".repeat(n));
    in_main(&line, first.len() + 13 + 5 * (n - 1))
}

/// The `n`th `?.`.
fn safe_chain(n: usize) -> Generated {
    let first = "    let n: N? = None\n";
    let line = format!("{first}    let _ = n{}\n", "?.next".repeat(n));
    in_main(&line, first.len() + 13 + 6 * (n - 1))
}

/// `head {` nested `n` times around `print(1)`; the `1` is where a block runs out of depth.
fn blocks(head: &str, n: usize) -> Generated {
    let open = format!("{head} {{\n");
    let line = format!("{}print(1)\n{}", open.repeat(n), "}\n".repeat(n));
    in_main(&line, open.len() * n + 6)
}

fn nested_if(n: usize) -> Generated {
    blocks("if true", n)
}

fn nested_if_let(n: usize) -> Generated {
    blocks("if let _ = some(1)", n)
}

fn nested_while(n: usize) -> Generated {
    blocks("while false", n)
}

fn nested_for(n: usize) -> Generated {
    blocks("for _ in 0..1", n)
}

/// `loop {` nested `n` times around `print(1)`, each level ended by a `break`.
fn nested_loop(n: usize) -> Generated {
    let open = "loop {\n";
    let line = format!("{}print(1)\n{}", open.repeat(n), "break\n}\n".repeat(n));
    in_main(&line, open.len() * n + 6)
}

/// An `else if` chain; the `==` of the last condition.
fn else_if(n: usize) -> Generated {
    let first = "    let x = 1\n    if x == 0 {\n";
    let clause = "    } else if x == 0 {\n";
    let body = format!("{first}{}    }}\n", clause.repeat(n));
    in_main(&body, first.len() + clause.len() * (n - 1) + 16)
}

/// The element type `n` brackets deep; fails at its name.
fn array_type(n: usize) -> Generated {
    in_main(&wrapped("let _: ", "[", "i64", "]", " = []", n), 11 + n)
}

/// `match` nested `n` times in its arm's body, then `1`; fails at the last `match`'s scrutinee.
fn match_body(n: usize) -> Generated {
    let first = "    let y = 1\n";
    let line = wrapped("let _ = ", "match y { _ => ", "1", " }", "", n);
    in_main(
        &format!("{first}{line}"),
        first.len() + 12 + 15 * (n - 1) + 6,
    )
}

/// A pattern `n` variants deep; fails at its `_`.
fn match_pattern(n: usize) -> Generated {
    let first = "    let r = R.Z\n    let _ = match r {\n";
    let arm = wrapped("    ", "A(", "_", ")", " => 1", n);
    in_main(
        &format!("{first}{arm}        _ => 2\n    }}\n"),
        first.len() + 8 + 2 * n,
    )
}

/// Where the byte `offset` of `source` is, as 1-based `(line, column)`.
fn line_col(source: &str, offset: usize) -> (usize, usize) {
    let before = &source[..offset];
    let line = before.matches('\n').count() + 1;
    let col = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
    (line, col)
}

fn sisu(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("sisu starts")
}

/// Builds and runs the program `n` deep: it must compile and exit 0 without a diagnostic.
fn compiles_at_the_limit(name: &str, program: fn(usize) -> Generated, limit: usize) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::write(dir.join(format!("{name}.sisu")), program(limit).source).expect("writes");
    let exe = format!("{name}.out");
    let build = sisu(dir, &["build", &format!("{name}.sisu"), "-o", &exe]);
    assert_eq!(
        (build.status.code(), String::from_utf8_lossy(&build.stderr)),
        (Some(0), "".into()),
        "the build should succeed without output"
    );
    let run = Command::new(dir.join(&exe))
        .output()
        .expect("program starts");
    assert_eq!(run.status.code(), Some(0), "the program should exit 0");
}

/// The message and help of the nesting error for nested constructs.
const TREE: (&str, &str) = ("nesting is too deep", "the limit is 256 levels");

/// The message and help of the nesting error for a chain, such as `a + b + c`.
const CHAIN: (&str, &str) = (
    "this chain makes the nesting deeper than 256 levels",
    "put a part of the chain in a `let`",
);

/// One level past the limit: `check` exits 1 with the one message of `form`, at the failing
/// token.
fn errors_past_the_limit(
    name: &str,
    program: fn(usize) -> Generated,
    limit: usize,
    (message, help): (&str, &str),
) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let Generated { source, fail } = program(limit + 1);
    fs::write(dir.join(format!("{name}.sisu")), &source).expect("writes");
    let out = sisu(dir, &["check", &format!("{name}.sisu")]);
    assert_eq!(out.status.code(), Some(1), "check should exit 1");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let (line, col) = line_col(&source, fail);
    let mut lines = stderr.lines();
    assert_eq!(
        lines.next(),
        Some(format!("error[E0071]: {message}").as_str()),
        "{name}"
    );
    assert_eq!(
        lines.next().map(str::trim_start),
        Some(format!("--> {name}.sisu:{line}:{col}").as_str()),
        "{name}"
    );
    assert!(stderr.contains(&format!("= help: {help}")), "{stderr}");
    assert_eq!(stderr.matches("error[").count(), 1, "one error: {name}");
}

macro_rules! construct {
    ($name:ident, $program:ident, $limit:expr, $form:ident) => {
        mod $name {
            /// GIVEN a generated program that nests one construct as deep as the limit allows
            /// WHEN it is built and run
            /// THEN the build prints no diagnostic and the program exits 0
            #[test]
            fn construct_should_compile_at_the_nesting_limit() {
                super::compiles_at_the_limit(
                    concat!("nesting_", stringify!($name)),
                    super::$program,
                    $limit,
                );
            }

            /// GIVEN a generated program that nests one construct one level past the limit
            /// WHEN `check` runs on it
            /// THEN it exits 1 with the one nesting error at the token where the depth runs out,
            /// in the form that names a chain for a chain
            #[test]
            fn construct_should_report_nesting_too_deep_one_level_past_the_limit() {
                super::errors_past_the_limit(
                    concat!("nesting_past_", stringify!($name)),
                    super::$program,
                    $limit,
                    super::$form,
                );
            }
        }
    };
}

construct!(parentheses, paren, 253, TREE);
construct!(calls, call, 253, TREE);
construct!(array_literals, array, 254, TREE);
construct!(unary_minus, neg, 253, TREE);
construct!(unary_not, not, 253, TREE);
construct!(sum_chain, plus, 253, CHAIN);
construct!(and_chain, and, 253, CHAIN);
construct!(coalesce_chain, coalesce, 253, CHAIN);
construct!(index_chain, index, 253, CHAIN);
construct!(method_chains, method_chain, 254, CHAIN);
construct!(safe_field_chain, safe_chain, 254, CHAIN);
construct!(ifs, nested_if, 126, TREE);
construct!(if_lets, nested_if_let, 126, TREE);
construct!(else_if_chain, else_if, 252, TREE);
construct!(whiles, nested_while, 253, TREE);
construct!(fors, nested_for, 253, TREE);
construct!(loops, nested_loop, 126, TREE);
construct!(array_types, array_type, 254, TREE);
construct!(match_bodies, match_body, 254, TREE);
construct!(match_patterns, match_pattern, 253, TREE);

/// GIVEN a `match` that nests in the assignment arm bodies of other `match` expressions
/// WHEN it is parsed with `--emit ast` at the limit and one level past it
/// THEN parsing succeeds at the limit and reports the nesting error one level past it
///
/// An assignment arm body sits in a block of its own, one level more, so `x = match …` nests half
/// as far as `match … => match …` (`match_bodies`). Its arms have no value, so the program stops
/// at `--emit ast`, before the checker.
#[test]
fn nested_assignment_match_should_parse_to_the_limit_and_fail_one_level_past_it() {
    type Program = fn(usize) -> String;
    let cases: [(&str, usize, Program); 1] = [("assignment", 127, |n| {
        "x = ".to_string() + &"match y { _ => x = ".repeat(n) + "1" + &" }".repeat(n)
    })];
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    for (name, limit, program) in cases {
        for (n, code) in [(limit, Some(0)), (limit + 1, Some(1))] {
            let file = format!("nesting_match_{name}.sisu");
            let source = format!("fn main() {{\n    {}\n}}\n", program(n));
            fs::write(dir.join(&file), source).expect("writes");
            let out = sisu(dir, &["build", "--emit", "ast", &file]);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert_eq!(out.status.code(), code, "{name} {n}: {stderr}");
            assert_eq!(
                stderr.contains("error[E0071]: nesting is too deep"),
                n > limit,
                "{name} {n}: {stderr}"
            );
        }
    }
}

/// GIVEN a pattern that nests variants, each holding an alternation of two patterns
/// WHEN it is parsed with `--emit ast` at 126 levels and at 127 levels
/// THEN parsing succeeds at 126 levels and reports the nesting error at 127 levels
///
/// An alternation is the parent of its alternatives, one level more than a pattern without one:
/// `R.A(<inner> | _)` nests half as far as `R.A(<inner>)` (`match_patterns`). It stops at
/// `--emit ast`, so the test isolates parsing.
#[test]
fn alternation_patterns_should_count_one_more_level_of_nesting_than_plain_patterns() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let limit = 126;
    for (n, code) in [(limit, Some(0)), (limit + 1, Some(1))] {
        let arm = wrapped("    ", "A(", "_", " | _)", " => 1", n);
        let first = "    let r = R.Z\n    let _ = match r {\n";
        let Generated { source, .. } = in_main(&format!("{first}{arm}        _ => 2\n    }}\n"), 0);
        fs::write(dir.join("nesting_alternatives.sisu"), source).expect("writes");
        let out = sisu(
            dir,
            &["build", "--emit", "ast", "nesting_alternatives.sisu"],
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), code, "{n}: {stderr}");
        assert_eq!(
            stderr.contains("error[E0071]: nesting is too deep"),
            n > limit,
            "{n}: {stderr}"
        );
    }
}

/// GIVEN a `**` whose right side is wrapped in 253 or in 254 parentheses
/// WHEN `check` runs on it
/// THEN it reports one `E0107` error at 253 parentheses and one `E0071` error at 254
///
/// The right side of a `**` is parsed as a `*`'s would be, one level in: the parser reports the
/// `**` up to 253 parentheses deep, and the depth past that, which no other construct ends with.
#[test]
fn power_right_side_should_count_one_level_of_nesting_more_than_its_operator() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    for (n, code) in [(253, "E0107"), (254, "E0071")] {
        let source = format!(
            "fn main() {{\n    let _ = x ** {}1{}\n}}\n",
            "(".repeat(n),
            ")".repeat(n)
        );
        fs::write(dir.join("nesting_power.sisu"), source).expect("writes");
        let out = sisu(dir, &["check", "nesting_power.sisu"]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(stderr.matches("error[").count(), 1, "{n}: {stderr}");
        assert!(stderr.contains(&format!("error[{code}]")), "{n}: {stderr}");
    }
}

/// GIVEN a chain of 11 `+` inside 250 parentheses, which together pass the limit
/// WHEN `check` runs on it
/// THEN it reports the nesting error for nested constructs, since the chain is the smaller part
#[test]
fn short_chain_in_deep_parentheses_should_report_the_tree_form() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let source = format!(
        "fn main() {{\n    print({}1{}{})\n}}\n",
        "(".repeat(250),
        " + 1".repeat(11),
        ")".repeat(250)
    );
    fs::write(dir.join("nesting_short_chain.sisu"), source).expect("writes");
    let out = sisu(dir, &["check", "nesting_short_chain.sisu"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(stderr.matches("error[").count(), 1, "{stderr}");
    assert!(
        stderr.starts_with(&format!("error[E0071]: {}", TREE.0)),
        "{stderr}"
    );
}

/// GIVEN a sum of 128 `+`, then `??`, 252 parentheses and a second `??`
/// WHEN `check` runs on it
/// THEN the depth runs out at the second `??` with the error for nested constructs
///
/// The right side of a `??` continues the chain of `??` above it. The sum is beside that path, so
/// its links do not count: the second `??` is the second link, under 252 parentheses.
#[test]
fn coalesce_right_side_should_not_count_the_links_on_its_left() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let source = format!(
        "fn main() {{\n    print(1{} ?? {}1{} ?? 1)\n}}\n",
        " + 1".repeat(128),
        "(".repeat(252),
        ")".repeat(252)
    );
    fs::write(dir.join("nesting_coalesce_side.sisu"), source).expect("writes");
    let out = sisu(dir, &["check", "nesting_coalesce_side.sisu"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with(&format!(
            "error[E0071]: {}\n --> nesting_coalesce_side.sisu:2:1034\n",
            TREE.0
        )),
        "{stderr}"
    );
}

/// GIVEN a sum of 128 `+` that ends with a `<`, inside 125 and inside 126 parentheses
/// WHEN `check` runs on each
/// THEN inside 125 the depth runs out at the `<` and the error names the chain, and inside 126
/// it runs out at the last `+` and the error is the one for nested constructs
///
/// The depth runs out at level 257. Inside 125 parentheses the `<` is the chain's 129th link,
/// more than half the levels; inside 126 the last `+` is its 128th, which is not.
#[test]
fn chain_form_should_need_more_links_than_half_the_levels() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    // Column 650 is the `<` inside 125 parentheses; 647 is the last `+` inside 126.
    for (parentheses, column, form) in [(125, 650, CHAIN), (126, 647, TREE)] {
        let source = format!(
            "fn main() {{\n    print({}1{} < 1{})\n}}\n",
            "(".repeat(parentheses),
            " + 1".repeat(128),
            ")".repeat(parentheses)
        );
        fs::write(dir.join("nesting_half.sisu"), source).expect("writes");
        let out = sisu(dir, &["check", "nesting_half.sisu"]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.starts_with(&format!(
                "error[E0071]: {}\n --> nesting_half.sisu:2:{column}\n",
                form.0
            )),
            "{parentheses}: {stderr}"
        );
    }
}

/// `check`, `build --emit ast` and a build of `source` all exit 1 with the one message of
/// `form`, however large it is: the parser stops at the first level past the limit and builds
/// nothing deeper.
fn assert_too_deep(name: &str, source: &str, (message, _): (&str, &str)) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let (file, exe) = (format!("{name}.sisu"), format!("{name}.out"));
    fs::write(dir.join(&file), source).expect("writes");
    let invocations = [
        vec!["check", &file],
        vec!["build", "--emit", "ast", &file],
        vec!["build", &file, "-o", &exe],
    ];
    for args in invocations {
        let out = sisu(dir, &args);
        assert_eq!(out.status.code(), Some(1), "{name} {args:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.starts_with(&format!("error[E0071]: {message}\n")),
            "{name} {args:?}: {stderr}"
        );
        assert_eq!(stderr.matches("error[").count(), 1, "{name} {args:?}");
    }
}

/// GIVEN programs that nest parentheses, sums, `else if`, patterns, `match` and types 100000 deep
/// WHEN `check`, `build --emit ast` and `build` run on each
/// THEN each run exits 1 with the one nesting error and does not abort
#[test]
fn programs_nested_a_hundred_thousand_levels_should_fail_with_one_nesting_error() {
    let n = 100_000;
    let body = |expr: String| format!("fn main() {{\n    print({expr})\n}}\n");
    assert_too_deep(
        "nesting_huge_parentheses",
        &body(format!("{}1{}", "(".repeat(n), ")".repeat(n))),
        TREE,
    );
    assert_too_deep(
        "nesting_huge_sum",
        &body(format!("1{}", "+1".repeat(n))),
        CHAIN,
    );
    assert_too_deep(
        "nesting_huge_else_if",
        &format!(
            "fn main() {{\n    let x = 1\n    if x == 0 {{\n    }}{} else {{\n    }}\n}}\n",
            " else if x == 0 {\n    }".repeat(n)
        ),
        TREE,
    );
    assert_too_deep(
        "nesting_huge_pattern",
        &format!(
            "fn main() {{\n    match e {{\n        {}_{} => 1\n    }}\n}}\n",
            "A(".repeat(n),
            ")".repeat(n)
        ),
        TREE,
    );
    assert_too_deep(
        "nesting_huge_match",
        &format!(
            "fn main() {{\n    {}1{}\n}}\n",
            "match e { _ => ".repeat(n),
            " }".repeat(n)
        ),
        TREE,
    );
    assert_too_deep(
        "nesting_huge_type",
        &format!(
            "fn main() {{\n    let a: {}i64{} = []\n}}\n",
            "[".repeat(n),
            "]".repeat(n)
        ),
        TREE,
    );
}

/// A `main` that prints the expression made by starting with `1` and, `rounds` times, wrapping
/// what there is in parentheses and chaining `suffix` onto it `links` times.
fn wrapped_chains(suffix: &str, links: usize, rounds: usize) -> String {
    let mut expr = "1".to_string();
    for _ in 0..rounds {
        expr = format!("({expr}){}", suffix.repeat(links));
    }
    format!("{PRELUDE}fn main() {{\n    print({expr})\n}}\n")
}

/// GIVEN programs that wrap a chain in parentheses and chain onto it again, for many rounds
/// WHEN `check`, `build --emit ast` and `build` run on each
/// THEN each run exits 1 with the one nesting error
///
/// Each round builds a subtree about `links` high and wraps it in parentheses, which are no node
/// of their own. The parser must still see the height when the next round's chain stacks onto it:
/// a hundred rounds of a hundred links is a tree ten thousand high, not a hundred.
#[test]
fn chains_stacked_on_wrapped_chains_should_add_their_heights() {
    // `links` is 1 for the operators that do not chain: `a == b == c` is an error.
    let shapes = [
        ("+1", 100),
        ("-1", 100),
        ("*1", 100),
        ("/1", 100),
        ("%1", 100),
        ("&&true", 100),
        ("||true", 100),
        ("==1", 1),
        ("!=1", 1),
        (" is x", 1),
        ("<1", 1),
        ("??1", 1),
        (".me()", 100),
        ("?.next", 100),
        (".me(1)", 100),
        ("[0]", 100),
    ];
    for (i, (suffix, links)) in shapes.into_iter().enumerate() {
        let rounds = if links == 1 { 300 } else { 100 };
        let name = format!("nesting_wrapped_{i}");
        assert_too_deep(&name, &wrapped_chains(suffix, links, rounds), TREE);
    }
}

/// GIVEN a program of a hundred rounds that wrap a sum of a hundred terms in parentheses
/// WHEN `check`, `build --emit ast` and `build` run on it
/// THEN each run exits 1 with the one nesting error
///
/// This is the repro of the review: it parsed with `--emit ast` and overflowed the stack in the
/// checker.
#[test]
fn sums_wrapped_a_hundred_times_should_fail_with_one_nesting_error() {
    assert_too_deep("nesting_wrapped_sum", &wrapped_chains("+1", 100, 100), TREE);
}

/// GIVEN a sum and a method chain of ten thousand steps
/// WHEN `check`, `build --emit ast` and `build` run on each
/// THEN each run exits 1 with the one nesting error
///
/// A flat chain is as tall as it is long when each operator or `.` wraps the one before, so a
/// long one is the same error and never an overflow.
#[test]
fn left_chains_of_ten_thousand_steps_should_fail_with_one_nesting_error() {
    let n = 10_000;
    let body = |expr: String| format!("{PRELUDE}fn main() {{\n    print({expr})\n}}\n");
    assert_too_deep(
        "nesting_long_sum",
        &body(format!("1{}", "+1".repeat(n))),
        CHAIN,
    );
    assert_too_deep(
        "nesting_long_methods",
        &format!(
            "{PRELUDE}fn main() {{\n    let n = N(next: None, v: 1)\n    let m = n{}\n}}\n",
            ".me()".repeat(n)
        ),
        CHAIN,
    );
}

/// GIVEN a comparison chain of ten thousand operands
/// WHEN it is built and run
/// THEN the build prints no diagnostic and the program prints `false`
///
/// A comparison chain `a < b < c` is one node with all its operands, so ten thousand of them are
/// a shallow tree and compile.
#[test]
fn comparison_chain_of_ten_thousand_operands_should_compile_as_a_shallow_tree() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let source = format!("fn main() {{\n    print(1{})\n}}\n", "<1".repeat(10_000));
    fs::write(dir.join("nesting_compare.sisu"), source).expect("writes");
    let build = sisu(
        dir,
        &["build", "nesting_compare.sisu", "-o", "nesting_compare.out"],
    );
    assert_eq!(
        (build.status.code(), String::from_utf8_lossy(&build.stderr)),
        (Some(0), "".into())
    );
    let run = Command::new(dir.join("nesting_compare.out"))
        .output()
        .expect("program starts");
    assert_eq!(String::from_utf8_lossy(&run.stdout), "false\n");
}

/// GIVEN a sum of `k` links inside an operand of a comparison, a method call, an index or a call
/// WHEN it is parsed with `--emit ast` for the deepest `k` that parses and for one more
/// THEN parsing succeeds for the deepest `k` and reports the nesting error for one more
///
/// A node that wraps what was parsed puts the new operand one level below itself and everything
/// it wrapped one level deeper. Each shape is `print(<expr>)` with a sum of `k` links inside the
/// operand, and parsing alone decides: the deepest `k` that parses, and the first that does not.
#[test]
fn operands_of_wrapping_nodes_should_count_the_wrapping_node_in_their_depth() {
    type Shape = (&'static str, fn(usize) -> String, usize);
    let shapes: [Shape; 6] = [
        ("sum", |k| format!("1{}", "+1".repeat(k)), 253),
        // The comparison is the parent of the sum on its left.
        ("compare lhs", |k| format!("1{}<1", "+1".repeat(k)), 252),
        ("compare rhs", |k| format!("1<1{}", "+1".repeat(k)), 252),
        (
            "method argument",
            |k| format!("n.me(1{})", "+1".repeat(k)),
            251,
        ),
        ("index", |k| format!("a[1{}]", "+1".repeat(k)), 251),
        ("call argument", |k| format!("f(1{})", "+1".repeat(k)), 252),
    ];
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    for (name, expr, limit) in shapes {
        for (k, parses) in [(limit, true), (limit + 1, false)] {
            let source = format!("fn main() {{\n    print({})\n}}\n", expr(k));
            fs::write(dir.join("nesting_operand.sisu"), source).expect("writes");
            let out = sisu(dir, &["build", "--emit", "ast", "nesting_operand.sisu"]);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert_eq!(
                out.status.success(),
                parses,
                "{name} with {k} links: {stderr}"
            );
            // The sum is the chain that runs out of depth.
            assert_eq!(
                stderr.starts_with(&format!("error[E0071]: {}", CHAIN.0)),
                !parses,
                "{name}"
            );
        }
    }
}

/// GIVEN a sum of a hundred parenthesized groups, each a sum of a hundred terms
/// WHEN `check` runs on it
/// THEN it exits 0 and prints no diagnostic
///
/// Many subtrees side by side are not nested: a hundred groups of a hundred terms, joined by `+`,
/// stand about two hundred high, far less than their ten thousand terms.
#[test]
fn wide_trees_should_pass_check_because_depth_is_not_size() {
    let group = format!("(1{})", "+1".repeat(100));
    let sum = vec![group; 100].join("+");
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let source = format!("fn main() {{\n    print({sum})\n}}\n");
    fs::write(dir.join("nesting_wide.sisu"), source).expect("writes");
    let out = sisu(dir, &["check", "nesting_wide.sisu"]);
    assert_eq!(
        (out.status.code(), String::from_utf8_lossy(&out.stderr)),
        (Some(0), "".into())
    );
}

/// GIVEN a function of 300 groups of statements that each nest a little
/// WHEN `check` runs on it
/// THEN it exits 0 and prints no diagnostic
///
/// Depth is the nesting at a point, not a count of everything parsed: a function of many
/// statements, each nesting a little, stays far below the limit at every statement.
#[test]
fn sibling_statements_should_not_add_up_to_the_nesting_limit() {
    let group = |i: usize| {
        format!(
            "\
    print((1))
    print(-1 + 2 * 3 - 4)
    print(id(1) == 1)
    let a{i} = [1, 2]
    print(a{i}[0])
    let _: [[i64]] = []
    if true {{
        print(1)
    }} else if false {{
        print(2)
    }}
    while false {{
    }}
    for _ in 0..1 {{
    }}
    print(some(1) ?? 2)
    print(1 < 2)
"
        )
    };
    let body: String = (0..300).map(group).collect();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let source = format!("{PRELUDE}fn main() {{\n{body}}}\n");
    fs::write(dir.join("nesting_siblings.sisu"), source).expect("writes");
    let out = sisu(dir, &["check", "nesting_siblings.sisu"]);
    assert_eq!(
        (out.status.code(), String::from_utf8_lossy(&out.stderr)),
        (Some(0), "".into())
    );
}
