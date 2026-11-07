//! The front end: from source to the checked program.

use std::iter;

use rustc_hash::FxHashSet;

use crate::diagnostic::{Code, Diagnostic, DiagnosticKind, Edit, Fix, Severity};
use crate::source::{PRELUDE_BASE, Span};
use crate::timings::{Stage, Timings};
use crate::{ast, check, lexer, mono, parser, tir};

/// The source in `bytes`. Bytes that are not UTF-8 are an error at the first invalid one, with
/// the text to render it over: the valid text before it, then U+FFFD in its place, so the
/// position is the bad byte's even when it starts a line or ends the file.
pub(crate) fn decode(bytes: Vec<u8>) -> Result<String, Box<(String, Diagnostic)>> {
    String::from_utf8(bytes).map_err(|e| {
        let valid_up_to = e.utf8_error().valid_up_to();
        let shown = String::from_utf8_lossy(&e.as_bytes()[..valid_up_to]) + "\u{FFFD}";
        let at = Span::new(valid_up_to, shown.len());
        Box::new((
            shown.into_owned(),
            DiagnosticKind::InvalidUtf8 { at }.into(),
        ))
    })
}

/// The prelude: Sisu source that is lexed, parsed and checked after the user's, so that its
/// items come after the user's in every list.
pub(crate) const PRELUDE_SOURCE: &str = include_str!("prelude.sisu");

/// The language interfaces (`Display`, `Eq`, `Ord` and `AnyObject`): Sisu source that
/// `prelude_program` puts ahead of every prelude.
pub(crate) const LANG_SOURCE: &str = include_str!("lang.sisu");

/// `checked_timed`, with its program instantiated by `mono`. `mono` reports a choice of an
/// associated type that the checker did not follow. It also reports a choice that an instance
/// needs itself, or needs too many steps.
pub(crate) fn front_end(source: &str) -> (Option<tir::Program>, Vec<Diagnostic>) {
    front_end_with(PRELUDE_SOURCE, source)
}

/// `front_end` with `prelude` in place of `PRELUDE_SOURCE`, for the tests of the prelude's rules.
pub(crate) fn front_end_with(
    prelude: &str,
    source: &str,
) -> (Option<tir::Program>, Vec<Diagnostic>) {
    front_end_timed(prelude, source, &mut Timings::off())
}

/// `front_end_with`, with the time of each stage recorded in `timings`. A program that fails the
/// check records no `mono` stage.
pub(crate) fn front_end_timed(
    prelude: &str,
    source: &str,
    timings: &mut Timings,
) -> (Option<tir::Program>, Vec<Diagnostic>) {
    let (program, mut diagnostics) = checked_timed(prelude, source, timings);
    match program
        .map(|p| timings.time(Stage::Mono, || mono::instantiate(p)))
        .transpose()
    {
        Ok(program) => (program, diagnostics),
        Err(found) => {
            // The checker judged liveness on a program that fails, as it does not for an error.
            diagnostics.retain(|d| !check::is_judged(d));
            diagnostics.extend(found);
            diagnostics.sort_by_key(|d| d.span().start());
            (None, diagnostics)
        }
    }
}

/// Lexes, parses and checks `source`, with `prelude` in place of `PRELUDE_SOURCE`, and records
/// the time of each stage in `timings`. Returns every diagnostic, sorted by position, and the
/// checked program, generic declarations and all, unless one is an error. The checker runs
/// only when every item is named: it checks the intact items, and treats a broken one as
/// declared. `sisu check` stops here: `mono` names each instantiation, which costs as much as
/// its type is long, and check prints none. A diagnostic that points into the prelude is an
/// internal error, never one the user sees: the prelude's text is not theirs.
pub(crate) fn checked_timed(
    prelude: &str,
    source: &str,
    timings: &mut Timings,
) -> (Option<tir::Program>, Vec<Diagnostic>) {
    let lexed = match timings.time(Stage::Lex, || lexer::lex(source)) {
        Ok(lexed) => lexed,
        Err(d) => return (None, vec![d]),
    };
    timings.count(|| crate::timings::counts(source, &lexed));
    let parsed = timings.time(Stage::Parse, || parser::parse(lexed, false));
    if !parsed.checkable {
        return (None, parsed.diagnostics);
    }
    let prelude = timings.time(Stage::Prelude, || prelude_program(prelude));
    let (program, checked) = timings.time(Stage::Check, || {
        check::check(&parsed.program, &prelude, source, &parsed.diagnostics)
    });
    if let Some(d) = checked.iter().find(|d| points_into_prelude(d)) {
        internal_error(d);
    }
    let mut diagnostics = parsed.diagnostics;
    diagnostics.extend(checked);
    diagnostics.sort_by_key(|d| d.span().start());
    let failed = diagnostics.iter().any(|d| d.severity() == Severity::Error);
    (program.filter(|_| !failed), diagnostics)
}

/// The prelude as the checker reads it: its syntax tree, and the text that its spans index.
pub(crate) struct PreludeProgram {
    pub(crate) program: ast::Program,
    /// `LANG_SOURCE`, a line break and the prelude, whose first byte has offset `PRELUDE_BASE`.
    pub(crate) text: String,
}

/// The syntax tree of `LANG_SOURCE` followed by `prelude`, whose spans start at `PRELUDE_BASE`,
/// with their text. Every prelude, an empty one included, declares the language interfaces.
pub(crate) fn prelude_program(prelude: &str) -> PreludeProgram {
    let text = format!("{LANG_SOURCE}\n{prelude}");
    let lexed = lexer::lex_at(&text, PRELUDE_BASE as usize).unwrap_or_else(|d| internal_error(&d));
    let parsed = parser::parse(lexed, true);
    if let Some(d) = parsed.diagnostics.first() {
        internal_error(d);
    }
    PreludeProgram {
        program: parsed.program,
        text,
    }
}

/// Whether a span of `d` lies in the prelude: its primary span, a label, or a fix's edit.
fn points_into_prelude(d: &Diagnostic) -> bool {
    let labels = d.secondary().iter().map(|&(span, _)| span);
    let edits = d.fixes().iter().flat_map(Fix::edits).map(Edit::span);
    iter::once(d.span())
        .chain(labels)
        .chain(edits)
        .any(Span::in_prelude)
}

/// A diagnostic about the prelude is the compiler's fault, so it stops the compiler.
fn internal_error(d: &Diagnostic) -> ! {
    panic!(
        "internal compiler error: the prelude does not check clean: {}",
        d.message()
    )
}

/// `diagnostics` without each fix of a lexer or parser diagnostic that leaves `source` lexing or
/// parsing worse than it does, and with each fix unsafe that edits over a comment or another
/// lexer error: a fix that is shown never breaks the syntax, and a safe one never takes text it
/// was not about. A checker fix is not judged: the checker runs only on items that parse, and
/// the fuzz target asserts that every fix shown keeps the syntax, the checker's included. One
/// case is judged, and demoted: a fix with an edit on each side of a dropped `;`, which puts the
/// `;` in new text. It costs little, because it needs a dropped `;` between the edits of one fix.
/// Judging one lexes and parses the whole source again, so judging every checker fix made N
/// warnings cost N parses. For the same reason only the first `JUDGED` syntax fixes are judged;
/// later ones are dropped. On clean syntax a W0007 fix is kept as it is: neither judged, counted
/// nor demoted. Beside a syntax error it stays only if the token kinds and the lexer codes of the
/// fixed source stay the same. That judgement has its own count, and it also stops at `JUDGED`.
pub(crate) fn sound_fixes(source: &str, diagnostics: &[Diagnostic]) -> Vec<Diagnostic> {
    let mut sound = diagnostics.to_vec();
    // On clean syntax a W0007 fix is kept unjudged; beside a syntax error it stays only if the
    // tokens and the lexer codes stay (`keep_lexing_semicolon_fixes`).
    let syntax_fix = |d: &Diagnostic| d.code() != Code::RedundantSemicolon;
    let failed = sound.iter().any(|d| d.severity() == Severity::Error);
    // Judging a fix lexes and parses `source` again, so skip that when no fix needs it: a syntax
    // fix, or a W0007 fix of a source that failed. A source with only warnings skips too.
    if sound
        .iter()
        .any(|d| (syntax_fix(d) || failed) && !d.fixes().is_empty())
    {
        let syntax = syntax(source);
        let before = badness(&syntax);
        if before != (false, 0) {
            keep_lexing_semicolon_fixes(source, &mut sound);
        }
        // Past a bracket error, the worst there is, no fix is worse: only the parser's reports
        // are judged.
        let reported: FxHashSet<(Code, Span)> = (syntax.iter().flatten())
            .map(|d| (d.code(), d.span()))
            .collect();
        let marks = marks(source);
        let mut judged = 0;
        // A dropped `;` between the edits of one fix lands in new text, where it can be an error.
        let around = |fix: &Fix, at: usize| {
            source.as_bytes().get(at) == Some(&b';')
                && (fix.edits().iter()).any(|e| e.span().start() <= at)
                && (fix.edits().iter()).any(|e| e.span().end() > at)
        };
        for d in sound.iter_mut().filter(|d| syntax_fix(d)) {
            let own = d.span().start();
            let reported = reported.contains(&(d.code(), d.span()));
            let spans = |fix: &Fix| marks.iter().any(|&at| at != own && around(fix, at));
            if reported || d.fixes().iter().any(spans) {
                d.retain_fixes(|fix| {
                    if !reported && !spans(fix) {
                        return true;
                    }
                    judged += 1;
                    judged <= JUDGED && syntax_errors(&fix.apply(source)) <= before
                });
            }
            d.demote_fixes(|fix| {
                let taken = |at: usize| fix.edits().iter().any(|e| e.span().range().contains(&at));
                marks
                    .iter()
                    .any(|&at| at != own && (taken(at) || around(fix, at)))
            });
        }
    }
    sound
}

/// How many syntax fixes `sound_fixes` judges, and how many W0007 fixes, with a count of their own.
// shortcut: a fix past the cap is dropped, not judged; judge only the edited item if files with
// more fixable syntax errors turn up.
const JUDGED: usize = 100;

/// Drops each W0007 fix in `diagnostics` that changes how `source` lexes: it keeps a fix only if
/// the fixed source gives the same token kinds and the same lexer codes. A fixed source that does
/// not lex counts as different. At most `JUDGED` fixes are judged, with a count of their own, and
/// the later ones are dropped. It lexes and does not parse.
fn keep_lexing_semicolon_fixes(source: &str, diagnostics: &mut [Diagnostic]) {
    let same = |a: &lexer::Lexed<'_>, b: &lexer::Lexed<'_>| {
        (a.tokens.iter().map(|t| &t.kind)).eq(b.tokens.iter().map(|t| &t.kind))
            && (a.diagnostics.iter().map(Diagnostic::code))
                .eq(b.diagnostics.iter().map(Diagnostic::code))
    };
    // A source that does not lex has no W0007.
    let Ok(lexed) = lexer::lex(source) else {
        return;
    };
    let mut judged = 0;
    for d in diagnostics
        .iter_mut()
        .filter(|d| d.code() == Code::RedundantSemicolon)
    {
        d.retain_fixes(|fix| {
            judged += 1;
            judged <= JUDGED
                && lexer::lex(&fix.apply(source)).is_ok_and(|fixed| same(&lexed, &fixed))
        });
    }
}

/// Where each comment, each lexer error and each dropped `;` of `source` starts: text that the
/// parser never sees, so that no rule behind a fix knows it is there.
fn marks(source: &str) -> Vec<usize> {
    match lexer::lex(source) {
        Err(d) => vec![d.span().start()],
        Ok(lexed) => (lexed.diagnostics.iter().chain(&lexed.warnings))
            .map(|d| d.span().start())
            .chain(lexed.comments)
            .collect(),
    }
}

/// What the lexer and the parser report about `source`; `Err` holds the bracket error that
/// stops the lexer.
fn syntax(source: &str) -> Result<Vec<Diagnostic>, Diagnostic> {
    lexer::lex(source).map(|lexed| parser::parse(lexed, false).diagnostics)
}

/// How badly a source fails to lex or to parse, given its `syntax`: a lexer error, which stops
/// the parser, is worse than any number of parser errors.
fn badness(syntax: &Result<Vec<Diagnostic>, Diagnostic>) -> (bool, usize) {
    match syntax {
        Err(_) => (true, 0),
        Ok(ds) => (
            false,
            ds.iter()
                .filter(|d| d.severity() == Severity::Error)
                .count(),
        ),
    }
}

/// How badly `source` fails to lex or to parse, as `badness` ranks it.
pub(crate) fn syntax_errors(source: &str) -> (bool, usize) {
    badness(&syntax(source))
}

/// `checked_with` with `PRELUDE_SOURCE`, for the tests.
#[cfg(test)]
pub(crate) fn checked(source: &str) -> (Option<tir::Program>, Vec<Diagnostic>) {
    checked_with(PRELUDE_SOURCE, source)
}

/// `checked_timed` with no timings.
#[cfg(test)]
pub(crate) fn checked_with(prelude: &str, source: &str) -> (Option<tir::Program>, Vec<Diagnostic>) {
    checked_timed(prelude, source, &mut Timings::off())
}

/// The sources that the W0007 tests and the fuzz tests share.
#[cfg(test)]
pub(crate) mod w0007_rows {
    use crate::diagnostic::Code;

    /// A `;` first on its line, after a token that cannot end a statement. Without it, the
    /// source has more syntax errors than with it.
    pub(crate) const NO_FIX_NEEDED: [&str; 5] = [
        "fn f() ->\n; Box<i64>\n{}",
        "fn main() {}\nfn f() ->\n; Array<i64>\n{\n    return []\n}\n",
        "fn main() {}\nextend\n;Box<i64>\n{ }\n",
        "extend\n;b<c>\n{}",
        "fn f() ->\n;b<c>\n{}",
    ];

    /// The same, where the source with no `;` has no more syntax errors than with it.
    pub(crate) const NO_FIX_HARMLESS: [&str; 3] = [
        "type M =\n; Option<i64>\n\nextend Option<i64> {}\n\nextend M {}\n\nfn main() {}\n",
        "fn main() {\n    let x =\n    ; y\n}\n",
        "fn main() {\n    let a = 1 &\n    ; { }\n}\n",
    ];

    /// A syntax error, and a `;` whose fix changes how the file lexes. The flag says that the
    /// token kinds stay and only the lexer codes change.
    pub(crate) const CHANGES_THE_LEXING: [(&str, bool); 6] = [
        ("fn f;fn g<T\nU>() {}", false),
        ("fn f();fn g<T\nU>() {}", false),
        ("fn f;enum F<T\nU> {}", false),
        ("fn f ; type A: B\nC", false),
        ("fn f;type A = Box<i64>\n& C", false),
        ("fn main() {}\na.b<c>();enum B<C>\n{}", true),
    ];

    /// A fix of another diagnostic that takes a `;` which the parser never sees.
    pub(crate) const TAKES_A_DROPPED_SEMICOLON: [&str; 3] = [
        "fn main() {\n    let n = 0.5\n    while n !=\n; f64.nan {\n    }\n}\n",
        "fn main() {\n    let n = 0.5\n    while n !=\n;; f64.nan {\n    }\n}\n",
        "fn main() {\n    let n = 0.5\n    while n ==\n; f64.nan {\n    }\n}\n",
    ];

    /// A fix with one edit on each side of a dropped `;`, and the code of its diagnostic. The flag
    /// says that the fixed source keeps the syntax, so the fix stays and is unsafe.
    pub(crate) const AROUND_A_DROPPED_SEMICOLON: [(&str, Code, bool); 6] = [
        (
            "fn main() {\n    let n = 0.5\n    let v = -\n    ; n != f64.nan\n}\n",
            Code::NanComparison,
            false,
        ),
        (
            "fn main() {\n    let n = 0.5\n    while -\n; n != f64.nan {\n    }\n}\n",
            Code::NanComparison,
            false,
        ),
        (
            "fn main() {\n    let m = 0.5\n    let n = 0.5\n    let v = m +\n; n != f64.nan\n}\n",
            Code::NanComparison,
            false,
        ),
        (
            "fn main() {\n    let n: i64 = 1\n    let x: f64 = -\n    ; n\n}\n",
            Code::Mismatch,
            false,
        ),
        (
            "fn main() {\n    let k = 1\n    let o: i64? = Some(2)\n    let Some(v) = match k\n;  { _ => o } else { return }\n    print(v)\n}\n",
            Code::LetElseValueEndsInBrace,
            true,
        ),
        (
            "fn main() {\n    let n = 0.5\n    let v = { print(1);\n    n } != f64.nan\n}\n",
            Code::NanComparison,
            true,
        ),
    ];
}

#[cfg(test)]
mod tests {
    use super::w0007_rows::{
        AROUND_A_DROPPED_SEMICOLON, CHANGES_THE_LEXING, NO_FIX_HARMLESS, NO_FIX_NEEDED,
        TAKES_A_DROPPED_SEMICOLON,
    };
    use super::*;
    use crate::diagnostic::Code;
    use crate::diagnostic::explanations::{apply_fix, fix_edits};
    use crate::source::line_col;
    use std::fs;
    use std::path::Path;

    /// `checked` stops before `mono`, which `front_end` runs: `sisu check` names no
    /// instantiation.
    #[test]
    fn only_the_front_end_should_instantiate() {
        let src = "fn id<T>(x: T) -> T { x }\nfn main() {\n    print(id(1))\n}";
        let names = |program: Option<tir::Program>| -> Vec<String> {
            let program = program.expect("the program should check");
            program.functions.into_iter().map(|f| f.name).collect()
        };
        // The prelude's functions follow the user's in the checked program, and `mono` drops the
        // ones that nothing calls.
        assert_eq!(
            names(checked(src).0),
            [
                "id",
                "main",
                "i64.min",
                "i64.max",
                "i64.clamp",
                "[i64].sum",
                "[f64].sum",
                "[T].contains",
                "[T].first",
                "[T].last",
                "[T].get",
                "exchange"
            ]
        );
        assert_eq!(names(front_end(src).0), ["main", "id<i64>"]);
    }

