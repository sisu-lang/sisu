//! The reference checker's reading of rule markers in the test files: the marker grammar, the
//! unknown-ID check, and the walk that finds the files.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

use rustc_hash::FxHashSet;

use crate::reference_check::markers::{
    MarkerFault, Walked, markers_in, read_markers, unknown_ids, walk, walked,
};
use crate::reference_check::rules::known_ids;
use crate::reference_check::{Page, read_page};
use crate::support;

/// The IDs of `ids` as a set.
fn id_set(ids: &[&str]) -> FxHashSet<String> {
    ids.iter().map(|id| (*id).to_string()).collect()
}

/// Asserts that `faults` holds one fault, at `file` and line 1, whose message holds `fragment`.
/// `row` names the table row in a failure.
fn assert_one_fault(row: &str, faults: &[MarkerFault], file: &str, fragment: &str) {
    let [fault] = faults else {
        panic!("row {row}: faults are {faults:?}")
    };
    assert_eq!(fault.file, file, "row {row}: the file of {fault:?}");
    assert_eq!(fault.line, 1, "row {row}: the line of {fault:?}");
    assert!(
        fault.message.contains(fragment),
        "row {row}: {:?} should hold {fragment:?}",
        fault.message
    );
}

/// GIVEN a test with the comment `// r[verify strings.text.nul@1234]`
/// AND no rule with the ID `strings.text.nul`
/// WHEN its markers are read and checked against the IDs of the joined pages
/// THEN the result is the fault "no rule has the ID strings.text.nul" at the file and line of the
/// marker
/// AND a marker that does not parse gives a fault for its form at its line
/// AND a marker in a file that the site shows gives a fault for the file
#[test]
fn bad_marker_should_fail_at_its_line() {
    let test = "crates/sisuc/tests/run/a.sisu";
    let known = id_set(&["strings.text.null"]);
    // Each row is a name, a file, the bytes of the file and the fragment of the one fault.
    let rows: [(&str, &str, &[u8], &str); 5] = [
        (
            "unknown ID",
            test,
            b"// r[verify strings.text.nul@1234]\n",
            "no rule has the ID",
        ),
        (
            "no hash",
            test,
            b"// r[verify strings.text.null]\n",
            "marker has a bad form",
        ),
        (
            "upper case hash",
            test,
            b"// r[verify strings.text.null@7C1E]\n",
            "marker has a bad form",
        ),
        (
            "text after the marker",
            test,
            b"// r[verify strings.text.null@7c1e] x\n",
            "marker has a bad form",
        ),
        (
            "file that the site shows",
            "site/programs/run/hello.sisu",
            b"// r[verify strings.text.null@3958]\n",
            "the site shows this file",
        ),
    ];
    for (row, file, bytes, fragment) in rows {
        let (markers, mut faults) = read_markers(file, bytes);
        if file == test {
            faults.extend(unknown_ids(&markers, &known));
        } else {
            assert!(markers.is_empty(), "row {row}: markers are {markers:?}");
        }
        assert_one_fault(row, &faults, file, fragment);
    }
}

/// GIVEN a `README` page with the item `- {#README.a.b} x`
/// AND the marker `r[verify README.a.b@1234]`
/// WHEN the marker is checked against the known IDs of the pages
/// THEN the result is the fault "no rule has the ID" at the line of the marker
#[test]
fn marker_should_fail_on_an_id_of_the_index() {
    let pages = [(
        "README".to_string(),
        read_page("**Rules**\n\n- {#README.a.b} x\n"),
    )];
    let (markers, faults) = read_markers("crates/a.rs", b"// r[verify README.a.b@1234]\n");
    assert!(faults.is_empty(), "faults are {faults:?}");
    let faults = unknown_ids(&markers, &known_ids(&pages));
    assert_one_fault("index ID", &faults, "crates/a.rs", "no rule has the ID");
}

/// GIVEN the pages `README` and `strings`
/// AND `strings` has a rule with an ID, a rule with an ID of another page, and a rule with no ID
/// WHEN the known IDs are found
/// THEN they are the two IDs of `strings`
#[test]
fn known_ids_should_hold_each_id_of_a_joined_page() {
    let pages: [(String, Page); 2] = [
        (
            "README".to_string(),
            read_page("**Rules**\n\n- {#README.a.b} x\n"),
        ),
        (
            "strings".to_string(),
            read_page("**Rules**\n\n- {#strings.a} x\n- {#text.null} y\n- z\n"),
        ),
    ];
    assert_eq!(known_ids(&pages), id_set(&["strings.a", "text.null"]));
}

