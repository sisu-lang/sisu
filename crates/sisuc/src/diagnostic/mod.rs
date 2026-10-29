//! Diagnostics: what the compiler reports about the source, and how it renders them.

#[cfg(test)]
pub(crate) mod explanations;
pub(crate) mod json;
mod registry;
mod render;
mod suggest;

pub(crate) use registry::{
    AddBound, Annotate, ArgumentsForm, BadRange, BuiltIn, CannotExtendForm, ClashForm, ClashKind,
    Code, ConformanceForm, Conversion, Definition, DiagnosticKind, Earlier, ForeignPattern,
    ForeignRange, GuardHelp, Hop, Immutable, InterfaceBodyForm, Join, LetField, Literal,
    LiteralKind, MainForm, MatchIdiomForm, MathFix, Member, MethodReceiver, MismatchFix,
    MismatchSource, NanFix, NoOperatorForm, NotConformForm, NotInterface, Operand, Owner, Panic,
    Peel, PowerFix, PrecisionHelp, PrintProblem, RETIRED, ReceiverArgument, Spelled, TypedLiteral,
    Unbuilt, Ungenerated, UnknownForm, Unwrap, UsedBy, VariantLabels, WideKind, invisible,
};
pub(crate) use suggest::{Suggestion, alias, closest, suggestion};

use crate::source::Span;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Severity {
    Error,
    Warning,
}

/// One error or warning about the source. Only `diagnostic` and its modules build one.
#[derive(Clone, Debug)]
pub(crate) struct Diagnostic {
    /// Its severity is the code's.
    code: Code,
    /// `Box<str>`, like `label` and `help`, keeps a `Diagnostic` under clippy's limit for a
    /// `Result`'s error.
    message: Box<str>,
    /// The offending code, underlined `^^^`.
    span: Span,
    /// Text under the primary span.
    label: Option<Box<str>>,
    /// Other spans that explain the error, underlined `---`.
    secondary: Vec<(Span, String)>,
    /// Advice that is not an edit.
    help: Option<Box<str>>,
    /// Alternative edits, each of which resolves the diagnostic.
    fixes: Vec<Fix>,
}

/// An edit to the source, with a message, that resolves a diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Fix {
    message: String,
    /// Applying the edits always removes the diagnostic, and the source still parses.
    safe: bool,
    /// They apply together, to the original source, and never overlap.
    edits: Vec<Edit>,
}

/// Replaces `span` of the original source with `replacement`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Edit {
    span: Span,
    replacement: String,
}

impl Diagnostic {
    /// A diagnostic of `code`, with the code's severity.
    fn coded(code: Code, span: Span, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            code,
            message: message.into().into_boxed_str(),
            span,
            label: None,
            secondary: Vec::new(),
            help: None,
            fixes: Vec::new(),
        }
    }

    fn with_label(mut self, text: impl Into<String>) -> Diagnostic {
        self.label = Some(text.into().into_boxed_str());
        self
    }

    fn with_secondary(mut self, span: Span, text: impl Into<String>) -> Diagnostic {
        self.secondary.push((span, text.into()));
        self
    }

    fn with_help(mut self, text: impl Into<String>) -> Diagnostic {
        self.help = Some(text.into().into_boxed_str());
        self
    }

    fn with_fix(mut self, fix: Fix) -> Diagnostic {
        self.fixes.push(fix);
        self
    }

    pub(crate) fn code(&self) -> Code {
        self.code
    }

    pub(crate) fn severity(&self) -> Severity {
        self.code.severity()
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    pub(crate) fn span(&self) -> Span {
        self.span
    }

    pub(crate) fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    pub(crate) fn secondary(&self) -> &[(Span, String)] {
        &self.secondary
    }

    pub(crate) fn help(&self) -> Option<&str> {
        self.help.as_deref()
    }

    pub(crate) fn fixes(&self) -> &[Fix] {
        &self.fixes
    }

    /// Keeps only the fixes that `keep` accepts.
    pub(crate) fn retain_fixes(&mut self, keep: impl FnMut(&Fix) -> bool) {
        self.fixes.retain(keep);
    }

    /// Marks unsafe each fix that `risky` accepts.
    pub(crate) fn demote_fixes(&mut self, mut risky: impl FnMut(&Fix) -> bool) {
        for fix in self.fixes.iter_mut().filter(|f| risky(f)) {
            fix.safe = false;
        }
    }

    /// A diagnostic at `span`, with each of `secondary`, and one fix of `edits` unless there is
    /// none: spans that no rule makes, for the tests of the span checks.
    #[cfg(test)]
    pub(crate) fn for_test(span: Span, secondary: &[Span], edits: &[(Span, &str)]) -> Diagnostic {
        let mut d = Diagnostic::coded(Code::InvalidUtf8, span, "");
        d.secondary = secondary.iter().map(|&s| (s, String::new())).collect();
        if !edits.is_empty() {
            d.fixes.push(Fix {
                message: String::new(),
                safe: false,
                edits: edits
                    .iter()
                    .map(|&(span, replacement)| Edit {
                        span,
                        replacement: replacement.to_string(),
                    })
                    .collect(),
            });
        }
        d
    }

    /// `self` with only the safe fix `edits`: a fix that no rule makes, for the tests of the
    /// safe-fix checks.
    #[cfg(test)]
    pub(crate) fn with_safe_fix_for_test(mut self, edits: &[(Span, &str)]) -> Diagnostic {
        self.fixes = vec![Fix {
            message: String::new(),
            safe: true,
            edits: edits
                .iter()
                .map(|&(span, replacement)| Edit {
                    span,
                    replacement: replacement.to_string(),
                })
                .collect(),
        }];
        self
    }
}

impl Fix {
    /// `source` with the edits applied. Two insertions at one offset keep their order.
    pub(crate) fn apply(&self, source: &str) -> String {
        let mut edits: Vec<&Edit> = self.edits.iter().collect();
        edits.sort_by_key(|e| e.span.start());
        let mut out = source.to_string();
        for edit in edits.iter().rev() {
            out.replace_range(edit.span.range(), &edit.replacement);
        }
        out
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    pub(crate) fn safe(&self) -> bool {
        self.safe
    }

    pub(crate) fn edits(&self) -> &[Edit] {
        &self.edits
    }
}

impl Edit {
    pub(crate) fn span(&self) -> Span {
        self.span
    }

    pub(crate) fn replacement(&self) -> &str {
        &self.replacement
    }
}
