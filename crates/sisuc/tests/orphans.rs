//! One test per case directory: `tests/run`, `tests/run-fail`, `tests/diagnostics`, and their
//! counterparts under `site/programs/`, with `site/programs/explanations`. Each lists its
//! directory once, and fails with every entry that no case reads: a file that is neither a
//! `.sisu` case nor an expectation file of one, as the directory's mode reads them, and any
//! hidden entry. The tests at the end check the helpers that read the top comment of a
//! diagnostic fixture.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

#[expect(dead_code, reason = "this binary uses only part of the shared helpers")]
mod support;

/// `dir`, a path under the crate directory, as an absolute path.
fn under_crate(dir: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(dir)
}

/// Lists `dir` once and returns one message for each entry that is not the `<code>.err` or
/// `<code>.json` of an explanation `docs/diagnostics/<code>.md`, and for each hidden or directory
/// entry. The messages come in name order. Panics when `dir` holds no `.err`, as a wrong `dir`
/// would otherwise pass with no entry checked.
fn explanation_orphans(dir: &Path) -> Vec<String> {
    let paths = support::sorted_entries(dir);
    assert!(
        paths
            .iter()
            .any(|path| path.extension().is_some_and(|ext| ext == "err")),
        "{} holds no `.err` file",
        dir.display()
    );
    paths
        .iter()
        .filter_map(|path| {
            support::stray_message(path).or_else(|| {
                let code = path
                    .file_stem()
                    .and_then(OsStr::to_str)
                    .expect("a file's name should be UTF-8");
                let belongs = matches!(
                    path.extension().and_then(OsStr::to_str),
                    Some("err" | "json")
                ) && support::is_code(code)
                    && under_crate(&format!("../../docs/diagnostics/{code}.md")).exists();
                (!belongs).then(|| {
                    format!(
                        "{} is not the `.err` or `.json` file of an explanation in docs/diagnostics",
                        path.display()
                    )
                })
            })
        })
        .collect()
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

/// GIVEN the directory `site/programs/mir`
/// WHEN it is listed once
/// THEN every entry is a `.sisu` case or the `.mir` file of one
#[test]
fn site_mir_directory_should_hold_only_cases_and_their_files() {
    assert_only_cases("../../site/programs/mir", &["mir"]);
}

/// GIVEN the directory `site/programs/explanations`
/// WHEN it is listed once
/// THEN every entry is the `.err` or `.json` file of an explanation in `docs/diagnostics`
#[test]
fn explanations_directory_should_hold_only_files_of_explanations() {
    assert_none(
        "../../site/programs/explanations",
        &explanation_orphans(&under_crate("../../site/programs/explanations")),
    );
}

/// GIVEN a directory with `E9999.err`, and no `docs/diagnostics/E9999.md`
/// WHEN `explanation_orphans` lists the directory
/// THEN it returns one message, which names `E9999.err`
#[test]
fn explanation_orphans_should_list_a_file_of_a_code_with_no_explanation() {
    let dir = support::fixture("explanation_without_page", &[("E9999.err", "")]);
    let messages = explanation_orphans(&dir);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("E9999.err"), "{messages:?}");
}

/// GIVEN a directory with only `E0001.json`
/// WHEN `explanation_orphans` lists the directory
/// THEN it panics and says that the directory holds no `.err` file
#[test]
#[should_panic(expected = "holds no `.err` file")]
fn explanation_orphans_should_fail_for_a_directory_with_no_err_file() {
    let dir = support::fixture("explanations_with_no_err", &[("E0001.json", "")]);
    explanation_orphans(&dir);
}

/// GIVEN a directory with `E0001.err` and a subdirectory `sub` that holds a file
/// WHEN `explanation_orphans` lists the directory
/// THEN it returns one message, which names `sub` as a directory
#[test]
fn explanation_orphans_should_list_a_subdirectory() {
    let dir = support::fixture(
        "explanations_with_subdirectory",
        &[("E0001.err", ""), ("sub/E0002.err", "")],
    );
    let messages = explanation_orphans(&dir);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("sub is a directory"), "{messages:?}");
}

/// GIVEN a top comment that names `7:9`, `(6:10)` and `E0038:3`, then code and a comment with `1:2`
/// WHEN `header_positions` reads the source
/// THEN it returns `7:9` and `6:10`, in that order
#[test]
fn header_positions_should_read_bare_and_parenthesized_positions_of_the_top_comment() {
    let source = "// THEN it reports E0038 at 7:9, and E0011 (6:10)\n\
                  // AND the code E0038:3 is not a position\n\
                  fn main() {}\n\
                  // 1:2\n";
    assert_eq!(support::header_positions(source), [(7, 9), (6, 10)]);
}

/// GIVEN a diagnostic with a primary span at 3:5 and a secondary span at 7:9
/// WHEN `reports_at` checks a position against it
/// THEN 3:5 and 7:9 match, and 7:8 and 9:7 do not
#[test]
fn reports_at_should_match_the_start_of_a_primary_or_a_secondary_span() {
    let json = r#"{"version":1,"kind":"diagnostic","primary":{"line":3,"column":5},"secondary":[{"line":7,"column":9}]}"#;
    assert!(support::reports_at(json, 3, 5));
    assert!(support::reports_at(json, 7, 9));
    assert!(!support::reports_at(json, 7, 8));
    assert!(!support::reports_at(json, 9, 7));
}
