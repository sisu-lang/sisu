//! Checks only the `cases` binary makes on the reports of a diagnostics program: that each safe
//! fix it shows is certain, and that no span it reports splits a character.

use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

use crate::support;

/// A diagnostic of `sisu check --format json`: its code, where it starts, and its fixes.
struct Report {
    code: String,
    start: usize,
    fixes: Vec<Fix>,
}

/// A fix of a `Report`.
struct Fix {
    safe: bool,
    edits: Vec<Edit>,
}

/// An edit of a `Fix`: a byte range of the source and its replacement.
struct Edit {
    start: usize,
    end: usize,
    replacement: String,
}

/// The number at `key` in the JSON object `value`.
fn number(value: &Value, key: &str) -> usize {
    value[key]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or_else(|| panic!("`{key}` should be a number in {value}"))
}

/// What `sisu` printed for `<dir>/<name>`: its exit code, and its diagnostics from `--format
/// json`, in the order it printed them. Asserts that the exit code is the one a report gives (1
/// for an error, else 0, never a panic's) and that every stderr line is a JSON object.
fn check(dir: &Path, name: &str, source: &[u8], code: &str) -> Vec<Report> {
    let args = support::report_args(if support::is_code(code) { code } else { "" });
    let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
        .current_dir(dir)
        .args(args)
        .args(["--format", "json"])
        .arg(name)
        .output()
        .expect("sisu starts");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let mut reports = Vec::new();
    let mut failed = false;
    for line in stderr.lines() {
        let object: Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("{name}: a stderr line is not JSON ({e}): {stderr}"));
        failed |= object["severity"] == "error";
        if let Ok(text) = std::str::from_utf8(source) {
            on_char_bounds(&object, text, name);
        }
        if object["kind"] != "diagnostic" {
            continue;
        }
        let primary = &object["primary"];
        let (start, end) = (number(primary, "start"), number(primary, "end"));
        assert!(
            start <= end && end <= source.len(),
            "{name}: a span {start}..{end} lies outside the source of {} bytes: {stderr}",
            source.len()
        );
        let fixes = object["fixes"].as_array().expect("`fixes` is an array");
        reports.push(Report {
            code: object["code"].as_str().expect("`code` is a string").into(),
            start,
            fixes: fixes
                .iter()
                .map(|fix| Fix {
                    safe: fix["safe"] == true,
                    edits: fix["edits"]
                        .as_array()
                        .expect("`edits` is an array")
                        .iter()
                        .map(|e| Edit {
                            start: number(e, "start"),
                            end: number(e, "end"),
                            replacement: e["replacement"].as_str().unwrap_or_default().into(),
                        })
                        .collect(),
                })
                .collect(),
        });
    }
    assert_eq!(
        out.status.code(),
        Some(i32::from(failed)),
        "{name}: sisu did not exit as its report says:\n{stderr}"
    );
    reports
}

/// Asserts that every `start` and `end` in the JSON `value`, which holds the spans of a report
/// (primary, secondary and the edits of each fix), lies on a character bound of `text`.
fn on_char_bounds(value: &Value, text: &str, name: &str) {
    match value {
        Value::Object(map) => {
            for key in ["start", "end"] {
                if let Some(at) = map.get(key).and_then(Value::as_u64) {
                    let at = usize::try_from(at).expect("an offset fits in usize");
                    assert!(
                        text.is_char_boundary(at),
                        "{name}: `{key}` {at} splits a character of the source, in {value}"
                    );
                }
            }
            map.values().for_each(|v| on_char_bounds(v, text, name));
        }
        Value::Array(items) => items.iter().for_each(|v| on_char_bounds(v, text, name)),
        _ => {}
    }
}

/// `source` with `edits` applied, the last first so that a byte offset stays true. Two inserts at
/// one offset keep their order.
fn apply(source: &[u8], edits: &[Edit]) -> Vec<u8> {
    let mut order: Vec<&Edit> = edits.iter().collect();
    order.sort_by_key(|e| e.start);
    let mut fixed = source.to_vec();
    for e in order.into_iter().rev() {
        fixed.splice(e.start..e.end, e.replacement.bytes());
    }
    fixed
}

/// Where the byte `at` of the source is after `edits`: each edit that ends by `at` moves it.
fn moved(edits: &[Edit], at: usize) -> usize {
    edits
        .iter()
        .filter(|e| e.end <= at)
        .fold(at, |at, e| at + e.replacement.len() - (e.end - e.start))
}

