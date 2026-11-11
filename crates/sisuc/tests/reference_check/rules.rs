//! The rules of a language reference page and their IDs.
//!
//! A rule is one top-level item of a list of rules. That list is a bullet list at the top level
//! of the page. It follows a label, a paragraph that is only bold text, and at most one more
//! paragraph, the note. `site/build.py` (`mark_rules`) uses the same test. A page joins when the
//! text of one of its list items starts with `{#`. From then on, each of its rules needs an ID.

use std::collections::hash_map::Entry;

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use rustc_hash::FxHashMap;

use super::{Fault, Page};

/// One top-level item of a list of rules.
#[derive(Debug)]
pub(crate) struct Rule {
    /// The Markdown line of the item.
    pub(crate) line: usize,
    /// The prefix as written, when the text of the item starts with `{#`: up to the first `}` or
    /// white space, with the `}`.
    pub(crate) prefix: Option<String>,
    /// The ID of the item, when its prefix is `{#id} ` and the text follows on the same line.
    pub(crate) id: Option<String>,
}

/// A block at the top level of the page, as far as the test for a list of rules needs it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    /// A paragraph that is only one bold span.
    Label,
    Paragraph,
    Other,
}

/// What a paragraph's events show about it: whether it is a label.
#[derive(Default)]
struct Inline {
    first_is_strong: Option<bool>,
    last_is_strong_end: bool,
    strongs: usize,
}

impl Inline {
    fn add(&mut self, event: &Event<'_>) {
        let start = matches!(event, Event::Start(Tag::Strong));
        self.first_is_strong.get_or_insert(start);
        self.last_is_strong_end = matches!(event, Event::End(TagEnd::Strong));
        self.strongs += usize::from(start);
    }

    fn is_label(&self) -> bool {
        self.first_is_strong == Some(true) && self.last_is_strong_end && self.strongs == 1
    }
}

/// The rules of `text`, a page, and the lines of its list items that start with `{#`.
pub(crate) fn read_rules(text: &str) -> (Vec<Rule>, Vec<usize>) {
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(at, _)| at + 1))
        .collect();
    let line_of = |offset: usize| line_starts.partition_point(|&start| start <= offset);
    let mut rules = Vec::new();
    let mut id_lines = Vec::new();
    // The open items and block quotes: 0 is the top level of the page.
    let mut depth = 0_usize;
    // For each open list, whether it is a list of rules.
    let mut lists: Vec<bool> = Vec::new();
    // The last two blocks at the top level, the older first.
    let mut previous = [Block::Other; 2];
    let mut paragraph: Option<Inline> = None;
    // The line of an item that just opened, and whether it is a rule.
    let mut item: Option<(usize, bool)> = None;
    for (event, range) in Parser::new_ext(text, Options::ENABLE_TABLES).into_offset_iter() {
        // The first event of an item starts its text.
        if let Some((line, is_rule)) = item.take() {
            let prefix = read_prefix(&text[range.start..]);
            if prefix.is_some() {
                id_lines.push(line);
            }
            if is_rule {
                let (prefix, id) = prefix.map_or((None, None), |(written, id)| (Some(written), id));
                rules.push(Rule { line, prefix, id });
            }
        }
        if let Some(inline) = &mut paragraph
            && !matches!(event, Event::End(TagEnd::Paragraph))
        {
            inline.add(&event);
        }
        let mut ended = None;
        match event {
            Event::Start(Tag::Paragraph) if depth == 0 => paragraph = Some(Inline::default()),
            Event::End(TagEnd::Paragraph) if depth == 0 => {
                ended = Some(if paragraph.take().is_some_and(|p| p.is_label()) {
                    Block::Label
                } else {
                    Block::Paragraph
                });
            }
            Event::Start(Tag::List(first)) => {
                let [older, last] = previous;
                let note = matches!(last, Block::Label | Block::Paragraph) && older == Block::Label;
                // An item is a rule only at depth 0 (see `Start(Tag::Item)`).
                lists.push(first.is_none() && (last == Block::Label || note));
            }
            Event::Start(Tag::Item) => {
                item = Some((
                    line_of(range.start),
                    depth == 0 && lists.last() == Some(&true),
                ));
                depth += 1;
            }
            Event::Start(Tag::BlockQuote(_)) => depth += 1,
            Event::End(TagEnd::Item) => depth -= 1,
            Event::End(TagEnd::BlockQuote(_)) => {
                depth -= 1;
                ended = Some(Block::Other);
            }
            Event::End(TagEnd::List(_)) => {
                lists.pop();
                ended = Some(Block::Other);
            }
            // The site parses with HTML off, so an HTML block is a paragraph there: a note.
            Event::End(TagEnd::HtmlBlock) => ended = Some(Block::Paragraph),
            Event::End(TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::Table) | Event::Rule => {
                ended = Some(Block::Other);
            }
            _ => {}
        }
        if let Some(block) = ended.filter(|_| depth == 0) {
            previous = [previous[1], block];
        }
    }
    (rules, id_lines)
}

