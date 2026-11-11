//! The rule markers in the test files: the walk that finds the files, and the reading of the
//! marker lines in them.

use std::fmt::Display;
use std::fs;
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
    /// The hash of the rule text when the test was last read: four lowercase hex digits.
    pub(crate) hash: String,
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

/// What the walk does with `path`, a path from the repository root. It reads a `.rs` or `.sisu`
/// file under `crates/`. It reads such a file under `site/programs/` only to find a marker fault.
pub(crate) fn walked(path: &str) -> Walked {
    let extension = Path::new(path).extension().and_then(|ext| ext.to_str());
    if !matches!(extension, Some("rs" | "sisu")) {
        Walked::No
    } else if path.starts_with("crates/") {
        Walked::Read
    } else if path.starts_with("site/programs/") {
        Walked::Shown
    } else {
        Walked::No
    }
}

/// Each file that `walked` reads, under `crates/` and `site/programs/` of `repo`, as its path
/// from `repo` and its bytes, sorted by path. A path that the walk cannot read is an error that
/// names it. The walk does not go into a directory that is a symlink.
pub(crate) fn walk(repo: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut files = Vec::new();
    for root in ["crates", "site/programs"] {
        walk_dir(repo, root, &mut files)?;
    }
    files.sort();
    Ok(files)
}

/// Adds the files under `dir`, a path from `repo`, to `files`.
fn walk_dir(repo: &Path, dir: &str, files: &mut Vec<(String, Vec<u8>)>) -> Result<(), String> {
    let fail = |path: &str, error: &dyn Display| format!("{path}: {error}");
    let entries = fs::read_dir(repo.join(dir)).map_err(|error| fail(dir, &error))?;
    for entry in entries {
        let entry = entry.map_err(|error| fail(dir, &error))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|name| fail(dir, &format!("{} is not UTF-8", name.to_string_lossy())))?;
        let path = format!("{dir}/{name}");
        let kind = entry.file_type().map_err(|error| fail(&path, &error))?;
        if kind.is_dir() {
            walk_dir(repo, &path, files)?;
        } else if walked(&path) != Walked::No {
            let bytes = fs::read(entry.path()).map_err(|error| fail(&path, &error))?;
            files.push((path, bytes));
        }
    }
    Ok(())
}

/// The markers and the marker faults in `bytes`, the file `file`. A line is a marker line when
/// `r[` is the first text after its comment opener. A marker line that does not parse is a fault.
/// In a file that the site shows, each marker line is a fault and there is no marker.
pub(crate) fn read_markers(file: &str, bytes: &[u8]) -> (Vec<RuleMarker>, Vec<MarkerFault>) {
    let shown = walked(file) == Walked::Shown;
    let text = String::from_utf8_lossy(bytes);
    let mut markers = Vec::new();
    let mut faults = Vec::new();
    for (index, text) in text.lines().enumerate() {
        let Some(rest) = after_opener(text) else {
            continue;
        };
        let line = index + 1;
        let fault = |message: &str| MarkerFault {
            file: file.to_string(),
            line,
            message: message.to_string(),
        };
        if shown {
            faults.push(fault(
                "the site shows this file, so a marker cannot go in it.",
            ));
        } else if let Some((id, hash)) = parse_marker(rest) {
            markers.push(RuleMarker {
                file: file.to_string(),
                line,
                id: id.to_string(),
                hash: hash.to_string(),
            });
        } else {
            faults.push(fault(
                "a comment line that starts with `r[` is a rule marker, and this marker has a bad \
                 form. Write it as `r[verify <id>@<4 lowercase hex digits>]`, alone on its line.",
            ));
        }
    }
    (markers, faults)
}

/// The text of `line` after `r[`, when `r[` is the first text after the opener of a comment: `//`,
/// `///` or `//!`.
fn after_opener(line: &str) -> Option<&str> {
    let comment = line.trim_start().strip_prefix("//")?;
    let comment = comment.strip_prefix(['/', '!']).unwrap_or(comment);
    comment.trim_start().strip_prefix("r[")
}

/// The ID and the hash of the marker whose text after `r[` is `rest`: `verify `, an ID, `@`, four
/// lowercase hex digits, `]`, and then only white space. The ID is not empty and holds no white
/// space or `]`.
fn parse_marker(rest: &str) -> Option<(&str, &str)> {
    let (id, rest) = rest.strip_prefix("verify ")?.split_once('@')?;
    let id_ok = !id.is_empty() && !id.contains(|c: char| c.is_whitespace() || c == ']');
    let hash = rest.get(..4)?;
    let hash_ok = hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    let after = rest[4..].strip_prefix(']')?;
    (id_ok && hash_ok && after.trim().is_empty()).then_some((id, hash))
}

/// The fault for each of `markers` whose ID is not in `known`.
pub(crate) fn unknown_ids(markers: &[RuleMarker], known: &FxHashSet<String>) -> Vec<MarkerFault> {
    markers
        .iter()
        .filter(|marker| !known.contains(&marker.id))
        .map(|marker| MarkerFault {
            file: marker.file.clone(),
            line: marker.line,
            message: format!("no rule has the ID {}.", marker.id),
        })
        .collect()
}

/// The markers and the marker faults of each file that the walk finds under `repo`.
pub(crate) fn markers_in(repo: &Path) -> Result<(Vec<RuleMarker>, Vec<MarkerFault>), String> {
    let mut markers = Vec::new();
    let mut faults = Vec::new();
    for (file, bytes) in walk(repo)? {
        let (found, bad) = read_markers(&file, &bytes);
        markers.extend(found);
        faults.extend(bad);
    }
    Ok((markers, faults))
}