/// GIVEN a file whose text is <line>
/// WHEN its markers are read
/// THEN the result is <result>
#[test]
fn only_a_marker_line_should_be_read() {
    let file = "crates/sisuc/tests/a.rs";
    // Each row is a name, the bytes of the file, and the line and ID of each marker. No row has
    // a fault.
    let rows: [Row<'_>; 10] = [
        (
            "// opener",
            b"// r[verify strings.text.null@7c1e]",
            &[(1, "strings.text.null")],
        ),
        (
            "/// opener",
            b"/// r[verify strings.text.null@7c1e]",
            &[(1, "strings.text.null")],
        ),
        (
            "//! opener",
            b"//! r[verify strings.text.null@7c1e]",
            &[(1, "strings.text.null")],
        ),
        (
            "string literal",
            br#"    ("a.sisu", "// r[verify strings.text.nul@1234]\n"),"#,
            &[],
        ),
        (
            "quoted marker",
            b"/// GIVEN a test with the comment `// r[verify x.y@1234]`",
            &[],
        ),
        ("other r[ text", b"// see order[k] and error[E0008]", &[]),
        (
            "trailing comment",
            b"print(x) // r[verify strings.text.null@7c1e]",
            &[],
        ),
        (
            "not UTF-8",
            b"// caf\xe9\n// r[verify strings.text.null@3958]\n", // spellchecker:disable-line
            &[(2, "strings.text.null")],
        ),
        (
            "trailing spaces",
            b"// r[verify strings.text.null@7c1e]  \n",
            &[(1, "strings.text.null")],
        ),
        (
            "CRLF line end",
            b"// r[verify strings.text.null@7c1e]\r\n",
            &[(1, "strings.text.null")],
        ),
    ];
    for (row, bytes, want) in rows {
        let (markers, faults) = read_markers(file, bytes);
        assert!(faults.is_empty(), "row {row}: faults are {faults:?}");
        let got: Vec<(usize, &str)> = markers.iter().map(|m| (m.line, m.id.as_str())).collect();
        assert_eq!(got, want, "row {row}: markers are {markers:?}");
        assert!(
            markers.iter().all(|m| m.file == file),
            "row {row}: {markers:?}"
        );
    }
}

/// A table row: its name, the bytes of a file, and the line and ID of each marker in it.
type Row<'a> = (&'a str, &'a [u8], &'a [(usize, &'a str)]);

/// The rows of the walk's path filter: a path and what the walk does with it.
const WALKED: [(&str, Walked); 12] = [
    ("crates/sisuc/tests/run/strings.sisu", Walked::Read),
    (
        "crates/sisuc/tests/run-fail/append_recheck_inner.sisu",
        Walked::Read,
    ),
    ("crates/sisuc/tests/diagnostics/E0068.sisu", Walked::Read),
    ("crates/sisuc/tests/programs/anychain.sisu", Walked::Read),
    ("crates/sisuc/tests/tokens/all_tokens.sisu", Walked::Read),
    ("crates/sisuc/tests/cases.rs", Walked::Read),
    ("crates/sisuc/src/check/tests/strings.rs", Walked::Read),
    ("crates/sisuc/src/lexer/mod.rs", Walked::Read),
    ("crates/runtime/src/text.rs", Walked::Read),
    ("site/programs/run/hello.sisu", Walked::Shown),
    ("crates/sisuc/tests/run/strings.out", Walked::No),
    ("selfhost/lexer.sisu", Walked::No),
];

/// GIVEN the path <path>
/// WHEN the walk's path filter reads it
/// THEN the result is <result>
#[test]
fn walk_should_read_only_test_files_under_the_two_roots() {
    for (path, want) in WALKED {
        assert_eq!(walked(path), want, "row {path}");
    }
}

/// GIVEN the repository
/// WHEN the walk runs
/// THEN it reads at least one file under each directory of a row whose result is read
/// AND it reads `crates/sisuc/tests/diagnostics/E0072.sisu`, which is not valid UTF-8
#[test]
fn walk_should_find_files_under_each_root() {
    let files = walk(Path::new("../..")).expect("the walk should read the repository");
    for (path, _) in WALKED.iter().filter(|(_, result)| *result != Walked::No) {
        let (dir, _) = path.rsplit_once('/').expect("a path has a directory");
        let under = format!("{dir}/");
        assert!(
            files.iter().any(|(file, _)| file.starts_with(&under)),
            "row {path}: no file under {dir}"
        );
    }
    let bytes = files
        .iter()
        .find(|(file, _)| file == "crates/sisuc/tests/diagnostics/E0072.sisu")
        .map(|(_, bytes)| bytes);
    let bytes = bytes.expect("the walk should read E0072.sisu");
    assert!(
        std::str::from_utf8(bytes).is_err(),
        "E0072.sisu should not be UTF-8"
    );
}

/// GIVEN a temporary directory with a `.sisu` path that cannot be read, such as a dangling
/// symlink
/// WHEN the walk runs on that directory
/// THEN the result is a failure that names the path, not a skip
#[test]
fn unreadable_file_should_fail_the_walk() {
    let dir = support::case_dir(Path::new("reference_checker/unreadable_file"));
    fs::create_dir_all(dir.join("crates")).expect("creates crates");
    fs::create_dir_all(dir.join("site/programs")).expect("creates site/programs");
    symlink(dir.join("absent"), dir.join("crates/x.sisu")).expect("creates the symlink");
    let walked_err = walk(&dir).expect_err("the walk should fail");
    assert!(
        walked_err.contains("crates/x.sisu"),
        "walk error: {walked_err}"
    );
    let markers_err = markers_in(&dir).expect_err("markers_in should keep the walk's error");
    assert!(
        markers_err.contains("crates/x.sisu"),
        "markers error: {markers_err}"
    );
}
