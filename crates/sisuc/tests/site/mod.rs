//! Checks only the `cases` binary makes: the site's pending program, and the output of each
//! explanation's Broken program, which the site's diagnostic pages show.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::support;

/// The directory of the site's checked programs and their outputs.
fn programs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../site/programs")
}

/// The code an explanation or expectation file at `path` is named after: its file stem.
fn code_of(path: &Path) -> &str {
    path.file_stem()
        .and_then(OsStr::to_str)
        .expect("a file's name should be UTF-8")
}

/// Checks the explanation `docs/diagnostics/<code>.md` at `path`: its Broken program, written to
/// `<code>.sisu` in its `case_dir`, makes `sisu` print `site/programs/explanations/<code>.err`
/// and `.json` (see `support::diagnose`). With `bless`, rewrites both files instead. A retired
/// code's page is skipped, as the site leaves it out.
pub(crate) fn explanation(path: &Path, bless: bool) {
    let code = code_of(path);
    let markdown = fs::read_to_string(path).expect("an explanation should be readable");
    if markdown.lines().any(|l| l.starts_with("Status: retired")) {
        return;
    }
    let source = support::case_dir(path).join(format!("{code}.sisu"));
    fs::write(&source, broken_program(code, &markdown))
        .expect("the case directory should be writable");
    support::diagnose(&source, &programs().join("explanations").join(code), bless);
}

/// The Broken program of `code`'s explanation `markdown`: the body of the one ```` ```sisu ````
/// block between the `## Broken` line and the next `## ` line, read as `sisu_block` in
/// `src/diagnostic/explanations.rs` reads it, so the two cannot disagree. E0072's is its text
/// saved in Latin-1, one byte per char, as that explanation says; every other is UTF-8.
fn broken_program(code: &str, markdown: &str) -> Vec<u8> {
    let file = format!("docs/diagnostics/{code}.md");
    let (_, rest) = markdown
        .split_once("\n## Broken\n")
        .unwrap_or_else(|| panic!("{file} has no `## Broken` line"));
    let section = rest
        .split_once("\n## ")
        .map_or(rest, |(section, _)| section);
    let blocks: Vec<&str> = section.split("```sisu\n").skip(1).collect();
    let [block] = blocks.as_slice() else {
        panic!(
            "{file}: its Broken section holds {} sisu blocks, not one",
            blocks.len()
        )
    };
    let (program, _) = block
        .split_once("```")
        .unwrap_or_else(|| panic!("{file}: its Broken sisu block is not closed"));
    if code != "E0072" {
        return program.as_bytes().to_vec();
    }
    program
        .chars()
        .map(|c| {
            u8::try_from(c)
                .unwrap_or_else(|_| panic!("{file}: {c:?} in its Broken program is not Latin-1"))
        })
        .collect()
}

/// Checks the pending case at `path`, a program the site shows before it compiles: `sisu
/// check <name>`, run from its directory, reports an error. The day it compiles, this fails,
/// asking for it to become a `site/programs/run/` case.
pub(crate) fn pending(path: &Path) {
    let dir = path.parent().expect("a case should sit in a directory");
    let name = path.file_name().expect("a case should have a file name");
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(dir)
        .arg("check")
        .arg(name)
        .output()
        .expect("sisu starts");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let display = path.display();
    assert!(
        out.status.code() != Some(0),
        "{display} compiles now: move it to site/programs/run/ with a .out file and delete the \
         pending case"
    );
    assert!(
        out.status.code() == Some(1) && stderr.lines().any(|l| l.starts_with("error[")),
        "{display}: sisu check exits {:?} without reporting an error:\n{stderr}",
        out.status
    );
}

/// Passes for `<code>.err` or `<code>.json` when `docs/diagnostics/<code>.md` exists; panics on
/// any other file, naming it, and on any hidden file beside it.
pub(crate) fn explanation_orphan_check(path: &Path) {
    let code = code_of(path);
    let belongs = matches!(
        path.extension().and_then(OsStr::to_str),
        Some("err" | "json")
    ) && support::is_code(code)
        && Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../docs/diagnostics/{code}.md"))
            .exists();
    assert!(
        belongs,
        "{} is not the `.err` or `.json` file of an explanation in docs/diagnostics",
        path.display()
    );
    support::no_hidden_sibling(path);
}
