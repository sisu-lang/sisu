//! `parity_found`, the one test helper that reads the wording of diagnostics. It compares the
//! output of a generic call with the output of its concrete twin, so it pins no text.
//! `scripts/check-test-wording.sh` exempts this file, so it holds nothing else.

use super::*;

/// A diagnostic's code, its primary position and its message.
pub(super) type ParityFound = (Code, (usize, usize), String);

/// Each diagnostic of `src`: its code, its place and its message. The message names `Two<X, bool>`
/// as the concrete twin's `Two<i64, bool>`.
pub(super) fn parity_found(src: &str) -> (bool, Vec<ParityFound>) {
    let (program, ds) = checked(src);
    let found = ds
        .iter()
        .map(|d| {
            let message = d.message().replace("Two<X, bool>", "Two<i64, bool>");
            (d.code(), at(src, d.span()), message)
        })
        .collect();
    (program.is_none(), found)
}
