//! The entry point of the front-end fuzz target: `sisuc --check` on one input, with checks that
//! every span it reports is well formed and every safe fix it shows keeps its promise. An input
//! that checks clean is also printed as a typed tree and compiled to LLVM IR, which LLVM must
//! verify. The bug signal is a panic.

use inkwell::context::Context;
use inkwell::module::Module;

use crate::codegen;
use crate::diagnostic::{Code, Diagnostic, Edit, Fix};
use crate::report::{Format, Reporter};
use crate::source::Span;
use crate::tir::Program;

/// Runs `sisuc --check` on `data` as the file `input.sisu`, reports it as colored text and then
/// as JSON, and asserts that every span of every diagnostic is well formed and that every safe
/// fix shown is safe. When the input checks clean, asserts that it compiles too. Returns the
/// text report followed by the JSON report.
///
/// The caller runs it on a thread with a stack of `STACK_SIZE`, as `main` runs `cli`: the
/// deepest program the parser accepts overflows a smaller stack.
pub fn front_end(data: &[u8]) -> String {
    let mut out = Vec::new();
    let mut text = Reporter::new(&mut out, Format::Text, true);
    let checked = crate::check_bytes(&mut text, "input.sisu", data.to_vec());
    text.finish();
    let mut json = Reporter::new(&mut out, Format::Json, false);
    json.diagnostics("input.sisu", &checked.source, &checked.diagnostics);
    json.finish();
    assert_spans(&checked.source, &checked.diagnostics);
    let shown = crate::front_end::sound_fixes(&checked.source, &checked.diagnostics);
    assert_fixes_keep_syntax(&checked.source, &shown);
    assert_safe_fixes(&checked.source, &shown);
    if let Some(program) = &checked.program {
        assert_compiles(program, &checked.source);
    }
    String::from_utf8(out).expect("the reports should be UTF-8, as the source they quote is")
}

/// Prints `program` as a typed tree, then compiles it, without and with `--rc-checks`, and asserts
/// that LLVM verifies each module. No pass runs and nothing links: `-O2` changes only the pipeline,
/// not the module codegen makes.
fn assert_compiles(program: &Program, source: &str) {
    // The printer's only check is that it does not panic.
    drop(program.to_string());
    let machine = codegen::target_machine(codegen::OptLevel::O0)
        .expect("LLVM should have a target machine for the host sisuc runs on");
    for rc_checks in [false, true] {
        let context = Context::create();
        let module = codegen::compile(&context, program, "input.sisu", source, &machine, rc_checks);
        assert_verified(&module, rc_checks);
    }
}

/// Asserts that LLVM verifies `module`, compiled with `rc_checks`.
fn assert_verified(module: &Module<'_>, rc_checks: bool) {
    if let Err(e) = module.verify() {
        panic!("LLVM rejects the module (rc_checks: {rc_checks}):\n{e}");
    }
    #[cfg(test)]
    VERIFIED.with(|n| {
        let mut counts = n.get();
        counts[usize::from(rc_checks)] += 1;
        n.set(counts);
    });
}

#[cfg(test)]
thread_local! {
    /// How many modules `assert_verified` has passed on this thread, without and with
    /// `--rc-checks`, so a test can see that `front_end` compiles a clean input both ways.
    static VERIFIED: std::cell::Cell<[usize; 2]> = const { std::cell::Cell::new([0; 2]) };
}

/// Asserts that every span of `diagnostics` (primary, secondary and each fix's edits) is a range
/// of `source` whose ends sit on char boundaries, and that no two edits of one fix overlap.
fn assert_spans(source: &str, diagnostics: &[Diagnostic]) {
    let len = source.len();
    for d in diagnostics {
        let code = d.code().as_str();
        let secondary = d.secondary().iter().map(|&(span, _)| span);
        let edits = d.fixes().iter().flat_map(Fix::edits).map(Edit::span);
        for span in std::iter::once(d.span()).chain(secondary).chain(edits) {
            assert!(
                span.start() <= span.end(),
                "{code}: span {span:?} starts after it ends, in a source of {len} bytes"
            );
            // `is_char_boundary` is false past the end, so this also rejects such a span.
            assert!(
                source.is_char_boundary(span.start()),
                "{code}: span {span:?} starts inside a char, in a source of {len} bytes"
            );
            assert!(
                source.is_char_boundary(span.end()),
                "{code}: span {span:?} ends inside a char, in a source of {len} bytes"
            );
        }
        for fix in d.fixes() {
            let edits = fix.edits();
            for (i, a) in edits.iter().enumerate() {
                for b in &edits[i + 1..] {
                    let (a, b) = (a.span(), b.span());
                    assert!(
                        !conflict(a, b),
                        "{code}: edits conflict: {a:?} then {b:?} of one fix, in a source of {len} bytes"
                    );
                }
            }
        }
    }
}

