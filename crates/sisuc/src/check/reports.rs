//! The checker's list of reports, and the token that proves that one was made.
//!
//! Only `error` and `proof` make a [`Reported`]. A promise (`proof`) says that the program has an
//! error. An error in the checker's list keeps it, wherever the error is, and so does a parser
//! mark. At the end of the check, `promises_kept` finds a promise that neither keeps.

use crate::diagnostic::{Code, Diagnostic, Severity};
use crate::source::Span;

/// Proof that the checker reported an error, or promised to. It is a copy of one report, so a
/// copy proves the same report. It holds no data.
#[derive(Clone, Copy, Debug)]
pub(super) struct Reported(());

const _: () = assert!(size_of::<Reported>() == 0, "Reported should hold no data");

/// The checker's reports, how many errors were added to them, and the promises of errors.
///
/// The error count never goes down, so a report that is cut or added again counts more than once:
/// that can only stop a W0011 warning, never cause one.
#[derive(Default)]
pub(super) struct Diagnostics {
    list: Vec<Diagnostic>,
    errors: u32,
    /// Where each promise of an error (`proof`) was made.
    promises: Vec<Span>,
}

/// A place in the list, for a rollback: how many reports and how many promises it held.
#[derive(Clone, Copy, Debug)]
pub(super) struct Mark {
    reports: usize,
    promises: usize,
}

/// The reports and the promises that `take_since` cut. `restore` puts them back.
#[must_use]
#[derive(Default)]
pub(super) struct Held {
    reports: Vec<Diagnostic>,
    promises: Vec<Span>,
}

impl Held {
    /// Adds the reports and the promises of `other` after the ones held.
    pub(super) fn absorb(&mut self, other: Held) {
        self.reports.extend(other.reports);
        self.promises.extend(other.promises);
    }

    /// Adds a report that no list holds yet, after the ones held.
    pub(super) fn push(&mut self, d: Diagnostic) {
        self.reports.push(d);
    }

    /// Puts `d` in place of the report held at `index`.
    pub(super) fn replace(&mut self, index: usize, d: Diagnostic) {
        self.reports[index] = d;
    }

    /// The number of reports held.
    pub(super) fn len(&self) -> usize {
        self.reports.len()
    }

    /// Drops the reports and the promises held, for good.
    pub(super) fn clear(&mut self) {
        self.reports.clear();
        self.promises.clear();
    }
}

impl Diagnostics {
    /// Counts `d` if it is an error.
    fn count(&mut self, d: &Diagnostic) {
        self.errors += u32::from(d.severity() == Severity::Error);
    }

    fn add(&mut self, d: Diagnostic) {
        self.count(&d);
        self.list.push(d);
    }

    /// Adds an error, and proves it.
    pub(super) fn error(&mut self, d: impl Into<Diagnostic>) -> Reported {
        let d = d.into();
        debug_assert_eq!(d.severity(), Severity::Error, "{:?} is no error", d.code());
        self.add(d);
        Reported(())
    }

    /// Adds a warning. A warning proves no error, so it gives no token.
    pub(super) fn warn(&mut self, d: impl Into<Diagnostic>) {
        let d = d.into();
        debug_assert_eq!(
            d.severity(),
            Severity::Warning,
            "{:?} is no warning",
            d.code()
        );
        self.add(d);
    }

    /// Records a promise made at `at`. An error in the list keeps it, wherever the error is, and
    /// so does a parser mark (see `promises_kept`).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the poisoned values hold its token from the next change on"
        )
    )]
    pub(super) fn proof(&mut self, at: Span) -> Reported {
        self.promises.push(at);
        Reported(())
    }

    /// How many errors were added.
    pub(super) fn errors(&self) -> u32 {
        self.errors
    }

    /// The place at the end of the list.
    pub(super) fn mark(&self) -> Mark {
        Mark {
            reports: self.list.len(),
            promises: self.promises.len(),
        }
    }

    /// Cuts the reports and the promises after `mark`, for good.
    pub(super) fn drop_since(&mut self, mark: Mark) {
        self.list.truncate(mark.reports);
        self.promises.truncate(mark.promises);
    }

    /// Cuts the reports and the promises after `mark`, and holds them.
    pub(super) fn take_since(&mut self, mark: Mark) -> Held {
        Held {
            reports: self.list.drain(mark.reports..).collect(),
            promises: self.promises.drain(mark.promises..).collect(),
        }
    }

    /// Adds the reports and the promises of `held` at the end. Each error counts again.
    pub(super) fn restore(&mut self, held: Held) {
        for d in held.reports {
            self.add(d);
        }
        self.promises.extend(held.promises);
    }

    /// Adds `d` at `mark`, before the reports that came after it.
    pub(super) fn insert_at(&mut self, mark: Mark, d: Diagnostic) {
        self.count(&d);
        self.list.insert(mark.reports, d);
    }

    /// Puts `d` in place of the report at `index`. The code and the severity stay.
    pub(super) fn rewrite(&mut self, index: usize, d: Diagnostic) {
        debug_assert_eq!(
            self.list[index].code(),
            d.code(),
            "a rewrite keeps the code"
        );
        self.list[index] = d;
    }

    /// Takes the fixes from each report of `code` after `mark`.
    pub(super) fn strip_fixes_since(&mut self, mark: Mark, code: Code) {
        for d in &mut self.list[mark.reports..] {
            if d.code() == code {
                d.retain_fixes(|_| false);
            }
        }
    }

    /// Whether each promise is backed: no promise is made, or the list holds an error, or the
    /// parser marked one (`broken`).
    pub(super) fn promises_kept(&self, broken: bool) -> bool {
        self.promises.is_empty()
            || broken
            || self.list.iter().any(|d| d.severity() == Severity::Error)
    }

    /// The reports.
    pub(super) fn finish(self) -> Vec<Diagnostic> {
        self.list
    }
}

impl std::ops::Deref for Diagnostics {
    type Target = [Diagnostic];

    fn deref(&self) -> &[Diagnostic] {
        &self.list
    }
}
