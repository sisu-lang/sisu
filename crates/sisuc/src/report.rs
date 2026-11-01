//! The reporter: everything `sisu` writes to stderr after the arguments parse, as text or as
//! JSON Lines, and the summary that ends it.

use std::ffi::OsStr;
use std::io::Write;

use crate::diagnostic::{Code, Diagnostic, Severity, json};
use crate::front_end;
use crate::source::LineIndex;

/// How the reporter writes.
#[derive(Clone, clap::ValueEnum)]
pub(crate) enum Format {
    Text,
    Json,
}

/// Writes diagnostics and tool errors to `out`, and counts the diagnostics for the summary.
pub(crate) struct Reporter<W: Write> {
    out: W,
    format: Format,
    color: bool,
    errors: usize,
    warnings: usize,
    codes: Vec<Code>,
}

impl<W: Write> Reporter<W> {
    pub(crate) fn new(out: W, format: Format, color: bool) -> Self {
        Reporter {
            out,
            format,
            color,
            errors: 0,
            warnings: 0,
            codes: Vec::new(),
        }
    }

    /// Writes each of `diagnostics` about `source`, read from `path`.
    pub(crate) fn diagnostics(&mut self, path: &str, source: &str, diagnostics: &[Diagnostic]) {
        // A clean source, as `--emit` passes it, needs no line index.
        if diagnostics.is_empty() {
            return;
        }
        let lines = LineIndex::new(source);
        for d in &front_end::sound_fixes(source, diagnostics) {
            match d.severity() {
                Severity::Error => self.errors += 1,
                Severity::Warning => self.warnings += 1,
            }
            self.codes.push(d.code());
            let text = match self.format {
                // A blank line after each diagnostic.
                Format::Text => format!("{}\n", d.render(path, source, &lines, self.color)),
                Format::Json => json::diagnostic(d, path, source, &lines),
            };
            self.line(&text);
        }
    }

    /// Writes a tool error; `message` has no `sisu: ` prefix.
    pub(crate) fn tool_error(&mut self, message: &str) {
        let text = tool_error_line(&self.format, message);
        self.line(&text);
    }

    /// Writes each of `lines`, the `timings:` lines of `--timings`.
    pub(crate) fn timings(&mut self, lines: &[String]) {
        for line in lines {
            self.line(line);
        }
    }

    /// Writes the summary. Text has one only after a diagnostic; JSON always ends with one.
    pub(crate) fn finish(mut self) {
        self.codes.sort_by_key(|code| code.as_str());
        self.codes.dedup();
        match self.format {
            Format::Text => {
                let counts: Vec<String> = [(self.errors, "error"), (self.warnings, "warning")]
                    .into_iter()
                    .filter(|&(n, _)| n > 0)
                    .map(|(n, noun)| format!("{n} {noun}{}", if n == 1 { "" } else { "s" }))
                    .collect();
                if counts.is_empty() {
                    return;
                }
                self.line(&format!("sisu: {}", counts.join(", ")));
                // Every counted diagnostic has a code, so there is at least one.
                let codes: Vec<&str> = self.codes.iter().map(|code| code.as_str()).collect();
                self.line(&format!(
                    "sisu: for more on a code, run `sisu explain <code>`: {}",
                    codes.join(", ")
                ));
            }
            Format::Json => {
                let summary = json::summary(self.errors, self.warnings, &self.codes);
                self.line(&summary);
            }
        }
    }

    fn line(&mut self, text: &str) {
        writeln!(self.out, "{text}")
            .expect("stderr should take the report, as `eprintln!` assumes");
    }
}

/// A tool error in `format`, without its newline.
fn tool_error_line(format: &Format, message: &str) -> String {
    match format {
        Format::Text => format!("sisu: {message}"),
        Format::Json => json::tool_error(message),
    }
}

/// Writes a tool error to stderr after `finish` has written the summary: the one exception to
/// the summary being the last line, for what goes wrong in `sisu run` once the program is
/// built.
#[expect(
    clippy::print_stderr,
    reason = "a failed write to stderr has nowhere left to be reported"
)]
pub(crate) fn tool_error_after_summary(format: &Format, message: &str) {
    eprintln!("{}", tool_error_line(format, message));
}