/// How many fixes `assert_safe_fixes` and `assert_fixes_keep_syntax` apply alone: each costs a
/// pass over the source, so a source of hundreds of W0007s would take minutes.
// shortcut: fixes past the first `ALONE` go unchecked; raise it if a bug hides behind one.
const ALONE: usize = 64;

/// Asserts that each safe fix of `diagnostics`, which the front end reports for `source`, keeps
/// ADR 0013's promise when applied alone: it deletes no comment, removes its diagnostic (none of
/// its code spans what it did), and keeps every diagnostic of another code, among those the
/// front end reports before the first lexer error, which stops the parser, and those the lexer
/// reports.
/// It may also remove other reports of its own code, as one `mut` resolves each assignment it
/// allowed, and it may reveal diagnostics it was hiding. Another diagnostic is kept when any
/// diagnostic lies within what it spanned, since the fixed source may let the front end name the
/// same problem more exactly, or reveal an error inside it that the checker stops at; otherwise
/// it is matched by code, since a fix may move or rename what one quotes.
fn assert_safe_fixes(source: &str, diagnostics: &[Diagnostic]) {
    let codes = |ds: &[Diagnostic]| ds.iter().map(Diagnostic::code).collect::<Vec<_>>();
    // Where the first lexer error starts: past it, what the parser reports depends on where it
    // stopped, which a fix may change, so only the lexer's own reports are compared there.
    let first = |ds: &[Diagnostic]| ds.iter().map(|e| e.span().start()).min();
    let (lexer_diagnostics, comments) = lexed(source);
    let stop = first(&lexer_diagnostics).unwrap_or(usize::MAX);
    let safe = diagnostics
        .iter()
        .flat_map(|d| d.fixes().iter().filter(|f| f.safe()).map(move |f| (d, f)));
    for (d, fix) in safe.take(ALONE) {
        let code = d.code();
        for edit in fix.edits() {
            let span = edit.span();
            assert!(
                !comments.iter().any(|c| span.range().contains(c)),
                "{code:?}: a safe fix deletes a comment in {span:?}"
            );
        }
        let fixed = fix.apply(source);
        let after = crate::front_end::front_end(&fixed).1;
        // Where a span of `source` would be in `fixed`: the edits before its ends move it.
        let moved = |at: usize| {
            let before = fix.edits().iter().filter(|e| e.span().end() <= at);
            let (added, removed) = before.fold((0, 0), |(added, removed), e| {
                (
                    added + e.replacement().len(),
                    removed + e.span().range().len(),
                )
            });
            at + added - removed
        };
        let moved_span = |s: Span| (moved(s.start()), moved(s.end()));
        // A diagnostic within `e`'s span reports the same problem, or one that stops the
        // checker before it reaches `e`'s.
        let spans_it = |e: &Diagnostic| {
            let (start, end) = moved_span(e.span());
            after
                .iter()
                .any(|a| start <= a.span().start() && a.span().end() <= end)
        };
        let (fixed_lexer_diagnostics, _) = lexed(&fixed);
        let fixed_stop = first(&fixed_lexer_diagnostics).unwrap_or(usize::MAX);
        let before_stops =
            |e: &Diagnostic| e.span().start() < stop && moved(e.span().start()) < fixed_stop;
        let unplaced: Vec<Code> = diagnostics
            .iter()
            .filter(|e| before_stops(e) && !spans_it(e))
            .map(Diagnostic::code)
            .collect();
        keeps_others(code, &unplaced, &mut codes(&after), &fixed);
        keeps_others(
            code,
            &codes(&lexer_diagnostics),
            &mut codes(&fixed_lexer_diagnostics),
            &fixed,
        );
        assert!(
            !after
                .iter()
                .any(|e| e.code() == code
                    && (e.span().start(), e.span().end()) == moved_span(d.span())),
            "{code:?}: a safe fix leaves its diagnostic:\n{fixed}"
        );
    }
}

