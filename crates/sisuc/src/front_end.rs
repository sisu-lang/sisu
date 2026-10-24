//! The front end: from source to the checked program.

use rustc_hash::FxHashSet;

use crate::diagnostic::{Code, Diagnostic, DiagnosticKind, Severity};
use crate::source::Span;
use crate::{check, lexer, mono, parser, tir};

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

/// `checked`, its program instantiated by `mono`.
pub(crate) fn front_end(source: &str) -> (Option<tir::Program>, Vec<Diagnostic>) {
    let (program, diagnostics) = checked(source);
    (program.map(mono::instantiate), diagnostics)
}

/// Lexes, parses and checks `source`. Returns every diagnostic, sorted by position, and the
/// checked program, generic declarations and all, unless one is an error. The checker runs
/// only when every item is named: it checks the intact items, and treats a broken one as
/// declared. `sisu check` stops here: `mono` names each instantiation, which costs as much as
/// its type is long, and check prints none.
pub(crate) fn checked(source: &str) -> (Option<tir::Program>, Vec<Diagnostic>) {
    let lexed = match lexer::lex(source) {
        Ok(lexed) => lexed,
        Err(d) => return (None, vec![d]),
    };
    let parsed = parser::parse(lexed);
    if !parsed.checkable {
        return (None, parsed.diagnostics);
    }
    let (program, checked) = check::check(&parsed.program, source, &parsed.diagnostics);
    let mut diagnostics = parsed.diagnostics;
    diagnostics.extend(checked);
    diagnostics.sort_by_key(|d| d.span().start());
    let failed = diagnostics.iter().any(|d| d.severity() == Severity::Error);
    (program.filter(|_| !failed), diagnostics)
}

/// `diagnostics` without each fix of a lexer or parser diagnostic that leaves `source` lexing or
/// parsing worse than it does, and with each fix unsafe that edits over a comment or another
/// lexer error: a fix that is shown never breaks the syntax, and a safe one never takes text it
/// was not about. A checker fix is not judged: the checker runs only on items that parse, and
/// the fuzz target asserts that every fix shown keeps the syntax, the checker's included.
/// Judging one lexes and parses the whole source again, so judging every checker fix made N
/// warnings cost N parses. For the same reason only the first `JUDGED` syntax fixes are judged;
/// later ones are dropped. A W0007 fix is safe by construction (`lexer::separate`), so it is kept
/// as it is: neither judged, counted nor demoted.
pub(crate) fn sound_fixes(source: &str, diagnostics: &[Diagnostic]) -> Vec<Diagnostic> {
    let mut sound = diagnostics.to_vec();
    // Skipping the reparse for W0007s alone only saves work, so no test pins it.
    let syntax_fix = |d: &Diagnostic| d.code() != Code::RedundantSemicolon;
    // Judging a fix lexes and parses `source` again, so skip that when no diagnostic has one.
    if sound.iter().any(|d| syntax_fix(d) && !d.fixes().is_empty()) {
        let syntax = syntax(source);
        let before = badness(&syntax);
        // Past a bracket error, the worst there is, no fix is worse: only the parser's reports
        // are judged.
        let reported: FxHashSet<(Code, Span)> = (syntax.iter().flatten())
            .map(|d| (d.code(), d.span()))
            .collect();
        let marks = marks(source);
        let mut judged = 0;
        for d in sound.iter_mut().filter(|d| syntax_fix(d)) {
            if reported.contains(&(d.code(), d.span())) {
                d.retain_fixes(|fix| {
                    judged += 1;
                    judged <= JUDGED && syntax_errors(&fix.apply(source)) <= before
                });
            }
            let own = d.span().start();
            d.demote_fixes(|fix| {
                let taken = |at: usize| fix.edits().iter().any(|e| e.span().range().contains(&at));
                marks.iter().any(|&at| at != own && taken(at))
            });
        }
    }
    sound
}

/// How many syntax fixes `sound_fixes` judges.
// shortcut: a fix past the cap is dropped, not judged; judge only the edited item if files with
// more fixable syntax errors turn up.
const JUDGED: usize = 100;

/// Where each comment and each lexer error of `source` starts: text that the parser never sees,
/// so that no rule behind a fix knows it is there.
fn marks(source: &str) -> Vec<usize> {
    match lexer::lex(source) {
        Err(d) => vec![d.span().start()],
        Ok(lexed) => (lexed.diagnostics.iter().map(|d| d.span().start()))
            .chain(lexed.comments)
            .collect(),
    }
}

