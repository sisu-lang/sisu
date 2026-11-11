//! Text rendering, through `annotate-snippets`.

use std::borrow::Cow;
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

/// The bytes that the renderer gets of a long line, before its leftmost span and after its
/// rightmost span.
///
/// `annotate-snippets` 0.12.17 shows at most `term_width - code_offset` columns of a long line, so
/// at most 140 (`Margin::compute` in `renderer/margin.rs`; the width is 140 by default). In the
/// cases where the spans and labels fit in the width (branches 2 and 3), it starts the shown part
/// at most `6 + 140 / 2` columns before the leftmost span (`ELLIPSIS_PASSING` and the padding),
/// and it ends it 140 columns after the start. When the spans are wider than the width (branch
/// 4), it shows the columns from 6 before the leftmost span to 6 after the rightmost span. So
/// `MARGIN` is more than 76 + 140, and the renderer shows the same text of the cut line as of the
/// whole line. Each byte is at least one column, because the cut needs ASCII text.
const MARGIN: usize = 384;

/// The fewest bytes between the start of a cut line and its leftmost span. `Margin::compute`
/// takes its first branch (trim only white space) when the label's right end is less than 140
/// columns past the white space that starts the line, and then the shown columns differ from
/// those of the whole line. A cut line starts with a char that is not white space, so its white
/// space is 0 columns, and the label's right end is at least the bytes before the span plus 6,
/// which is more than 140 when there are 256. The other branches need only 76 columns before
/// the span. The whole line takes the first branch only when its leading white space ends less
/// than 140 columns before the span, and the cut then starts after that white space and is
/// refused, so the whole line and the cut line always take the same branch.
const LEFT_MIN: usize = 256;

/// The text of a snippet, as the renderer gets it, and where the spans of the source sit in it.
struct Excerpt<'s> {
    text: Cow<'s, str>,
    /// The number of the first line of `text`.
    first: usize,
    /// The offset in the source where `text` starts.
    from: usize,
    /// The bytes of the source that a cut dropped from the middle of its long line; empty
    /// when no cut dropped bytes there.
    gap: Range<usize>,
    /// The line, the column that the renderer gives in the header, and the true column, when a
    /// cut that drops the middle of the line moved the column: the renderer takes it from the
    /// text. `annotate-snippets` has no way to set the column of a snippet with a path, and an
    /// `Origin` before a snippet without one makes the snippet a secondary one, so the render
    /// puts the true column in place of the other.
    header: Option<(usize, usize, usize)>,
}

impl<'s> Excerpt<'s> {
    /// The lines of `source` that `spans` cover; when they all lie on one long line, only the
    /// columns of that line near them. `primary` is the span that the header names.
    ///
    /// A cut needs ASCII text, so that a column is a byte, and some other conditions, which
    /// the constants above give. When one does not hold, the excerpt is the whole lines, as
    /// before. The renderer shows the same text for both, and the cut costs the renderer at
    /// most `2 x MARGIN` bytes more than the extent of the spans. The checks read the long line
    /// once, as bytes, which is far cheaper than what the renderer does with each char.
    // shortcut: one byte scan of the long line for each diagnostic, so a line of megabytes with
    // thousands of diagnostics still costs them; cache the check in `LineIndex` then.
    fn new(
        source: &'s str,
        lines: &LineIndex,
        spans: impl Iterator<Item = Span> + Clone,
        primary: Span,
    ) -> Self {
        let (range, first) = window(source, lines, spans.clone());
        Self::cut(source, lines, spans, primary, &range, first).unwrap_or_else(|| Self {
            text: Cow::Borrowed(&source[range.clone()]),
            first,
            from: range.start,
            gap: 0..0,
            header: None,
        })
    }

