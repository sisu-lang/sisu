//! The explanation, README and fixture-coverage tests, and `apply`, which applies fixes.

use std::fs;
use std::path::{Path, PathBuf};

use super::{Code, Diagnostic, Edit, Fix, RETIRED};
use crate::front_end::{decode, front_end};
use crate::lexer::lex;
use crate::parser::{parse, parse_ok};

/// `source` with `edits` applied. An edit equal to one already taken is dropped, as the same fix
/// can come from several diagnostics; the rest must not overlap, and two insertions must not
/// share an offset, as their order would be arbitrary.
pub(crate) fn apply(source: &str, edits: &[&Edit]) -> String {
    let mut taken: Vec<&Edit> = Vec::new();
    for &edit in edits {
        if !taken.contains(&edit) {
            taken.push(edit);
        }
    }
    taken.sort_by_key(|e| (e.span().start(), e.span().end()));
    for pair in taken.windows(2) {
        assert!(
            pair[0].span().end() <= pair[1].span().start(),
            "edits {:?} and {:?} overlap",
            pair[0],
            pair[1]
        );
        assert!(
            pair[0].span().start() != pair[1].span().start()
                || pair[0].span().end() != pair[0].span().start()
                || pair[1].span().end() != pair[1].span().start(),
            "insertions {:?} and {:?} share an offset",
            pair[0],
            pair[1]
        );
    }
    let mut out = source.to_string();
    for edit in taken.iter().rev() {
        out.replace_range(edit.span().range(), edit.replacement());
    }
    out
}

/// The edits of a fix as `(start, end, replacement)`.
pub(crate) type FixEdits = Vec<(usize, usize, String)>;

/// Whether the one fix of `d` is safe, and its edits; `None` when `d` has no fix. A parser
/// or lexer diagnostic has at most one.
pub(crate) fn fix_edits(d: &Diagnostic) -> Option<(bool, FixEdits)> {
    let [fix] = d.fixes() else {
        assert!(d.fixes().is_empty(), "at most one fix, got {:?}", d.fixes());
        return None;
    };
    let edits = fix
        .edits()
        .iter()
        .map(|e| {
            (
                e.span().start(),
                e.span().end(),
                e.replacement().to_string(),
            )
        })
        .collect();
    Some((fix.safe(), edits))
}

/// `source` with the one fix of `d` applied.
pub(crate) fn apply_fix(source: &str, d: &Diagnostic) -> String {
    let edits: Vec<&Edit> = d.fixes().iter().flat_map(Fix::edits).collect();
    apply(source, &edits)
}

/// `source` with every safe fix of every diagnostic it reports applied, and how many fixes
/// that was, or `None` when no diagnostic of `code` has one. The result must still lex and
/// parse; the checker may still report errors it reveals.
fn fixed(source: &str, code: Code) -> Option<(String, usize)> {
    let (_, diagnostics) = front_end(source);
    let safe = |d: &&Diagnostic| d.fixes().iter().any(Fix::safe);
    diagnostics.iter().filter(|d| d.code() == code).find(safe)?;
    let fixes: Vec<&Fix> = diagnostics
        .iter()
        .flat_map(Diagnostic::fixes)
        .filter(|f| f.safe())
        .collect();
    let edits: Vec<&Edit> = fixes.iter().flat_map(|f| f.edits()).collect();
    let result = apply(source, &edits);
    parse_ok(&result);
    Some((result, fixes.len()))
}

fn manifest() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn docs() -> PathBuf {
    manifest().join("../../docs/diagnostics")
}

fn codes(source: &str) -> Vec<Code> {
    front_end(source).1.iter().map(Diagnostic::code).collect()
}

/// An explanation's title, and its Broken and Fixed programs.
struct Explanation {
    title: String,
    broken: String,
    fixed: String,
}

/// Parses `code`'s explanation, asserting its fixed shape.
fn explanation(code: Code) -> Explanation {
    let text = code.explanation();
    let first = text.lines().next().unwrap_or_default();
    let prefix = format!("# {}: ", code.as_str());
    let title = first
        .strip_prefix(&prefix)
        .unwrap_or_else(|| panic!("{code:?}: the first line {first:?} is not `{prefix}<title>`"));
    assert!(!title.trim().is_empty(), "{code:?} has an empty title");
    let heading = |name: &str| {
        text.find(&format!("\n## {name}\n"))
            .unwrap_or_else(|| panic!("{code:?} has no `## {name}`"))
    };
    let (why, broken, fixed) = (heading("Why"), heading("Broken"), heading("Fixed"));
    assert!(
        why < broken && broken < fixed,
        "{code:?}: Why, Broken, Fixed out of order"
    );
    let fixed_end = text[fixed + 1..]
        .find("\n## ")
        .map_or(text.len(), |i| fixed + 1 + i);
    Explanation {
        title: title.to_string(),
        broken: sisu_block(code, &text[broken..fixed]),
        fixed: sisu_block(code, &text[fixed..fixed_end]),
    }
}

