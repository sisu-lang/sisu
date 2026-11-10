//! The report list and its promises. A promise of an error (`proof`) holds only while the list
//! holds an error, or the parser marked one. A rollback cuts the promises of the reports it cuts.

use super::*;
use crate::check::reports::Diagnostics;

/// An error at `start`.
fn error_at(start: usize) -> Diagnostic {
    DiagnosticKind::SelfOutsideMethod {
        at: Span::new(start, start + 1),
    }
    .into()
}

/// A warning at `start`.
fn warning_at(start: usize) -> Diagnostic {
    DiagnosticKind::TodoLeft {
        at: Span::new(start, start + 1),
    }
    .into()
}

/// The code and the start of each report of `list`.
fn places(list: &Diagnostics) -> Vec<(Code, usize)> {
    list.iter().map(|d| (d.code(), d.span().start())).collect()
}

#[test]
fn a_warning_should_be_no_proof_of_an_error() {
    // S20: GIVEN a list that holds one warning, WHEN a promise is recorded, THEN no error keeps it.
    let mut list = Diagnostics::default();
    let () = list.warn(warning_at(1));
    list.proof(Span::new(2, 3));
    assert!(!list.promises_kept(false));
}

#[test]
fn a_rollback_should_drop_a_promise() {
    // S30: GIVEN a promise made after a mark, WHEN `drop_since` cuts the reports from the mark,
    // THEN the promise goes with them.
    let mut list = Diagnostics::default();
    let mark = list.mark();
    list.proof(Span::new(2, 3));
    list.drop_since(mark);
    assert!(list.promises_kept(false));
}

#[test]
fn a_taken_promise_should_return_with_restore() {
    // S30, second case: `take_since` holds the promise out of the list, and `restore` puts it back.
    let mut list = Diagnostics::default();
    let mark = list.mark();
    list.proof(Span::new(2, 3));
    let held = list.take_since(mark);
    assert!(list.promises_kept(false), "the promise is held out");
    list.restore(held);
    assert!(!list.promises_kept(false), "the promise is back");
}

#[test]
fn an_error_should_keep_a_promise() {
    let mut list = Diagnostics::default();
    list.proof(Span::new(2, 3));
    list.error(error_at(5));
    assert!(list.promises_kept(false));
}

#[test]
fn a_parser_mark_should_keep_a_promise() {
    let mut list = Diagnostics::default();
    list.proof(Span::new(2, 3));
    assert!(list.promises_kept(true));
}

#[test]
fn take_and_restore_should_keep_the_order() {
    // A restored error counts again, as `extend` counted it, so W0011 sees no change.
    let mut list = Diagnostics::default();
    list.error(error_at(1));
    let mark = list.mark();
    list.error(error_at(2));
    list.error(error_at(3));
    let before = places(&list);
    let held = list.take_since(mark);
    assert_eq!(places(&list), [(Code::SelfOutsideMethod, 1)]);
    list.restore(held);
    assert_eq!(places(&list), before);
    assert_eq!(list.errors(), 5);
}

#[test]
fn insert_at_should_put_a_report_at_its_mark_and_count_it() {
    // A push would put the report last. The report goes before the one that follows the mark.
    let mut list = Diagnostics::default();
    list.error(error_at(1));
    let mark = list.mark();
    list.error(error_at(3));
    list.insert_at(mark, error_at(2));
    assert_eq!(
        places(&list),
        [
            (Code::SelfOutsideMethod, 1),
            (Code::SelfOutsideMethod, 2),
            (Code::SelfOutsideMethod, 3),
        ]
    );
    assert_eq!(list.errors(), 3);
}
