//! One test per file in `tests/run`, `tests/run-fail` and `tests/diagnostics`, and in their
//! counterparts under `site/programs/`, which hold every program the site shows. A `run` case exits
//! 0 and prints its `.out`, and its `.err` on stderr when it has one, else nothing there; a
//! `run_fail` case panics with its `.out` on stdout and its `.err` on stderr. Both read their
//! `.in`, when they have one, on stdin. A `diagnostics` case makes `sisu check` report its
//! `.err` on stderr, and `sisu check --format json` its `.json`; a case of a code that only `mono`
//! reports (`MONO_CODES` in `support`) runs `sisu build --emit mir` in place of `sisu check`. The
//! `orphans` binary checks that every file in these directories belongs to a case, as a file its
//! mode reads, and that none is hidden.
//!
//! A `site_pending` case is a program the site shows before it compiles: `sisu check` reports
//! an error, until the day it compiles. An `explanation` case is the Broken program of a
//! `docs/diagnostics` explanation, whose `sisu` output is under `site/programs/explanations/`.
//! A `selfhost_lexer` case is a Sisu file of the tests, of `selfhost/` or of the compiler's own
//! Sisu sources. The lexer in Sisu must print the tokens that `sisu build --emit tokens` prints for
//! it, unless the file has a lexer error, which excludes it.
//! An `every_program_certifies` case builds the mir of a program in `tests/run`, `tests/run-fail`,
//! `tests/programs`, `bench` or `selfhost`, which a debug build certifies.
//!
//! `SISU_BLESS=1` rewrites the `.out`, `.err` and `.json` files of passing cases. It never creates
//! one.

use std::path::Path;
use std::process::Command;

mod site;
#[expect(dead_code, reason = "this binary uses only part of the shared helpers")]
mod support;

/// Whether `SISU_BLESS=1` is set.
fn bless() -> bool {
    std::env::var("SISU_BLESS").as_deref() == Ok("1")
}

// datatest-stable names each test `<function>::<path under root>`, so these function names are
// the `run::` and `run_fail::` prefixes that nextest filters match.

#[expect(
    clippy::unnecessary_wraps,
    reason = "datatest-stable accepts only test functions that return a `Result`"
)]
fn run(path: &Path) -> datatest_stable::Result<()> {
    support::run(path, bless());
    Ok(())
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "datatest-stable accepts only test functions that return a `Result`"
)]
fn run_fail(path: &Path) -> datatest_stable::Result<()> {
    support::run_fail(path, bless());
    Ok(())
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "datatest-stable accepts only test functions that return a `Result`"
)]
fn diagnostics(path: &Path) -> datatest_stable::Result<()> {
    support::diagnostics(path, bless());
    Ok(())
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "datatest-stable accepts only test functions that return a `Result`"
)]
fn site_run(path: &Path) -> datatest_stable::Result<()> {
    support::run(path, bless());
    Ok(())
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "datatest-stable accepts only test functions that return a `Result`"
)]
fn site_run_fail(path: &Path) -> datatest_stable::Result<()> {
    support::run_fail(path, bless());
    Ok(())
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "datatest-stable accepts only test functions that return a `Result`"
)]
fn site_diagnostics(path: &Path) -> datatest_stable::Result<()> {
    support::diagnostics(path, bless());
    Ok(())
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "datatest-stable accepts only test functions that return a `Result`"
)]
fn site_pending(path: &Path) -> datatest_stable::Result<()> {
    site::pending(path);
    Ok(())
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "datatest-stable accepts only test functions that return a `Result`"
)]
fn explanation(path: &Path) -> datatest_stable::Result<()> {
    site::explanation(path, bless());
    Ok(())
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "datatest-stable accepts only test functions that return a `Result`"
)]
fn every_program_certifies(path: &Path) -> datatest_stable::Result<()> {
    certifies(path);
    Ok(())
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "datatest-stable accepts only test functions that return a `Result`"
)]
fn selfhost_lexer(path: &Path) -> datatest_stable::Result<()> {
    support::selfhost::check(path);
    Ok(())
}

/// Asserts that `sisu build --emit mir` builds `path`, which the debug build the tests use
/// certifies function by function, and that a traversal from each function's `bb0` visits every
/// block.
fn certifies(path: &Path) {
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .args(["build", "--emit", "mir"])
        .arg(path)
        .output()
        .expect("sisu starts");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}: {}",
        path.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    let mir = String::from_utf8_lossy(&out.stdout);
    for function in mir.split("\nfn ") {
        let first = function.lines().next().unwrap_or_default();
        // Each block's successors, from its last statement line: its terminator. The
        // function's closing `}` is not one.
        let mut successors: Vec<Vec<usize>> = Vec::new();
        for line in function.lines() {
            if let Some(label) = line.strip_prefix("  bb") {
                assert!(label.ends_with(':'), "{line}");
                successors.push(Vec::new());
            } else if line.starts_with("    ")
                && let Some(block) = successors.last_mut()
            {
                *block = line
                    .split_whitespace()
                    .filter_map(|token| token.trim_matches([',', '[', ']']).strip_prefix("bb"))
                    .filter_map(|n| n.parse().ok())
                    .collect();
            }
        }
        assert!(
            !successors.is_empty(),
            "{}: {first}: no blocks",
            path.display()
        );
        let mut seen = vec![false; successors.len()];
        let mut stack = vec![0];
        while let Some(block) = stack.pop() {
            if !std::mem::replace(&mut seen[block], true) {
                stack.extend(&successors[block]);
            }
        }
        assert!(
            seen.iter().all(|&seen| seen),
            "{}: {first}: a block that bb0 does not reach",
            path.display()
        );
    }
}

datatest_stable::harness! {
    { test = run, root = "tests/run", pattern = r"\.sisu$" },
    { test = run_fail, root = "tests/run-fail", pattern = r"\.sisu$" },
    { test = diagnostics, root = "tests/diagnostics", pattern = r"\.sisu$" },
    { test = site_run, root = "../../site/programs/run", pattern = r"\.sisu$" },
    { test = site_run_fail, root = "../../site/programs/run-fail", pattern = r"\.sisu$" },
    { test = site_diagnostics, root = "../../site/programs/diagnostics", pattern = r"\.sisu$" },
    { test = site_pending, root = "../../site/programs/pending", pattern = r"\.sisu$" },
    { test = explanation, root = "../../docs/diagnostics", pattern = r"[EW]\d{4}\.md$" },
    { test = every_program_certifies, root = "tests", pattern = r"^(run|run-fail|programs)/[^/]+\.sisu$" },
    { test = every_program_certifies, root = "../../bench", pattern = r"main\.sisu$" },
    { test = every_program_certifies, root = "../../selfhost", pattern = r"^lexer\.sisu$" },
    { test = selfhost_lexer, root = "tests", pattern = r"\.sisu$" },
    { test = selfhost_lexer, root = "../../selfhost", pattern = r"\.sisu$" },
    { test = selfhost_lexer, root = "src", pattern = r"^(prelude|lang)\.sisu$" },
}