    /// An argument fixes `T` before an `any` context does, and a generic function over an `any`
    /// type is one instance: `id(5)` against `any Display` is `id<i64>`, and `total` over an
    /// `[any Shape]` is `total<any Shape>`, with no `id<any Display>` and no `total<Circle>`.
    #[test]
    fn an_any_context_should_not_make_an_instance_of_its_own() {
        let src = "interface Shape {\n    fn area(self) -> f64\n}\nclass Circle: Shape {\n    r: f64\n    fn area(self) -> f64 { self.r }\n}\nfn id<T>(x: T) -> T { x }\nfn total<T: Shape>(xs: [T]) -> f64 {\n    let mut sum = 0.0\n    for x in xs { sum += x.area() }\n    sum\n}\nfn main() {\n    let d: any Display = id(5)\n    let xs: [any Shape] = [Circle(r: 1.0)]\n    print(d)\n    print(total(xs))\n}";
        let names: Vec<String> = front_end(src)
            .0
            .expect("the program should check")
            .functions
            .into_iter()
            .map(|f| f.name)
            .collect();
        for name in ["id<i64>", "total<any Shape>"] {
            assert!(names.iter().any(|n| n == name), "{name} in {names:?}");
        }
        for name in ["id<any Display>", "total<Circle>"] {
            assert!(!names.iter().any(|n| n == name), "{name} in {names:?}");
        }
    }