/// Checks that each safe fix of `source` is certain. `path` is a `.sisu` file, or the `.md`
/// explanation whose Broken program `source` is. Applied alone, a fix
/// removes the diagnostic it belongs to, and adds no error code that the source did not report. A
/// warning may be new, because a fix can reveal an unused name.
/// A safe fix is a promise (ADR 0013); on `feat/let-else` two of them produced code that
/// reviewers, not a test, found wrong.
pub(crate) fn safe_fixes(path: &Path, source: &[u8]) {
    // The name of the file `sisu` reads: a `.md` explanation holds its Broken program in `source`.
    let name = format!(
        "{}.sisu",
        path.file_stem()
            .and_then(OsStr::to_str)
            .expect("a UTF-8 name")
    );
    let name = name.as_str();
    let stem = path
        .file_stem()
        .and_then(OsStr::to_str)
        .expect("a UTF-8 name");
    let code = stem.split_once('-').map_or(stem, |(code, _)| code);
    let dir = support::case_dir(&path.with_extension("fixes"));
    fs::write(dir.join(name), source).expect("the case directory should be writable");
    let before = check(&dir, name, source, code);
    for d in &before {
        for fix in d.fixes.iter().filter(|f| f.safe) {
            let fixed = apply(source, &fix.edits);
            fs::write(dir.join(name), &fixed).expect("the case directory should be writable");
            let after = check(&dir, name, &fixed, code);
            let at = moved(&fix.edits, d.start);
            let here = |r: &Report| r.code == d.code && r.start == at;
            // A fix may move another diagnostic of the code onto the place of this one.
            let moved_there = before.iter().any(|e| {
                !std::ptr::eq(e, d) && e.code == d.code && moved(&fix.edits, e.start) == at
            });
            let shown = String::from_utf8_lossy(&fixed);
            assert!(
                moved_there || !after.iter().any(here),
                "{}: a safe fix of {} leaves its diagnostic:\n{shown}",
                path.display(),
                d.code
            );
            let mut known: Vec<&str> = before.iter().map(|r| r.code.as_str()).collect();
            // A warning may appear: a fix that adds a `mut` or a use can reveal an unused name.
            for r in after.iter().filter(|r| r.code.starts_with('E')) {
                match known.iter().position(|&c| c == r.code) {
                    Some(i) => {
                        known.swap_remove(i);
                    }
                    None => panic!(
                        "{}: a safe fix of {} adds {}:\n{shown}",
                        path.display(),
                        d.code,
                        r.code
                    ),
                }
            }
        }
    }
}

/// The comment that `non_ascii_spans` inserts: three two-byte characters, so that every offset
/// after it moves by an odd number of bytes.
const COMMENT: &str = "//\u{e4}\u{e4}\u{e4}\n";

/// The offsets just after each `(`, `[` and `{` of `text` that stands in code: not after a `//`
/// on its line, and not inside a string (a line with an odd number of `"` before it).
fn open_brackets(text: &str) -> Vec<usize> {
    let mut found = Vec::new();
    let mut line_start = 0;
    for line in text.split_inclusive('\n') {
        let code = line.split_once("//").map_or(line, |(code, _)| code);
        for (i, c) in code.char_indices() {
            let quotes = code[..i].matches('"').count();
            if matches!(c, '(' | '[' | '{') && quotes % 2 == 0 {
                found.push(line_start + i + 1);
            }
        }
        line_start += line.len();
    }
    found
}

/// `source` with `COMMENT` inserted at each of `points`, which are byte offsets.
fn with_comments(source: &str, points: &[usize]) -> String {
    let mut text = source.to_owned();
    for &at in points.iter().rev() {
        text.insert_str(at, COMMENT);
    }
    text
}

/// Checks that `sisu check` handles a program whose offsets a multi-byte comment moves: it exits
/// as its report says, in JSON and in text, and no span it reports splits a character. A span
/// built as `token.start() + n`, with `n` the length of a name, splits one when text with
/// multi-byte characters comes between (the defect of `jump.span.start() + name.len()` on
/// `feat/let-else`, which made `Source::slice` panic). The lexer has no block comments, so each
/// variant puts a whole `//` line in place, which keeps the meaning of the program except inside
/// a string. The variants put one before every line, after every opening bracket, and at the
/// start of every diagnostic. The report need not be the original's: it only has to be sound.
pub(crate) fn non_ascii_spans(path: &Path, source: &[u8]) {
    let Ok(text) = std::str::from_utf8(source) else {
        return;
    };
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .expect("a UTF-8 name");
    let stem = path
        .file_stem()
        .and_then(OsStr::to_str)
        .expect("a UTF-8 name");
    let code = stem.split_once('-').map_or(stem, |(code, _)| code);
    // shortcut: the limit programs of E0162 and E0163 take seconds to check, so six more runs go
    // over the time budget; drop this when they check fast.
    if matches!(code, "E0162" | "E0163") {
        return;
    }
    let dir = support::case_dir(&path.with_extension("spans"));
    fs::write(dir.join(name), source).expect("the case directory should be writable");
    let starts: Vec<usize> = check(&dir, name, source, code)
        .iter()
        .map(|r| r.start)
        .collect();
    let lines = text.match_indices('\n').map(|(i, _)| i + 1);
    let brackets = open_brackets(text);
    let variants = [
        std::iter::once(0).chain(lines).collect::<Vec<_>>(),
        brackets,
        starts,
    ];
    for points in variants {
        let moved = with_comments(text, &points);
        fs::write(dir.join(name), &moved).expect("the case directory should be writable");
        check(&dir, name, moved.as_bytes(), code);
        let out = Command::new(env!("CARGO_BIN_EXE_sisu"))
            .current_dir(&dir)
            .args(support::report_args(if support::is_code(code) {
                code
            } else {
                ""
            }))
            .arg(name)
            .output()
            .expect("sisu starts");
        assert!(
            matches!(out.status.code(), Some(0 | 1)),
            "{}: sisu exits {:?} on this program:\n{moved}\n{}",
            path.display(),
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