    fn cut(
        source: &'s str,
        lines: &LineIndex,
        spans: impl Iterator<Item = Span> + Clone,
        primary: Span,
        range: &Range<usize>,
        first: usize,
    ) -> Option<Self> {
        let line = lines.render_line(source, primary.start());
        if spans.clone().any(|s| {
            lines.render_line(source, s.start()) != line
                || lines.render_line(source, s.end()) != line
        }) {
            return None;
        }
        let start = lines
            .line_start(line)
            .expect("a span's rendered line should be in the index");
        let previous = &source[range.start..start];
        let whole = &source[start..range.end];
        let content = whole
            .strip_suffix('\n')
            .map_or(whole, |c| c.strip_suffix('\r').unwrap_or(c));
        let near = spans
            .clone()
            .map(Span::start)
            .min()
            .expect("a snippet should have a span")
            - start;
        let far = spans
            .map(Span::end)
            .max()
            .expect("a snippet should have a span")
            - start;
        // An interior carriage return would join the line break that is put back after the cut.
        if far > content.len() || !content.is_ascii() || content.contains('\r') {
            return None;
        }
        let mut left = near.saturating_sub(MARGIN);
        let cut_left = left > 0;
        if cut_left {
            // Start at text, not at white space: the renderer reads a run of white space at the
            // start of a line as its indent.
            left += content.as_bytes()[left..near]
                .iter()
                .take_while(|b| char::from(**b).is_whitespace())
                .count();
            if near - left < LEFT_MIN {
                return None;
            }
        }
        let right = (far + MARGIN).min(content.len());
        if !cut_left && right == content.len() {
            return None;
        }
        let kept = &content[left..right];
        // The renderer leaves a line of white space out of the indent of the snippet, so the cut
        // line must have text when the whole line has.
        if kept.bytes().all(|b| char::from(b).is_whitespace()) {
            return None;
        }
        // The renderer hides the line before the long line, because a snippet folds its lines
        // without a span; only that line's break counts, for the width of the line numbers.
        let break_len = if previous.ends_with("\r\n") {
            2
        } else {
            previous.len().min(1)
        };
        let mut text = String::with_capacity(break_len + kept.len() + 2);
        text.push_str(&previous[previous.len() - break_len..]);
        text.push_str(kept);
        text.push_str(&whole[content.len()..]);
        let primary_at = primary.start() - start;
        let header = cut_left.then(|| (line, primary_at - left + 1, primary_at + 1));
        Some(Self {
            text: Cow::Owned(text),
            first,
            from: start - break_len,
            gap: start..start + left,
            header,
        })
    }

    /// `span` of the source, as a range of `text`. A cut keeps every span, so each lies after the
    /// gap, and the gap is empty when there is no cut.
    fn rebased(&self, span: Span) -> Range<usize> {
        let at = |x: usize| x - self.from - self.gap.len();
        at(span.start())..at(span.end())
    }
}

/// The most lines between two labelled spans that share a snippet.
const NEAR: usize = 3;

/// Labelled spans that render in one snippet.
struct Cluster {
    /// The greatest last line of a span in the cluster.
    last: usize,
    has_primary: bool,
    /// The indexes into the secondary spans.
    held: Vec<usize>,
}

/// One excerpt for each cluster of labelled spans, with the indexes into `secondary` that it
/// holds. A span joins the cluster before it when its first line is at most `NEAR` lines after
/// the cluster's last line. That is the greatest last line of a span in the cluster. The
/// cluster of `primary` comes first, then the others in source order.
fn excerpts<'s>(
    source: &'s str,
    lines: &LineIndex,
    primary: Span,
    secondary: &[Span],
) -> Vec<(Excerpt<'s>, Vec<usize>)> {
    let line_of = |s: Span| {
        (
            lines.render_line(source, s.start()),
            lines.render_line(source, s.end()),
        )
    };
    // `None` is the primary span. The sort is stable, so equal lines keep this order.
    let mut spans: Vec<(usize, usize, Option<usize>)> = std::iter::once((primary, None))
        .chain(secondary.iter().enumerate().map(|(i, s)| (*s, Some(i))))
        .map(|(s, i)| {
            let (first, last) = line_of(s);
            (first, last, i)
        })
        .collect();
    spans.sort_by_key(|&(first, ..)| first);
    let mut clusters: Vec<Cluster> = Vec::new();
    for (first, last, index) in spans {
        let cluster = match clusters.last_mut() {
            Some(cluster) if first <= cluster.last + NEAR => cluster,
            _ => {
                clusters.push(Cluster {
                    last,
                    has_primary: false,
                    held: Vec::new(),
                });
                clusters
                    .last_mut()
                    .expect("a cluster should have been pushed")
            }
        };
        cluster.last = cluster.last.max(last);
        match index {
            Some(i) => cluster.held.push(i),
            None => cluster.has_primary = true,
        }
    }
    let primary_at = clusters
        .iter()
        .position(|c| c.has_primary)
        .expect("a cluster should hold the primary span");
    clusters[..=primary_at].rotate_right(1);
    clusters
        .into_iter()
        .map(
            |Cluster {
                 has_primary,
                 mut held,
                 ..
             }| {
                // A `:::` header names the first annotation of its first line
                // (`render_snippet_annotations` in annotate-snippets 0.12.17). In this order that
                // is the leftmost label, as a `-->` header names the leftmost primary span.
                held.sort_unstable_by_key(|&i| (secondary[i].start(), i));
                let header = if has_primary {
                    primary
                } else {
                    secondary[held[0]]
                };
                let spans = has_primary
                    .then_some(primary)
                    .into_iter()
                    .chain(held.iter().map(|&i| secondary[i]));
                (Excerpt::new(source, lines, spans, header), held)
            },
        )
        .collect()
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
        let spans: Vec<Span> = self.secondary().iter().map(|(s, _)| *s).collect();
        let excerpts = excerpts(source, lines, self.span(), &spans);
        let mut snippets = excerpts.iter().enumerate().map(|(n, (excerpt, held))| {
            let mut snippet = Snippet::source(&*excerpt.text).line_start(excerpt.first);
            // `excerpts` gives the cluster of the primary span first.
            if n == 0 {
                let mut primary = AnnotationKind::Primary.span(excerpt.rebased(self.span()));
                if let Some(label) = self.label() {
                    primary = primary.label(label);
                }
                snippet = snippet.annotation(primary);
            }
            for &i in held {
                let (span, text) = &self.secondary()[i];
                snippet = snippet.annotation(
                    AnnotationKind::Context
                        .span(excerpt.rebased(*span))
                        .label(text),
                );
            }
            snippet.path(path)
        });
        let first = snippets
            .next()
            .expect("`excerpts` should give the cluster of the primary span");
        let mut group = title.element(first);
        for snippet in snippets {
            group = group.element(snippet);
        }
        for note in self.notes() {
            group = group.element(Level::NOTE.message(&**note));
        }
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
        let text = renderer.render(&groups);
        if excerpts.iter().all(|(e, _)| e.header.is_none()) {
            return text;
        }
        true_headers(&text, path, excerpts.iter().map(|(e, _)| e.header))
    }
}

