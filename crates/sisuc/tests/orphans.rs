//! One test per case directory: `tests/run`, `tests/run-fail`, `tests/diagnostics`, and their
//! counterparts under `site/programs/`, with `site/programs/explanations`. Each lists its
//! directory once, and fails with every entry that no case reads: a file that is neither a
//! `.sisu` case nor an expectation file of one, as the directory's mode reads them, and any
//! hidden entry.

use std::path::{Path, PathBuf};

#[expect(dead_code, reason = "this binary uses only part of the shared helpers")]
mod site;
#[expect(dead_code, reason = "this binary uses only part of the shared helpers")]
mod support;

/// `dir`, a path under the crate directory, as an absolute path.
fn under_crate(dir: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(dir)
}

/// Asserts that `orphans`, the messages for `dir`, is empty, and prints all of them if it is not.
fn assert_none(dir: &str, orphans: &[String]) {
    assert!(
        orphans.is_empty(),
        "{dir}: no case reads {} of its entries:\n{}",
        orphans.len(),
        orphans.join("\n")
    );
}

/// Asserts that the case directory `dir` holds only cases and the `expectations` files of them.
fn assert_only_cases(dir: &str, expectations: &[&str]) {
    assert_none(dir, &support::orphans(&under_crate(dir), expectations));
}

/// GIVEN the directory `tests/run`
/// WHEN it is listed once
/// THEN every entry is a `.sisu` case or an `.out`, `.err` or `.in` file of one
#[test]
fn run_directory_should_hold_only_cases_and_their_files() {
    assert_only_cases("tests/run", &support::RUN_EXPECTATIONS);
}

/// GIVEN the directory `tests/run-fail`
/// WHEN it is listed once
/// THEN every entry is a `.sisu` case or an `.out`, `.err` or `.in` file of one
#[test]
fn run_fail_directory_should_hold_only_cases_and_their_files() {
    assert_only_cases("tests/run-fail", &support::RUN_EXPECTATIONS);
}

/// GIVEN the directory `tests/diagnostics`
/// WHEN it is listed once
/// THEN every entry is a `.sisu` case or an `.err` or `.json` file of one
#[test]
fn diagnostics_directory_should_hold_only_cases_and_their_files() {
    assert_only_cases("tests/diagnostics", &["err", "json"]);
}

/// GIVEN the directory `site/programs/run`
/// WHEN it is listed once
/// THEN every entry is a `.sisu` case or an `.out`, `.err` or `.in` file of one
#[test]
fn site_run_directory_should_hold_only_cases_and_their_files() {
    assert_only_cases("../../site/programs/run", &support::RUN_EXPECTATIONS);
}

/// GIVEN the directory `site/programs/run-fail`
/// WHEN it is listed once
/// THEN every entry is a `.sisu` case or an `.out`, `.err` or `.in` file of one
#[test]
fn site_run_fail_directory_should_hold_only_cases_and_their_files() {
    assert_only_cases("../../site/programs/run-fail", &support::RUN_EXPECTATIONS);
}

/// GIVEN the directory `site/programs/diagnostics`
/// WHEN it is listed once
/// THEN every entry is a `.sisu` case or an `.err` or `.json` file of one
#[test]
fn site_diagnostics_directory_should_hold_only_cases_and_their_files() {
    assert_only_cases("../../site/programs/diagnostics", &["err", "json"]);
}

/// GIVEN the directory `site/programs/pending`
/// WHEN it is listed once
/// THEN every entry is a `.sisu` case
#[test]
fn site_pending_directory_should_hold_only_cases() {
    assert_only_cases("../../site/programs/pending", &[]);
}

/// GIVEN the directory `site/programs/explanations`
/// WHEN it is listed once
/// THEN every entry is the `.err` or `.json` file of an explanation in `docs/diagnostics`
#[test]
fn explanations_directory_should_hold_only_files_of_explanations() {
    assert_none(
        "../../site/programs/explanations",
        &site::explanation_orphans(&under_crate("../../site/programs/explanations")),
    );
}