/// Asserts that no fix shown leaves `source` lexing or parsing worse than it does:
/// `sound_fixes` drops such a fix of a lexer or parser diagnostic, but shows the checker's
/// unjudged.
fn assert_fixes_keep_syntax(source: &str, diagnostics: &[Diagnostic]) {
    let before = crate::front_end::syntax_errors(source);
    let shown = diagnostics
        .iter()
        .flat_map(|d| d.fixes().iter().map(move |f| (d, f)));
    for (d, fix) in shown.take(ALONE) {
        let fixed = fix.apply(source);
        assert!(
            crate::front_end::syntax_errors(&fixed) <= before,
            "{:?}: a fix breaks the syntax:\n{fixed}",
            d.code()
        );
    }
}

/// The diagnostics the lexer reports for `source`, and where each comment starts.
fn lexed(source: &str) -> (Vec<Diagnostic>, Vec<usize>) {
    match crate::lexer::lex(source) {
        Ok(lexed) => (lexed.diagnostics, lexed.comments),
        Err(d) => (vec![d], Vec::new()),
    }
}

/// Takes from `after` one report of each code in `before` other than `code`, the code of the
/// safe fix that turned the source into `fixed`; panics when one is missing.
fn keeps_others(code: Code, before: &[Code], after: &mut Vec<Code>, fixed: &str) {
    for &other in before.iter().filter(|&&c| c != code) {
        let Some(at) = after.iter().position(|&c| c == other) else {
            panic!("{code:?}: a safe fix removes {other:?} too:\n{fixed}");
        };
        after.swap_remove(at);
    }
}