/// Whether `line` of the rendered text is the header of a snippet, plain or styled: white space
/// and ANSI styles, then `-->` or `:::`. A source line starts with its gutter, so it is not one.
fn is_header(line: &str) -> bool {
    let mut rest = line.trim_start_matches(' ');
    while let Some(style) = rest.strip_prefix("\u{1b}[") {
        // An ANSI style ends at its first letter.
        let Some(end) = style.find(|c: char| c.is_ascii_alphabetic()) else {
            return false;
        };
        rest = style[end + 1..].trim_start_matches(' ');
    }
    rest.starts_with("--> ") || rest.starts_with("::: ")
}

/// `text` with the true column in the header of each cut snippet. `headers` gives the header fix
/// of each snippet of the first group, in render order. The header lines of the first group come
/// first in the text. Only those lines change, because a source line can show the same text.
/// One pass copies each line once, so the cost is linear in the text.
fn true_headers(
    text: &str,
    path: &str,
    mut headers: impl Iterator<Item = Option<(usize, usize, usize)>>,
) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for line in text.split_inclusive('\n') {
        let fix = if is_header(line) {
            headers.next().flatten()
        } else {
            None
        };
        let repaired = fix.and_then(|(number, cut, column)| {
            let shown = format!("{path}:{number}:{cut}");
            let at = line.rfind(&shown)?;
            Some((at, shown.len(), format!("{path}:{number}:{column}")))
        });
        match repaired {
            Some((at, len, header)) => {
                out.push_str(&line[..at]);
                out.push_str(&header);
                out.push_str(&line[at + len..]);
            }
            None => out.push_str(line),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use annotate_snippets::{AnnotationKind, Level, Patch, Renderer, Snippet};

    use super::{Excerpt, MARGIN, excerpts, window};
    use crate::diagnostic::{
        Code, Diagnostic, DiagnosticKind, Edit, Fix, Immutable, NeverReassignedForm, Severity,
    };
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
    fn warning_should_render_with_its_fix() {
        let source = "fn main() {\n    let mut count = 0\n}\n";
        let d: Diagnostic = DiagnosticKind::NeverReassigned {
            name: "count".to_string(),
            at: Span::new(24, 29),
            form: NeverReassignedForm::Let {
                remove: Span::new(20, 24),
            },
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

    #[test]
    fn a_note_should_render_before_the_help() {
        let source = "fn f(n: i64) {\n    n = 1\n}\n";
        let mut d: Diagnostic = DiagnosticKind::AssignToImmutable {
            name: "n".to_string(),
            at: Span::new(19, 24),
            element: false,
            cause: Immutable::Param {
                declared: Span::new(5, 6),
            },
        }
        .into();
        d.notes = Some(Box::new(vec!["`n` is a parameter".into()]));
        let expected = "\
error[E0001]: cannot assign to `n`
 --> test.sisu:2:5
  |
1 | fn f(n: i64) {
  |      - declared as a parameter here
2 |     n = 1
  |     ^^^^^
  |
  = note: `n` is a parameter
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

    /// The rendering before the windowing: the renderer gets the whole source for the snippet of
    /// each cluster that `excerpts` gives, in the same order.
    fn render_whole(d: &Diagnostic, path: &str, source: &str) -> String {
        let level = match d.severity() {
            Severity::Error => Level::ERROR,
            Severity::Warning => Level::WARNING,
        };
        let spans: Vec<Span> = d.secondary().iter().map(|(s, _)| *s).collect();
        let clusters = excerpts(source, &LineIndex::new(source), d.span(), &spans);
        let mut snippets = clusters.iter().enumerate().map(|(n, (_, held))| {
            let mut snippet = Snippet::source(source).path(path);
            if n == 0 {
                let mut primary = AnnotationKind::Primary.span(d.span().range());
                if let Some(label) = d.label() {
                    primary = primary.label(label);
                }
                snippet = snippet.annotation(primary);
            }
            for &i in held {
                let (span, text) = &d.secondary()[i];
                snippet =
                    snippet.annotation(AnnotationKind::Context.span(span.range()).label(text));
            }
            snippet
        });
        let title = level.primary_title(d.message()).id(d.code().as_str());
        let mut group = title.element(snippets.next().expect("a diagnostic should have a snippet"));
        for snippet in snippets {
            group = group.element(snippet);
        }
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

    /// At the cluster boundary, with LF and CRLF, each snippet renders as the whole source does.
    #[test]
    fn labels_three_and_four_lines_apart_should_render_as_the_whole_source_does() {
        for source in [numbered(12), numbered(12).replace('\n', "\r\n")] {
            for primary in [6, 7] {
                let d = error(
                    between(&source, &format!("l{primary}"), &format!("l{primary}")),
                    "m",
                )
                .with_secondary(between(&source, "l3", "l3"), "s");
                assert_eq!(
                    rendered(&d, &source),
                    render_whole(&d, "t.sisu", &source),
                    "{source:?} {d:?}"
                );
            }
        }
    }

    /// A source of `n` lines, where line `k` is `l<k>`.
    fn numbered(n: usize) -> String {
        let lines: Vec<String> = (1..=n).map(|k| format!("l{k}\n")).collect();
        lines.concat()
    }

    /// The span of the text of line `k` of `source`, without its line break.
    fn line(source: &str, k: usize) -> Span {
        let start = LineIndex::new(source)
            .line_start(k)
            .expect("the line should be in the source");
        let len = source[start..]
            .find('\n')
            .expect("the line should end with a line break");
        Span::new(start, start + len)
    }

    /// `d` rendered without color, with the path `t.sisu`.
    fn rendered(d: &Diagnostic, source: &str) -> String {
        d.render("t.sisu", source, &LineIndex::new(source), false)
    }

    /// The header of each snippet of `rendered`, in order: `-->` or `:::`, then the position.
    fn headers(rendered: &str) -> Vec<&str> {
        rendered
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with("--> ") || l.starts_with("::: "))
            .collect()
    }

    /// The number of each source line that `rendered` shows, in order.
    fn shown_lines(rendered: &str) -> Vec<usize> {
        rendered
            .lines()
            .filter_map(|l| l.split_once(" |"))
            .filter_map(|(number, _)| number.trim().parse().ok())
            .collect()
    }

    #[test]
    fn near_labels_should_share_a_snippet() {
        let source = numbered(8);
        let d = error(line(&source, 5), "m").with_secondary(line(&source, 2), "s");
        assert_eq!(headers(&rendered(&d, &source)), ["--> t.sisu:5:1"]);
    }

    #[test]
    fn a_far_label_should_get_its_own_snippet() {
        let source = numbered(8);
        let d = error(line(&source, 6), "m").with_secondary(line(&source, 2), "s");
        let text = rendered(&d, &source);
        assert_eq!(
            headers(&text),
            ["--> t.sisu:6:1", "::: t.sisu:2:1"],
            "{text}"
        );
        assert_eq!(shown_lines(&text), [6, 2], "{text}");
        assert!(!text.contains("..."), "{text}");
    }

    #[test]
    fn the_primary_snippet_should_come_first() {
        let source = numbered(41);
        let d = error(line(&source, 40), "m").with_secondary(line(&source, 2), "s");
        let text = rendered(&d, &source);
        assert_eq!(
            headers(&text),
            ["--> t.sisu:40:1", "::: t.sisu:2:1"],
            "{text}"
        );
        assert_eq!(shown_lines(&text), [40, 2], "{text}");
    }

    #[test]
    fn a_far_origin_should_render_two_snippets() {
        let source = numbered(5002);
        let d = error(line(&source, 5002), "m").with_secondary(line(&source, 1), "s");
        let text = rendered(&d, &source);
        assert_eq!(
            headers(&text),
            ["--> t.sisu:5002:1", "::: t.sisu:1:1"],
            "{text}"
        );
        assert_eq!(shown_lines(&text), [5002, 1], "{text}");
    }

    /// A cut of line 1 moves the column that the renderer gives, so the `:::` header needs the
    /// fix that a `-->` header gets.
    #[test]
    fn a_far_label_on_a_long_line_should_keep_its_column() {
        let pad = "y".repeat(999);
        let source = format!("{pad}ab{pad}\n{}", &numbered(10)[3..]);
        let label = Span::new(999, 1001);
        let d = error(line(&source, 10), "m").with_secondary(label, "s");
        // Without the cut, the renderer gives the true column itself.
        let far = excerpts(&source, &LineIndex::new(&source), d.span(), &[label]);
        assert!(far[1].0.text.len() < 2000, "the far snippet should be cut");
        let text = rendered(&d, &source);
        assert_eq!(
            headers(&text),
            ["--> t.sisu:10:1", "::: t.sisu:1:1000"],
            "{text}"
        );
    }

    /// A line of 2,000 ASCII bytes with `ab` at column 1000: a span on `ab` is cut, and the
    /// renderer gives column 385 for it.
    fn long_line() -> String {
        let pad = "y".repeat(999);
        format!("{pad}ab{pad}\n")
    }

    /// The span of `ab` on line `k` of `source`, a line from `long_line`.
    fn ab(source: &str, k: usize) -> Span {
        let start = LineIndex::new(source)
            .line_start(k)
            .expect("the line should be in the source")
            + 999;
        Span::new(start, start + 2)
    }

    /// Line 10 shows the text of the `:::` header before the renderer gives that header.
    #[test]
    fn source_text_like_a_later_header_should_not_change() {
        let source = format!("{}{}x // t.sisu:1:385\n", long_line(), &numbered(9)[3..]);
        let d = error(Span::new(source.len() - 18, source.len() - 17), "m")
            .with_secondary(ab(&source, 1), "s");
        assert_eq!(rendered(&d, &source), render_whole(&d, "t.sisu", &source));
    }

    #[test]
    fn source_text_like_a_later_header_should_not_change_in_styled_output() {
        let source = format!("{}{}x // t.sisu:1:385\n", long_line(), &numbered(9)[3..]);
        let d = error(Span::new(source.len() - 18, source.len() - 17), "m")
            .with_secondary(ab(&source, 1), "s");
        let text = d.render("t.sisu", &source, &LineIndex::new(&source), true);
        assert!(text.contains("// t.sisu:1:385"), "{text}");
        assert_eq!(text.matches("t.sisu:1:1000").count(), 1, "{text}");
    }

    /// The label with the lower index is to the right of the other on the far cut line.
    #[test]
    fn a_far_header_should_name_the_leftmost_label_of_its_line() {
        let pad = "y".repeat(999);
        let source = format!("{pad}ab{}cd{pad}\n{}", "y".repeat(100), &numbered(10)[3..]);
        let d = error(line(&source, 10), "m")
            .with_secondary(Span::new(1101, 1103), "right")
            .with_secondary(Span::new(999, 1001), "left");
        let text = rendered(&d, &source);
        assert_eq!(
            headers(&text),
            ["--> t.sisu:10:1", "::: t.sisu:1:1000"],
            "{text}"
        );
    }

    /// Text that a header shows, in a later snippet and on a cut line of its own snippet: both
    /// render as the whole source does, before and after the repair reads only header lines.
    #[test]
    fn source_text_like_an_earlier_header_should_not_change() {
        let later = format!("{}{}x // t.sisu:1:385\n", long_line(), &numbered(9)[3..]);
        let pad = "y".repeat(999);
        let own = format!("{pad}ab t.sisu:1:385 {pad}\n{}", &numbered(10)[3..]);
        let cases = [
            (
                error(ab(&later, 1), "m")
                    .with_secondary(Span::new(later.len() - 18, later.len() - 17), "s"),
                later.as_str(),
            ),
            (
                error(line(&own, 10), "m").with_secondary(Span::new(999, 1001), "s"),
                own.as_str(),
            ),
        ];
        for (d, source) in &cases {
            assert_eq!(
                rendered(d, source),
                render_whole(d, "t.sisu", source),
                "{d:?}"
            );
        }
    }

    /// 64 far labels, each on its own cut line, and a cut primary span: each header names its
    /// true column.
    #[test]
    fn many_far_cut_labels_should_each_keep_their_column() {
        let block = format!("{}f\nf\nf\nf\n", long_line());
        let source = block.repeat(65);
        let lines = |k: usize| 5 * k + 1;
        let mut d = error(ab(&source, lines(64)), "m");
        for k in 0..64 {
            d = d.with_secondary(ab(&source, lines(k)), "s");
        }
        let want: Vec<String> = std::iter::once(format!("--> t.sisu:{}:1000", lines(64)))
            .chain((0..64).map(|k| format!("::: t.sisu:{}:1000", lines(k))))
            .collect();
        assert_eq!(headers(&rendered(&d, &source)), want);
    }

    /// The sum of the bytes of the text that the renderer gets for `d`.
    fn excerpt_bytes(d: &Diagnostic, source: &str, lines: &LineIndex) -> usize {
        let spans: Vec<Span> = d.secondary().iter().map(|(s, _)| *s).collect();
        excerpts(source, lines, d.span(), &spans)
            .iter()
            .map(|(e, _)| e.text.len())
            .sum()
    }

    #[test]
    fn a_far_origin_should_cost_a_bounded_source() {
        let source = numbered(5002);
        let d = error(line(&source, 5002), "m").with_secondary(line(&source, 1), "s");
        let bytes = excerpt_bytes(&d, &source, &LineIndex::new(&source));
        assert!(bytes < 1024, "{bytes} bytes");
    }

    /// Each case line, and each line between line 3 and the first case, has the same length at
    /// every `N`. The first case is more than 3 lines after line 3. So each diagnostic costs
    /// the same, and the sum is exactly `N` times that cost. The ratio is then 4 for correct
    /// code, and a line that grows with `N` fails the test.
    #[test]
    fn many_labels_on_one_early_line_should_cost_linear_source() {
        let costs: Vec<usize> = [64, 256, 1024]
            .into_iter()
            .map(|n| {
                let cases: String = (0..n)
                    .map(|i| format!("case {i:05}\n"))
                    .collect::<Vec<_>>()
                    .concat();
                let source = format!("a\nb\nlet x = 1\n{}{cases}", "case -----\n".repeat(4));
                let lines = LineIndex::new(&source);
                let label = line(&source, 3);
                (0..n)
                    .map(|i| {
                        let start = lines
                            .line_start(8 + i)
                            .expect("the case should be in the source");
                        let d = error(Span::new(start, start + 10), "m").with_secondary(label, "s");
                        excerpt_bytes(&d, &source, &lines)
                    })
                    .sum()
            })
            .collect();
        for pair in costs.windows(2) {
            assert!(pair[1] <= 4 * pair[0], "{costs:?} bytes");
        }
    }

    /// The label on line 14 is 2 lines after the end of the primary span, and 10 lines after
    /// the label on line 4.
    #[test]
    fn a_label_near_the_end_of_a_long_span_should_share_its_snippet() {
        let source = numbered(16);
        let primary = line(&source, 2).to(line(&source, 12));
        let d = error(primary, "m")
            .with_secondary(line(&source, 4), "s")
            .with_secondary(line(&source, 14), "s");
        assert_eq!(headers(&rendered(&d, &source)), ["--> t.sisu:2:1"]);
    }

    /// Whether the renderer gets less than the whole lines of `d`'s main snippet.
    fn is_cut(d: &Diagnostic, source: &str) -> bool {
        let lines = LineIndex::new(source);
        let spans: Vec<Span> = d.secondary().iter().map(|(s, _)| *s).collect();
        let (excerpt, held) = excerpts(source, &lines, d.span(), &spans).swap_remove(0);
        let labelled = std::iter::once(d.span()).chain(held.iter().map(|&i| spans[i]));
        let (range, _) = window(source, &lines, labelled);
        excerpt.text.len() < range.len()
    }

    /// A source, a diagnostic, and whether the renderer should get less than the whole lines.
    type Case = (String, Diagnostic, bool);

    /// Each case renders as the whole source does, and is cut or not as it says.
    fn check_cut_cases(cases: &[Case]) {
        for (source, d, cut) in cases {
            assert_eq!(is_cut(d, source), *cut, "cut of {d:?}");
            assert_eq!(
                d.render("t.sisu", source, &LineIndex::new(source), false),
                render_whole(d, "t.sisu", source),
                "{d:?}"
            );
        }
    }

    /// Long lines: each row of a cut that should apply, and of a cut that should not, renders
    /// as the whole source does.
    #[test]
    fn a_cut_long_line_should_render_as_the_whole_line_does() {
        let pad = "y".repeat(3000);
        let ascii = format!(
            "fn a(u: i64) {{ p(\"{pad}\") }} fn b(v: i64) {{ p(\"{pad}\") }} fn c(w: i64) {{}}\n"
        );
        let at = |source: &str, name: &str| between(source, name, name);
        let mut cases: Vec<Case> = Vec::new();
        let mut add =
            |source: &str, d: Diagnostic, cut: bool| cases.push((source.to_string(), d, cut));
        // Near the start, in the middle, near the end of the line.
        for name in ["u: ", "v: ", "w: "] {
            add(
                &ascii,
                error(at(&ascii, name), "m").with_label("here"),
                true,
            );
        }
        // A warning, an empty span, and a secondary label at the other end of the line.
        add(
            &ascii,
            Diagnostic::coded(Code::NeverReassigned, at(&ascii, "v: "), "m"),
            true,
        );
        let end = ascii.trim_end().len();
        add(
            &ascii,
            error(Span::new(end, end), "m").with_label("end of line"),
            true,
        );
        // Spans at both ends of the line leave nothing to cut; spans at both ends of a part of
        // it leave the rest.
        add(
            &ascii,
            error(at(&ascii, "w: "), "m")
                .with_secondary(at(&ascii, "u: "), "first")
                .with_help("h"),
            false,
        );
        add(
            &ascii,
            error(at(&ascii, "v: "), "m")
                .with_secondary(at(&ascii, "u: "), "first")
                .with_help("h"),
            true,
        );
        // A span that covers most of the line.
        add(&ascii, error(between(&ascii, "fn a", "fn c"), "m"), false);
        // The end of the file, with and without a final newline.
        let eof = format!("fn main() {{ p(\"{pad}{pad}\"");
        add(
            &eof,
            error(Span::new(eof.len(), eof.len()), "m").with_label("end of file"),
            true,
        );
        let eof_newline = format!("{eof}\n");
        add(
            &eof_newline,
            error(Span::new(eof_newline.len(), eof_newline.len()), "m").with_label("end of file"),
            false,
        );
        add(
            &eof_newline,
            error(Span::new(eof.len(), eof.len()), "m").with_label("end of line"),
            true,
        );
        // Text that is not ASCII before and after the span: the renderer gets the whole lines.
        for filler in ["ä", "日本語"] {
            let wide = format!(
                "fn a(u: i64) {{ p(\"{}{pad}\") }} fn b(v: i64) {{ p(\"{pad}{}\") }}\n",
                filler.repeat(40),
                filler.repeat(40)
            );
            for name in ["u: ", "v: "] {
                add(&wide, error(at(&wide, name), "m").with_label("here"), false);
            }
        }
        // A run of 600 spaces where the cut would start, and then `n` bytes before the span. At
        // 80, fewer than `LEFT_MIN` bytes would stay before the span, so the line is not cut; at
        // 300 the cut starts after the run.
        for (n, cut) in [(80, false), (300, true)] {
            let run = format!(
                "fn main() {{\n    let a = 1;{}let b = \"{}\"; let x: i64 = true\n}}\n",
                " ".repeat(600),
                "y".repeat(n)
            );
            add(&run, error(at(&run, "x: "), "m").with_label("here"), cut);
        }
        // A wide line before the long line is not drawn, so it does not stop the cut.
        for before in ["x".repeat(300), "x".repeat(10_000), "\t".repeat(300)] {
            for eol in ["\n", "\r\n"] {
                let prev = format!("{before}{eol}fn a(u: i64) {{ p(\"{pad}\") }}\n");
                add(&prev, error(at(&prev, "u: "), "m"), true);
            }
        }
        check_cut_cases(&cases);
    }

    /// Long lines with tabs, an indent, a fix, spans on two lines, and a line before them.
    #[test]
    fn a_long_line_among_other_lines_should_render_as_the_whole_source_does() {
        let pad = "y".repeat(3000);
        let ascii = format!("fn a(u: i64) {{ p(\"{pad}\") }} fn b(v: i64) {{}}\n");
        let at = |source: &str, name: &str| between(source, name, name);
        let mut cases: Vec<Case> = Vec::new();
        let mut add =
            |source: &str, d: Diagnostic, cut: bool| cases.push((source.to_string(), d, cut));
        // Tabs before and after the span, in the line and as its indent.
        let tabs = format!("fn x() {{\tp(\"{pad}\")\t}}\tfn a(u: i64) {{\t\tp(\"{pad}\")\t}}\n");
        add(&tabs, error(at(&tabs, "u: "), "m").with_label("here"), true);
        let indented = format!("fn main() {{\n\t\tp(\"{pad}\"); let n = 0; n = 1\n}}\n");
        add(
            &indented,
            error(between(&indented, "n = 1", "n = 1"), "m")
                .with_secondary(at(&indented, "n = 0"), "s"),
            true,
        );
        // An indent of 8 columns and one of 40.
        for indent in [8, 40] {
            let spaces = " ".repeat(indent);
            let deep = format!("fn f() {{\n{spaces}p(\"{pad}\"); x\n}}\n");
            add(&deep, error(between(&deep, "x\n", "x"), "m"), true);
        }
        // Only white space near the spans, and a deeper indent on the line before.
        let blank = format!("{}x\n  a{}b\n", " ".repeat(30), " ".repeat(1500));
        let in_blank = blank.find("b\n").expect("the line should end in `b`") - 700;
        add(&blank, error(Span::new(in_blank, in_blank), "m"), false);
        // A span in white space, with the text more than a margin after it: the kept part has none.
        let spaces = format!("{}x\n", " ".repeat(800));
        add(&spaces, error(Span::new(5, 5), "m"), false);
        // A fix on the long line; its snippet shows the whole line, and is not cut.
        add(
            &ascii,
            error(at(&ascii, "v: "), "m").with_fix(fix(&[(at(&ascii, "v: "), "_v")])),
            true,
        );
        // Spans on two lines, one of them long; a line before the long line.
        let two = format!("fn main() {{\n    f(1,\n        2 + {pad})\n}}\n");
        add(&two, error(between(&two, "f(1", "2 + y"), "m"), false);
        add(
            &two,
            error(between(&two, "f(1", "f(1"), "m").with_secondary(between(&two, "y)", "y)"), "s"),
            false,
        );
        for before in ["x", &"x".repeat(200), &"x".repeat(400)] {
            let prev = format!("{before}\nfn a(u: i64) {{ p(\"{pad}\") }}\n");
            add(&prev, error(at(&prev, "u: "), "m"), true);
        }
        check_cut_cases(&cases);
    }

    /// The span moves along a line that is cut at its start, its middle and its end, with a
    /// label, a secondary span on each side, a line number of one, two and three digits, and a
    /// run of white space in the middle of the line, so that the cut starts in it for some spans.
    #[test]
    fn a_span_at_any_column_of_a_long_line_should_render_as_the_whole_source_does() {
        let half = "ab cd ".repeat(500);
        for run in ["", &" ".repeat(200), &" ".repeat(600), &"\t".repeat(150)] {
            let line = format!("{half}{run}{half}");
            let before: &[usize] = if run.is_empty() { &[0, 11, 120] } else { &[11] };
            for &lines_before in before {
                let source = format!("{}// x\n    {line}\n", "\n".repeat(lines_before));
                let start = source.find("ab").expect("the line should start with `ab`");
                // Every 101st column, and the columns near the end of the run, where the cut
                // starts in it.
                let run_end = half.len() + run.len();
                let columns = (0..line.len() - 2)
                    .step_by(101)
                    .chain(
                        (run_end - 100..run_end + 700)
                            .step_by(37)
                            .filter(|_| !run.is_empty()),
                    )
                    .chain(line.len() - 8..line.len() - 2);
                for column in columns {
                    let span = Span::new(start + column, start + column + 2);
                    let alone = error(span, "m").with_label("here");
                    let mut with_others = alone.clone();
                    for other in [
                        column.checked_sub(900),
                        Some(column + 900),
                        Some(column + 5),
                    ] {
                        if let Some(other) = other.filter(|o| o + 2 < line.len()) {
                            let at = start + other;
                            with_others =
                                with_others.with_secondary(Span::new(at, at + 2), "other");
                        }
                    }
                    for d in [alone, with_others] {
                        assert_eq!(
                            d.render("t.sisu", &source, &LineIndex::new(&source), false),
                            render_whole(&d, "t.sisu", &source),
                            "{lines_before} lines before, a run of {} bytes, column {column}",
                            run.len()
                        );
                    }
                }
            }
        }
    }

    /// A path that holds a line break, or text like a header, still gets the true column of a cut
    /// line: for a cut primary span and for a cut far label, plain and styled.
    #[test]
    fn a_header_whose_path_holds_a_line_break_should_give_the_true_column() {
        let source = format!("{}{}", long_line(), &numbered(10)[3..]);
        let cases = [
            error(ab(&source, 1), "m"),
            error(line(&source, 10), "m").with_secondary(ab(&source, 1), "s"),
        ];
        for path in [
            "long\nname.sisu",
            "end.sisu\n",
            "a\n--> b.sisu",
            "a --> b.sisu",
            "a ::: b.sisu",
        ] {
            for d in &cases {
                for color in [false, true] {
                    let text = d.render(path, &source, &LineIndex::new(&source), color);
                    assert!(
                        text.contains(&format!("{path}:1:1000")),
                        "{path:?}, color {color}: {text}"
                    );
                }
            }
        }
    }

    /// With the styled renderer the header's column is still the true one.
    #[test]
    fn styled_header_of_a_cut_line_should_give_the_true_column() {
        let pad = "y".repeat(3000);
        let source = format!("fn a(u: i64) {{ p(\"{pad}\") }} fn b(v: i64) {{}}\n");
        let d = error(between(&source, "v: ", "v: "), "m");
        let at = source.find("v: ").expect("the source should hold `v: `") + 1;
        assert!(is_cut(&d, &source));
        for color in [false, true] {
            let rendered = d.render("t.sisu", &source, &LineIndex::new(&source), color);
            assert!(rendered.contains(&format!("t.sisu:1:{at}")), "{rendered}");
        }
    }

    /// The text that the renderer gets of a long line does not grow with the line, or with the
    /// line before it.
    #[test]
    fn the_text_that_the_renderer_gets_should_not_grow_with_the_line() {
        for before in ["a", &"w".repeat(10_000)] {
            for len in [10_000, 100_000, 1_000_000] {
                let source = format!("{before}\n{}\n", "z".repeat(len));
                let lines = LineIndex::new(&source);
                let first = before.len() + 1;
                let mut longest = 0;
                for at in (first..first + len).step_by(len / 50) {
                    let span = Span::new(at, at + 3);
                    let excerpt = Excerpt::new(&source, &lines, std::iter::once(span), span);
                    longest = longest.max(excerpt.text.len());
                    assert_eq!(excerpt.rebased(span).len(), 3, "span at {at}");
                }
                // The break of the line before, the span, a margin on each side, and the line
                // break.
                assert!(
                    longest <= 1 + 3 + 2 * MARGIN + 1,
                    "{longest} bytes for a line of {len} after one of {}",
                    before.len()
                );
            }
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
