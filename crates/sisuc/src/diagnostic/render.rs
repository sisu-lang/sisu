//! Text rendering, through `annotate-snippets`.

use std::ops::Range;

use annotate_snippets::{AnnotationKind, Level, Patch, Renderer, Snippet};

use super::{Diagnostic, Edit, Fix, Severity};
use crate::source::{LineIndex, Span};

/// The lines of `source` that `spans` cover, as the renderer needs them: the byte range, and the
/// number of its first line. The renderer rebuilds a map of every line it is handed, so it gets
/// only these, and many diagnostics in a large file cost linear time.
fn window(
    source: &str,
    lines: &LineIndex,
    spans: impl Iterator<Item = Span> + Clone,
) -> (Range<usize>, usize) {
    let first = spans
        .clone()
        .map(|s| lines.render_line(source, s.start()))
        .min();
    let last = spans.map(|s| lines.render_line(source, s.end())).max();
    let (first, last) = (
        first.expect("a snippet should have a span"),
        last.expect("a snippet should have a span"),
    );
    // Start one line early. The renderer sizes the line-number gutter from the newlines
    // before the last span's end, counted with `lines().count() - 1`, which reads an empty
    // prefix as one newline too many; the whole source never has an empty prefix there. A
    // window with a line before it matches, and folding hides that line. This is
    // `newline_count` in annotate-snippets 0.12.16 `render.rs`; recheck it on a version bump.
    let first = first.saturating_sub(1).max(1);
    let from = lines
        .line_start(first)
        .expect("a span's first rendered line should be in the index");
    let to = lines.line_start(last + 1).unwrap_or(source.len());
    (from..to, first)
}

impl Diagnostic {
    /// Renders the diagnostic as text; `path` is shown as given, `lines` is the index of
    /// `source`, and `color` adds ANSI styles. Each fix renders as its own `help:` with the
    /// patched source under it.
    pub(crate) fn render(
        &self,
        path: &str,
        source: &str,
        lines: &LineIndex,
        color: bool,
    ) -> String {
        let labelled = std::iter::once(self.span()).chain(self.secondary().iter().map(|(s, _)| *s));
        let edits = self.fixes().iter().flat_map(Fix::edits).map(Edit::span);
        for span in labelled.clone().chain(edits) {
            debug_assert!(
                source.is_char_boundary(span.start()) && source.is_char_boundary(span.end()),
                "span {span:?} does not sit on char boundaries"
            );
        }
        // A labelled span that swallows the line break underlines into the next line. An edit
        // may cover one.
        for span in labelled.clone() {
            debug_assert!(
                span.start() == span.end() || !source[..span.end()].ends_with('\n'),
                "span {span:?} ends with a line break"
            );
        }
        let level = match self.severity() {
            Severity::Error => Level::ERROR,
            Severity::Warning => Level::WARNING,
        };
        let title = level.primary_title(self.message()).id(self.code().as_str());
        let (range, first) = window(source, lines, labelled.clone());
        let rebased = |span: Span| span.start() - range.start..span.end() - range.start;
        let mut primary = AnnotationKind::Primary.span(rebased(self.span()));
        if let Some(label) = self.label() {
            primary = primary.label(label);
        }
        let mut snippet = Snippet::source(&source[range.clone()])
            .line_start(first)
            .path(path)
            .annotation(primary);
        for (span, text) in self.secondary() {
            snippet = snippet.annotation(AnnotationKind::Context.span(rebased(*span)).label(text));
        }
        let mut group = title.element(snippet);
        if let Some(help) = self.help() {
            group = group.element(Level::HELP.message(help));
        }
        let mut groups = vec![group];
        for fix in self.fixes() {
            let (range, first) = window(source, lines, fix.edits().iter().map(Edit::span));
            let offset = range.start;
            let patches = fix.edits().iter().map(|e| {
                Patch::new(
                    e.span().start() - offset..e.span().end() - offset,
                    e.replacement(),
                )
            });
            let snippet = Snippet::source(&source[range])
                .line_start(first)
                .path(path)
                .patches(patches);
            groups.push(Level::HELP.secondary_title(fix.message()).element(snippet));
        }
        let renderer = if color {
            Renderer::styled()
        } else {
            Renderer::plain()
        };
        renderer.render(&groups)
    }
}

#[cfg(test)]
mod tests {
    use annotate_snippets::{AnnotationKind, Level, Patch, Renderer, Snippet};