/// The prefix that `at`, the text of an item, starts with, as written, and its ID. The prefix runs
/// from `{#` to the first `}` or white space. It has an ID when `}` ends it and exactly one space,
/// and then the text of the rule, follow.
fn read_prefix(at: &str) -> Option<(String, Option<String>)> {
    let rest = at.strip_prefix("{#")?;
    let end = rest
        .find(|c: char| c == '}' || c.is_whitespace())
        .unwrap_or(rest.len());
    let name = &rest[..end];
    let after = rest[end..].strip_prefix('}');
    let written = format!("{{#{name}{}", if after.is_some() { "}" } else { "" });
    let id = after
        .and_then(|text| text.strip_prefix(' '))
        .is_some_and(|text| text.starts_with(|c: char| !c.is_whitespace()))
        .then(|| name.to_string());
    Some((written, id))
}

/// The faults of the IDs of `page`, the page `stem`, by line. A page with no `{#` item has no
/// fault. In the index, each `{#` item is a fault and the rules need nothing. In any other
/// page, an item gets one fault, the first that applies: an ID on an item that is not a rule, a
/// bad prefix, a bad form, a wrong stem, a repeated ID. A rule with no `{#` has no ID.
pub(crate) fn id_faults(stem: &str, page: &Page) -> Vec<Fault> {
    let fault = |line, message: String| Fault { line, message };
    if is_index(stem) {
        return page
            .id_lines
            .iter()
            .map(|&line| fault(line, "the index cannot have rule IDs.".to_string()))
            .collect();
    }
    if page.id_lines.is_empty() {
        return Vec::new();
    }
    let mut faults = Vec::new();
    let mut seen: FxHashMap<&str, usize> = FxHashMap::default();
    for rule in &page.rules {
        let line = rule.line;
        let message = match (&rule.id, &rule.prefix) {
            (None, Some(written)) if written.ends_with('}') => {
                format!("write one space and then the text of the rule after `{written}`.")
            }
            (None, Some(written)) => {
                format!("write `}}`, one space and then the text of the rule after `{written}`.")
            }
            (None, None) => {
                "this rule has no ID. The page has rule IDs, so each rule needs one.".to_string()
            }
            (Some(id), _) if !has_valid_form(id) => format!(
                "the ID \"{id}\" has a bad form. Write lowercase parts that start with a letter, \
                 joined by dots."
            ),
            (Some(id), _)
                if !id
                    .strip_prefix(stem)
                    .is_some_and(|rest| rest.starts_with('.')) =>
            {
                format!("the ID {id} must start with \"{stem}.\".")
            }
            (Some(id), _) => match seen.entry(id) {
                Entry::Occupied(first) => {
                    format!("{id} is also the ID of the rule at line {}.", first.get())
                }
                Entry::Vacant(slot) => {
                    slot.insert(line);
                    continue;
                }
            },
        };
        faults.push(fault(line, message));
    }
    // A `{#` item is a rule only when it is the item of a rule that has the prefix. The test is
    // by item, not by line: a nested item can start on the line of its parent rule.
    let has_prefix = |line| {
        page.rules
            .binary_search_by_key(&line, |rule| rule.line)
            .is_ok_and(|at| page.rules[at].prefix.is_some())
    };
    faults.extend(
        page.id_lines
            .iter()
            .filter(|&&line| !has_prefix(line))
            .map(|&line| {
                fault(
                    line,
                    "an ID is only for a top-level item of a list of rules.".to_string(),
                )
            }),
    );
    faults.sort_by_key(|fault| fault.line);
    faults
}

/// Whether `id` is two or more parts joined by dots. A part is lowercase letters, digits and
/// single hyphens, and starts with a letter.
fn has_valid_form(id: &str) -> bool {
    let word = |word: &str| {
        word.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    };
    let part = |part: &str| {
        let mut words = part.split('-');
        let first = words.next().unwrap_or_default();
        first.starts_with(|c: char| c.is_ascii_lowercase())
            && word(first)
            && words.all(|next| !next.is_empty() && word(next))
    };
    id.split('.').count() >= 2 && id.split('.').all(part)
}

/// Whether the page `stem` is the index of the reference, which never has rule IDs.
pub(crate) fn is_index(stem: &str) -> bool {
    stem == "README"
}