/// What the lexer and the parser report about `source`; `Err` holds the bracket error that
/// stops the lexer.
fn syntax(source: &str) -> Result<Vec<Diagnostic>, Diagnostic> {
    lexer::lex(source).map(|lexed| parser::parse(lexed).diagnostics)
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

#[cfg(test)]
mod tests {
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
        assert_eq!(names(checked(src).0), ["id", "main"]);
        assert_eq!(names(front_end(src).0), ["main", "id<i64>"]);
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
    fn fixes_past_the_cap_are_dropped() {
        let src = "fn f() {\n    print(0X1)\n}\n".repeat(JUDGED + 1) + "fn main() {}\n";
        let ds = sound_fixes(&src, &front_end(&src).1);
        assert_eq!(ds.len(), JUDGED + 1);
        let mut fixed = ds.iter().map(|d| !d.fixes().is_empty());
        assert!(fixed.clone().take(JUDGED).all(|f| f));
        assert_eq!(fixed.next_back(), Some(false));
    }

    /// A safe fix that deletes up to a token is unsafe when a comment or a stray character lies in
    /// what it deletes: the rule behind it sees only tokens.
    #[test]
    fn a_fix_over_a_comment_or_a_lexer_error_is_unsafe() {
        for (src, code, safe) in [
            ("class C {\n    fn f(mut self) {}\n}\n", Code::MutSelf, true),
            (
                "class C {\n    fn f(mut // c\n        self) {}\n}\n",
                Code::MutSelf,
                false,
            ),
            (
                "class C {\n    fn f(mut $ self) {}\n}\n",
                Code::MutSelf,
                false,
            ),
            (
                "fn main() {\n    let mut // c\n        n = 0\n    print(n)\n}\n",
                Code::NeverReassigned,
                false,
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
    fn broken_name_poisons_a_built_in() {
        let src = "fn print() {\n    let = 1\n}\nfn main() {\n    print(1, 2)\n}\n";
        assert_eq!(codes(src), [(Code::Expected, (2, 9))]);
        let src = "class i64 {\n    x y\n}\nfn main() {\n    let n: i64 = true\n}\n";
        assert_eq!(codes(src), [(Code::Expected, (2, 7))]);
        // A broken constant poisons a call of its name too, whose arguments are still checked.
        let src = "const print = 1 2\nfn main() {\n    print(1, 2)\n    print(nope)\n}\n";
        assert_eq!(
            codes(src),
            [(Code::Expected, (1, 17)), (Code::BindingNotFound, (4, 11))]
        );
        let src = "const i64 = 1 2\nfn main() {\n    let n = i64(true, 1)\n}\n";
        assert_eq!(codes(src), [(Code::Expected, (1, 15))]);
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
    /// name, and a broken function poisons a constant of the same name.
    #[test]
    fn broken_constant_poisons_its_uses() {
        for (src, at) in [
            (
                "const A = 1 2\nfn main() {\n    let b: bool = A\n    A = 2\n}\n",
                (1, 13),
            ),
            (
                "const A = 1 2\nfn A() {}\nfn main() {\n    A(1)\n}\n",
                (1, 13),
            ),
            (
                "fn A(x) {}\nconst A = 1\nfn main() {\n    let b: bool = A\n    A = 2\n}\n",
                (1, 7),
            ),
        ] {
            assert_eq!(codes(src), [(Code::Expected, at)], "{src}");
        }
        // A line that starts with `const` ends the unnamed item before it.
        let src = "def f()\nconst A = 1 2\nfn main() {}\n";
        assert_eq!(
            codes(src),
            [(Code::Expected, (1, 1)), (Code::Expected, (2, 13))]
        );
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
    fn brace_in_string_fix_passes_the_front_end() {
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

    /// A W0007 fix is not judged, so it does not count against `JUDGED`.
    #[test]
    fn sound_fixes_keep_every_redundant_semicolon_fix() {
        let src = format!("fn main() {{\n{}}}\n", "    print(1);\n".repeat(150));
        let shown = sound_fixes(&src, &front_end(&src).1);
        assert_eq!(shown.len(), 150);
        for d in &shown {
            assert_eq!(d.code(), Code::RedundantSemicolon);
            assert!(fix_edits(d).is_some_and(|(safe, _)| safe), "{d:?}");
        }
    }

    /// Applying one W0007 fix alone leaves every other diagnostic of the lexer and of the front
    /// end where it was, up to the length the fix removes. The lexer's list catches what the front
    /// end hides when an item is broken.
    #[test]
    fn redundant_semicolon_fix_changes_nothing_else() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/diagnostics");
        let mut sources: Vec<String> = [
            "fn main() {\n    let x = 1;\n    {\n    }\n}\n",
            "fn main() {\n    let x = 1\n    ; { }\n}\n",
            "fn main() {\n    if true\n    ; { }\n}\n",
            "fn main() {\n    let t = 1\n    ; - 2\n}\n",
            "fn f() -> i64 {\n;\n- 1\n}\nfn main() { print(f()) }\n",
            "fn helper();\nfn main() {}\n",
            "class C {\n    x: i64;\n}\nfn main() {}\n",
            "fn main() {\n    if true { print(1) }; else { print(2) }\n}\n",
            "fn main() {\n    let x = 1 $\n    ; - 2\n}\n",
            "fn main() { return;else { } }\n",
            "class C;fn main() {}\n",
            "fn main() {\n    if c;\n    ; { }\n}\n",
            "fn main() {\n    let a = 1 &\n    ; { }\n}\n",
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
    fn broken_item_is_never_a_duplicate() {
        let src = "fn total() {\n    let = 1\n}\nfn total() {}\nfn main() {\n    total()\n}\n";
        assert_eq!(codes(src), [(Code::Expected, (2, 9))]);
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
    fn broken_name_poisons_an_enum_path() {
        // The broken function `E` poisons the enum `E`: its paths report only their arguments.
        let src = "fn E() {\n    let = 1\n}\nenum E { A }\nfn main() {\n    let _ = E.A(nope)\n}\n";
        assert_eq!(
            codes(src),
            [(Code::Expected, (2, 9)), (Code::BindingNotFound, (6, 17))]
        );
    }

    #[test]
    fn broken_method_is_no_field_confusion() {
        let src = "class C {\n    n: i64\n    fn m(self) {\n        let = 1\n    }\n}\nfn main() {\n    let c = C(n: 1)\n    print(c.m)\n}\n";
        assert_eq!(codes(src), [(Code::Expected, (4, 13))]);
    }

    #[test]
    fn broken_name_poisons_the_other_kind() {
        // Functions and classes share a namespace: the broken function `Tree` poisons the type.
        let src = "class Tree {\n    n: i64\n}\nfn Tree() {\n    let = 1\n}\nfn main() {\n    let t: Tree = 1\n}\n";
        assert_eq!(codes(src), [(Code::Expected, (5, 9))]);
    }
}