    use crate::diagnostic::{Code, Diagnostic, DiagnosticKind, Edit, Fix, Immutable, Severity};
    use crate::source::{LineIndex, Span};

    const ASSIGN: &str = "fn main() {\n    let n = 0\n    n = 1\n}\n";

    /// `ASSIGN`'s E0001.
    fn assign() -> Diagnostic {
        DiagnosticKind::AssignToImmutable {
            name: "n".to_string(),
            at: Span::new(30, 35),
            element: false,
            cause: Immutable::Let {
                declared: Span::new(20, 21),
                redeclared: false,
            },
        }
        .into()
    }

    #[test]
    #[should_panic(expected = "ends with a line break")]
    fn span_over_a_line_break_is_rejected() {
        let d: Diagnostic = DiagnosticKind::UnreachableCode {
            at: Span::new(5, 6),
        }
        .into();
        d.render("a.sisu", "let y\n}", &LineIndex::new("let y\n}"), false);
    }

    #[test]
    fn renders_warning() {
        let source = "fn main() {\n    let mut count = 0\n}\n";
        let d: Diagnostic = DiagnosticKind::NeverReassigned {
            name: "count".to_string(),
            at: Span::new(24, 29),
            keyword: Span::new(20, 23),
        }
        .into();
        let rendered = d.render("w.sisu", source, &LineIndex::new(source), false);
        assert!(
            rendered.starts_with("warning[W0002]: `count` is never reassigned\n --> w.sisu:2:13"),
            "{rendered}"
        );
        assert!(
            rendered.contains("help: declare it with `let`"),
            "{rendered}"
        );
    }