/// The one ` ```sisu ` block of `section`.
fn sisu_block(code: Code, section: &str) -> String {
    let blocks: Vec<&str> = section.split("```sisu\n").skip(1).collect();
    let [block] = blocks.as_slice() else {
        panic!(
            "{code:?}: a section holds {} sisu blocks, not one",
            blocks.len()
        )
    };
    let (program, _) = block
        .split_once("```")
        .unwrap_or_else(|| panic!("{code:?}: a sisu block is not closed"));
    program.to_string()
}

#[test]
fn every_code_has_an_explanation_of_the_fixed_shape() {
    for &code in Code::ALL {
        explanation(code);
    }
}

#[test]
fn every_explanation_file_is_a_code() {
    let mut seen = 0;
    for entry in fs::read_dir(docs()).expect("docs/diagnostics should be readable") {
        let path = entry.expect("an entry should be readable").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if name == "README.md" {
            continue;
        }
        let stem = name
            .strip_suffix(".md")
            .unwrap_or_else(|| panic!("{name} in docs/diagnostics is not an explanation"));
        let retired = RETIRED.iter().any(|&(code, _)| code == stem);
        assert!(
            Code::parse(stem).is_some() || retired,
            "{name} names no code"
        );
        seen += 1;
    }
    assert_eq!(seen, Code::ALL.len() + RETIRED.len());
}

/// The site leaves out a retired page by its status line (`site/build.py`, `tests/site/mod.rs`).
#[test]
fn retired_explanations_say_so() {
    for &(code, text) in RETIRED {
        assert!(
            text.lines().any(|l| l.starts_with("Status: retired ")),
            "{code} has no `Status: retired` line"
        );
    }
}

/// The codes of `code`'s Broken example. E0072's stands for its text saved in Latin-1, since an
/// explanation is UTF-8.
fn broken_codes(code: Code, broken: &str) -> Vec<Code> {
    if code != Code::InvalidUtf8 {
        return codes(broken);
    }
    let latin1 = broken
        .chars()
        .map(|c| u8::try_from(c).expect("the example should be Latin-1"))
        .collect();
    decode(latin1)
        .err()
        .map(|e| e.1.code())
        .into_iter()
        .collect()
}

/// The text of the fixture at `path`, or `None` when it is not UTF-8 and so has nothing to fix.
fn fixture_text(path: &Path) -> Option<String> {
    decode(fs::read(path).expect("a fixture should be readable")).ok()
}

#[test]
fn broken_examples_produce_exactly_their_code() {
    for &code in Code::ALL {
        assert_eq!(
            broken_codes(code, &explanation(code).broken),
            [code],
            "{code:?}"
        );
    }
}

/// The codes of `source`, a fixed program of `code`'s explanation. The fix of a `todo` written
/// another way leaves a `todo`, which always warns, so that warning does not count there.
fn fixed_codes(code: Code, source: &str) -> Vec<Code> {
    let mut found = codes(source);
    if code == Code::TodoForm {
        found.retain(|&c| c != Code::TodoLeft);
    }
    found
}

#[test]
fn fixed_examples_check_clean() {
    for &code in Code::ALL {
        assert_eq!(fixed_codes(code, &explanation(code).fixed), [], "{code:?}");
    }
}

/// A code whose diagnostic has no safe fix has nothing to apply.
#[test]
fn safe_fixes_make_broken_examples_clean() {
    let mut applied = 0;
    for &code in Code::ALL {
        if let Some((result, count)) = fixed(&explanation(code).broken, code) {
            assert!(count > 0, "{code:?}: no fix was applied");
            assert_eq!(fixed_codes(code, &result), [], "{code:?}:\n{result}");
            applied += count;
        }
    }
    assert!(applied > 0, "no explanation's Broken example has a fix");
}