    /// Only an unusual prelude reaches some faults in the checker. One such fault, a span read
    /// from the wrong text, once crashed the compiler with a plain panic. The diagnostics cases
    /// are a corpus of unusual programs, so each one, as the prelude, must check or give the
    /// internal error.
    #[test]
    fn every_diagnostics_case_as_the_prelude_should_check_or_give_the_prelude_internal_error() {
        let cases = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/diagnostics");
        let mut paths: Vec<_> = fs::read_dir(cases)
            .expect("tests/diagnostics should be readable")
            .map(|entry| entry.expect("an entry should be readable").path())
            .filter(|path| path.extension().is_some_and(|e| e == "sisu"))
            .collect();
        paths.sort();
        assert!(!paths.is_empty(), "no diagnostics case");
        let hook = std::panic::take_hook();
        // The expected internal errors would fill the output.
        std::panic::set_hook(Box::new(|_| {}));
        // Deep cases overflow the default test stack, as `main` runs `cli` on a larger one.
        let joined = std::panic::catch_unwind(|| {
            crate::on_compiler_stack(|| {
                let mut failures = Vec::new();
                for path in &paths {
                    // A prelude is text, so a case that is not UTF-8 (E0072) is no prelude.
                    let Ok(case) = fs::read_to_string(path) else {
                        continue;
                    };
                    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        checked_with(&case, "fn main() {}")
                    }));
                    let Err(payload) = caught else { continue };
                    let message = payload
                        .downcast_ref::<String>()
                        .map(String::as_str)
                        .or_else(|| payload.downcast_ref::<&str>().copied())
                        .unwrap_or_default();
                    if !message
                        .starts_with("internal compiler error: the prelude does not check clean")
                    {
                        let name = path.file_name().unwrap_or_default().to_string_lossy();
                        failures.push(format!("{name}: {message}"));
                    }
                }
                failures
            })
        });
        std::panic::set_hook(hook);
        let failures = joined.expect("the loop should not panic");
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    /// The code and `(line, col)` of every diagnostic `src` reports, in order.
    fn codes(src: &str) -> Vec<(Code, (usize, usize))> {
        front_end(src)
            .1
            .iter()
            .map(|d| (d.code(), line_col(src, d.span().start())))
            .collect()
    }

    /// Whether each fix of `d` is kept by `sound_fixes` over `src`.
    fn kept(src: &str, d: &Diagnostic) -> Vec<bool> {
        let sound = sound_fixes(src, std::slice::from_ref(d));
        let kept = sound[0].fixes();
        d.fixes().iter().map(|f| kept.contains(f)).collect()
    }

    /// A lexer or parser diagnostic's fix that leaves the source lexing and parsing is shown; one
    /// that breaks either is not. A fix of a syntax error is held only to not making things
    /// worse. A checker fix is shown unjudged: the fuzz target holds the checker to its rules.
    #[test]
    fn a_fix_that_breaks_the_syntax_is_dropped() {
        // An unclosed `(` is a bracket error, which stops the lexer.
        let unclosed = [(Span::new(0, 0), "(")];
        // Two lexer errors: the fix of the first leaves the second, which is no worse.
        let src = "fn main() {\n    print(\"}\")\n    print(\"}\")\n}\n";
        let [d] = <[Diagnostic; 1]>::try_from(front_end(src).1).expect("one diagnostic");
        assert_eq!(kept(src, &d), [true]);
        assert_eq!(
            kept(src, &d.clone().with_safe_fix_for_test(&unclosed)),
            [false]
        );
        // A broken function after the others lexes, but is one more parser error.
        let item = [(
            Span::new(src.len(), src.len()),
            "fn g() {\n    let = 1\n}\n",
        )];
        assert_eq!(kept(src, &d.with_safe_fix_for_test(&item)), [false]);
        let src = "fn f(s: string, n: i64) {\n    print(s + n)\n}\nfn main() {}\n";
        let [d] = <[Diagnostic; 1]>::try_from(front_end(src).1).expect("one diagnostic");
        assert_eq!(kept(src, &d.with_safe_fix_for_test(&unclosed)), [true]);
        // Two parser errors: likewise.
        let src = "fn main() {\n    let mut _ = 1\n}\nfn g() {\n    let mut _ = 2\n}\n";
        let ds = front_end(src).1;
        assert_eq!(ds.len(), 2, "{ds:?}");
        for d in &ds {
            assert_eq!(kept(src, d), [true]);
        }
    }

    /// Past `JUDGED` syntax fixes, a fix is dropped unjudged: each judgement parses the whole
    /// source again.
    #[test]
    fn fixes_past_the_judged_cap_should_be_dropped() {
        // Each function has its own name: a broken item is a duplicate of one with its name.
        let src: String = (0..=JUDGED)
            .map(|n| format!("fn g{n}() {{\n    print(0X1)\n}}\n"))
            .chain(["fn main() {}\n".to_string()])
            .collect();
        let ds = sound_fixes(&src, &front_end(&src).1);
        assert_eq!(ds.len(), JUDGED + 1);
        let mut fixed = ds.iter().map(|d| !d.fixes().is_empty());
        assert!(fixed.clone().take(JUDGED).all(|f| f));
        assert_eq!(fixed.next_back(), Some(false));
    }

    /// A safe fix that deletes up to a token is unsafe when a comment or a stray character lies in
    /// what it deletes: the rule behind it sees only tokens. The fixes of the parser and of the
    /// checker stop at a comment, so a comment does not make them unsafe; a stray character does.
    #[test]
    fn a_safe_fix_should_be_unsafe_when_it_deletes_a_comment_or_a_lexer_error() {
        for (src, code, safe) in [
            ("class C {\n    fn f(mut self) {}\n}\n", Code::MutSelf, true),
            // The parser's fix stops at the comment, so it deletes no comment and stays safe.
            (
                "class C {\n    fn f(mut // c\n        self) {}\n}\n",
                Code::MutSelf,
                true,
            ),
            // The checker's fix of an extension method stops at the comment too.
            (
                "class C {}\n\nextend C {\n    fn f(mut // c\n        self) {}\n}\n",
                Code::MutSelf,
                true,
            ),
            (
                "class C {\n    fn f(mut $ self) {}\n}\n",
                Code::MutSelf,
                false,
            ),
            (
                "fn main() {\n    let mut // c\n        n = 0\n    print(n)\n}\n",
                Code::NeverReassigned,
                true,
            ),
            (
                "fn main() {\n    let mut // c\n    _ = 1\n}\n",
                Code::MutOnUnderscore,
                true,
            ),
            ("fn main() {\n    todo!$()\n}\n", Code::TodoForm, false),
        ] {
            let ds = sound_fixes(src, &front_end(src).1);
            let d = ds
                .iter()
                .find(|d| d.code() == code)
                .expect("the diagnostic should be reported");
            assert_eq!(fix_edits(d).map(|(s, _)| s), Some(safe), "{src}");
        }
    }

    /// The W0007 diagnostics among `ds`.
    fn semicolons(ds: &[Diagnostic]) -> Vec<&Diagnostic> {
        (ds.iter())
            .filter(|d| d.code() == Code::RedundantSemicolon)
            .collect()
    }

    /// The token kinds and the lexer codes of `src`; `None` when the lexer stops at a bracket.
    fn lexing(src: &str) -> Option<(Vec<lexer::TokenKind>, Vec<Code>)> {
        lexer::lex(src).ok().map(|l| {
            (
                l.tokens.iter().map(|t| t.kind.clone()).collect(),
                l.diagnostics.iter().map(Diagnostic::code).collect(),
            )
        })
    }

    /// A `;` first on its line after an unfinished statement ends it. Removing the `;` would
    /// keep it open, so the warning has no fix.
    #[test]
    fn a_w0007_after_a_token_that_cannot_end_a_statement_should_have_no_fix() {
        for src in NO_FIX_NEEDED.into_iter().chain(NO_FIX_HARMLESS) {
            let all = front_end(src).1;
            let found = semicolons(&all);
            assert!(!found.is_empty(), "{src:?}: no W0007");
            for d in found {
                assert!(d.fixes().is_empty(), "{src:?}: the W0007 has a fix");
            }
        }
        for src in NO_FIX_NEEDED {
            let without = src.replacen(';', "", 1);
            assert!(
                syntax_errors(&without) > syntax_errors(src),
                "{src:?}: removing the `;` should add a syntax error"
            );
        }
    }

    /// Only the first `;` of a run holds the statement open. The others are redundant with it.
    #[test]
    fn a_second_semicolon_after_such_a_token_should_keep_its_fix() {
        let src = "fn main() {\n    let x =\n    ;; y\n}\n";
        let all = front_end(src).1;
        let fixes: Vec<bool> = (semicolons(&all).iter())
            .map(|d| !d.fixes().is_empty())
            .collect();
        assert_eq!(fixes, [false, true]);
    }

    /// Beside a syntax error, a W0007 fix stays only if the file lexes as before.
    #[test]
    fn a_w0007_fix_that_changes_the_tokens_should_go_beside_a_syntax_error() {
        for (src, only_codes) in CHANGES_THE_LEXING {
            let all = front_end(src).1;
            let [w] = semicolons(&all)[..] else {
                panic!("{src:?}: one W0007 wanted")
            };
            let [fix] = w.fixes() else {
                panic!("{src:?}: the W0007 should have a fix")
            };
            let (before, after) = (lexing(src), lexing(&fix.apply(src)));
            let kinds =
                |l: &Option<(Vec<lexer::TokenKind>, Vec<Code>)>| l.as_ref().map(|l| l.0.clone());
            let codes =
                |l: &Option<(Vec<lexer::TokenKind>, Vec<Code>)>| l.as_ref().map(|l| l.1.clone());
            if only_codes {
                assert_eq!(kinds(&before), kinds(&after), "{src:?}");
                assert_ne!(codes(&before), codes(&after), "{src:?}");
            } else {
                assert_ne!(kinds(&before), kinds(&after), "{src:?}");
            }
            let sound = sound_fixes(src, &all);
            let spans =
                |ds: &[Diagnostic]| -> Vec<_> { ds.iter().map(|d| (d.code(), d.span())).collect() };
            assert_eq!(spans(&sound), spans(&all), "{src:?}");
            let [kept] = semicolons(&sound)[..] else {
                panic!("{src:?}: one W0007 wanted")
            };
            assert!(kept.fixes().is_empty(), "{src:?}: the W0007 keeps its fix");
        }
    }

    /// A fix that leaves the tokens and the lexer codes as they were stays, as it does on clean
    /// syntax.
    #[test]
    fn a_w0007_fix_that_keeps_the_tokens_should_stay_beside_a_syntax_error() {
        for src in [
            "fn main() {\n    let a = 1;\n    let = 2\n}\n",
            "fn f() {}; fn h() {}\nfn g {}\n",
        ] {
            assert_ne!(syntax_errors(src), (false, 0), "{src:?}: no syntax error");
            let sound = sound_fixes(src, &front_end(src).1);
            let found = semicolons(&sound);
            assert!(!found.is_empty(), "{src:?}: no W0007");
            for d in found {
                assert!(!d.fixes().is_empty(), "{src:?}: the W0007 lost its fix");
            }
        }
    }

    /// Beside a syntax error, the first `JUDGED` W0007 fixes are judged and the next is dropped.
    #[test]
    fn a_w0007_fix_should_be_judged_up_to_the_cap_beside_a_syntax_error() {
        let lines = "    print(1);\n".repeat(JUDGED + 1);
        let src = format!("fn main() {{\n{lines}    let = 2\n}}\n");
        let sound = sound_fixes(&src, &front_end(&src).1);
        let fixed: Vec<bool> = (semicolons(&sound).iter())
            .map(|d| !d.fixes().is_empty())
            .collect();
        assert_eq!(fixed.len(), JUDGED + 1);
        assert!(fixed[..JUDGED].iter().all(|&f| f));
        assert!(!fixed[JUDGED]);
    }

    /// A W0007 fix does not take the cap of another syntax fix.
    #[test]
    fn a_w0007_fix_should_not_take_the_cap_of_another_syntax_fix() {
        let lines = "    print(1);\n".repeat(150);
        let src = format!("fn main() {{\n{lines}    print(0X1)\n}}\n");
        let sound = sound_fixes(&src, &front_end(&src).1);
        let d = (sound.iter())
            .find(|d| d.code() == Code::BasePrefixUpperCase)
            .expect("the E0075 should be reported");
        assert!(!d.fixes().is_empty(), "the E0075 lost its fix");
    }

    /// The fix of the W0005 replaces text up to the `nan`, so it takes a `;` that the parser
    /// never sees. It stays shown, and it is not safe.
    #[test]
    fn a_fix_that_takes_a_dropped_semicolon_should_be_unsafe() {
        for src in TAKES_A_DROPPED_SEMICOLON {
            let sound = sound_fixes(src, &front_end(src).1);
            let d = (sound.iter())
                .find(|d| d.code() == Code::NanComparison)
                .unwrap_or_else(|| panic!("{src:?}: no W0005"));
            assert_eq!(fix_edits(d).map(|(safe, _)| safe), Some(false), "{src:?}");
        }
    }

    /// The diagnostic of `code` in `src` after `sound_fixes`, and whether the W0007 is still there.
    fn judged_with_semicolon(src: &str, code: Code) -> (Diagnostic, bool) {
        let sound = sound_fixes(src, &front_end(src).1);
        let kept = !semicolons(&sound).is_empty();
        let d = (sound.into_iter())
            .find(|d| d.code() == code)
            .unwrap_or_else(|| panic!("{src:?}: no {code:?}"));
        (d, kept)
    }

    /// A fix with an edit on each side of a dropped `;` puts the `;` in new text, where the
    /// parser reads it. The fix goes when that breaks the syntax.
    #[test]
    fn a_fix_around_a_dropped_semicolon_should_go_when_it_breaks_the_syntax() {
        for (src, code, keeps) in AROUND_A_DROPPED_SEMICOLON {
            if keeps {
                continue;
            }
            let (d, semicolon) = judged_with_semicolon(src, code);
            assert!(semicolon, "{src:?}: the W0007 went");
            assert!(d.fixes().is_empty(), "{src:?}: the {code:?} keeps its fix");
        }
    }

    /// A fix around a dropped `;` that keeps the syntax can still change another report, so it
    /// stays shown and is not safe.
    #[test]
    fn a_fix_around_a_dropped_semicolon_should_be_unsafe_when_it_keeps_the_syntax() {
        for (src, code, keeps) in AROUND_A_DROPPED_SEMICOLON {
            if !keeps {
                continue;
            }
            let (d, semicolon) = judged_with_semicolon(src, code);
            assert!(semicolon, "{src:?}: the W0007 went");
            assert_eq!(fix_edits(&d).map(|(safe, _)| safe), Some(false), "{src:?}");
        }
    }

    /// A comment between the edits is not a dropped `;`, so the fix stays safe.
    #[test]
    fn a_fix_with_only_a_comment_between_its_edits_should_stay_safe() {
        let src = "fn main() {\n    let n = 0.5\n    let v = - // c\n    n != f64.nan\n}\n";
        let (d, _) = judged_with_semicolon(src, Code::NanComparison);
        assert_eq!(fix_edits(&d).map(|(safe, _)| safe), Some(true), "{src:?}");
    }

    /// A dropped `;` before the first edit or after the last is not between the edits, so the fix
    /// stays safe.
    #[test]
    fn a_fix_with_a_dropped_semicolon_outside_its_edits_should_stay_safe() {
        for src in [
            "fn main() {\n    let n = 0.5\n    print(1);\n    let v = n != f64.nan\n}\n",
            "fn main() {\n    let n = 0.5\n    let v = n != f64.nan\n    print(1);\n}\n",
        ] {
            let (d, semicolon) = judged_with_semicolon(src, Code::NanComparison);
            assert!(semicolon, "{src:?}: no W0007");
            assert_eq!(fix_edits(&d).map(|(safe, _)| safe), Some(true), "{src:?}");
        }
    }

    /// The text of `src` after the one fix of the first diagnostic of `code`, and whether the fix
    /// is safe once `sound_fixes` judged it.
    fn fixed(src: &str, code: Code) -> (String, bool) {
        fixed_nth(src, code, 0)
    }

    /// Like `fixed`, for the diagnostic of `code` that is `n`th in the order of the report.
    fn fixed_nth(src: &str, code: Code, n: usize) -> (String, bool) {
        let ds = sound_fixes(src, &front_end(src).1);
        let d = ds
            .iter()
            .filter(|d| d.code() == code)
            .nth(n)
            .unwrap_or_else(|| panic!("{code:?} should be reported for:\n{src}"));
        let (safe, _) = fix_edits(d).expect("the diagnostic should have a fix");
        (apply_fix(src, d), safe)
    }

    /// `body` as the body of `main`.
    fn in_main(body: &str) -> String {
        format!("fn main() {{\n{body}\n}}\n")
    }

    /// `arms` after the arm `1 => print(1)` of a `match` on an `i64`, and before `_ => print(3)`.
    fn in_match(arms: &str) -> String {
        in_main(&format!(
            "    let x = 1\n    match x {{\n        1 => print(1)\n{arms}\n        _ => print(3)\n    }}"
        ))
    }

    /// `arms` as the arms of a `match` on an `i64?` in `f`, which `main` calls.
    fn in_optional_match(arms: &str) -> String {
        format!(
            "fn f(o: i64?) {{\n    match o {{\n{arms}\n        _ => print(3)\n    }}\n}}\n\nfn main() {{\n    f(None)\n}}\n"
        )
    }

    /// The program with an `extend` method `fn bump({params}) {{ self.n = 1 }}`.
    fn in_extend(params: &str) -> String {
        format!(
            "class C {{\n    mut n: i64\n}}\n\nextend C {{\n    fn bump({params}) {{\n        self.n = 1\n    }}\n}}\n\nfn main() {{\n    let c = C(n: 0)\n    c.bump()\n    print(c.n)\n}}\n"
        )
    }

    /// `body` as the body of `main`, after the enum `E` with the variants `P(i64)` and
    /// `Q(i64, i64)` and the binding `let mut y = 1`.
    fn in_variants(body: &str) -> String {
        format!(
            "enum E {{\n    P(i64)\n    Q(i64, i64)\n}}\n\nfn main() {{\n    let mut y = 1\n{body}\n}}\n"
        )
    }

    /// GIVEN each fix of the checker that removes a keyword, a label or an alternative, with a
    /// comment in the gap before the next token
    /// WHEN the fix is applied after `sound_fixes`
    /// THEN the removal ends where the comment starts, so the fix keeps the comment and, but for
    /// E0055 and W0006, stays safe
    #[test]
    #[expect(clippy::too_many_lines, reason = "one row of the table for each case")]
    fn a_checker_removal_should_keep_a_comment_in_its_gap() {
        let rows = [
            // W0002 of a `let mut`: the `Binding` path, then the `Pending` path of a body with an
            // error, then a nested block, two comments, a touching `mut` and a `let ... else`.
            (
                in_main("    let mut // c\n    x = 1\n    print(x)"),
                Code::NeverReassigned,
                in_main("    let // c\n    x = 1\n    print(x)"),
                true,
            ),
            (
                in_main("    let mut // c\n    x = 1\n    let y: i64 = \"a\""),
                Code::NeverReassigned,
                in_main("    let // c\n    x = 1\n    let y: i64 = \"a\""),
                true,
            ),
            (
                in_main("    if true {\n        let mut // c\n        x = 1\n        print(x)\n    }"),
                Code::NeverReassigned,
                in_main("    if true {\n        let // c\n        x = 1\n        print(x)\n    }"),
                true,
            ),
            (
                in_main("    let mut // a\n    // b\n    x = 1\n    print(x)"),
                Code::NeverReassigned,
                in_main("    let // a\n    // b\n    x = 1\n    print(x)"),
                true,
            ),
            (
                in_main("    let mut// c\n    x = 1\n    print(x)"),
                Code::NeverReassigned,
                in_main("    let // c\n    x = 1\n    print(x)"),
                true,
            ),
            (
                "fn f(o: i64?) {\n    let mut // c\n    x = o else { return }\n    print(x)\n}\n\nfn main() {\n    f(1)\n}\n".to_string(),
                Code::NeverReassigned,
                "fn f(o: i64?) {\n    let // c\n    x = o else { return }\n    print(x)\n}\n\nfn main() {\n    f(1)\n}\n".to_string(),
                true,
            ),
            // W0008 of a label, before each kind of loop, nested, with two comments and a
            // touching `:`.
            (
                in_main("    outer: // c\n    loop {\n        break\n    }"),
                Code::UnusedLabel,
                in_main("    // c\n    loop {\n        break\n    }"),
                true,
            ),
            (
                in_main("    outer: // c\n    while true {\n        break\n    }"),
                Code::UnusedLabel,
                in_main("    // c\n    while true {\n        break\n    }"),
                true,
            ),
            (
                in_main("    let o: i64? = None\n    outer: // c\n    while let Some(x) = o {\n        print(x)\n    }"),
                Code::UnusedLabel,
                in_main("    let o: i64? = None\n    // c\n    while let Some(x) = o {\n        print(x)\n    }"),
                true,
            ),
            (
                in_main("    outer: // c\n    for i in 0..3 {\n        print(i)\n    }"),
                Code::UnusedLabel,
                in_main("    // c\n    for i in 0..3 {\n        print(i)\n    }"),
                true,
            ),
            (
                in_main("    loop {\n        inner: // c\n        loop {\n            break\n        }\n        break\n    }"),
                Code::UnusedLabel,
                in_main("    loop {\n        // c\n        loop {\n            break\n        }\n        break\n    }"),
                true,
            ),
            (
                in_main("    outer: // a\n    // b\n    loop {\n        break\n    }"),
                Code::UnusedLabel,
                in_main("    // a\n    // b\n    loop {\n        break\n    }"),
                true,
            ),
            (
                in_main("    outer:// c\n    loop {\n        break\n    }"),
                Code::UnusedLabel,
                in_main("    // c\n    loop {\n        break\n    }"),
                true,
            ),
            // E0018 of an extension method: the receiver on the next line, touching, and with a
            // second parameter.
            (
                in_extend("mut // c\n        self"),
                Code::MutSelf,
                in_extend("// c\n        self"),
                true,
            ),
            (
                in_extend("mut// c\n        self"),
                Code::MutSelf,
                in_extend("// c\n        self"),
                true,
            ),
            (
                in_extend("mut // c\n        self, k: i64"),
                Code::MutSelf,
                in_extend("// c\n        self, k: i64"),
                true,
            ),
            // E0055 of a label stays unsafe. The removal stops at a comment after the label, after
            // the `:` or after a `mut`. A comment before the `:` splits it in two edits.
            (
                in_variants("    let e = E.P(x: // c\n        1)\n    print(e)"),
                Code::LabeledArgument,
                in_variants("    let e = E.P(// c\n        1)\n    print(e)"),
                false,
            ),
            (
                in_main("    let o = Some(x: // c\n        1)\n    print(o)"),
                Code::LabeledArgument,
                in_main("    let o = Some(// c\n        1)\n    print(o)"),
                false,
            ),
            (
                in_main("    let o = Some(x // c\n        : 1)\n    print(o)"),
                Code::LabeledArgument,
                in_main("    let o = Some(// c\n        1)\n    print(o)"),
                false,
            ),
            (
                in_main("    let o = Some(x // c\n        : // d\n        1)\n    print(o)"),
                Code::LabeledArgument,
                in_main("    let o = Some(// c\n        // d\n        1)\n    print(o)"),
                false,
            ),
            (
                in_variants("    let e = E.Q(a: // c\n        1, b: 2)\n    print(e)"),
                Code::LabeledArgument,
                in_variants("    let e = E.Q(// c\n        1, 2)\n    print(e)"),
                false,
            ),
            (
                in_variants("    let e = E.P(x: mut // c\n        y)\n    print(e)"),
                Code::LabeledArgument,
                in_variants("    let e = E.P(// c\n        y)\n    print(e)"),
                false,
            ),
            (
                in_variants("    let e = E.P(x: // c\n        mut y)\n    print(e)"),
                Code::LabeledArgument,
                in_variants("    let e = E.P(// c\n        mut y)\n    print(e)"),
                false,
            ),
            // W0006 of an alternative stays unsafe. The first one removes forward and stops at the
            // comment. A later one removes forward, when a comment lies before it and another
            // alternative follows.
            (
                in_match("        1 | // c\n        2 => print(2)"),
                Code::UnreachableArm,
                in_match("        // c\n        2 => print(2)"),
                false,
            ),
            (
                in_match("        1 |// c\n        2 => print(2)"),
                Code::UnreachableArm,
                in_match("        // c\n        2 => print(2)"),
                false,
            ),
            (
                in_match("        1 | // a\n        // b\n        2 => print(2)"),
                Code::UnreachableArm,
                in_match("        // a\n        // b\n        2 => print(2)"),
                false,
            ),
            (
                in_match("        2 | // c\n        1 | 3 => print(2)"),
                Code::UnreachableArm,
                in_match("        2 | // c\n        3 => print(2)"),
                false,
            ),
            (
                in_optional_match("        Some(1) => print(1)\n        Some(1 | // c\n            2) => print(2)"),
                Code::UnreachableArm,
                in_optional_match("        Some(1) => print(1)\n        Some(// c\n            2) => print(2)"),
                false,
            ),
            // A later alternative with a comment in its backward gap and one after its own `|`:
            // the removal keeps the second comment.
            (
                in_optional_match("        Some(1) => print(1)\n        Some(2 | // a\n            1 | // b\n            3) => print(2)"),
                Code::UnreachableArm,
                in_optional_match("        Some(1) => print(1)\n        Some(2 | // a\n            // b\n            3) => print(2)"),
                false,
            ),
        ];
        for (src, code, want, safe) in rows {
            assert_eq!(fixed(&src, code), (want, safe), "{src}");
        }
    }

    /// `arms` as the arms of a `match` on an enum `E` with the variant `A(i64)`, after the arm
    /// `A(1) => print(1)`.
    fn in_variant_match(arms: &str) -> String {
        format!(
            "enum E {{\n    A(i64)\n    B\n}}\n\nfn f(e: E) {{\n    match e {{\n        A(1) => print(1)\n{arms}\n        _ => print(3)\n    }}\n}}\n\nfn main() {{\n    f(E.B)\n}}\n"
        )
    }

    /// `arms` as the arms of a `match` on an enum `E` with the variant `A(i64?)`, after the arm
    /// `A(Some(1)) => print(1)`.
    fn in_variant_optional_match(arms: &str) -> String {
        format!(
            "enum E {{\n    A(i64?)\n    B\n}}\n\nfn f(e: E) {{\n    match e {{\n        A(Some(1)) => print(1)\n{arms}\n        _ => print(3)\n    }}\n}}\n\nfn main() {{\n    f(E.B)\n}}\n"
        )
    }

    /// GIVEN a fix of W0006 for an alternative in a nested pattern, with a comment in the gap
    /// before the `|` that the removal takes
    /// WHEN the fix is applied after `sound_fixes`
    /// THEN the removal takes the `|` and the comment before it, and the fixed text parses
    #[test]
    fn a_forward_alternative_removal_should_take_the_bar_when_a_comment_lies_before_it() {
        let rows = [
            // A comment lies before the `|` that the first alternative removes with itself: the
            // removal does not stop at it, or it would keep the `|`. The comment goes with the
            // alternative.
            (
                in_optional_match(
                    "        Some(1) => print(1)\n        Some(1 // c\n            | 2) => print(2)",
                ),
                Code::UnreachableArm,
                in_optional_match("        Some(1) => print(1)\n        Some(2) => print(2)"),
                false,
            ),
            // The same for a later alternative whose backward gap holds a comment, and another
            // follows: the comment after the alternative lies before the `|`.
            (
                in_optional_match(
                    "        Some(1) => print(1)\n        Some(2 | // a\n            1 // b\n            | 3) => print(2)",
                ),
                Code::UnreachableArm,
                in_optional_match(
                    "        Some(1) => print(1)\n        Some(2 // b\n            | 3) => print(2)",
                ),
                false,
            ),
            (
                in_variant_match("        A(1 // c\n            | 2) => print(2)"),
                Code::UnreachableArm,
                in_variant_match("        A(2) => print(2)"),
                false,
            ),
            (
                in_variant_match(
                    "        A(2 | // a\n            1 // b\n            | 3) => print(2)",
                ),
                Code::UnreachableArm,
                in_variant_match("        A(2 // b\n            | 3) => print(2)"),
                false,
            ),
            (
                in_optional_match(
                    "        Some(1) => print(1)\n        Some(5) | Some(1 // c\n            | 2) => print(2)",
                ),
                Code::UnreachableArm,
                in_optional_match(
                    "        Some(1) => print(1)\n        Some(5) | Some(2) => print(2)",
                ),
                false,
            ),
            (
                in_variant_optional_match(
                    "        A(None | // a\n            Some(1) // b\n            | Some(2)) => print(2)",
                ),
                Code::UnreachableArm,
                in_variant_optional_match(
                    "        A(None // b\n            | Some(2)) => print(2)",
                ),
                false,
            ),
            (
                in_optional_match(
                    "        Some(1) => print(1)\n        Some(1 // a\n            | // b\n            2) => print(2)",
                ),
                Code::UnreachableArm,
                in_optional_match("        Some(1) => print(1)\n        Some(2) => print(2)"),
                false,
            ),
        ];
        for (src, code, want, safe) in rows {
            assert_eq!(fixed(&src, code), (want, safe), "{src}");
        }
        // Two unreachable alternatives next to each other, with a comment after each: each fix
        // alone leaves the second comment and one `|`.
        let src = in_optional_match(
            "        Some(1) => print(1)\n        Some(1 // a\n            | 1 // b\n            | 2) => print(2)",
        );
        let want = in_optional_match(
            "        Some(1) => print(1)\n        Some(1 // b\n            | 2) => print(2)",
        );
        for n in 0..2 {
            assert_eq!(
                fixed_nth(&src, Code::UnreachableArm, n),
                (want.clone(), false)
            );
        }
    }

    /// GIVEN each site of the two tests above with no comment in the gap, a comment outside the
    /// gap, and the alternatives and arms that the cut leaves alone
    /// WHEN the fix is applied after `sound_fixes`
    /// THEN the text and the safety are as before: the removal reaches the next token, and the
    /// side of an alternative's removal does not change
    #[test]
    #[expect(clippy::too_many_lines, reason = "one row of the table for each case")]
    fn a_checker_removal_with_no_comment_in_its_gap_should_reach_the_next_token() {
        let rows = [
            (
                in_main("    let mut x = 1\n    print(x)"),
                Code::NeverReassigned,
                in_main("    let x = 1\n    print(x)"),
                true,
            ),
            (
                in_main("    let mut\n    x = 1\n    print(x)"),
                Code::NeverReassigned,
                in_main("    let x = 1\n    print(x)"),
                true,
            ),
            (
                in_main("    // before\n    let mut x = 1 // after\n    print(x)"),
                Code::NeverReassigned,
                in_main("    // before\n    let x = 1 // after\n    print(x)"),
                true,
            ),
            (
                in_main("    outer: loop {\n        break\n    }"),
                Code::UnusedLabel,
                in_main("    loop {\n        break\n    }"),
                true,
            ),
            (
                in_main("    // before\n    outer:\n    loop { // after\n        break\n    }"),
                Code::UnusedLabel,
                in_main("    // before\n    loop { // after\n        break\n    }"),
                true,
            ),
            (
                in_extend("mut\n        self"),
                Code::MutSelf,
                in_extend("self"),
                true,
            ),
            (
                in_extend("mut self"),
                Code::MutSelf,
                in_extend("self"),
                true,
            ),
            // E0055: the removal of a label reaches the value, whatever the white space is between.
            (
                in_main("    let o = Some(x: 1)\n    print(o)"),
                Code::LabeledArgument,
                in_main("    let o = Some(1)\n    print(o)"),
                false,
            ),
            (
                in_main("    let o = Some(x:\n        1)\n    print(o)"),
                Code::LabeledArgument,
                in_main("    let o = Some(1)\n    print(o)"),
                false,
            ),
            (
                in_main("    let o = Some(x : 1)\n    print(o)"),
                Code::LabeledArgument,
                in_main("    let o = Some(1)\n    print(o)"),
                false,
            ),
            (
                in_variants("    let e = E.P(x: mut y)\n    print(e)"),
                Code::LabeledArgument,
                in_variants("    let e = E.P(y)\n    print(e)"),
                false,
            ),
            (
                in_variants("    let e = E.P(mut x: y)\n    print(e)"),
                Code::LabeledArgument,
                in_variants("    let e = E.P(mut y)\n    print(e)"),
                false,
            ),
            // W0006: a later alternative removes backward, so `|` goes with it, whatever the white
            // space is on its two sides.
            (
                in_match("        2 | 1 | 3 => print(2)"),
                Code::UnreachableArm,
                in_match("        2 | 3 => print(2)"),
                false,
            ),
            (
                in_match("        2 |1 | 3 => print(2)"),
                Code::UnreachableArm,
                in_match("        2 | 3 => print(2)"),
                false,
            ),
            (
                in_match("        2 | 1 |\n            3 => print(2)"),
                Code::UnreachableArm,
                in_match("        2 |\n            3 => print(2)"),
                false,
            ),
            // A comment after the alternative, not before it, stays after a backward removal.
            (
                in_match("        2 | 1 | // c\n        3 => print(2)"),
                Code::UnreachableArm,
                in_match("        2 | // c\n        3 => print(2)"),
                false,
            ),
            // A whole arm is not cut at a comment.
            (
                in_main(
                    "    let x = 1\n    match x {\n        1 => print(1)\n        1 => { // c\n            print(1)\n        }; _ => print(2)\n    }",
                ),
                Code::UnreachableArm,
                in_main(
                    "    let x = 1\n    match x {\n        1 => print(1)\n        _ => print(2)\n    }",
                ),
                false,
            ),
            (
                in_main(
                    "    let x = 1\n    match x {\n1 => print(1)\n1 => print(1) // c\n_ => print(2)\n    }",
                ),
                Code::UnreachableArm,
                in_main("    let x = 1\n    match x {\n1 => print(1)\n_ => print(2)\n    }"),
                false,
            ),
        ];
        for (src, code, want, safe) in rows {
            assert_eq!(fixed(&src, code), (want, safe), "{src}");
        }
    }

    /// GIVEN an unreached last alternative with a comment between it and its `|`, at the top of an
    /// arm and in a payload
    /// WHEN the W0006 fix is applied after `sound_fixes`
    /// THEN the removal is backward, as on main, and the comment goes: no other alternative
    /// follows, so no edit keeps it and parses
    #[test]
    fn a_last_alternative_removal_should_drop_a_comment_before_it() {
        for (src, want) in [
            (
                in_optional_match(
                    "        None | // c\n        None => print(1)\n        Some(_) => print(2)",
                ),
                in_optional_match("        None => print(1)\n        Some(_) => print(2)"),
            ),
            (
                in_optional_match(
                    "        Some(1) => print(1)\n        Some(2 | // c\n            1) => print(2)",
                ),
                in_optional_match("        Some(1) => print(1)\n        Some(2) => print(2)"),
            ),
        ] {
            assert_eq!(fixed(&src, Code::UnreachableArm), (want, false), "{src}");
        }
    }

    /// A bracket error stops the lexer, which then reports only it.
    #[test]
    fn marks_a_bracket_error() {
        let src = "fn main() {\n    )\n}\n";
        assert_eq!(marks(src), [src.rfind(')').expect("src has a `)`")]);
    }

    /// The spec's `a.sisu`.
    const SAMPLE: &str = "fn total(xs: [i64]) -> i64 {
    let sum = 0
    for i in 0..len(xs) {
        sum += xs[i]
    }
    sum
}

