//! The rule markers in the test files: the walk that finds the files, and the reading of the
//! marker lines in them.

use std::path::Path;

use rustc_hash::FxHashSet;

/// A rule marker: a line that names a reference rule by its ID.
#[derive(Debug)]
pub(crate) struct RuleMarker {
    /// The path of the file, from the repository root.
    pub(crate) file: String,
    /// The 1-based line of the marker.
    pub(crate) line: usize,
    pub(crate) id: String,
}

/// A fault in a marker line, or in the file that holds it.
#[derive(Debug)]
pub(crate) struct MarkerFault {
    pub(crate) file: String,
    pub(crate) line: usize,
    pub(crate) message: String,
}

/// What the walk does with a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Walked {
    /// The walk does not read the file.
    No,
    /// The walk reads the file for its markers.
    Read,
    /// The site shows the file, so the walk reads it only to find a marker fault.
    Shown,
}

/// What the walk does with `path`, a path from the repository root.
pub(crate) fn walked(_path: &str) -> Walked {
    Walked::No
}

/// Each file that `walked` reads, under `crates/` and `site/programs/` of `repo`, as its path
/// from `repo` and its bytes, sorted by path.
#[expect(clippy::unnecessary_wraps, reason = "red commit: Task 2 fills it")]
pub(crate) fn walk(_repo: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    Ok(Vec::new())
}

/// The markers and the marker faults in `bytes`, the file `file`.
pub(crate) fn read_markers(_file: &str, _bytes: &[u8]) -> (Vec<RuleMarker>, Vec<MarkerFault>) {
    (Vec::new(), Vec::new())
}

/// The fault for each of `markers` whose ID is not in `known`.
pub(crate) fn unknown_ids(_markers: &[RuleMarker], _known: &FxHashSet<String>) -> Vec<MarkerFault> {
    Vec::new()
}

/// The markers and the marker faults of each file that the walk finds under `repo`.
#[expect(clippy::unnecessary_wraps, reason = "red commit: Task 2 fills it")]
pub(crate) fn markers_in(_repo: &Path) -> Result<(Vec<RuleMarker>, Vec<MarkerFault>), String> {
    Ok((Vec::new(), Vec::new()))
}
