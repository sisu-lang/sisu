//! The rules of a language reference page and their IDs.
//!
//! A rule is one top-level item of a list of rules. That list is a bullet list at the top level
//! of the page. It follows a label, a paragraph that is only bold text, and at most one more
//! paragraph, the note. `site/build.py` (`mark_rules`) uses the same test. A page joins when the
//! text of one of its list items starts with `{#`. From then on, each of its rules needs an ID.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use super::{Fault, Page};

/// One top-level item of a list of rules.
#[derive(Debug)]
pub(crate) struct Rule {
    /// The Markdown line of the item.
    pub(crate) line: usize,
    /// The ID of the item, when its text starts with a well-formed `{#id} ` prefix.
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
            let at = &text[range.start..];
            if at.starts_with("{#") {
                id_lines.push(line);
            }
            if is_rule {
                rules.push(Rule {
                    line,
                    id: prefix_id(at),
                });
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
                lists.push(depth == 0 && first.is_none() && (last == Block::Label || note));
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
            Event::End(
                TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::HtmlBlock | TagEnd::Table,
            )
            | Event::Rule => ended = Some(Block::Other),
            _ => {}
        }
        if let Some(block) = ended.filter(|_| depth == 0) {
            previous = [previous[1], block];
        }
    }
    (rules, id_lines)
}

/// The ID of the prefix `{#id} ` that `at`, the text of an item, starts with. The ID runs to the
/// first `}` or white space. Exactly one space, and then the text of the rule, must follow `}`.
fn prefix_id(at: &str) -> Option<String> {
    let rest = at.strip_prefix("{#")?;
    let (id, after) = rest.split_at(rest.find(|c: char| c == '}' || c.is_whitespace())?);
    let text = after.strip_prefix("} ")?;
    text.starts_with(|c: char| !c.is_whitespace())
        .then(|| id.to_string())
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
    let is_rule = |line| {
        page.rules
            .binary_search_by_key(&line, |rule| rule.line)
            .is_ok()
    };
    let mut faults: Vec<Fault> = page
        .id_lines
        .iter()
        .filter(|&&line| !is_rule(line))
        .map(|&line| {
            fault(
                line,
                "an ID is only for a top-level item of a list of rules.".to_string(),
            )
        })
        .collect();
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for rule in &page.rules {
        let line = rule.line;
        let message = match &rule.id {
            None if page.id_lines.binary_search(&line).is_ok() => {
                "write one space and then the text of the rule on the same line, after the \
                 `{#id}` prefix."
                    .to_string()
            }
            None => {
                "this rule has no ID. The page has rule IDs, so each rule needs one.".to_string()
            }
            Some(id) if !has_valid_form(id) => format!(
                "the ID {id} has a bad form. Write lowercase parts that start with a letter, \
                 joined by dots."
            ),
            Some(id)
                if !id
                    .strip_prefix(stem)
                    .is_some_and(|rest| rest.starts_with('.')) =>
            {
                format!("the ID {id} must start with \"{stem}.\".")
            }
            Some(id) => match seen.entry(id) {
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
