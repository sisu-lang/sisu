//! The rules of a language reference page and their IDs.

use super::{Fault, Page};

/// One top-level item of a list of rules.
#[derive(Debug)]
pub(crate) struct Rule {
    /// The Markdown line of the item.
    pub(crate) line: usize,
    /// The ID of the item, when its text starts with a well-formed `{#id} ` prefix.
    pub(crate) id: Option<String>,
}

/// The rules of `text`, a page, and the lines of its list items that start with `{#`.
pub(crate) fn read_rules(_text: &str) -> (Vec<Rule>, Vec<usize>) {
    (Vec::new(), Vec::new())
}

/// The faults of the IDs of `page`, the page `stem`, by line.
pub(crate) fn id_faults(_stem: &str, _page: &Page) -> Vec<Fault> {
    Vec::new()
}

/// Whether the page `stem` is the index of the reference, which never has rule IDs.
pub(crate) fn is_index(_stem: &str) -> bool {
    false
}