    #[test]
    fn renders_error_with_secondary_and_help() {
        let source = "fn f(n: i64) {\n    n = 1\n}\n";
        let d: Diagnostic = DiagnosticKind::AssignToImmutable {
            name: "n".to_string(),
            at: Span::new(19, 24),
            element: false,
            cause: Immutable::Param {
                declared: Span::new(5, 6),
            },
        }
        .into();
        let expected = "\
error[E0001]: cannot assign to `n`
 --> test.sisu:2:5
  |
1 | fn f(n: i64) {
  |      - declared as a parameter here
2 |     n = 1
  |     ^^^^^
  |
  = help: copy it into a variable: `let mut n = n`";
        assert_eq!(
            d.render("test.sisu", source, &LineIndex::new(source), false),
            expected
        );
    }

    /// The check that stopped at the depth limit has no fixture: it needs a program of 2^18
    /// nested types, so this test pins its text.
    #[test]
    fn a_conformance_check_that_is_too_deep_should_render_with_its_limit() {
        let source = "class W<T> {\n    value: T\n}\n";
        let expected = "\
error[E0140]: the check of `Eq` for `E0<i64>` stopped after 262144 nested types
 --> test.sisu:1:7
  |
1 | class W<T> {
  |       ^ `W` is 262144 types deep here
  |
  = help: a type that nests this deep may instantiate itself without end";
        let d: Diagnostic = DiagnosticKind::ConformanceTooDeep {
            interface: "Eq".to_string(),
            root: "E0<i64>".to_string(),
            limit: 262_144,
            holder: "W".to_string(),
            at: Span::new(6, 7),
        }
        .into();
        assert_eq!(
            d.render("test.sisu", source, &LineIndex::new(source), false),
            expected
        );
    }

    #[test]
    fn empty_span_at_the_end_of_a_line_renders_one_caret() {
        let source = "fn main() {\n    let y\n}";
        let expected = "\
error[E0011]: expected `=`, found end of line
 --> a.sisu:2:10
  |
2 |     let y
  |          ^";
        let d: Diagnostic = DiagnosticKind::Expected {
            what: "`=`".to_string(),
            found: "end of line".to_string(),
            at: Span::new(21, 21),
        }
        .into();
        assert_eq!(
            d.render("a.sisu", source, &LineIndex::new(source), false),
            expected
        );
    }

    #[test]
    fn header_carries_the_code() {
        let rendered = assign().render("t.sisu", ASSIGN, &LineIndex::new(ASSIGN), false);
        assert!(
            rendered.starts_with("error[E0001]: cannot assign to `n`\n --> t.sisu:3:5"),
            "{rendered}"
        );
    }

    #[test]
    fn fix_renders_as_a_patch() {
        let rendered = assign().render("t.sisu", ASSIGN, &LineIndex::new(ASSIGN), false);
        let (_, fix) = rendered
            .split_once("help: declare it with `let mut`")
            .unwrap_or_else(|| panic!("no fix in {rendered}"));
        let lines: Vec<&str> = fix.lines().collect();
        let patched = lines
            .iter()
            .position(|l| l.ends_with("let mut n = 0"))
            .unwrap_or_else(|| panic!("no patched line in {fix}"));
        assert!(
            lines[patched + 1..]
                .iter()
                .any(|l| l.trim_end().ends_with("+++")),
            "{fix}"
        );
    }

    #[test]
    fn styled_render_has_escapes() {
        assert!(
            assign()
                .render("t.sisu", ASSIGN, &LineIndex::new(ASSIGN), true)
                .contains("\u{1b}[")
        );
        assert!(
            !assign()
                .render("t.sisu", ASSIGN, &LineIndex::new(ASSIGN), false)
                .contains("\u{1b}[")
        );
    }

    /// An error with message `m` at `span`.
    fn error(span: Span, message: &str) -> Diagnostic {
        Diagnostic::coded(Code::AssignToImmutable, span, message)
    }

    /// A fix that replaces each span with its text.
    fn fix(edits: &[(Span, &str)]) -> Fix {
        Fix {
            message: "f".to_string(),
            safe: true,
            edits: edits
                .iter()
                .map(|&(span, text)| Edit {
                    span,
                    replacement: text.to_string(),
                })
                .collect(),
        }
    }

    /// The rendering before the windowing: the renderer gets the whole source.
    fn render_whole(d: &Diagnostic, path: &str, source: &str) -> String {
        let level = match d.severity() {
            Severity::Error => Level::ERROR,
            Severity::Warning => Level::WARNING,
        };
        let mut primary = AnnotationKind::Primary.span(d.span().range());
        if let Some(label) = d.label() {
            primary = primary.label(label);
        }
        let mut snippet = Snippet::source(source).path(path).annotation(primary);
        for (span, text) in d.secondary() {
            snippet = snippet.annotation(AnnotationKind::Context.span(span.range()).label(text));
        }
        let title = level.primary_title(d.message()).id(d.code().as_str());
        let mut group = title.element(snippet);
        if let Some(help) = d.help() {
            group = group.element(Level::HELP.message(help));
        }
        let mut groups = vec![group];
        for fix in d.fixes() {
            let patches = fix
                .edits()
                .iter()
                .map(|e| Patch::new(e.span().range(), e.replacement()));
            let snippet = Snippet::source(source).path(path).patches(patches);
            groups.push(Level::HELP.secondary_title(fix.message()).element(snippet));
        }
        Renderer::plain().render(&groups)
    }

    /// The span of `from`'s first occurrence in `source` through the end of `to`'s.
    fn between(source: &str, from: &str, to: &str) -> Span {
        let start = source
            .find(from)
            .expect("`from` should occur in the source");
        let end = source.find(to).expect("`to` should occur in the source") + to.len();
        Span::new(start, end)
    }

    #[test]
    fn windowed_rendering_matches_rendering_the_whole_source() {
        let twelve = "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\nl11\nl12\n";
        let mut cases: Vec<(&str, Diagnostic)> = vec![
            // The first line.
            (
                "ab\ncd\nef\n",
                error(between("ab\ncd\nef\n", "ab", "ab"), "m").with_label("here"),
            ),
            // The last line, with and without a final newline.
            (
                "ab\ncd\nef\n",
                error(between("ab\ncd\nef\n", "ef", "ef"), "m"),
            ),
            ("ab\ncd\nef", error(between("ab\ncd\nef", "ef", "ef"), "m")),
            // Empty spans at the end of the text, after a final newline and after none.
            (
                "ab\ncd\n",
                error(Span::new(6, 6), "m").with_label("end of file"),
            ),
            (
                "ab\ncd",
                error(Span::new(5, 5), "m").with_label("end of file"),
            ),
            (
                "ab\ncd\n",
                error(Span::new(2, 2), "m").with_label("end of line"),
            ),
            ("ab\n\ncd", error(Span::new(3, 3), "m")),
            ("", error(Span::new(0, 0), "m").with_label("empty")),
            ("\n", error(Span::new(1, 1), "m")),
            // A span over several lines, with a secondary span before it and help after.
            (
                "fn f() {\n    a\n    b\n    c\n    d\n    e\n    f\n}\n",
                error(
                    between(
                        "fn f() {\n    a\n    b\n    c\n    d\n    e\n    f\n}\n",
                        "a\n    b\n    c\n    d\n    e",
                        "e",
                    ),
                    "m",
                )
                .with_label("many lines")
                .with_secondary(Span::new(3, 4), "open")
                .with_help("close it"),
            ),
            // Non-ASCII text and CRLF line breaks.
            (
                "ää\r\nöö x\r\n€€",
                error(between("ää\r\nöö x\r\n€€", "x", "€€"), "m"),
            ),
            (
                "ää\r\nöö\r\n",
                Diagnostic::coded(Code::NeverReassigned, Span::new(11, 11), "m"),
            ),
            // An empty span at the start of line 10: the renderer sizes the gutter from the
            // newlines before the span, and an empty window prefix counts one too many.
            (
                "1\n2\n3\n4\n5\n6\n7\n8\n9\nx",
                error(Span::new(18, 18), "m"),
            ),
            (
                "\n\n\n\n\n\n\r\n//\r\n€\r\n }",
                error(Span::new(17, 17), "m"),
            ),
            // The line numbers grow from one digit to two.
            (
                twelve,
                error(between(twelve, "l9", "l9"), "m")
                    .with_secondary(between(twelve, "l11", "l11"), "later"),
            ),
            (
                twelve,
                error(between(twelve, "l1\n", "l1"), "m")
                    .with_secondary(between(twelve, "l12", "l12"), "far"),
            ),
            (
                twelve,
                error(between(twelve, "l10", "l10"), "m")
                    .with_secondary(between(twelve, "l2", "l2"), "before"),
            ),
        ];
        // Every single-char and every empty span of one text, as primary.
        let text = "ä\nbö€\n\nc";
        for start in 0..=text.len() {
            for end in start..=text.len().min(start + 1) {
                let on_boundary = text.is_char_boundary(start) && text.is_char_boundary(end);
                if on_boundary && (start == end || !text[..end].ends_with('\n')) {
                    cases.push((text, error(Span::new(start, end), "m")));
                }
            }
        }
        for (source, d) in &cases {
            assert_eq!(
                d.render("t.sisu", source, &LineIndex::new(source), false),
                render_whole(d, "t.sisu", source),
                "{source:?} {d:?}"
            );
        }
    }

    /// Fixes: an insertion, a deletion of a whole line, edits on several lines, an insertion at
    /// the end of the text, and a patch on line 10 below a span on line 1.
    #[test]
    fn windowed_fix_rendering_matches_rendering_the_whole_source() {
        let twelve = "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\nl11\nl12\n";
        let lets = "fn main() {\n    let n = 0\n    n = 1\n}\n";
        let cases = [
            (
                lets,
                error(between(lets, "n = 1", "n = 1"), "m")
                    .with_fix(fix(&[(Span::new(20, 20), "mut ")])),
            ),
            (
                lets,
                error(between(lets, "n = 1", "n = 1"), "m")
                    .with_fix(fix(&[(between(lets, "    n = 1\n", "    n = 1\n"), "")])),
            ),
            (
                lets,
                error(between(lets, "n = 1", "n = 1"), "m").with_fix(fix(&[
                    (Span::new(16, 19), "var"),
                    (between(lets, "1", "1"), "2"),
                ])),
            ),
            (
                "ab\ncd",
                error(Span::new(5, 5), "m").with_fix(fix(&[(Span::new(5, 5), "\n}")])),
            ),
            (
                twelve,
                error(between(twelve, "l1\n", "l1"), "m")
                    .with_fix(fix(&[(between(twelve, "l10", "l10"), "x")])),
            ),
        ];
        for (source, d) in &cases {
            assert_eq!(
                d.render("t.sisu", source, &LineIndex::new(source), false),
                render_whole(d, "t.sisu", source),
                "{source:?} {d:?}"
            );
        }
    }

    /// `.config/nextest.toml` kills this test after 30 s, matching it by name: a rename must
    /// update the override there. Every render used to cost the whole file.
    #[test]
    fn rendering_many_diagnostics_in_a_large_file_is_linear() {
        const LINES: usize = 200_000;
        let source = "let x = 1\n".repeat(LINES);
        let lines = LineIndex::new(&source);
        let mut len = 0;
        for i in (0..LINES).step_by(10) {
            let start = i * 10;
            let d = error(Span::new(start, start + 3), "m");
            len += d.render("big.sisu", &source, &lines, false).len();
        }
        assert!(len > 0);
    }
}