/// Every `tests/diagnostics` fixture, and its file stem.
fn all_fixtures() -> Vec<(PathBuf, String)> {
    let dir = manifest().join("tests/diagnostics");
    let mut found = Vec::new();
    for entry in fs::read_dir(dir).expect("tests/diagnostics should be readable") {
        let path = entry.expect("an entry should be readable").path();
        let Some(stem) = path
            .extension()
            .filter(|e| *e == "sisu")
            .and_then(|_| path.file_stem())
            .and_then(|s| s.to_str())
        else {
            continue;
        };
        let stem = stem.to_string();
        found.push((path, stem));
    }
    found
}

/// The `tests/diagnostics` fixtures whose name starts with a code, and that code.
fn fixtures() -> Vec<(PathBuf, Code)> {
    all_fixtures()
        .into_iter()
        .filter_map(|(path, stem)| {
            let code = stem.split_once('-').map_or(stem.as_str(), |(code, _)| code);
            Code::parse(code).map(|code| (path, code))
        })
        .collect()
}

/// The lexer and parser diagnostics of `source`.
fn syntax_codes(source: &str) -> Vec<Code> {
    let diagnostics = match lex(source) {
        Ok(lexed) => parse(lexed, false).diagnostics,
        Err(d) => vec![d],
    };
    diagnostics.iter().map(Diagnostic::code).collect()
}

/// `a` with one occurrence of each code of `b` removed.
fn minus(mut a: Vec<Code>, b: &[Code]) -> Vec<Code> {
    for code in b {
        if let Some(i) = a.iter().position(|c| c == code) {
            a.remove(i);
        }
    }
    a
}

/// Applying one safe fix removes the diagnostics that carry it and no other, and adds no syntax
/// error: a diagnostic that disappears with it was a cascade of the same mistake (ADR 0013). The
/// checker may still report errors the fix reveals.
#[test]
fn each_safe_fix_removes_exactly_its_diagnostic() {
    let mut applied = 0;
    for (path, _) in all_fixtures() {
        let Some(source) = fixture_text(&path) else {
            continue;
        };
        let diagnostics = front_end(&source).1;
        let before: Vec<Code> = diagnostics.iter().map(Diagnostic::code).collect();
        for d in &diagnostics {
            for fix in d.fixes().iter().filter(|f| f.safe()) {
                let edits: Vec<&Edit> = fix.edits().iter().collect();
                let result = apply(&source, &edits);
                let at = format!(
                    "{}: fixing {:?} at {:?}",
                    path.display(),
                    d.code(),
                    d.span()
                );
                assert_eq!(
                    minus(syntax_codes(&result), &syntax_codes(&source)),
                    [],
                    "{at} added a syntax error:\n{result}"
                );
                let carriers: Vec<Code> = diagnostics
                    .iter()
                    .filter(|e| e.fixes().contains(fix))
                    .map(Diagnostic::code)
                    .collect();
                assert_eq!(
                    minus(before.clone(), &codes(&result)),
                    carriers,
                    "{at} removed other diagnostics:\n{result}"
                );
                applied += 1;
            }
        }
    }
    assert!(applied > 0, "no fixture has a safe fix");
}

/// A fixture whose code's diagnostics have no safe fix has nothing to apply.
#[test]
fn safe_fixes_remove_their_code_in_every_fixture() {
    let mut applied = 0;
    for (path, code) in fixtures() {
        let Some(source) = fixture_text(&path) else {
            continue;
        };
        let Some((result, count)) = fixed(&source, code) else {
            continue;
        };
        assert!(count > 0, "{}: no fix was applied", path.display());
        applied += count;
        assert!(
            !codes(&result).contains(&code),
            "{}:\n{result}",
            path.display()
        );
    }
    assert!(applied > 0, "no fixture has a fix");
}

#[test]
fn every_code_has_a_diagnostics_fixture() {
    let fixtures = fixtures();
    for &code in Code::ALL {
        assert!(
            fixtures.iter().any(|(_, c)| *c == code),
            "{code:?} has no fixture in tests/diagnostics"
        );
    }
}

#[test]
fn readme_lists_every_code() {
    let readme =
        fs::read_to_string(docs().join("README.md")).expect("the README should be readable");
    for &code in Code::ALL {
        let code_str = code.as_str();
        let severity = match code.severity() {
            super::Severity::Error => "error",
            super::Severity::Warning => "warning",
        };
        let title = explanation(code).title;
        let row = format!("| [{code_str}]({code_str}.md) | {severity} | {title} |");
        assert!(readme.contains(&row), "the README has no row `{row}`");
    }
}