/// Whether text output is colored: only on a terminal, and only when `NO_COLOR` is unset or
/// empty.
pub(crate) fn use_color(stderr_is_terminal: bool, no_color: Option<&OsStr>) -> bool {
    stderr_is_terminal && no_color.is_none_or(OsStr::is_empty)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{DiagnosticKind, Immutable};
    use crate::source::Span;

    const SOURCE: &str = "fn main() {\n    let n = 0\n    n = 1\n    let mut m = 0\n}\n";

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

    fn never_reassigned() -> Diagnostic {
        DiagnosticKind::NeverReassigned {
            name: "m".to_string(),
            at: Span::new(48, 49),
            keyword: Span::new(44, 47),
        }
        .into()
    }

    /// Everything a reporter in `format` writes for `diagnostics`, then `tool_error`, if any,
    /// and the summary.
    fn report(format: Format, diagnostics: &[Diagnostic], tool_error: Option<&str>) -> String {
        let mut out = Vec::new();
        let mut reporter = Reporter::new(&mut out, format, false);
        reporter.diagnostics("a.sisu", SOURCE, diagnostics);
        if let Some(message) = tool_error {
            reporter.tool_error(message);
        }
        reporter.finish();
        String::from_utf8(out).expect("the report should be UTF-8")
    }

    #[test]
    fn text_summary_counts_and_lists_codes() {
        let out = report(
            Format::Text,
            &[assign(), never_reassigned(), assign()],
            None,
        );
        assert!(
            out.ends_with(
                "\n\nsisu: 2 errors, 1 warning\nsisu: for more on a code, run `sisu explain <code>`: E0001, W0002\n"
            ),
            "{out}"
        );
    }

    #[test]
    fn text_singular_and_omitted_counts() {
        let out = report(Format::Text, &[never_reassigned()], None);
        assert!(
            out.ends_with(
                "\n\nsisu: 1 warning\nsisu: for more on a code, run `sisu explain <code>`: W0002\n"
            ),
            "{out}"
        );
    }

    #[test]
    fn json_summary_lists_each_code_once() {
        let out = report(
            Format::Json,
            &[assign(), never_reassigned(), assign()],
            None,
        );
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 4, "{out}");
        let summary: serde_json::Value =
            serde_json::from_str(lines[3]).expect("the summary should be JSON");
        assert_eq!(summary["kind"], "summary");
        assert_eq!(summary["errors"], 2);
        assert_eq!(summary["warnings"], 1);
        let codes: Vec<&String> = summary["explanations"]
            .as_object()
            .expect("explanations should be an object")
            .keys()
            .collect();
        assert_eq!(codes, ["E0001", "W0002"]);
    }

    #[test]
    fn text_clean_run_prints_nothing() {
        assert_eq!(report(Format::Text, &[], None), "");
    }

    #[test]
    fn text_tool_error_line() {
        assert_eq!(
            report(Format::Text, &[], Some("cannot read x")),
            "sisu: cannot read x\n"
        );
    }

    #[test]
    fn json_clean_run_prints_only_the_summary() {
        assert_eq!(
            report(Format::Json, &[], None),
            "{\"version\":1,\"kind\":\"summary\",\"errors\":0,\"warnings\":0,\"explanations\":{}}\n"
        );
    }

    #[test]
    fn json_tool_error_then_summary() {
        let out = report(Format::Json, &[], Some("cannot read x"));
        assert_eq!(
            out.lines().collect::<Vec<_>>(),
            [
                r#"{"version":1,"kind":"tool_error","message":"cannot read x"}"#,
                r#"{"version":1,"kind":"summary","errors":0,"warnings":0,"explanations":{}}"#
            ]
        );
    }

    #[test]
    fn color_reaches_text_only() {
        for (format, colored) in [(Format::Text, true), (Format::Json, false)] {
            let mut out = Vec::new();
            let mut reporter = Reporter::new(&mut out, format, true);
            reporter.diagnostics("a.sisu", SOURCE, &[assign()]);
            reporter.finish();
            let out = String::from_utf8(out).expect("the report should be UTF-8");
            assert_eq!(out.contains("\x1b["), colored, "{out}");
        }
    }

    #[test]
    fn color_follows_terminal_and_no_color() {
        assert!(use_color(true, None));
        assert!(use_color(true, Some(OsStr::new(""))));
        assert!(!use_color(true, Some(OsStr::new("1"))));
        assert!(!use_color(false, None));
    }
}