/// Edit `a`, listed before edit `b` in one fix, conflicts with it: they overlap, or `b` inserts at
/// the start of a non-empty `a`, where `Fix::apply` would let `a` replace the insertion.
fn conflict(a: Span, b: Span) -> bool {
    (a.start() < b.end() && b.start() < a.end())
        || (b.start() == b.end() && b.start() == a.start() && a.start() < a.end())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::thread;

    use inkwell::context::Context;

    use super::{
        VERIFIED, assert_fixes_keep_syntax, assert_safe_fixes, assert_spans, assert_verified,
        front_end,
    };
    use crate::diagnostic::{Code, Diagnostic};
    use crate::source::Span;

    /// `front_end` on a thread with a `STACK_SIZE` stack, as its contract asks.
    fn on_stack(data: &[u8]) -> String {
        thread::scope(|scope| {
            thread::Builder::new()
                .stack_size(crate::STACK_SIZE)
                .spawn_scoped(scope, || front_end(data))
                .expect("the OS should start a thread with a 64 MiB stack")
                .join()
                .expect("front_end should not panic")
        })
    }

    /// Every `.sisu` file under `dir`, recursively.
    fn seeds(dir: &Path, found: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("the tests directory should be readable") {
            let path = entry.expect("a directory entry should be readable").path();
            if path.is_dir() {
                seeds(&path, found);
            } else if path.extension().is_some_and(|e| e == "sisu") {
                found.push(path);
            }
        }
    }

    /// The `0` that fixes `.9223372036854775809` leaves every lexer report of another code: the
    /// lexer reports none for digits after a `.`.
    #[test]
    fn a_leading_dot_float_fix_keeps_the_lexer_reports() {
        on_stack(b"fn main() {\n    let x = .9223372036854775809\n}\n");
    }

    #[test]
    fn every_seed_passes() {
        let mut found = Vec::new();
        seeds(
            Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests")),
            &mut found,
        );
        assert!(found.len() > 300, "{} seeds", found.len());
        for path in found {
            on_stack(&fs::read(&path).expect("a seed should be readable"));
        }
    }

    #[test]
    fn reports_in_color_text_then_json() {
        let seed = fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/diagnostics/E0001-const.sisu"
        ))
        .expect("the seed should be readable");
        let out = on_stack(&seed);
        let (text, json) = split(&out);
        assert!(text.contains("\x1b["), "{out}");
        // The `.json` fixture of this seed records its one diagnostic at 35.
        assert_eq!(diagnostic(&json, "E0001")["primary"]["start"], 35, "{out}");
        let last = json.last().expect("the JSON report should not be empty");
        assert_eq!(last["kind"], "summary", "{out}");
    }

    /// `out` split at its first line that starts with `{`: the text report, and each line of
    /// the JSON report parsed.
    fn split(out: &str) -> (&str, Vec<serde_json::Value>) {
        let at = out
            .find("\n{")
            .expect("the JSON report should follow the text report")
            + 1;
        let json = out[at..]
            .lines()
            .map(|line| serde_json::from_str(line).expect("each JSON line should parse"))
            .collect();
        (&out[..at], json)
    }

    /// The one diagnostic line of `json`, which must have `code`.
    fn diagnostic<'a>(json: &'a [serde_json::Value], code: &str) -> &'a serde_json::Value {
        let found: Vec<_> = json.iter().filter(|v| v["kind"] == "diagnostic").collect();
        assert_eq!(found.len(), 1, "{json:?}");
        assert_eq!(found[0]["code"], code, "{json:?}");
        found[0]
    }

    #[test]
    fn invalid_utf8_reports_over_the_replacement_text() {
        for (data, valid_up_to) in [
            (&b"\xff"[..], 0),
            (b"fn main() {}\n\xff", 13),
            (b"fn main() {}\n\xe2\x82", 13),
        ] {
            let out = on_stack(data);
            let (_, json) = split(&out);
            let d = diagnostic(&json, Code::InvalidUtf8.as_str());
            assert_eq!(d["primary"]["start"], valid_up_to, "{out}");
        }
    }

    /// The deepest parentheses `tests/nesting.rs` compiles: 253 levels inside `print(`.
    /// Measured in a debug build, `front_end` on this program overflows a 4 MiB stack and every
    /// smaller power of two from 64 KiB, and fits in 8 MiB.
    #[test]
    fn deepest_nesting_fits_the_stack() {
        let n = 253;
        let source = format!(
            "fn main() {{\n    print({}1{})\n}}\n",
            "(".repeat(n),
            ")".repeat(n)
        );
        let out = on_stack(source.as_bytes());
        let summary: serde_json::Value =
            serde_json::from_str(out.lines().last().expect("the report should not be empty"))
                .expect("the summary should be JSON");
        assert_eq!(summary["errors"], 0, "{out}");
    }

    /// `aä b`: 5 bytes, and `ä` is bytes 1..3.
    const SOURCE: &str = "aä b";

    fn span(start: usize, end: usize) -> Span {
        Span::new(start, end)
    }

    /// A diagnostic at `0..1` with the one fix `edits`.
    fn fixed(edits: &[(Span, &str)]) -> Diagnostic {
        Diagnostic::for_test(span(0, 1), &[], edits)
    }

    #[test]
    fn accepts_spans_and_edits_at_the_edges() {
        assert_spans(
            SOURCE,
            &[
                Diagnostic::for_test(span(5, 5), &[], &[]),
                Diagnostic::for_test(span(0, 5), &[], &[]),
                Diagnostic::for_test(span(0, 1), &[span(3, 5)], &[]),
                // An insertion at the end of a replaced range, in either order.
                fixed(&[(span(0, 1), "x"), (span(1, 1), "y")]),
                fixed(&[(span(1, 1), "y"), (span(0, 1), "x")]),
                // An insertion listed before the replacement at its start.
                fixed(&[(span(0, 0), "y"), (span(0, 1), "x")]),
                fixed(&[(span(5, 5), "a"), (span(5, 5), "b")]),
                // Adjacent replacements, in either order.
                fixed(&[(span(0, 1), ""), (span(1, 3), "")]),
                fixed(&[(span(1, 3), ""), (span(0, 1), "")]),
            ],
        );
    }

    /// How many modules `front_end` verifies for `source`, run on this thread, without and with
    /// `--rc-checks`.
    fn verified(source: &str) -> [usize; 2] {
        let before = VERIFIED.with(std::cell::Cell::get);
        front_end(source.as_bytes());
        let after = VERIFIED.with(std::cell::Cell::get);
        [after[0] - before[0], after[1] - before[1]]
    }

    #[test]
    fn compiles_a_clean_input_with_and_without_rc_checks() {
        assert_eq!(verified("fn main() {\n    print(1)\n}\n"), [1, 1]);
        assert_eq!(verified("fn main() {\n    print(x)\n}\n"), [0, 0]);
    }

    #[test]
    #[should_panic(expected = "LLVM rejects the module (rc_checks: true)")]
    fn rejects_a_module_llvm_does_not_verify() {
        // A block without a terminator.
        let context = Context::create();
        let module = context.create_module("main");
        let f = module.add_function("f", context.void_type().fn_type(&[], false), None);
        context.append_basic_block(f, "entry");
        assert_verified(&module, true);
    }

    #[test]
    #[should_panic(expected = "starts after it ends")]
    fn rejects_start_after_end() {
        assert_spans(SOURCE, &[Diagnostic::for_test(span(4, 3), &[], &[])]);
    }

    #[test]
    #[should_panic(expected = "ends inside a char")]
    fn rejects_end_past_the_source() {
        assert_spans(SOURCE, &[Diagnostic::for_test(span(4, 6), &[], &[])]);
    }

    #[test]
    #[should_panic(expected = "starts inside a char")]
    fn rejects_start_inside_a_char() {
        assert_spans(SOURCE, &[Diagnostic::for_test(span(2, 3), &[], &[])]);
    }

    #[test]
    #[should_panic(expected = "ends inside a char")]
    fn rejects_end_inside_a_char() {
        assert_spans(SOURCE, &[Diagnostic::for_test(span(0, 2), &[], &[])]);
    }

    #[test]
    #[should_panic(expected = "ends inside a char")]
    fn rejects_a_bad_secondary_span() {
        assert_spans(
            SOURCE,
            &[Diagnostic::for_test(span(0, 1), &[span(4, 6)], &[])],
        );
    }

    #[test]
    #[should_panic(expected = "starts inside a char")]
    fn rejects_a_bad_edit_span() {
        assert_spans(SOURCE, &[fixed(&[(span(2, 3), "")])]);
    }

    #[test]
    #[should_panic(expected = "edits conflict")]
    fn rejects_overlapping_edits() {
        assert_spans(SOURCE, &[fixed(&[(span(1, 4), ""), (span(0, 3), "")])]);
    }

    #[test]
    #[should_panic(expected = "edits conflict")]
    fn rejects_an_insertion_inside_a_replacement() {
        assert_spans(SOURCE, &[fixed(&[(span(0, 3), ""), (span(1, 1), "")])]);
    }

    #[test]
    #[should_panic(expected = "edits conflict")]
    fn rejects_an_insertion_after_a_replacement_at_its_start() {
        assert_spans(SOURCE, &[fixed(&[(span(0, 1), "x"), (span(0, 0), "y")])]);
    }

    /// Two lexer errors: a `$`, and a `_` that ends a number.
    const STRAY: &str = "fn main() {\n    $ // c\n}\nfn f() {\n    let x = 1_\n}\n";

    /// The diagnostics of `STRAY`, the one of the `$` given only the safe fix `edits`.
    fn stray_with_fix(edits: &[(Span, &str)]) -> Vec<Diagnostic> {
        let mut ds = crate::front_end::front_end(STRAY).1;
        let at = STRAY.find('$').expect("STRAY has a `$`");
        let d = ds
            .iter_mut()
            .find(|d| d.span().start() == at)
            .expect("the `$` should be reported");
        *d = d.clone().with_safe_fix_for_test(edits);
        ds
    }

    #[test]
    fn accepts_a_safe_fix_that_removes_only_its_diagnostic() {
        let at = STRAY.find('$').expect("STRAY has a `$`");
        assert_safe_fixes(STRAY, &stray_with_fix(&[(span(at, at + 1), "")]));
    }

    #[test]
    #[should_panic(expected = "a safe fix leaves its diagnostic")]
    fn rejects_a_safe_fix_that_leaves_its_diagnostic() {
        assert_safe_fixes(STRAY, &stray_with_fix(&[(span(0, 0), " ")]));
    }

    #[test]
    #[should_panic(expected = "a safe fix removes")]
    fn rejects_a_safe_fix_that_removes_another_diagnostic() {
        let at = STRAY.find('$').expect("STRAY has a `$`");
        let underscore = STRAY.find('_').expect("STRAY has a `_`");
        let edits = [
            (span(at, at + 1), ""),
            (span(underscore, underscore + 1), ""),
        ];
        assert_safe_fixes(STRAY, &stray_with_fix(&edits));
    }

    /// No lexer error, so the checker runs: W0002 on `x`, E0052 on `Q`, W0003 on `todo`.
    const CHECKED: &str = "enum E {\n    P(i64)\n}\nfn main() {\n    let mut x = 1\n    print(x)\n    let _ = Q(1)\n    todo\n}\n";

    /// The diagnostics of `CHECKED`, the W0002 given only the safe fix that drops `mut ` and
    /// writes `call` in place of `Q`.
    fn checked_with_fix(call: &str) -> Vec<Diagnostic> {
        let mut ds = crate::front_end::front_end(CHECKED).1;
        let codes: Vec<Code> = ds.iter().map(Diagnostic::code).collect();
        assert_eq!(
            codes,
            [
                Code::NeverReassigned,
                Code::FunctionNotFound,
                Code::TodoLeft
            ]
        );
        let mutable = CHECKED.find("mut ").expect("CHECKED has a `mut`");
        let q = CHECKED.find('Q').expect("CHECKED calls `Q`");
        ds[0] = ds[0]
            .clone()
            .with_safe_fix_for_test(&[(span(mutable, mutable + 4), ""), (span(q, q + 1), call)]);
        ds
    }

    /// Calling `P` in place of `Q` turns the E0052 into an E0117 at the same place: the same
    /// problem, named more exactly.
    #[test]
    fn accepts_a_safe_fix_that_renames_another_diagnostic() {
        assert_safe_fixes(CHECKED, &checked_with_fix("P"));
    }

    /// Calling `print` in place of `Q` drops the E0052, while the W0003, outside it, stays.
    #[test]
    #[should_panic(expected = "a safe fix removes FunctionNotFound too")]
    fn rejects_a_safe_fix_that_removes_another_diagnostic_of_the_checker() {
        assert_safe_fixes(CHECKED, &checked_with_fix("print"));
    }

    /// An edit before the `$` that adds a byte moves where its report must not be.
    #[test]
    #[should_panic(expected = "a safe fix leaves its diagnostic")]
    fn rejects_a_safe_fix_that_leaves_its_diagnostic_moved() {
        let at = STRAY.find('$').expect("STRAY has a `$`");
        assert_safe_fixes(STRAY, &stray_with_fix(&[(span(at - 1, at), "  ")]));
    }

    /// Past the `$`, the first lexer error, the parser's E0011 is not compared: a fix may remove
    /// it, as this one does by also completing the `let`.
    #[test]
    fn accepts_a_safe_fix_that_removes_a_diagnostic_past_a_lexer_error() {
        let src = "fn main() {\n    $\n}\nfn f() {\n    let x =\n}\n";
        let mut ds = crate::front_end::front_end(src).1;
        let codes: Vec<Code> = ds.iter().map(Diagnostic::code).collect();
        assert_eq!(codes, [Code::UnexpectedCharacter, Code::Expected]);
        let at = src.find('$').expect("src has a `$`");
        let end = src.find('=').expect("src has a `=`") + 1;
        ds[0] = ds[0]
            .clone()
            .with_safe_fix_for_test(&[(span(at, at + 1), ""), (span(end, end), " 1")]);
        assert_safe_fixes(src, &ds);
    }

    /// `sound_fixes` shows a checker fix unjudged, so the fuzz target judges it.
    #[test]
    #[should_panic(expected = "NeverReassigned: a fix breaks the syntax")]
    fn rejects_a_checker_fix_that_breaks_the_syntax() {
        let mut ds = crate::front_end::front_end(CHECKED).1;
        ds[0] = ds[0].clone().with_safe_fix_for_test(&[(span(0, 0), "(")]);
        assert_fixes_keep_syntax(CHECKED, &crate::front_end::sound_fixes(CHECKED, &ds));
    }

    #[test]
    #[should_panic(expected = "a safe fix deletes a comment")]
    fn rejects_a_safe_fix_that_deletes_a_comment() {
        let at = STRAY.find('$').expect("STRAY has a `$`");
        let end = STRAY.find("c\n").expect("STRAY has a comment") + 1;
        assert_safe_fixes(STRAY, &stray_with_fix(&[(span(at, end), "")]));
    }
}
