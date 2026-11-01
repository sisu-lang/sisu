//! JSON Lines output: one object per diagnostic, per tool error, and the summary that ends the
//! stream. `docs/diagnostics/README.md` describes every key.

use std::collections::BTreeMap;

use serde::Serialize;

use super::{Code, Diagnostic, Edit, Fix, Severity};
use crate::source::{LineIndex, Span};

/// Bumped on any breaking change to the objects.
const VERSION: u32 = 1;

/// The field order of each struct is the key order of its object.
#[derive(Serialize)]
struct DiagnosticLine<'a> {
    version: u32,
    kind: &'static str,
    code: &'static str,
    severity: &'static str,
    message: &'a str,
    primary: Location<'a>,
    secondary: Vec<Location<'a>>,
    help: Option<&'a str>,
    fixes: Vec<FixObject<'a>>,
}

/// A labelled span: byte offsets, and the 1-based line and char column of `start`.
#[derive(Serialize)]
struct Location<'a> {
    file: &'a str,
    start: usize,
    end: usize,
    line: usize,
    column: usize,
    label: Option<&'a str>,
}

#[derive(Serialize)]
struct FixObject<'a> {
    message: &'a str,
    safe: bool,
    edits: Vec<EditObject<'a>>,
}

#[derive(Serialize)]
struct EditObject<'a> {
    start: usize,
    end: usize,
    line: usize,
    column: usize,
    replacement: &'a str,
}

#[derive(Serialize)]
struct ToolErrorLine<'a> {
    version: u32,
    kind: &'static str,
    message: &'a str,
}

#[derive(Serialize)]
struct SummaryLine {
    version: u32,
    kind: &'static str,
    errors: usize,
    warnings: usize,
    /// A `BTreeMap`, so the keys come out sorted by code.
    explanations: BTreeMap<&'static str, &'static str>,
}

/// The JSON line for `d`, without its `'\n'`; `path` is the input as given, and `lines` is the
/// index of `source`.
pub(crate) fn diagnostic(d: &Diagnostic, path: &str, source: &str, lines: &LineIndex) -> String {
    let location = |span, label| Location::new(path, source, lines, span, label);
    to_line(&DiagnosticLine {
        version: VERSION,
        kind: "diagnostic",
        code: d.code().as_str(),
        severity: match d.severity() {
            Severity::Error => "error",
            Severity::Warning => "warning",
        },
        message: d.message(),
        primary: location(d.span(), d.label()),
        secondary: d
            .secondary()
            .iter()
            .map(|(span, label)| location(*span, Some(label)))
            .collect(),
        help: d.help(),
        fixes: d
            .fixes()
            .iter()
            .map(|fix| FixObject::new(fix, source, lines))
            .collect(),
    })
}

impl<'a> Location<'a> {
    fn new(
        file: &'a str,
        source: &str,
        lines: &LineIndex,
        span: Span,
        label: Option<&'a str>,
    ) -> Self {
        let (line, column) = lines.line_col(source, span.start());
        Location {
            file,
            start: span.start(),
            end: span.end(),
            line,
            column,
            label,
        }
    }
}

impl<'a> FixObject<'a> {
    fn new(fix: &'a Fix, source: &str, lines: &LineIndex) -> Self {
        FixObject {
            message: fix.message(),
            safe: fix.safe(),
            edits: fix
                .edits()
                .iter()
                .map(|edit| EditObject::new(edit, source, lines))
                .collect(),
        }
    }
}

impl<'a> EditObject<'a> {
    fn new(edit: &'a Edit, source: &str, lines: &LineIndex) -> Self {
        let (line, column) = lines.line_col(source, edit.span().start());
        EditObject {
            start: edit.span().start(),
            end: edit.span().end(),
            line,
            column,
            replacement: edit.replacement(),
        }
    }
}

/// The JSON line for a tool error, without its `'\n'`.
pub(crate) fn tool_error(message: &str) -> String {
    to_line(&ToolErrorLine {
        version: VERSION,
        kind: "tool_error",
        message,
    })
}

/// The JSON summary line, without its `'\n'`: the counts, and each of `codes`' explanation.
pub(crate) fn summary(errors: usize, warnings: usize, codes: &[Code]) -> String {
    to_line(&SummaryLine {
        version: VERSION,
        kind: "summary",
        errors,
        warnings,
        explanations: codes
            .iter()
            .map(|code| (code.as_str(), code.explanation()))
            .collect(),
    })
}

fn to_line(value: &impl Serialize) -> String {
    serde_json::to_string(value)
        .expect("serializing should not fail: every map key is a string and no writer can fail")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::DiagnosticKind;
    use crate::front_end::front_end;
    use crate::source::Span;

    #[test]
    fn json_offsets_are_bytes() {
        // The `ä` on line 1 takes two bytes, so byte offsets run one ahead of char offsets.
        let source = "// ä\nfn main() {\n    let n = 0\n    n = 1\n}\n";
        let (_, diagnostics) = front_end(source);
        let [d] = diagnostics.as_slice() else {
            panic!("expected one diagnostic, got {diagnostics:?}");
        };
        assert_eq!(
            diagnostic(d, "a.sisu", source, &LineIndex::new(source)),
            r#"{"version":1,"kind":"diagnostic","code":"E0001","severity":"error","message":"cannot assign to `n`","primary":{"file":"a.sisu","start":36,"end":41,"line":4,"column":5,"label":"cannot assign twice"},"secondary":[{"file":"a.sisu","start":26,"end":27,"line":3,"column":9,"label":"declared with `let` here"}],"help":null,"fixes":[{"message":"declare it with `let mut`","safe":true,"edits":[{"start":26,"end":26,"line":3,"column":9,"replacement":"mut "}]}]}"#
        );
    }

    #[test]
    fn may_be_none_with_an_access_should_have_help_and_an_unsafe_fix() {
        let source = "fn f(n: N?) {\n    print(n.v)\n}\n";
        let d: Diagnostic = DiagnosticKind::MayBeNone {
            value: Some("n".to_string()),
            at: Span::new(24, 25),
            access: Some(Span::new(25, 26)),
            write: false,
            peel: None,
        }
        .into();
        assert_eq!(
            diagnostic(&d, "a.sisu", source, &LineIndex::new(source)),
            r#"{"version":1,"kind":"diagnostic","code":"E0062","severity":"error","message":"`n` may be `None`","primary":{"file":"a.sisu","start":24,"end":25,"line":2,"column":11,"label":null},"secondary":[],"help":"to use the value only when it is present, unwrap it with `if let`","fixes":[{"message":"if a `None` result is fine, use `?.`","safe":false,"edits":[{"start":25,"end":26,"line":2,"column":12,"replacement":"?."}]}]}"#
        );
    }

    #[test]
    fn tool_error_line() {
        assert_eq!(
            tool_error("x \"y\""),
            r#"{"version":1,"kind":"tool_error","message":"x \"y\""}"#
        );
    }

    #[test]
    fn summary_orders_explanations_by_code() {
        let escaped = |code: Code| {
            serde_json::to_string(code.explanation()).expect("a string should serialize")
        };
        assert_eq!(
            summary(1, 1, &[Code::NeverReassigned, Code::AssignToImmutable]),
            format!(
                r#"{{"version":1,"kind":"summary","errors":1,"warnings":1,"explanations":{{"E0001":{},"W0002":{}}}}}"#,
                escaped(Code::AssignToImmutable),
                escaped(Code::NeverReassigned)
            )
        );
    }
}