fn main() {
    let xs = [1, 2, 3]
    print(totl(xs))
}
";

    #[test]
    fn one_error_per_broken_item() {
        let src = "fn f() {\n    let = 1\n    let = 2\n}\nfn g() {\n    var x = 1\n    var y = 2\n}\nfn main() {\n    let b: bool = 1\n}\n";
        assert_eq!(
            codes(src),
            [
                (Code::Expected, (2, 9)),
                (Code::Var, (6, 5)),
                (Code::Mismatch, (10, 19))
            ]
        );
    }

    /// Only the parser sees the error in `broken`, so the checker's own list holds no error.
    #[test]
    fn front_end_should_not_warn_about_unused_names_when_an_item_fails_to_parse() {
        let src = "fn helper() {\n    print(1)\n}\nfn broken() {\n    let = 1\n}\nfn main() {\n    let mut m = 1\n    print(m)\n}\n";
        assert_eq!(
            codes(src),
            [(Code::Expected, (5, 9)), (Code::NeverReassigned, (8, 13))]
        );
    }

    /// A program that `mono` stops after the check gets no warning that judges liveness, as a
    /// program with an error gets none (W0012). `Outer` and `W` are never built in the program, so
    /// the check warns of them until `mono` fails.
    #[test]
    fn front_end_should_drop_the_liveness_warnings_when_mono_stops_the_program() {
        crate::on_compiler_stack(|| {
            let src = check::tests::interfaces::long_loop(200, "fn main() {\n    get(Leaf())\n}");
            let (program, diagnostics) = front_end(&src);
            let found: Vec<_> = diagnostics.iter().map(Diagnostic::code).collect();
            assert_eq!(found, [Code::UnknownType], "{diagnostics:?}");
            assert!(program.is_none());
            // The check alone passes the program, and warns.
            let checked = checked(&src).1;
            assert!(checked.iter().any(check::is_judged), "{checked:?}");
        });
    }

    /// The unnamed `const enum` starts where its line does, inside the unnamed `1` before it;
    /// the `$` between them is reported once.
    #[test]
    fn overlapping_unnamed_items() {
        assert_eq!(
            codes("1\n$\nconst enum{}"),
            [
                (Code::Expected, (1, 1)),
                (Code::UnexpectedCharacter, (2, 1))
            ]
        );
    }

    #[test]
    fn lexer_error_hides_its_parse_error() {
        let src = "fn main() {\n    let n: i64 = $\n}\n";
        assert_eq!(codes(src), [(Code::UnexpectedCharacter, (2, 18))]);
    }

    #[test]
    fn spec_sample_reports_three_errors() {
        assert_eq!(
            codes(SAMPLE),
            [
                (Code::MethodAsFunction, (3, 17)),
                (Code::AssignToImmutable, (4, 9)),
                (Code::FunctionNotFound, (11, 11))
            ]
        );
    }

    /// An unclosed string breaks `main`, so the lexer's error stands for it and `totl` goes
    /// unreported.
    #[test]
    fn spec_sample_with_quote() {
        let src = SAMPLE.replace(
            "let xs = [1, 2, 3]\n",
            "let xs = [1, 2, 3]\n    let n: i64 = \"three\n",
        );
        assert_eq!(
            codes(&src),
            [
                (Code::MethodAsFunction, (3, 17)),
                (Code::AssignToImmutable, (4, 9)),
                (Code::LiteralNotClosed, (11, 18))
            ]
        );
    }

    /// A bad spec is reported once and breaks nothing around it; a spec that a `"` cuts off is
    /// not read, and the unclosed literal is its function's one error.
    #[test]
    fn format_spec_recovery() {
        use Code::{
            EmptyInterpolation, Expected, LiteralNotClosed, Mismatch, UnsupportedFormatSpec,
        };
        for (src, expected) in [
            (
                "fn main() {\n    let w = 3\n    print(\"{w:>{w}}\")\n}\n",
                vec![(UnsupportedFormatSpec, (3, 14))],
            ),
            (
                "fn main() {\n    let x = 1.5\n    let s = \"{x:>8\"\n}\nfn f() {\n    let y: i64 = \"a\"\n}\n",
                vec![(LiteralNotClosed, (3, 13)), (Mismatch, (6, 18))],
            ),
            (
                "fn main() {\n    let x = 1\n    print(\"{x:+\" + \"abc}\")\n}\n",
                vec![(LiteralNotClosed, (3, 11))],
            ),
            // A spec with no expression before it, as Rust and Python allow.
            (
                "fn main() {\n    print(\"{:>8}\")\n}\n",
                vec![(EmptyInterpolation, (2, 12))],
            ),
            (
                "fn main() {\n    let x = 1\n    let y = 2\n    print(\"{x:03} {y:+}\")\n}\n",
                vec![
                    (UnsupportedFormatSpec, (4, 14)),
                    (UnsupportedFormatSpec, (4, 21)),
                ],
            ),
            // An item that another error breaks reports its earliest error, a spec's included.
            (
                "fn main() {\n    let x = 1\n    print(\"{x:+}\")\n    let = 2\n}\n",
                vec![(UnsupportedFormatSpec, (3, 14))],
            ),
            (
                "fn main() {\n    let = 2\n    print(\"{2:+}\")\n}\n",
                vec![(Expected, (2, 9))],
            ),
            (
                "class A {\n    fn m(self) {\n        print(\"{1:+}\")\n    }\n    let n = 1\n}\nfn main() {}\n",
                vec![(UnsupportedFormatSpec, (3, 18))],
            ),
        ] {
            assert_eq!(codes(src), expected, "{src}");
        }
    }

    /// A spec with an error is the one report of its part: an optional there gets no warning.
    #[test]
    fn a_bad_format_spec_on_an_optional_should_give_no_warning() {
        use Code::{FormatSpecLimit, MalformedFormatSpec, UnsupportedFormatSpec};
        for (spec, expected) in [
            ("08.2", UnsupportedFormatSpec),
            ("+", UnsupportedFormatSpec),
            ("2000", FormatSpecLimit),
            ("8.", MalformedFormatSpec),
        ] {
            let src =
                format!("fn main() {{\n    let p: f64? = 1.5\n    print(\"{{p:{spec}}}\")\n}}\n");
            assert_eq!(codes(&src), [(expected, (3, 14))], "spec {spec}");
        }
    }

    #[test]
    fn call_to_broken_function_checks_arguments() {
        let src = "fn f() {\n    let = 1\n}\nfn main() {\n    f(nope)\n}\n";
        assert_eq!(
            codes(src),
            [(Code::Expected, (2, 9)), (Code::BindingNotFound, (5, 7))]
        );
    }

    #[test]
    fn broken_class_poisons_its_uses() {
        // `t` has the broken type, so its method call has a poisoned receiver. That path checks
        // the call's arguments with `stray`, so `other`, a real mistake, is still reported.
        let src = "class Tree {\n    value i64\n    fn m(self, x: i64) {}\n}\nfn main() {\n    let t: Tree = Tree(value: nope)\n    t.m(other)\n}\n";
        assert_eq!(
            codes(src),
            [
                (Code::Expected, (2, 11)),
                (Code::BindingNotFound, (6, 31)),
                (Code::BindingNotFound, (7, 9))
            ]
        );
    }

    #[test]
    fn field_list_error_skips_methods() {
        let src = "class Tree {\n    value i64\n    fn m(self) {\n        let b: bool = 1\n    }\n}\nfn main() {}\n";
        assert_eq!(codes(src), [(Code::Expected, (2, 11))]);
    }

    #[test]
    fn broken_method_only_breaks_itself() {
        let src = "class Counter {\n    n: i64\n    fn broken(self, x: i64) {\n        let = 1\n    }\n    fn typed(self, x: i64) {\n        let b: bool = x\n    }\n}\nfn main() {\n    let c = Counter(n: 0)\n    c.broken(nope)\n    c.typed(1)\n}\n";
        assert_eq!(
            codes(src),
            [
                (Code::Expected, (4, 13)),
                (Code::Mismatch, (7, 23)),
                (Code::BindingNotFound, (12, 14))
            ]
        );
    }

    #[test]
    fn error_before_method_name_breaks_the_class() {
        // `x: true` is a mismatch only if the class is intact.
        let src = "class P {\n    x: i64\n    fn (self) {}\n}\nfn main() {\n    let p: P = P(x: true)\n    print(p.x)\n}\n";
        assert_eq!(codes(src), [(Code::Expected, (3, 8))]);
    }

    #[test]
    fn lexer_error_between_members_breaks_the_class() {
        // `n: true` is a mismatch only if the class is intact.
        let src = "class C {\n    n: i64\n    $\n}\nfn main() {\n    let c = C(n: true)\n}\n";
        assert_eq!(codes(src), [(Code::UnexpectedCharacter, (3, 5))]);
    }

    #[test]
    fn method_without_self_breaks_only_itself() {
        let src = "class C {\n    n: i64\n    fn m() {}\n    fn k(self) {\n        let b: bool = 1\n    }\n}\nfn main() {\n    let c = C(n: 1)\n    c.m()\n}\n";
        assert_eq!(
            codes(src),
            [(Code::MethodNeedsSelf, (3, 8)), (Code::Mismatch, (5, 23))]
        );
    }

    #[test]
    fn broken_class_reports_its_earliest_error() {
        // Two broken methods come before the field list error, which breaks the class.
        let src = "class C {\n    n: i64\n    fn a(self) {\n        let = 1\n    }\n    fn b(self) {\n        let = 2\n    }\n    x i64\n}\nfn main() {}\n";
        assert_eq!(codes(src), [(Code::Expected, (4, 13))]);
    }

    #[test]
    fn skipped_text_before_a_cut_lies_in_the_item() {
        // `Eof` cuts the item off after `->`; the `$` is still its own.
        assert_eq!(codes("fn main() -> $"), [(Code::Expected, (1, 13))]);
        assert_eq!(codes("def total(xs): $"), [(Code::Expected, (1, 1))]);
        // A skip-only line before the next item still belongs to the unnamed segment.
        let src = "def total(xs):\n    return 0\n$\nfn main() {}\n";
        assert_eq!(codes(src), [(Code::Expected, (1, 1))]);
        assert_eq!(codes("def total(xs): x\n$"), [(Code::Expected, (1, 1))]);
        // The class's `}` cuts the method off; the `$` breaks the method, not the class.
        let src = "class C {\n    n: i64\n    fn m(self) -> $\n}\nfn main() {\n    let c = C(n: true)\n}\n";
        assert_eq!(
            codes(src),
            [
                (Code::UnexpectedCharacter, (3, 19)),
                (Code::Mismatch, (6, 18))
            ]
        );
    }

    #[test]
    fn broken_name_should_poison_a_built_in() {
        // A broken item also gets the name's error, as an intact item does.
        let src = "fn print() {\n    let = 1\n}\nfn main() {\n    print(1, 2)\n}\n";
        assert_eq!(
            codes(src),
            [(Code::DefinedTwice, (1, 4)), (Code::Expected, (2, 9))]
        );
        let src = "class i64 {\n    x y\n}\nfn main() {\n    let n: i64 = true\n}\n";
        assert_eq!(
            codes(src),
            [(Code::BuiltInName, (1, 7)), (Code::Expected, (2, 7))]
        );
        // A broken constant poisons a call of its name too, whose arguments are still checked.
        let src = "const print = 1 2\nfn main() {\n    print(1, 2)\n    print(nope)\n}\n";
        assert_eq!(
            codes(src),
            [
                (Code::DefinedTwice, (1, 7)),
                (Code::Expected, (1, 17)),
                (Code::BindingNotFound, (4, 11))
            ]
        );
        let src = "const i64 = 1 2\nfn main() {\n    let n = i64(true, 1)\n}\n";
        assert_eq!(
            codes(src),
            [(Code::BuiltInName, (1, 7)), (Code::Expected, (1, 15))]
        );
    }

    /// A broken item gets the duplicate check of an intact item: it is reported against the first
    /// item of its name, or the later item is reported against it, and it still poisons its
    /// name. In each row: the program and the code and place of each diagnostic.
    #[test]
    fn a_broken_item_should_get_the_duplicate_check_of_an_intact_item() {
        use Code::{
            BraceOnNewLine as Brace, DefinedTwice as Twice, Expected, Mismatch, NoMember as NoNext,
        };
        let rows: &[(&str, &str, &[Found])] = &[
            // The E0063 rows stay until the failed bound of a poisoned interface stops
            // cascading (#506): `Source` is intact and poisoned, so the bound of `drain` fails.
            (
                "broken interface after an intact one, and a use",
                "interface Source {\n    fn next(self) -> i64\n}\n\nfn drain<S: Source>(s: S) {\n    print(s.next())\n}\n\ninterface Source\n{   fn get(self) -> i64\n}\n\nfn main() {\n    let x: i64 = \"s\"\n}\n",
                &[
                    (NoNext, (6, 13)),
                    (Twice, (9, 11)),
                    (Brace, (10, 1)),
                    (Mismatch, (14, 18)),
                ],
            ),
            (
                "broken interface after an intact one",
                "interface Source {\n    fn next(self) -> i64\n}\n\nfn drain<S: Source>(s: S) {\n    print(s.next())\n}\n\ninterface Source\n{   fn get(self) -> i64\n}\n\nfn main() {}\n",
                &[(NoNext, (6, 13)), (Twice, (9, 11)), (Brace, (10, 1))],
            ),
            (
                "broken interface, no use",
                "interface Source {\n    fn next(self) -> i64\n}\n\ninterface Source\n{   fn get(self) -> i64\n}\n\nfn main() {}\n",
                &[(Twice, (5, 11)), (Brace, (6, 1))],
            ),
            (
                "broken class",
                "class A {\n    x: i64\n}\n\nclass A\n{   y: i64\n}\n\nfn main() {\n    let x: i64 = \"s\"\n}\n",
                &[(Twice, (5, 7)), (Brace, (6, 1)), (Mismatch, (10, 18))],
            ),
            (
                "broken enum",
                "enum E {\n    A\n}\n\nfn f(e: E) {\n    match e { A => print(1) }\n}\n\nenum E\n{   B\n}\n\nfn main() {}\n",
                &[(Twice, (9, 6)), (Brace, (10, 1))],
            ),
            (
                "broken function",
                "fn g() -> i64 {\n    1\n}\n\nfn f() {\n    print(g())\n}\n\nfn g() -> string\n{   \"a\"\n}\n\nfn main() {}\n",
                &[(Twice, (9, 4)), (Brace, (10, 1))],
            ),
            (
                "broken function after a class",
                "class A {\n    x: i64\n}\n\nfn A()\n{\n}\n\nfn main() {}\n",
                &[(Twice, (5, 4)), (Brace, (6, 1))],
            ),
            (
                "two broken classes",
                "class A\n{   x: i64\n}\n\nclass A\n{   y: i64\n}\n\nfn main() {}\n",
                &[(Brace, (2, 1)), (Twice, (5, 7)), (Brace, (6, 1))],
            ),
            (
                "broken interface before an intact one",
                "interface Source\n{   fn get(self) -> i64\n}\n\nfn drain<S: Source>(s: S) {\n    print(s.next())\n}\n\ninterface Source {\n    fn next(self) -> i64\n}\n\nfn main() {}\n",
                &[(Brace, (2, 1)), (NoNext, (6, 13)), (Twice, (9, 11))],
            ),
            (
                "error in a body",
                "fn total() {\n    let = 1\n}\nfn total() {}\nfn main() {\n    total()\n}\n",
                &[(Expected, (2, 9)), (Twice, (4, 4))],
            ),
            (
                "broken constant",
                "const A = 1 2\nfn A() {}\nfn main() {}\n",
                &[(Expected, (1, 13)), (Twice, (2, 4))],
            ),
            // The name of a prelude function keeps the report of the prelude: one error.
            (
                "broken function with a prelude function's name",
                "fn print(x: i64)\n{\n}\n\nfn main() {}\n",
                &[(Twice, (1, 4)), (Brace, (2, 1))],
            ),
            // An error inside a requirement leaves the interface intact, so the check was there.
            (
                "interface with an error in a requirement",
                "interface Source {\n    fn next(self) -> i64\n}\n\ninterface Source {\n    fn get(self) -> \n}\n\nfn main() {}\n",
                &[(Twice, (5, 11)), (Expected, (7, 1))],
            ),
        ];
        for &(row, src, want) in rows {
            assert_eq!(codes(src), want, "row `{row}`: {src}");
        }
    }

    /// A diagnostic's code and `(line, col)`.
    type Found = (Code, (usize, usize));

    /// The code and `(line, col)` of every diagnostic that the checker's front end reports for
    /// `src`, in order. It runs the lexer, the parser and the checker, as `sisu check` does.
    fn checked_codes(src: &str) -> Vec<Found> {
        checked(src)
            .1
            .iter()
            .map(|d| (d.code(), line_col(src, d.span().start())))
            .collect()
    }

    /// The names that an intact item may not take, with the code that it gets: E0026 for a
    /// built-in type, a language interface or a reserved name, E0034 for a prelude function. A
    /// user's name gets none.
    const TAKEN_NAMES: &[(&str, Option<Code>)] = &[
        ("Eq", Some(Code::BuiltInName)),
        ("Ord", Some(Code::BuiltInName)),
        ("Display", Some(Code::BuiltInName)),
        ("AnyObject", Some(Code::BuiltInName)),
        ("Option", Some(Code::BuiltInName)),
        ("Result", Some(Code::BuiltInName)),
        ("Some", Some(Code::BuiltInName)),
        ("Ok", Some(Code::BuiltInName)),
        ("Err", Some(Code::BuiltInName)),
        ("i64", Some(Code::BuiltInName)),
        ("u8", Some(Code::BuiltInName)),
        ("f64", Some(Code::BuiltInName)),
        ("char", Some(Code::BuiltInName)),
        ("string", Some(Code::BuiltInName)),
        ("bool", Some(Code::BuiltInName)),
        ("print", Some(Code::DefinedTwice)),
        ("eprint", Some(Code::DefinedTwice)),
        ("panic", Some(Code::DefinedTwice)),
        ("dbg", Some(Code::DefinedTwice)),
        ("read_line", Some(Code::DefinedTwice)),
        ("Node", None),
    ];

    const MAIN: &str = "fn main() {\n    print(1)\n}\n";

    /// Each broken item gets the name error of an intact item of its kind, at its name, before
    /// its own error. The wanted list holds the name's code, if any, and then the break's.
    #[test]
    #[expect(clippy::too_many_lines, reason = "one table of rows")]
    fn broken_items_should_get_the_name_errors_of_intact_items() {
        // `(row, text before the name, text after it, the token of the break in that text, the
        // break's code)`. `nl` has `{` on the next line, `lex` a `$` in the item, `parse` a
        // parse error in it.
        let forms: &[(&str, &str, &str, &str, Code)] = &[
            (
                "nl interface",
                "interface ",
                "\n{}\n",
                "{}",
                Code::BraceOnNewLine,
            ),
            (
                "nl comment interface",
                "interface ",
                " // note\n{}\n",
                "{}",
                Code::BraceOnNewLine,
            ),
            ("nl class", "class ", "\n{}\n", "{}", Code::BraceOnNewLine),
            (
                "nl enum",
                "enum ",
                "\n{\n    A\n}\n",
                "{",
                Code::BraceOnNewLine,
            ),
            ("nl fn", "fn ", "()\n{}\n", "{}", Code::BraceOnNewLine),
            (
                "lex interface",
                "interface ",
                " {\n    $\n}\n",
                "$",
                Code::UnexpectedCharacter,
            ),
            (
                "lex class",
                "class ",
                " {\n    $\n}\n",
                "$",
                Code::UnexpectedCharacter,
            ),
            (
                "lex enum",
                "enum ",
                " {\n    A $\n}\n",
                "$",
                Code::UnexpectedCharacter,
            ),
            (
                "lex fn",
                "fn ",
                "() {\n    $\n}\n",
                "$",
                Code::UnexpectedCharacter,
            ),
            (
                "lex const",
                "const ",
                " = 1 $\n",
                "$",
                Code::UnexpectedCharacter,
            ),
            (
                "lex type",
                "type ",
                " = i64 $\n",
                "$",
                Code::UnexpectedCharacter,
            ),
            (
                "parse interface",
                "interface ",
                " {\n    x y\n}\n",
                "y",
                Code::Expected,
            ),
            (
                "parse class",
                "class ",
                " {\n    x y\n}\n",
                "y",
                Code::Expected,
            ),
            (
                "parse enum",
                "enum ",
                " {\n    A B\n}\n",
                "B",
                Code::Expected,
            ),
            (
                "parse fn",
                "fn ",
                "() {\n    let = 1\n}\n",
                "= 1",
                Code::Expected,
            ),
            ("parse const", "const ", " = 1 2\n", "2", Code::Expected),
            ("parse type", "type ", " = i64 2\n", "2", Code::Expected),
        ];
        let mut failures = Vec::new();
        for &(row, head, tail, token, code) in forms {
            for &(name, name_code) in TAKEN_NAMES {
                let src = format!("{head}{name}{tail}{MAIN}");
                let at = |offset| line_col(&src, offset);
                let mut want = Vec::new();
                if let Some(name_code) = name_code {
                    want.push((name_code, at(head.len())));
                }
                want.push((
                    code,
                    at(head.len() + name.len() + tail.find(token).unwrap()),
                ));
                let got = checked_codes(&src);
                if got != want {
                    failures.push(format!("{row} {name}: got {got:?}, want {want:?}"));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        // An `extend` has no name of its own: its target is checked, and no name error is added.
        for &(name, _) in TAKEN_NAMES {
            let src = format!("extend {name}\n{{}}\n{MAIN}");
            assert_eq!(
                checked_codes(&src),
                [(Code::BraceOnNewLine, (2, 1))],
                "extend {name}"
            );
        }
    }

    /// A bracket error stops the checker, so a broken item gets no name error.
    #[test]
    fn broken_items_with_a_bracket_error_should_get_no_name_error() {
        for (head, tail) in [
            ("interface ", " {\n    )\n}\n"),
            ("class ", " {\n    )\n}\n"),
            ("fn ", "() {\n    )\n}\n"),
        ] {
            let src = format!("{head}Eq{tail}{MAIN}");
            assert_eq!(
                checked_codes(&src),
                [(Code::UnexpectedClosingBracket, (2, 5))],
                "{head}"
            );
        }
    }

    /// Items of one name, a generic class and the uses of a broken name. The prelude keeps the
    /// name of its interface, the uses of a broken name stay poisoned, and an item of the same
    /// name as a broken one is a duplicate.
    #[test]
    fn broken_items_should_get_the_name_errors_beside_items_of_the_same_name() {
        use Code::{BraceOnNewLine as Brace, BuiltInName as Built, DefinedTwice as Twice};
        let rows: &[(&str, &str, &[Found])] = &[
            (
                "broken then intact",
                "interface Eq\n{}\ninterface Eq {}\n",
                &[
                    (Built, (1, 11)),
                    (Brace, (2, 1)),
                    (Built, (3, 11)),
                    (Twice, (3, 11)),
                ],
            ),
            (
                "intact then broken",
                "interface Eq {}\ninterface Eq\n{}\n",
                &[
                    (Built, (1, 11)),
                    (Built, (2, 11)),
                    (Twice, (2, 11)),
                    (Brace, (3, 1)),
                ],
            ),
            (
                "two broken",
                "interface Eq\n{}\ninterface Eq\n{}\n",
                &[
                    (Built, (1, 11)),
                    (Brace, (2, 1)),
                    (Built, (3, 11)),
                    (Twice, (3, 11)),
                    (Brace, (4, 1)),
                ],
            ),
            (
                "broken then intact, a user's name",
                "interface Node\n{}\ninterface Node {}\n",
                &[(Brace, (2, 1)), (Twice, (3, 11))],
            ),
            (
                "generic class",
                "class Eq<T>\n{}\n",
                &[(Built, (1, 7)), (Brace, (2, 1))],
            ),
            // The call of a broken `fn Eq` adds no E0105: the item is poisoned in every namespace.
            (
                "call of a broken function",
                "fn Eq()\n{}\nfn main() {\n    Eq()\n}\n",
                &[(Built, (1, 4)), (Brace, (2, 1))],
            ),
            (
                "broken class Option",
                "class Option\n{}\nfn main() {\n    let x = Some(1)\n    let y: i64? = None\n    print(1)\n}\n",
                &[(Built, (1, 7)), (Brace, (2, 1))],
            ),
            (
                "broken class Result",
                "class Result\n{}\nfn f() -> Result<i64, string> {\n    Ok(1)\n}\nfn main() {\n    print(1)\n}\n",
                &[(Built, (1, 7)), (Brace, (2, 1))],
            ),
            (
                "broken fn Some",
                "fn Some()\n{}\nfn main() {\n    let x = Some(1)\n    print(1)\n}\n",
                &[(Built, (1, 4)), (Brace, (2, 1))],
            ),
        ];
        for &(row, src, want) in rows {
            let src = if src.contains("fn main") {
                src.to_string()
            } else {
                format!("{src}{MAIN}")
            };
            assert_eq!(checked_codes(&src), want, "{row}");
        }
    }

    /// Checks each row `(name, program, wanted codes)`, and names every row that fails.
    fn assert_rows(rows: &[(&str, &str, &[Found])]) {
        let failed: Vec<String> = rows
            .iter()
            .filter_map(|&(row, src, want)| {
                let got = codes(src);
                (got != want).then(|| format!("{row}: got {got:?}, want {want:?}"))
            })
            .collect();
        assert!(failed.is_empty(), "{}", failed.join("\n"));
    }

    /// A rejected item (a name that a prelude function takes, or a duplicate) poisons its name:
    /// the item's own error is reported, and a use of the name gives no more errors.
    #[test]
    #[expect(clippy::too_many_lines, reason = "one table of rows")]
    fn a_use_of_a_rejected_name_should_give_no_second_error() {
        use Code::{
            BindingNotFound as NoBind, DefinedTwice as Twice, MutMarkerMissing as MutMissing,
        };
        let rows: &[(&str, &str, &[Found])] = &[
            (
                "call, enum",
                "enum print { A }\nfn main() { print(1, 2) }\n",
                &[(Twice, (1, 6))],
            ),
            (
                "call, alias",
                "type print = i64\nfn main() { print(1, 2) }\n",
                &[(Twice, (1, 6))],
            ),
            (
                "call dbg, enum",
                "enum dbg { A }\nfn main() { dbg(1, 2) }\n",
                &[(Twice, (1, 6))],
            ),
            (
                "call dbg, alias",
                "type dbg = i64\nfn main() { dbg(1, 2) }\n",
                &[(Twice, (1, 6))],
            ),
            (
                "call, arguments stay checked",
                "enum print { A }\nfn main() { print(nope) }\n",
                &[(Twice, (1, 6)), (NoBind, (2, 19))],
            ),
            (
                "read, fn",
                "fn print(x: i64, y: i64) {}\nfn main() { let p = print }\n",
                &[(Twice, (1, 4))],
            ),
            (
                "read, class",
                "class print {}\nfn main() { let p = print }\n",
                &[(Twice, (1, 7))],
            ),
            (
                "read, enum",
                "enum print { A }\nfn main() { let p = print }\n",
                &[(Twice, (1, 6))],
            ),
            (
                "read, alias",
                "type print = i64\nfn main() { let p = print }\n",
                &[(Twice, (1, 6))],
            ),
            (
                "read, interface",
                "interface print {}\nfn main() { let p = print }\n",
                &[(Twice, (1, 11))],
            ),
            (
                "read, constant",
                "const print = 1\nfn main() { let p = print }\n",
                &[(Twice, (1, 7))],
            ),
            (
                "argument of a call of it",
                "class print {}\nfn main() { print(print) }\n",
                &[(Twice, (1, 7))],
            ),
            (
                "argument",
                "class print {}\nfn g(x: i64) {}\nfn main() { g(print) }\n",
                &[(Twice, (1, 7))],
            ),
            (
                "field read",
                "class print {}\nfn main() { let a = print.x }\n",
                &[(Twice, (1, 7))],
            ),
            (
                "index read",
                "class print {}\nfn main() { let a = print[0] }\n",
                &[(Twice, (1, 7))],
            ),
            (
                "method call",
                "class print {}\nfn main() { print.go() }\n",
                &[(Twice, (1, 7))],
            ),
            (
                "for",
                "class print {}\nfn main() { for x in print { } }\n",
                &[(Twice, (1, 7))],
            ),
            (
                "interpolation",
                "class print {}\nfn main() { let a = \"{print}\" }\n",
                &[(Twice, (1, 7))],
            ),
            (
                "tail",
                "class print {}\nfn g() -> i64 { print }\nfn main() {}\n",
                &[(Twice, (1, 7))],
            ),
            (
                "assign",
                "class print {}\nfn main() { print = 1 }\n",
                &[(Twice, (1, 7))],
            ),
            (
                "compound assign",
                "class print {}\nfn main() { print += 1 }\n",
                &[(Twice, (1, 7))],
            ),
            (
                "field assign",
                "class print {}\nfn main() { print.x = 1 }\n",
                &[(Twice, (1, 7))],
            ),
            (
                "index assign",
                "class print {}\nfn main() { print[0] = 1 }\n",
                &[(Twice, (1, 7))],
            ),
            (
                "a local takes the name",
                "class print {}\nfn main() {\n    let print = 1\n    let p = print\n}\n",
                &[(Twice, (1, 7))],
            ),
            (
                "mut argument",
                "class print {}\nfn g(mut x: i64) { x = 1 }\nfn main() { g(mut print) }\n",
                &[(Twice, (1, 7))],
            ),
            (
                "constant's value",
                "class print {}\nconst K = print\nfn main() {}\n",
                &[(Twice, (1, 7))],
            ),
            (
                "call with a mut argument",
                "enum print { A }\nfn main() { let mut a = 1\n print(mut a) }\n",
                &[(Twice, (1, 6))],
            ),
            (
                "call inside a generic argument",
                "enum dbg { A }\nfn id<T>(x: T) -> T { x }\nfn main() { let a = id(dbg(1, 2)) }\n",
                &[(Twice, (1, 6))],
            ),
            (
                "the argument's missing `mut` stays",
                "class print {}\nfn g(mut x: i64) { x = 1 }\nfn main() { g(print) }\n",
                &[(Twice, (1, 7)), (MutMissing, (3, 15))],
            ),
            (
                "type position, fn",
                "fn print() {}\nfn g(x: print) {}\nfn main() {}\n",
                &[(Twice, (1, 4))],
            ),
            (
                "conformance, fn",
                "fn print() {}\nclass C: print {}\nfn main() {}\n",
                &[(Twice, (1, 4))],
            ),
            (
                "any, fn",
                "fn print() {}\nfn g(x: any print) {}\nfn main() {}\n",
                &[(Twice, (1, 4))],
            ),
            (
                "bound, fn",
                "fn print() {}\nfn g<T: print>(x: T) {}\nfn main() {}\n",
                &[(Twice, (1, 4))],
            ),
        ];
        assert_rows(rows);
    }

    /// A broken item (a syntax error in it) poisons its name: a use of the name gives no more
    /// errors. A use before the item is poisoned too.
    #[test]
    #[expect(clippy::too_many_lines, reason = "one table of rows")]
    fn a_use_of_a_broken_name_should_give_no_second_error() {
        use Code::{
            BindingNotFound as NoBind, BraceOnNewLine as Brace, BuiltInName as Built,
            Expected as Exp, NeverReassigned as Never,
        };
        let rows: &[(&str, &str, &[Found])] = &[
            (
                "call, enum",
                "enum E\n{ A }\nfn main() { E(1) }\n",
                &[(Brace, (2, 1))],
            ),
            (
                "call, alias",
                "type T = 1\nfn main() { T(1) }\n",
                &[(Exp, (1, 10))],
            ),
            (
                "read, fn",
                "fn f()\n{}\nfn main() { let p = f }\n",
                &[(Brace, (2, 1))],
            ),
            (
                "read, class",
                "class C\n{}\nfn main() { let p = C }\n",
                &[(Brace, (2, 1))],
            ),
            (
                "read, enum",
                "enum E\n{ A }\nfn main() { let p = E }\n",
                &[(Brace, (2, 1))],
            ),
            (
                "read, interface",
                "interface I\n{}\nfn main() { let p = I }\n",
                &[(Brace, (2, 1))],
            ),
            (
                "read, alias",
                "type T = 1\nfn main() { let p = T }\n",
                &[(Exp, (1, 10))],
            ),
            (
                "read, a variant's name too",
                "enum E { A }\nclass A\n{}\nfn main() { let p = A }\n",
                &[(Brace, (3, 1))],
            ),
            (
                "assign, fn",
                "fn f()\n{}\nfn main() { f = 1 }\n",
                &[(Brace, (2, 1))],
            ),
            (
                "assign, class",
                "class C\n{}\nfn main() { C = 1 }\n",
                &[(Brace, (2, 1))],
            ),
            (
                "assign, enum",
                "enum E\n{ A }\nfn main() { E = 1 }\n",
                &[(Brace, (2, 1))],
            ),
            (
                "assign gives the warning",
                "class C\n{}\nfn main() {\n    let mut x = 1\n    C = 2\n    print(x)\n}\n",
                &[(Brace, (2, 1)), (Never, (4, 13))],
            ),
            (
                "mut argument, class",
                "class C\n{}\nfn g(mut x: i64) { x = 1 }\nfn main() { g(mut C) }\n",
                &[(Brace, (2, 1))],
            ),
            (
                "mut argument, fn",
                "fn f()\n{}\nfn g(mut x: i64) { x = 1 }\nfn main() { g(mut f) }\n",
                &[(Brace, (2, 1))],
            ),
            (
                "mut argument, field",
                "class C\n{}\nfn g(mut x: i64) { x = 1 }\nfn main() { g(mut C.x) }\n",
                &[(Brace, (2, 1))],
            ),
            (
                "constant's value",
                "class C\n{}\nconst K = C\nfn main() {}\n",
                &[(Brace, (2, 1))],
            ),
            (
                "method call",
                "class C\n{}\nfn main() { C.go() }\n",
                &[(Brace, (2, 1))],
            ),
            (
                "method call, arguments stay checked",
                "class C\n{}\nfn main() { C.go(nope) }\n",
                &[(Brace, (2, 1)), (NoBind, (3, 18))],
            ),
            (
                "read before the item",
                "fn main() { let p = C }\nclass C\n{}\n",
                &[(Brace, (3, 1))],
            ),
            (
                "type position, fn",
                "fn C()\n{}\nfn g(x: C) {}\nfn main() {}\n",
                &[(Brace, (2, 1))],
            ),
            (
                "type position, constant",
                "const C = 1 2\nfn g(x: C) {}\nfn main() {}\n",
                &[(Exp, (1, 13))],
            ),
            (
                "pattern path, fn",
                "enum E { X }\nfn C()\n{}\nfn f(e: E) { match e { C.X => {}; _ => {} } }\nfn main() {}\n",
                &[(Brace, (3, 1))],
            ),
            (
                "read of a broken Ok",
                "class Ok\n{}\nfn main() { let a = Ok }\n",
                &[(Built, (1, 7)), (Brace, (2, 1))],
            ),
            (
                "read of a broken Some",
                "class Some\n{}\nfn main() { let a = Some }\n",
                &[(Built, (1, 7)), (Brace, (2, 1))],
            ),
            (
                "broken Ok, call",
                "class Ok\n{}\nfn main() { let a = Ok(1) }\n",
                &[(Built, (1, 7)), (Brace, (2, 1))],
            ),
            (
                "broken Err, call",
                "class Err\n{}\nfn f() -> Result<i64, string> {\n    Err(\"e\")\n}\nfn main() {}\n",
                &[(Built, (1, 7)), (Brace, (2, 1))],
            ),
            (
                "broken Option, Some",
                "class Option\n{}\nfn main() { let a = Some(1)\n    print(1) }\n",
                &[(Built, (1, 7)), (Brace, (2, 1))],
            ),
            (
                "type position, fn Eq",
                "fn Eq()\n{}\nfn g(x: Eq) {}\nfn main() {}\n",
                &[(Built, (1, 4)), (Brace, (2, 1))],
            ),
            (
                "return type, fn Eq",
                "fn Eq()\n{}\nfn g() -> Eq { }\nfn main() {}\n",
                &[(Built, (1, 4)), (Brace, (2, 1))],
            ),
            (
                "annotation, fn Eq",
                "fn Eq()\n{}\nfn main() { let x: Eq = 1 }\n",
                &[(Built, (1, 4)), (Brace, (2, 1))],
            ),
            (
                "conformance, fn",
                "fn I()\n{}\nclass C: I {}\nfn main() {}\n",
                &[(Brace, (2, 1))],
            ),
            (
                "any, fn",
                "fn I()\n{}\nfn g(x: any I) {}\nfn main() {}\n",
                &[(Brace, (2, 1))],
            ),
            (
                "bound, fn",
                "fn I()\n{}\nfn g<T: I>(x: T) {}\nfn main() {}\n",
                &[(Brace, (2, 1))],
            ),
            (
                "extend, fn",
                "fn I()\n{}\nextend I {\n    fn m(self) {}\n}\nfn main() {}\n",
                &[(Brace, (2, 1))],
            ),
            (
                "extend, fn Eq",
                "fn Eq()\n{}\nextend Eq {\n    fn m(self) {}\n}\nfn main() {}\n",
                &[(Built, (1, 4)), (Brace, (2, 1))],
            ),
            (
                "optional bound, fn Eq",
                "fn Eq()\n{}\nfn g<T: Eq?>(x: T) {}\nfn main() {}\n",
                &[(Built, (1, 4)), (Brace, (2, 1))],
            ),
            (
                "array conformance, fn Eq",
                "fn Eq()\n{}\nclass C: [Eq] {}\nfn main() {}\n",
                &[(Built, (1, 4)), (Brace, (2, 1))],
            ),
        ];
        assert_rows(rows);
    }

    /// A broken `Result` poisons `Ok` and `Err`, which belong to the prelude's `Result`, unless a
    /// user's function or class takes the name.
    #[test]
    fn a_broken_result_should_poison_ok_and_err() {
        use Code::{BraceOnNewLine as Brace, BuiltInName as Built, Expected as Exp};
        let rows: &[(&str, &str, &[Found])] = &[
            (
                "class, call",
                "class Result\n{}\nfn f() -> Result<i64, string> {\n    Ok(1)\n}\nfn main() {}\n",
                &[(Built, (1, 7)), (Brace, (2, 1))],
            ),
            (
                "enum, call",
                "enum Result\n{}\nfn f() -> Result<i64, string> {\n    Ok(1)\n}\nfn main() {}\n",
                &[(Built, (1, 6)), (Brace, (2, 1))],
            ),
            (
                "interface, call",
                "interface Result\n{}\nfn f() -> Result<i64, string> {\n    Ok(1)\n}\nfn main() {}\n",
                &[(Built, (1, 11)), (Brace, (2, 1))],
            ),
            (
                "fn, call",
                "fn Result()\n{}\nfn f() -> Result<i64, string> {\n    Ok(1)\n}\nfn main() {}\n",
                &[(Built, (1, 4)), (Brace, (2, 1))],
            ),
            (
                "constant, call",
                "const Result = 1 2\nfn f() -> Result<i64, string> {\n    Ok(1)\n}\nfn main() {}\n",
                &[(Built, (1, 7)), (Exp, (1, 18))],
            ),
            (
                "alias, call",
                "type Result = 1\nfn f() -> Result<i64, string> {\n    Ok(1)\n}\nfn main() {}\n",
                &[(Built, (1, 6)), (Exp, (1, 15))],
            ),
            (
                "Err with no annotation",
                "class Result\n{}\nfn main() { let x = Err(\"e\")\n    print(1) }\n",
                &[(Built, (1, 7)), (Brace, (2, 1))],
            ),
            (
                "read of Ok",
                "class Result\n{}\nfn main() { let x = Ok }\n",
                &[(Built, (1, 7)), (Brace, (2, 1))],
            ),
            (
                "if let",
                "class Result\n{}\nfn main() { if let Ok(x) = f() { print(x) } }\nfn f() -> Result<i64, string> { Err(\"e\") }\n",
                &[(Built, (1, 7)), (Brace, (2, 1))],
            ),
            (
                "generic argument",
                "class Result\n{}\nfn id<T>(x: T) -> T { x }\nfn main() { let a = id(Ok(1)) }\n",
                &[(Built, (1, 7)), (Brace, (2, 1))],
            ),
            (
                "path",
                "class Result\n{}\nfn main() { let x: Result<i64, string> = Result.Ok(1)\n    print(1) }\n",
                &[(Built, (1, 7)), (Brace, (2, 1))],
            ),
            (
                "call of Result",
                "class Result\n{}\nfn main() { Result(1) }\n",
                &[(Built, (1, 7)), (Brace, (2, 1))],
            ),
            (
                "assign to Ok",
                "class Result\n{}\nfn main() { Ok = 1 }\n",
                &[(Built, (1, 7)), (Brace, (2, 1))],
            ),
            (
                "a user's constant Ok, call",
                "class Result\n{}\nconst Ok = 1\nfn main() { let a = Ok(1) }\n",
                &[(Built, (1, 7)), (Brace, (2, 1)), (Built, (3, 7))],
            ),
        ];
        assert_rows(rows);
    }

    /// Names that nothing poisons keep their errors, and a poisoned `Ok`, `Err` or type name
    /// stays apart from a user's own function or class of that name.
    #[test]
    #[expect(clippy::too_many_lines, reason = "one table of rows")]
    fn a_use_that_nothing_poisons_should_keep_its_error() {
        use Code::{
            AssignToFunction as Assign, BindingNotFound as NoBind, BraceOnNewLine as Brace,
            BuiltInName as Built, CannotInfer as Infer, DefinedTwice as Twice,
            InterfaceAsType as IfaceType, Mismatch, NotAnInterface as NotIface,
            PrintArgument as Print, TypeAsValue as AsValue, UnknownType as NoType,
            VariantForm as Variant,
        };
        let rows: &[(&str, &str, &[Found])] = &[
            (
                "built-in, too many arguments",
                "fn main() { print(1, 2) }\n",
                &[(Print, (1, 13))],
            ),
            (
                "built-in read",
                "fn main() { let p = print }\n",
                &[(AsValue, (1, 21))],
            ),
            (
                "built-in assigned",
                "fn main() { print = 1 }\n",
                &[(Assign, (1, 13))],
            ),
            (
                "unknown name assigned",
                "fn main() { zz = 1 }\n",
                &[(NoBind, (1, 13))],
            ),
            (
                "intact fn read",
                "fn f(x: i64) {}\nfn main() { let p = f }\n",
                &[(AsValue, (2, 21))],
            ),
            (
                "intact class assigned",
                "class C {}\nfn main() { C = 1 }\n",
                &[(AsValue, (2, 13))],
            ),
            (
                "intact enum called",
                "enum E { A }\nfn main() { E(1) }\n",
                &[(AsValue, (2, 13))],
            ),
            (
                "duplicate class read: the first stays",
                "class C {}\nclass C {}\nfn main() { let p = C }\n",
                &[(Twice, (2, 7)), (AsValue, (3, 21))],
            ),
            (
                "duplicate fn read",
                "fn f() {}\nfn f() {}\nfn main() { let p = f }\n",
                &[(Twice, (2, 4)), (AsValue, (3, 21))],
            ),
            (
                "duplicate enum called",
                "enum E { A }\nenum E { B }\nfn main() { E(1) }\n",
                &[(Twice, (2, 6)), (AsValue, (3, 13))],
            ),
            (
                "intact class Result, read of Ok",
                "class Result {}\nfn main() { let x = Ok }\n",
                &[(Built, (1, 7)), (Variant, (2, 21))],
            ),
            (
                "intact class Result, Err with no annotation",
                "class Result {}\nfn main() { let x = Err(\"e\")\n    print(1) }\n",
                &[(Built, (1, 7)), (Infer, (2, 21))],
            ),
            (
                "intact class Result, Ok stays checked",
                "class Result {}\nfn f() -> Result<i64, string> {\n    Ok(1)\n}\nfn main() {}\n",
                &[(Built, (1, 7))],
            ),
            // `Some` does not belong to the `Ok` and `Err` part of the rule.
            (
                "broken Option, read of Some",
                "class Option\n{}\nfn main() { let a = Some }\n",
                &[(Built, (1, 7)), (Brace, (2, 1)), (Variant, (3, 21))],
            ),
            (
                "a language interface is not poisoned, read",
                "interface Eq\n{}\nfn main() { let p = Eq }\n",
                &[(Built, (1, 11)), (Brace, (2, 1)), (AsValue, (3, 21))],
            ),
            (
                "a language interface is not poisoned, call",
                "interface Eq\n{}\nfn main() { Eq() }\n",
                &[(Built, (1, 11)), (Brace, (2, 1)), (AsValue, (3, 13))],
            ),
            (
                "a broken extend does not poison Ok",
                "extend Result\n{}\nfn f() -> Result<i64, string> { Ok(1) }\nfn main() {}\n",
                &[(Brace, (2, 1))],
            ),
            (
                "method with type arguments on an intact class",
                "class C {}\nfn main() { C.go<i64>(1) }\n",
                &[(AsValue, (2, 13))],
            ),
            (
                "a user's fn Ok stays checked after a broken Result",
                "class Result\n{}\nfn Ok(x: i64) {}\nfn main() { Ok(\"s\") }\n",
                &[
                    (Built, (1, 7)),
                    (Brace, (2, 1)),
                    (Built, (3, 4)),
                    (Mismatch, (4, 16)),
                ],
            ),
            (
                "a user's class Ok stays checked after a broken Result",
                "class Result\n{}\nclass Ok { x: i64 }\nfn main() { let a: string = Ok(x: 1).x\n    print(1) }\n",
                &[
                    (Built, (1, 7)),
                    (Brace, (2, 1)),
                    (Built, (3, 7)),
                    (Mismatch, (4, 29)),
                ],
            ),
            (
                "Ok in a type position keeps its error after a broken Result",
                "class Result\n{}\nfn g(x: Ok) {}\nfn main() {}\n",
                &[(Built, (1, 7)), (Brace, (2, 1)), (NoType, (3, 9))],
            ),
            (
                "unknown type",
                "fn g(x: Nope) {}\nfn main() {}\n",
                &[(NoType, (1, 9))],
            ),
            (
                "intact fn in a type position",
                "fn f() {}\nfn g(x: f) {}\nfn main() {}\n",
                &[(NoType, (2, 9))],
            ),
            (
                "an interface is no type",
                "fn g(x: Eq) {}\nfn main() {}\n",
                &[(IfaceType, (1, 9))],
            ),
            (
                "a language interface is not poisoned, type position",
                "interface Eq\n{}\nfn g(x: Eq) {}\nfn main() {}\n",
                &[(Built, (1, 11)), (Brace, (2, 1)), (IfaceType, (3, 9))],
            ),
            (
                "an intact fn Eq is not poisoned",
                "fn Eq() {}\nfn g(x: Eq) {}\nfn main() {}\n",
                &[(Built, (1, 4)), (IfaceType, (2, 9))],
            ),
            (
                "extend of an interface",
                "extend Eq {\n    fn m(self) {}\n}\nfn main() {}\n",
                &[(IfaceType, (1, 8))],
            ),
            (
                "optional bound",
                "fn g<T: Eq?>(x: T) {}\nfn main() {}\n",
                &[(NotIface, (1, 9))],
            ),
            (
                "a language interface is not poisoned, extend",
                "interface Eq\n{}\nextend Eq {\n    fn m(self) {}\n}\nfn main() {}\n",
                &[(Built, (1, 11)), (Brace, (2, 1)), (IfaceType, (3, 8))],
            ),
            (
                "a language interface is not poisoned, optional bound",
                "interface Eq\n{}\nfn g<T: Eq?>(x: T) {}\nfn main() {}\n",
                &[(Built, (1, 11)), (Brace, (2, 1)), (NotIface, (3, 9))],
            ),
        ];
        assert_rows(rows);
    }

    /// The prelude's code reads a language interface by its name, so a bound on that name stays
    /// the interface after a broken item of the name.
    #[test]
    fn a_language_interface_should_stay_a_bound_after_a_broken_item_of_its_name() {
        use Code::{
            BraceOnNewLine as Brace, BuiltInName as Built, CannotBeAny as CannotAny,
            ConformanceBody as ConfBody,
        };
        let rows: &[(&str, &str, &[Found])] = &[
            (
                "conformance",
                "fn Eq()\n{}\nclass C: Eq {}\nfn main() {}\n",
                &[(Built, (1, 4)), (Brace, (2, 1)), (ConfBody, (3, 10))],
            ),
            (
                "any",
                "fn Eq()\n{}\nfn g(x: any Eq) {}\nfn main() {}\n",
                &[(Built, (1, 4)), (Brace, (2, 1)), (CannotAny, (3, 13))],
            ),
        ];
        assert_rows(rows);
    }

    /// A broken `fn` or `const` comes first, then an intact item of a type's kind, then a use as
    /// a type. The intact item is the duplicate, and the type gives no second error.
    #[test]
    fn a_type_after_a_broken_item_of_its_name_should_give_no_second_error() {
        use Code::{
            BraceOnNewLine as Brace, BuiltInName as Built, DefinedTwice as Twice, Expected as Exp,
        };
        let rows: &[(&str, &str, &[Found])] = &[
            (
                "fn, class",
                "fn A()\n{}\nclass A {}\nfn g(x: A) {}\nfn main() {}\n",
                &[(Brace, (2, 1)), (Twice, (3, 7))],
            ),
            (
                "fn, enum",
                "fn A()\n{}\nenum A { X }\nfn g(x: A) {}\nfn main() {}\n",
                &[(Brace, (2, 1)), (Twice, (3, 6))],
            ),
            (
                "fn, alias",
                "fn A()\n{}\ntype A = i64\nfn g(x: A) {}\nfn main() {}\n",
                &[(Brace, (2, 1)), (Twice, (3, 6))],
            ),
            (
                "fn, interface, bound",
                "fn A()\n{}\ninterface A {}\nfn g<T: A>(x: T) {}\nfn main() {}\n",
                &[(Brace, (2, 1)), (Twice, (3, 11))],
            ),
            (
                "fn, class, extend",
                "fn A()\n{}\nclass A {}\nextend A {\n    fn m(self) {}\n}\nfn main() {}\n",
                &[(Brace, (2, 1)), (Twice, (3, 7))],
            ),
            (
                "constant, class",
                "const A = 1 2\nclass A {}\nfn g(x: A) {}\nfn main() {}\n",
                &[(Exp, (1, 13)), (Twice, (2, 7))],
            ),
            (
                "Eq, fn, class",
                "fn Eq()\n{}\nclass Eq {}\nfn g(x: Eq) {}\nfn main() {}\n",
                &[
                    (Built, (1, 4)),
                    (Brace, (2, 1)),
                    (Built, (3, 7)),
                    (Twice, (3, 7)),
                ],
            ),
            (
                "Eq, fn, enum",
                "fn Eq()\n{}\nenum Eq { X }\nfn g(x: Eq) {}\nfn main() {}\n",
                &[
                    (Built, (1, 4)),
                    (Brace, (2, 1)),
                    (Built, (3, 6)),
                    (Twice, (3, 6)),
                ],
            ),
            (
                "Eq, fn, alias",
                "fn Eq()\n{}\ntype Eq = i64\nfn g(x: Eq) {}\nfn main() {}\n",
                &[
                    (Built, (1, 4)),
                    (Brace, (2, 1)),
                    (Built, (3, 6)),
                    (Twice, (3, 6)),
                ],
            ),
            (
                "Eq, constant, enum",
                "const Eq = 1 2\nenum Eq { X }\nfn g(x: Eq) {}\nfn main() {}\n",
                &[
                    (Built, (1, 7)),
                    (Exp, (1, 14)),
                    (Built, (2, 6)),
                    (Twice, (2, 6)),
                ],
            ),
            (
                "Ord, constant, class",
                "const Ord = 1 2\nclass Ord {}\nfn g(x: Ord) {}\nfn main() {}\n",
                &[
                    (Built, (1, 7)),
                    (Exp, (1, 15)),
                    (Built, (2, 7)),
                    (Twice, (2, 7)),
                ],
            ),
        ];
        assert_rows(rows);
    }

    /// A constant runs to the end of its line, so text skipped after its value is its own: the
    /// constant is broken, and the items after it are still checked.
    #[test]
    fn skipped_text_after_a_constant_lies_in_it() {
        for (skipped, code) in [
            ("$", Code::UnexpectedCharacter),
            ("\"const", Code::LiteralNotClosed),
        ] {
            let src = format!(
                "const A = {{ 1 }} {skipped}\nconst B: u8 = 300\nfn main() {{\n    print(A)\n    let b: bool = 1\n}}\n"
            );
            assert_eq!(
                codes(&src),
                [
                    (code, (1, 17)),
                    (Code::IntegerDoesNotFit, (2, 15)),
                    (Code::Mismatch, (5, 19))
                ],
                "{src}"
            );
        }
    }

    /// A broken constant is declared, so its uses are poisoned; it poisons a function of the same
    /// name, and a broken function poisons a constant of the same name. Each pair is a duplicate.
    #[test]
    fn a_broken_constant_should_poison_its_uses_and_get_the_duplicate_check() {
        use Code::{DefinedTwice as Twice, Expected};
        for (src, want) in [
            (
                "const A = 1 2\nfn main() {\n    let b: bool = A\n    A = 2\n}\n",
                &[(Expected, (1, 13))][..],
            ),
            (
                "const A = 1 2\nfn A() {}\nfn main() {\n    A(1)\n}\n",
                &[(Expected, (1, 13)), (Twice, (2, 4))],
            ),
            (
                "fn A(x) {}\nconst A = 1\nfn main() {\n    let b: bool = A\n    A = 2\n}\n",
                &[(Expected, (1, 7)), (Twice, (2, 7))],
            ),
        ] {
            assert_eq!(codes(src), want, "{src}");
        }
        // A line that starts with `const` ends the unnamed item before it.
        let src = "def f()\nconst A = 1 2\nfn main() {}\n";
        assert_eq!(codes(src), [(Expected, (1, 1)), (Expected, (2, 13))]);
    }

    /// The fix of a `const` statement leaves a program that passes the whole front end.
    #[test]
    fn const_statement_fix_passes_the_front_end() {
        let src = "fn main() {\n    const n = 1\n    print(n)\n}\n";
        let ds = front_end(src).1;
        let [d] = ds.as_slice() else {
            panic!("expected one diagnostic, got {ds:?}");
        };
        assert_eq!(d.code(), Code::ConstInFunction);
        assert!(fix_edits(d).is_some_and(|(safe, _)| safe));
        let fixed = apply_fix(src, d);
        assert!(front_end(&fixed).1.is_empty(), "{fixed}");
    }

    /// The fix of a `}` in a string leaves a program that passes the whole front end, after an
    /// escaped backslash, after an interpolation and in a string inside one.
    #[test]
    fn brace_fix_should_pass_the_front_end() {
        for text in [r#""}""#, r#""\\}""#, r#""{n}}""#, r#""{ "}" }""#] {
            let src = format!("fn main() {{\n    let n = 1\n    print(n)\n    print({text})\n}}\n");
            let ds = front_end(&src).1;
            let [d] = ds.as_slice() else {
                panic!("expected one diagnostic, got {ds:?}");
            };
            assert_eq!(d.code(), Code::BraceInString, "{src}");
            assert!(fix_edits(d).is_some_and(|(safe, _)| safe));
            let fixed = apply_fix(&src, d);
            assert!(front_end(&fixed).1.is_empty(), "{fixed}");
        }
    }

    /// A bracket left open in an interpolation that is not closed does not stop the lexer, so a
    /// later function is still checked.
    #[test]
    fn unclosed_interpolation_keeps_later_items() {
        let src = "fn main() {\n    let s = \"a{ f(x\n}\nfn g() {\n    let b: bool = 1\n}\n";
        assert_eq!(
            codes(src),
            [(Code::LiteralNotClosed, (2, 13)), (Code::Mismatch, (5, 19))]
        );
    }

    /// A literal cut at its line end leaves one token, whichever segment the cut ends: its
    /// first, one resumed after a `}`, or a literal nested in an interpolation. So the error in
    /// the literal is reported, its function ends at its `}`, and the next function is checked.
    #[test]
    fn cut_literal_leaves_one_token() {
        for (literal, escape) in [
            ("\"a\\q", 15),
            ("\"a{x}b\\q", 19),
            ("\"a{x}b{y}c\\q", 23),
            ("\"a{x}b\\q{y", 19),
            ("\"a{ \"b\\q", 19),
            ("\"a{ \"b{x}c\\q", 23),
        ] {
            let src = format!(
                "fn main() {{\n    let s = {literal}\n}}\nfn g() {{\n    let b: bool = 1\n}}\n"
            );
            assert_eq!(
                codes(&src),
                [
                    (Code::UnknownEscape, (2, escape)),
                    (Code::Mismatch, (5, 19))
                ],
                "{literal}"
            );
        }
    }

    #[test]
    fn unnamed_item_stops_the_checker() {
        let src = "def total() {}\nfn main() {\n    let b: bool = 1\n}\n";
        assert_eq!(codes(src), [(Code::Expected, (1, 1))]);
    }

    /// A string that is not closed outside every item is an unnamed item, so the checker does
    /// not run: the string may have swallowed an item, and no name lookup can be trusted.
    #[test]
    fn unclosed_string_outside_items_stops_the_checker() {
        for (src, at) in [
            ("\"fn helper() {}\nfn main() {\n    helper()\n}\n", (1, 1)),
            (
                "\"class Helper {}\nfn helper() -> Helper {\n    return Helper()\n}\nfn main() {}\n",
                (1, 1),
            ),
            ("\"fn main() {}\n", (1, 1)),
            ("\"const A = 1\nfn main() {\n    print(A)\n}\n", (1, 1)),
            (
                "fn f() {} \"a note\nfn main() {\n    let b: bool = 1\n}\n",
                (1, 11),
            ),
        ] {
            assert_eq!(codes(src), [(Code::LiteralNotClosed, at)], "{src}");
        }
        // Skipped text keeps the checker on.
        let src = "fn f() {} $\nfn main() {\n    let b: bool = 1\n}\n";
        assert_eq!(
            codes(src),
            [
                (Code::UnexpectedCharacter, (1, 11)),
                (Code::Mismatch, (3, 19))
            ]
        );
    }

    /// Text skipped at the start of a top-level line lies in the unnamed segment after it, whose
    /// one error is the skip.
    #[test]
    fn skip_before_an_unnamed_segment_is_its_error() {
        for (src, line) in [
            (
                "# total of the list\nfn main() {\n    let b: bool = 1\n}\n",
                1,
            ),
            ("@decorator\nfn main() {}\n", 1),
            ("#[test]\nfn main() {}\n", 1),
            ("fn main() {}\n# the end\n", 2),
        ] {
            assert_eq!(
                codes(src),
                [(Code::UnexpectedCharacter, (line, 1))],
                "{src}"
            );
        }
    }

    /// A `;` after an item's `}` is redundant, as it is in a block, and the checker still runs.
    #[test]
    fn semicolon_after_an_item() {
        let src = "class C {\n    x: i64\n};\nfn main() {\n    let b: bool = 1\n}\n";
        assert_eq!(
            codes(src),
            [
                (Code::RedundantSemicolon, (3, 2)),
                (Code::Mismatch, (5, 19))
            ]
        );
        let d = &front_end(src).1[0];
        assert!(fix_edits(d).is_some_and(|(safe, _)| safe));
        let fixed = apply_fix(src, d);
        assert_eq!(codes(&fixed), [(Code::Mismatch, (5, 19))]);
        // So is one before the first item.
        assert_eq!(
            codes(";\nfn main() {}\n"),
            [(Code::RedundantSemicolon, (1, 1))]
        );
    }

    /// `{ x; }` gives `x`: with no value, the checker would report a mismatch with `i64`.
    #[test]
    fn block_value_survives_a_semicolon() {
        let src = "fn f() -> i64 { let x = 1; let y = { x; }; y }\nfn main() { print(f()) }\n";
        assert_eq!(codes(src), [(Code::RedundantSemicolon, (1, 39))]);
    }

    /// The warning survives the item it stands in, which is broken.
    #[test]
    fn redundant_semicolon_in_a_broken_item() {
        assert_eq!(
            codes("fn helper();\nfn main() {}\n"),
            [
                (Code::RedundantSemicolon, (1, 12)),
                (Code::Expected, (1, 13))
            ]
        );
    }

    /// On clean syntax, a W0007 fix is not judged, so it does not count against `JUDGED`. A source
    /// with a type error still has no syntax error to judge a fix against.
    #[test]
    fn sound_fixes_should_keep_every_redundant_semicolon_fix_on_clean_syntax() {
        let clean = format!("fn main() {{\n{}}}\n", "    print(1);\n".repeat(150));
        let typed = format!(
            "fn main() {{\n{}    let z: string = 1\n}}\n",
            "    print(1);\n".repeat(150)
        );
        for src in [clean, typed] {
            let shown = sound_fixes(&src, &front_end(&src).1);
            let semicolons: Vec<_> = (shown.iter())
                .filter(|d| d.code() == Code::RedundantSemicolon)
                .collect();
            assert_eq!(semicolons.len(), 150, "{src}");
            for d in semicolons {
                assert!(fix_edits(d).is_some_and(|(safe, _)| safe), "{d:?}");
            }
        }
    }

    /// Applying one W0007 fix alone leaves every other diagnostic of the lexer and of the front
    /// end where it was, up to the length the fix removes. The lexer's list catches what the front
    /// end hides when an item is broken.
    #[test]
    fn redundant_semicolon_fix_should_leave_other_diagnostics_unchanged() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/diagnostics");
        let mut sources: Vec<String> = [
            "fn main() {\n    let x = 1;\n    {\n    }\n}\n",
            "fn main() {\n    let x = 1\n    ; { }\n}\n",
            "fn main() {\n    if true\n    ; { }\n}\n",
            "fn main() {\n    let t = 1\n    ; - 2\n}\n",
            "fn f() -> i64 {\n;\n- 1\n}\nfn main() { print(f()) }\n",
            "fn helper();\nfn main() {}\n",
            "class _C {\n    x: i64;\n}\nfn main() {}\n",
            "fn main() {\n    if true { print(1) }; else { print(2) }\n}\n",
            "fn main() {\n    let x = 1 $\n    ; - 2\n}\n",
            "fn main() { return;else { } }\n",
            "class C;fn main() {}\n",
            "fn main() {\n    if c;\n    ; { }\n}\n",
        ]
        .map(String::from)
        .into();
        let before = sources.len();
        for entry in fs::read_dir(fixtures).expect("tests/diagnostics should be readable") {
            let path = entry.expect("an entry should be readable").path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if name.starts_with("W0007") && path.extension().is_some_and(|e| e == "sisu") {
                sources.push(fs::read_to_string(&path).expect("a fixture should be readable"));
            }
        }
        assert!(sources.len() > before, "no W0007 fixture");
        let lexed = |src: &str| -> Vec<(Code, usize)> {
            let lexed = lexer::lex(src).expect("the source should lex without a bracket error");
            starts(&lexed.diagnostics)
        };
        for src in &sources {
            let all = front_end(src).1;
            let mut fixed_any = false;
            for (i, d) in all.iter().enumerate() {
                if d.code() != Code::RedundantSemicolon {
                    continue;
                }
                let [fix] = d.fixes() else {
                    panic!("{src}: a W0007 should have one fix")
                };
                let [edit] = fix.edits() else {
                    panic!("{src}: a W0007 fix should have one edit")
                };
                let semicolon = d.span().start();
                let moved = |pairs: Vec<(Code, usize)>| -> Vec<(Code, usize)> {
                    (pairs.into_iter())
                        .map(|(c, at)| {
                            let at = if at > semicolon {
                                at - (edit.span().end() - edit.span().start())
                                    + edit.replacement().len()
                            } else {
                                at
                            };
                            (c, at)
                        })
                        .collect()
                };
                let result = fix.apply(src);
                assert_eq!(lexed(&result), moved(lexed(src)), "{result}");
                let mut others = all.clone();
                others.remove(i);
                assert_eq!(
                    starts(&front_end(&result).1),
                    moved(starts(&others)),
                    "{result}"
                );
                fixed_any = true;
            }
            assert!(fixed_any, "{src} has no W0007");
        }
    }

    /// The code and start of each of `ds`.
    fn starts(ds: &[Diagnostic]) -> Vec<(Code, usize)> {
        ds.iter().map(|d| (d.code(), d.span().start())).collect()
    }

    #[test]
    fn foreign_function_is_one_error() {
        let src = "def total(xs):\n    s = 0\n    for x in xs:\n        s += x\n    return s\nfn main() {}\n";
        assert_eq!(codes(src), [(Code::Expected, (1, 1))]);
    }

    #[test]
    fn broken_len_is_not_the_idiom() {
        let src = "fn len(xs: [i64]) -> i64 {\n    let = 1\n}\nfn main() {\n    let xs = [1, 2]\n    print(len(xs))\n}\n";
        assert_eq!(codes(src), [(Code::Expected, (2, 9))]);
    }

    #[test]
    fn damaged_name_stops_the_checker() {
        let src = "fn to$tal() {}\nfn main() {\n    total()\n}\n";
        assert_eq!(codes(src), [(Code::UnexpectedCharacter, (1, 6))]);
        let src = "fn $f() {}\nfn main() {\n    f()\n    let b: bool = 1\n}\n";
        assert_eq!(codes(src), [(Code::UnexpectedCharacter, (1, 4))]);
    }

    #[test]
    fn bracket_error_is_alone() {
        let src = "fn main() {\n    let b: bool = 1\n}\nfn f() { print(1 }\n";
        assert_eq!(codes(src), [(Code::UnexpectedClosingBracket, (4, 18))]);
    }

    #[test]
    fn broken_main_raises_no_main_error() {
        let src = "fn f() {}\nfn main() {\n    let = 1\n}\n";
        assert_eq!(codes(src), [(Code::Expected, (3, 9))]);
        let src = "fn f() {}\nfn main(x: i64) -> i64 {\n    let = 1\n}\n";
        assert_eq!(codes(src), [(Code::Expected, (3, 9))]);
    }

    #[test]
    fn broken_field_type_is_never_never_constructed() {
        let src = "class A {\n    b: B\n}\nclass B {\n    a A\n}\nfn main() {}\n";
        assert_eq!(codes(src), [(Code::Expected, (5, 7))]);
    }

    #[test]
    fn warnings_print_beside_syntax_errors() {
        let src = "fn f() {\n    let mut a = 1\n    let = 1\n}\nfn g() {\n    let mut b = 1\n    print(b)\n}\nfn main() {}\n";
        assert_eq!(
            codes(src),
            [(Code::Expected, (3, 9)), (Code::NeverReassigned, (6, 13))]
        );
    }

    #[test]
    fn broken_item_is_a_candidate() {
        let src = "fn total(n: i64) {\n    let = 1\n}\nfn main() {\n    totl(1)\n}\n";
        assert_eq!(
            codes(src),
            [(Code::Expected, (2, 9)), (Code::FunctionNotFound, (5, 5))]
        );
        let (_, diagnostics) = front_end(src);
        assert_eq!(
            fix_edits(&diagnostics[1]),
            Some((false, vec![(49, 53, "total".to_string())]))
        );
    }

    #[test]
    fn constant_with_dangling_operator_should_not_swallow_the_next_function() {
        let src = "const A = 1 +\nfn main() {\n    let x: i64 = \"s\"\n}\n";
        assert_eq!(
            codes(src),
            [(Code::Expected, (2, 1)), (Code::Mismatch, (3, 18))]
        );
    }

    #[test]
    fn header_with_dangling_arrow_should_end_at_the_next_function() {
        let src = "fn f() ->\nfn main() {\n    let x: i64 = \"s\"\n}\n";
        assert_eq!(
            codes(src),
            [(Code::Expected, (2, 1)), (Code::Mismatch, (3, 18))]
        );
    }

    #[test]
    fn method_with_dangling_arrow_should_not_swallow_the_next_method() {
        let src = "class C {\n    fn a() ->\n    fn b(self) {}\n}\nfn main() {\n    let c = C()\n    c.b(1)\n}\n";
        assert_eq!(
            codes(src),
            [(Code::Expected, (3, 5)), (Code::ArgumentCount, (7, 5))]
        );
    }

    #[test]
    fn line_break_ends_an_item_without_a_body() {
        assert_eq!(codes("fn f()\nfn main() {}\n"), [(Code::Expected, (1, 7))]);
        assert_eq!(
            codes("fn f() $\nfn main() {}\n"),
            [(Code::UnexpectedCharacter, (1, 8))]
        );
    }

    #[test]
    fn unnamed_item_runs_to_the_next_line_item() {
        let src = "def total() {}\nfn main() {\n    let = 1\n}\n";
        assert_eq!(
            codes(src),
            [(Code::Expected, (1, 1)), (Code::Expected, (3, 9))]
        );
        // `fn` within a line does not start an item.
        let src = "export fn main() {\n    let = 1\n}\n";
        assert_eq!(codes(src), [(Code::Expected, (1, 1))]);
        assert_eq!(
            codes("fn main() {}\nstruct P {}\n"),
            [(Code::Expected, (2, 1))]
        );
    }

    #[test]
    fn lexer_error_after_a_nested_block() {
        let src = "fn f() {\n    if true {\n    }\n    let n: i64 = $\n}\nfn main() {}\n";
        assert_eq!(codes(src), [(Code::UnexpectedCharacter, (4, 18))]);
    }

    #[test]
    fn lexer_error_breaks_only_its_method() {
        let src = "class C {\n    n: i64\n    fn m(self) {\n        let n: i64 = $\n    }\n}\nfn main() {\n    let c = C(n: true)\n}\n";
        assert_eq!(
            codes(src),
            [
                (Code::UnexpectedCharacter, (4, 22)),
                (Code::Mismatch, (8, 18))
            ]
        );
    }

    #[test]
    fn broken_enum_method_poisons_its_calls() {
        // The call reports nothing: the method's own error is reported already.
        // Read as a field, or named through the enum, it reports nothing either; the arguments
        // are still checked.
        let src = "enum E {\n    A\n    fn m(self) {\n        let = 1\n    }\n}\nfn main() {\n    E.A.m()\n    let _ = E.A.m\n    E.m(nope)\n}\n";
        assert_eq!(
            codes(src),
            [(Code::Expected, (4, 13)), (Code::BindingNotFound, (10, 9))]
        );
    }

    #[test]
    fn a_broken_function_should_poison_an_enum_path_of_its_name() {
        // The broken function `E` poisons the enum `E`: its paths report only their arguments,
        // and the enum is a duplicate.
        let src = "fn E() {\n    let = 1\n}\nenum E { A }\nfn main() {\n    let _ = E.A(nope)\n}\n";
        assert_eq!(
            codes(src),
            [
                (Code::Expected, (2, 9)),
                (Code::DefinedTwice, (4, 6)),
                (Code::BindingNotFound, (6, 17))
            ]
        );
    }

    #[test]
    fn broken_method_is_no_field_confusion() {
        let src = "class C {\n    n: i64\n    fn m(self) {\n        let = 1\n    }\n}\nfn main() {\n    let c = C(n: 1)\n    print(c.m)\n}\n";
        assert_eq!(codes(src), [(Code::Expected, (4, 13))]);
    }

    /// A broken `interface` header or `extend` block is a name for the checker to handle: it
    /// reports only the parse error and goes on, with no panic.
    #[test]
    fn a_broken_interface_or_extend_block_should_give_its_parse_error_only() {
        for (src, at) in [
            ("extend C: {}\nfn main() {}\n", (1, 11)),
            ("interface Shape: {}\nfn main() {}\n", (1, 18)),
        ] {
            assert_eq!(codes(src), [(Code::Expected, at)], "{src}");
        }
    }

    /// A broken `extend` block with a target that is a class or an enum poisons that type only for
    /// what the block could have given it: a call that finds no method. Every other use of the
    /// type, and every other item, is checked.
    #[test]
    fn broken_extend_should_poison_only_its_target() {
        let src = "class C {\n    x: i64\n}\nextend C: {}\nfn good() {\n    let b: bool = 1\n}\nfn main() {\n    let c = C(x: 1)\n    let y: bool = c.x\n    c.grow()\n}\n";
        assert_eq!(
            codes(src),
            [
                (Code::Expected, (4, 11)),
                (Code::Mismatch, (6, 19)),
                (Code::Mismatch, (10, 19))
            ]
        );
    }

    /// Comparing or ordering the target of a broken `extend` block gives the parse error alone: the
    /// target holds `Eq` and `Ord` only through the poison, so no code is made and no operator is
    /// reported, with no panic.
    #[test]
    fn comparing_a_broken_extend_target_should_give_the_parse_error_alone() {
        let src = "class C {\n    x: i64\n}\nextend C: {}\nfn same(a: C, b: C) -> bool {\n    a == b\n}\nfn main() {}\n";
        assert_eq!(codes(src), [(Code::Expected, (4, 11))]);
    }

    #[test]
    fn ordering_a_broken_extend_target_should_give_the_parse_error_alone() {
        let src = "class C {\n    x: i64\n}\nextend C: {}\nfn same(a: C, b: C) -> bool {\n    a < b\n}\nfn main() {}\n";
        assert_eq!(codes(src), [(Code::Expected, (4, 11))]);
    }

    /// A block with no name for its target stops the checker, as an unnamed broken item does.
    #[test]
    fn unnamed_broken_extend_should_stop_the_checker() {
        let src = "extend {}\nfn good() {\n    let b: bool = 1\n}\n";
        assert_eq!(codes(src), [(Code::Expected, (1, 8))]);
    }

    /// A method that a syntax error took from an intact block poisons a call of its name only,
    /// and is no missing requirement; a broken block's target conforms to every interface that a
    /// conformance of it needs.
    #[test]
    fn a_method_lost_to_a_syntax_error_should_poison_only_its_calls() {
        let src = "class C {\n    x: i64\n}\nextend C {\n    fn grow(self) {\n        let = 1\n    }\n}\nfn main() {\n    let c = C(x: 1)\n    c.grow()\n    c.shrink()\n}\n";
        assert_eq!(
            codes(src),
            [(Code::Expected, (6, 13)), (Code::NoMember, (12, 7))]
        );
        // Nothing else is reported, so the program is withheld for the lost method alone.
        let lost = [
            "class C {\n    x: i64\n}\nextend C {\n    fn grow(self) {\n        let = 1\n    }\n}\nfn main() {\n    C(x: 1).grow()\n}\n",
            "enum E {\n    A\n    fn grow(self) {\n        let = 1\n    }\n}\nfn main() {\n    E.A.grow()\n}\n",
        ];
        for (src, at) in lost.into_iter().zip([(6, 13), (4, 13)]) {
            assert_eq!(codes(src), [(Code::Expected, at)], "{src}");
        }
        // A lost method in a generic call's argument is a call of unknown shape.
        let generic = "fn take<T>(x: T, y: T) -> T { x }\n";
        for (src, at) in [
            (
                "class C {\n    x: i64\n    fn grow(self) {\n        let = 1\n    }\n}\nfn main() {\n    take(C(x: 1).grow(), 1)\n}\n",
                (4, 13),
            ),
            (
                "class C {\n    x: i64\n}\nextend C {\n    fn grow(self) {\n        let = 1\n    }\n}\nfn main() {\n    take(C(x: 1).grow(), 1)\n}\n",
                (6, 13),
            ),
        ] {
            assert_eq!(
                codes(&format!("{src}{generic}")),
                [(Code::Expected, at)],
                "{src}"
            );
        }
        let interfaces = "interface Named {\n    fn name(self) -> string\n}\ninterface Solid: Named {\n    fn volume(self) -> f64\n}\nclass C {\n    x: i64\n}\n";
        let src = format!(
            "{interfaces}extend C: Solid {{\n    fn volume(self) -> f64 {{ 1.0 }}\n}}\nextend C: Named {{\n    fn name(self) -> string {{\n        let = 1\n    }}\n}}\nfn main() {{}}\n"
        );
        assert_eq!(codes(&src), [(Code::Expected, (15, 13))]);
        let src = format!(
            "{interfaces}extend C: Solid {{\n    fn volume(self) -> f64 {{ 1.0 }}\n}}\nextend C: {{}}\nfn main() {{}}\n"
        );
        assert_eq!(codes(&src), [(Code::Expected, (13, 11))]);
    }

    /// A bound that the target of a broken `extend` block may have met holds, so the call of a
    /// function with that bound reports the parse error alone.
    #[test]
    fn a_bound_should_hold_for_the_target_of_a_broken_extend() {
        let src = "interface Same {\n    fn same(self, other: Self) -> bool\n}\nclass C {\n    x: i64\n}\nextend C: {}\nfn f<T: Same>(x: T) {}\nfn main() {\n    f(C(x: 1))\n}\n";
        assert_eq!(codes(src), [(Code::Expected, (7, 11))]);
    }

    /// A bound that the prelude writes has no label for the user: nothing in the prelude is
    /// theirs to read. The failure itself is reported at the user's code.
    #[test]
    fn a_bound_in_the_prelude_should_give_a_report_with_no_label_into_it() {
        let prelude = "interface Weigh {\n    fn weigh(self) -> i64\n}\nclass Scale<T: Weigh> {\n    item: T\n}\n";
        let src = "class Stone {}\nfn put(s: Scale<Stone>) {}\nfn main() {}\n";
        let (_, ds) = front_end_with(prelude, src);
        let [d] = &ds[..] else {
            panic!("one diagnostic expected: {ds:?}");
        };
        assert_eq!(d.code(), Code::NotConform);
        assert_eq!(line_col(src, d.span().start()), (2, 17));
        assert_eq!(d.secondary(), []);
    }

    /// A prelude may extend a built-in type: an array type or `i64`. A user's file may not.
    #[test]
    fn prelude_should_check_clean_when_it_extends_a_built_in_type() {
        let prelude = "extend i64 {}\nextend [i64] {}\nextend<T> [T] {}";
        let (program, diagnostics) = front_end_with(prelude, "fn main() {}");
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert!(program.is_some());
    }

    /// A built-in type has `Display` from the compiler, so a `to_string` in a prelude block that
    /// extends it is a plain method, and `print` does not call it.
    #[test]
    fn a_to_string_in_a_prelude_block_of_a_built_in_type_should_stay_a_plain_method() {
        for (row, block, src) in [
            (
                "i64",
                "extend i64 {\n    fn to_string(self) -> string { \"custom\" }\n}\n",
                "fn main() {\n    print(1)\n}",
            ),
            (
                "an array",
                "extend<T> [T] {\n    fn to_string(self) -> string { \"custom\" }\n}\n",
                "fn main() {\n    print([1])\n}",
            ),
        ] {
            let prelude = format!("{PRELUDE_SOURCE}\n{block}");
            let (program, diagnostics) = front_end_with(&prelude, src);
            assert!(diagnostics.is_empty(), "{row}: {diagnostics:?}");
            let program = program.unwrap_or_else(|| panic!("{row}: no program"));
            let names: Vec<&str> = program.functions.iter().map(|f| f.name.as_str()).collect();
            assert!(
                !names.iter().any(|n| n.ends_with("to_string")),
                "{row}: {names:?}"
            );
        }
    }

    /// A broken interface poisons its name for every use, as a broken class does, and so does a
    /// broken function of the name an intact interface has, which is a duplicate.
    #[test]
    fn a_broken_interface_should_poison_its_name() {
        for (src, want) in [
            (
                "interface Shape: {}\nfn f<T: Shape>(x: T, y: Shape) { Shape(1) }\nfn main() {}\n",
                &[(Code::Expected, (1, 18))][..],
            ),
            (
                "interface Shape {}\nfn Shape() {\n    let = 1\n}\nfn f(s: Shape) {}\nfn main() {}\n",
                &[(Code::DefinedTwice, (2, 4)), (Code::Expected, (3, 9))],
            ),
        ] {
            assert_eq!(codes(src), want, "{src}");
        }
    }

    /// A line break or a space before a `?` is no part of the type: the report covers the type
    /// alone and renders (the renderer rejects a span that ends with a line break).
    #[test]
    fn an_unsupported_type_should_be_reported_without_the_question_marks_after_it() {
        for (src, code, text) in [
            (
                "fn f(x: Self\n?) {}\nfn main() {}\n",
                Code::SelfOutside,
                "Self",
            ),
            (
                "class C {}\nfn f(x: C.Item\n?) {}\nfn main() {}\n",
                Code::UnknownType,
                "C.Item",
            ),
            (
                "fn f<T: Order ? ?>(x: T) {}\nfn main() {}\n",
                Code::UnknownType,
                "Order",
            ),
            // An optional bound is reported through its `?`, which is the error.
            (
                "fn f<T: [i64] ?>(x: T) {}\nfn main() {}\n",
                Code::NotAnInterface,
                "[i64] ?",
            ),
            (
                "fn f<T: [i64]\n?>(x: T) {}\nfn main() {}\n",
                Code::NotAnInterface,
                "[i64]\n?",
            ),
        ] {
            let diagnostics = front_end(src).1;
            let first = diagnostics.first().expect("the type should be reported");
            assert_eq!(first.code(), code, "{src}");
            assert_eq!(&src[first.span().range()], text, "{src}");
            first.render("t.sisu", src, &crate::source::LineIndex::new(src), false);
        }
    }

    #[test]
    fn a_broken_function_should_poison_the_type_of_its_name() {
        // Functions and classes share a namespace: the broken function `Tree` poisons the type,
        // and it is a duplicate.
        let src = "class Tree {\n    n: i64\n}\nfn Tree() {\n    let = 1\n}\nfn main() {\n    let t: Tree = 1\n}\n";
        assert_eq!(
            codes(src),
            [(Code::DefinedTwice, (4, 4)), (Code::Expected, (5, 9))]
        );
    }

    /// The names of `program`'s classes, enums and functions, in list order.
    fn names(program: &tir::Program) -> [Vec<&str>; 3] {
        [
            program.classes.iter().map(|c| c.name.as_str()).collect(),
            program.enums.iter().map(|e| e.name.as_str()).collect(),
            program.functions.iter().map(|f| f.name.as_str()).collect(),
        ]
    }

    /// With `Result`, `Ok` and `Err` unused, this also shows that the unused-names pass reports
    /// nothing in the prelude: W0012 and W0013 would be an internal error here.
    #[test]
    fn prelude_should_check_clean_with_no_diagnostic() {
        for (program, diagnostics) in [front_end("fn main() {}"), checked("fn main() {}")] {
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
            assert!(program.is_some());
        }
    }

    /// The lexer stops on `$`, which is no token.
    #[test]
    #[should_panic(expected = "internal compiler error: the prelude does not check clean")]
    fn prelude_should_be_an_internal_error_when_it_does_not_lex() {
        front_end_with("enum P { A } $", "fn main() {}");
    }

    /// `42` is no name. The parser quotes it from the prelude's text, so this also shows that
    /// it slices the text at the prelude's offsets.
    #[test]
    #[should_panic(expected = "internal compiler error: the prelude does not check clean")]
    fn prelude_should_be_an_internal_error_when_it_does_not_parse() {
        front_end_with("fn 42() {}", "fn main() {}");
    }

    /// The same variant twice is a checker error.
    #[test]
    #[should_panic(expected = "internal compiler error: the prelude does not check clean")]
    fn prelude_should_be_an_internal_error_when_it_does_not_check() {
        front_end_with("enum Result<T, E> { Ok(T)\n Ok(E) }", "fn main() {}");
    }

    /// Each row has one span in the prelude and the others in the user's text, in the place
    /// that the row names.
    #[test]
    fn diagnostic_should_point_into_the_prelude_by_any_of_its_spans() {
        let (user, prelude) = (
            Span::new(0, 1),
            Span::new(PRELUDE_BASE as usize, PRELUDE_BASE as usize + 1),
        );
        for (d, expected, place) in [
            (
                Diagnostic::for_test(prelude, &[user], &[(user, "")]),
                true,
                "the primary span",
            ),
            (
                Diagnostic::for_test(user, &[prelude], &[(user, "")]),
                true,
                "a label",
            ),
            (
                Diagnostic::for_test(user, &[user], &[(prelude, "")]),
                true,
                "an edit of a fix",
            ),
            (
                Diagnostic::for_test(user, &[user], &[(user, "")]),
                false,
                "none",
            ),
        ] {
            assert_eq!(points_into_prelude(&d), expected, "{place}");
        }
    }

    /// The prelude's items follow the user's in every list, so a user item keeps the id it has
    /// without a prelude, and `prelude` counts where the prelude's start. `helper` is unused, and
    /// `P`, `A` and `Q` are never built: none is reported, and `helper`'s name is not read as the
    /// user's text.
    #[test]
    fn prelude_items_should_follow_the_users_in_every_list() {
        let prelude = "enum P { A }\nclass Q {}\nfn helper() {}";
        let src = "enum E { A }\nclass C {}\nfn main() {\n    let _ = E.A\n    let _ = C()\n}";
        let (program, diagnostics) = checked_with(prelude, src);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let program = program.expect("the program should check");
        assert_eq!(
            names(&program),
            [vec!["C", "Q"], vec!["E", "P"], vec!["main", "helper"]]
        );
        let prelude = tir::ItemCounts {
            classes: 1,
            enums: 1,
            funcs: 1,
        };
        assert_eq!(program.prelude, prelude);
    }

    /// A user's own method counts in `prelude.funcs`: the prelude's first function comes after
    /// every method of a user's class and enum.
    #[test]
    fn prelude_should_start_after_the_methods_of_the_users_items() {
        let src =
            "class C {\n    fn m(self) {}\n}\nenum E {\n    A\n    fn n(self) {}\n}\nfn main() {}";
        let (program, _) = checked_with("fn helper() {}", src);
        let program = program.expect("the program should check");
        assert_eq!(program.prelude.funcs, 3);
        assert_eq!(program.functions[3].name, "helper");
    }

    /// `mono` drops what a program does not reach: with no type argument and no generic item of
    /// the user's, it cuts the prelude's items off, and `Result` makes no instance.
    #[test]
    fn mono_should_drop_the_prelude_when_a_program_instantiates_nothing() {
        let (program, diagnostics) = front_end("fn main() {}");
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let program = program.expect("the program should check");
        assert_eq!(names(&program), [vec![], vec![], vec!["main"]]);
    }

    /// A generic item of the user's that nothing calls records no type argument. The program
    /// takes the normal path, which drops the template too.
    #[test]
    fn mono_should_drop_an_uncalled_generic_function_of_the_user() {
        let (program, _) = front_end("fn id<T>(x: T) -> T { x }\nfn main() {}");
        let program = program.expect("the program should check");
        assert_eq!(names(&program), [vec![], vec![], vec!["main"]]);
    }

    /// `mono` roots the user's functions only. A function of the prelude joins the program when
    /// a body in it calls the function, and then so do the functions that it calls: `id` takes
    /// the path that instantiates, and no prelude function comes with it.
    #[test]
    fn mono_should_add_only_the_prelude_functions_that_a_call_reaches() {
        for (src, want) in [
            (
                "fn id<T>(x: T) -> T { x }\nfn main() {\n    print(id(1))\n}",
                vec!["main", "id<i64>"],
            ),
            (
                "fn main() {\n    print(5.min(3))\n}",
                vec!["main", "i64.min"],
            ),
            (
                "fn main() {\n    print(5.max(3))\n}",
                vec!["main", "i64.max"],
            ),
            // `clamp` calls `min` and `max`, in the order its body writes them.
            (
                "fn main() {\n    print(5.clamp(1, 3))\n}",
                vec!["main", "i64.clamp", "i64.min", "i64.max"],
            ),
        ] {
            let (program, diagnostics) = front_end(src);
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
            let program = program.expect("the program should check");
            assert_eq!(names(&program)[2], want, "{src}");
        }
    }

    /// A type argument that a type alone writes, with no call of a generic item, is enough to make
    /// the enum of the prelude an enum of the program.
    #[test]
    fn mono_should_instantiate_a_prelude_enum_that_only_a_type_argument_names() {
        let src = "fn main() {\n    let _: [Result<i64, string>] = []\n}";
        let (program, diagnostics) = front_end(src);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let program = program.expect("the program should check");
        assert_eq!(
            names(&program),
            [vec![], vec!["Result<i64, string>"], vec!["main"]]
        );
    }

    /// A written type argument is what makes `Result<i64, string>` an enum of the program.
    #[test]
    fn mono_should_instantiate_a_prelude_enum_that_a_type_names() {
        let src = "fn main() {\n    let _: Result<i64, string> = Result.Ok(1)\n}";
        let (program, diagnostics) = front_end(src);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let program = program.expect("the program should check");
        assert_eq!(
            names(&program),
            [vec![], vec!["Result<i64, string>"], vec!["main"]]
        );
    }

    /// The only type argument is inferred from a call: no angle brackets are written, and the
    /// user's items are not generic.
    #[test]
    fn mono_should_instantiate_a_prelude_function_that_a_call_infers() {
        let prelude = "fn first<T>(x: T) -> T { x }";
        let (program, diagnostics) = front_end_with(prelude, "fn main() {\n    print(first(1))\n}");
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let program = program.expect("the program should check");
        assert_eq!(
            names(&program),
            [vec![], vec![], vec!["main", "first<i64>"]]
        );
    }

    /// `Wrap<T.Item>` holds the associated type in an argument, and `Item` chooses the enum
    /// that holds it. The box goes on the field of the enum that is declared, not on the one
    /// inside `Wrap`. The instance whose `Item` is another enum, with no cycle, stays inline.
    #[test]
    fn mono_should_box_an_associated_type_field_that_names_its_enum() {
        let src = "interface I { type Item }\n\
            class W<T>: I { type Item = E<W<T>> }\n\
            enum L {\n    Z\n}\n\
            class Q: I { type Item = L }\n\
            enum Wrap<T> {\n    N\n    X(T)\n}\n\
            enum E<T: I> {\n    End\n    A(Wrap<T.Item>)\n}\n\
            fn main() {\n    let _w = W<i64>()\n    let _q = Q()\n    let _z: L = L.Z\n    let _k: E<Q> = E.End\n    let _n: Wrap<i64> = Wrap.N\n    let e: E<W<i64>> = E.End\n    let g: E<W<i64>> = E.A(Wrap.X(e))\n    print(g == e)\n}";
        let (program, diagnostics) = front_end(src);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let program = program.expect("the program should check");
        let field = |enum_name: &str, variant: &str| {
            let e = (program.enums.iter())
                .find(|e| e.name == enum_name)
                .unwrap_or_else(|| panic!("the program should hold the enum {enum_name}"));
            let v = (e.variants.iter())
                .find(|v| v.name == variant)
                .expect("the enum should hold the variant");
            (v.fields[0].boxed, v.fields[0].rank)
        };
        assert_eq!(field("E<W<i64>>", "A"), (true, tir::Rank::Assoc));
        assert_eq!(field("E<Q>", "A"), (false, tir::Rank::Assoc));
        assert_eq!(field("Wrap<E<W<i64>>>", "X"), (false, tir::Rank::Never));
    }
}
