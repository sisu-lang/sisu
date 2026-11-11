//! The rules of a language reference page and their IDs.
//!
//! A rule is one top-level item of a list of rules. That list is a bullet list at the top level
//! of the page. It follows a label, a paragraph that is only bold text, and at most one more
//! paragraph, the note. `site/build.py` (`mark_rules`) uses the same test. A page joins when the
//! text of one of its list items starts with `{#`. From then on, each of its rules needs an ID.

use std::collections::hash_map::Entry;

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use rustc_hash::{FxHashMap, FxHashSet};

use super::markers::RuleMarker;
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
    /// Whether a white space character that the site does not see stands before the prefix.
    pub(crate) after_space: bool,
    /// The hash of the text of the rule (see `hash_of`): four lowercase hex digits.
    pub(crate) hash: String,
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
    // The plain text of the open rule, which the hash covers.
    let mut body: Option<String> = None;
    for (event, range) in Parser::new_ext(text, Options::ENABLE_TABLES).into_offset_iter() {
        // The text of an item starts at its first event, when that event is text, code or the
        // start of a paragraph. Any other first block (such as indented code) has no prefix.
        if let Some((line, is_rule)) = item.take() {
            // `Event::Code` can never give a prefix, because a code span does not start with
            // `{#`. It stays in the list so that every first event of a text is one case.
            let starts_text = matches!(
                event,
                Event::Text(_) | Event::Code(_) | Event::Start(Tag::Paragraph)
            );
            let at = &text[range.start..];
            let after_space = at.starts_with(is_stripped);
            // The site accepts `\{#` after a stripped character, because `pulldown-cmark` then
            // keeps the `\` in the text. With no stripped character first, `at` starts at the
            // escaped `\` of `\\{#`. That `\` is text on the site, so it gives no prefix.
            let trimmed = at.trim_start_matches(is_stripped_or_blank);
            let trimmed = trimmed
                .strip_prefix('\\')
                .filter(|rest| after_space && rest.starts_with("{#"))
                .unwrap_or(trimmed);
            let prefix = starts_text.then(|| read_prefix(trimmed)).flatten();
            if prefix.is_some() {
                id_lines.push(line);
            }
            if is_rule {
                let after_space = after_space && prefix.is_some();
                let (prefix, id) = prefix.map_or((None, None), |(written, id)| (Some(written), id));
                rules.push(Rule {
                    line,
                    prefix,
                    id,
                    after_space,
                    hash: String::new(),
                });
            }
        }
        if let Some(body) = &mut body {
            push_plain(body, &event);
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
                let is_rule = depth == 0 && lists.last() == Some(&true);
                item = Some((line_of(range.start), is_rule));
                if is_rule {
                    body = Some(String::new());
                }
                depth += 1;
            }
            Event::Start(Tag::BlockQuote(_)) => depth += 1,
            Event::End(TagEnd::Item) => {
                depth -= 1;
                // The end of the rule, not of an item nested in it.
                if depth == 0
                    && let (Some(body), Some(rule)) = (body.take(), rules.last_mut())
                {
                    rule.hash = hash_of(&body, rule.prefix.as_deref());
                }
            }
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

/// Whether markdown-it strips `c` from the start of a paragraph, and the reader of the check does
/// not: Python `str.strip` white space that is not an ASCII space or tab. `pulldown-cmark` skips
/// a space or a tab after the bullet and keeps any other character. The site then sees the ID
/// that this reader does not (`site/build.py`, `rule_anchors`).
fn is_stripped(c: char) -> bool {
    c.is_whitespace() && !matches!(c, ' ' | '\t') || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Whether Python `str.strip` strips `c`.
fn is_stripped_or_blank(c: char) -> bool {
    is_stripped(c) || matches!(c, ' ' | '\t')
}

/// Adds what `event` gives to the plain text of a rule. A `Text`, `Code` or `InlineHtml` event
/// gives its text. A line break gives one space. So does the start or the end of a block, an item
/// or a table cell. Any other event gives nothing.
fn push_plain(body: &mut String, event: &Event<'_>) {
    let inline_start = |tag: &Tag<'_>| {
        matches!(
            tag,
            Tag::Emphasis
                | Tag::Strong
                | Tag::Strikethrough
                | Tag::Superscript
                | Tag::Subscript
                | Tag::Link { .. }
                | Tag::Image { .. }
        )
    };
    let inline_end = |tag: &TagEnd| {
        matches!(
            tag,
            TagEnd::Emphasis
                | TagEnd::Strong
                | TagEnd::Strikethrough
                | TagEnd::Superscript
                | TagEnd::Subscript
                | TagEnd::Link
                | TagEnd::Image
        )
    };
    match event {
        Event::Text(text) | Event::Code(text) | Event::InlineHtml(text) => body.push_str(text),
        Event::SoftBreak | Event::HardBreak => body.push(' '),
        Event::Start(tag) if !inline_start(tag) => body.push(' '),
        Event::End(tag) if !inline_end(tag) => body.push(' '),
        _ => {}
    }
}

/// The hash of a rule whose plain text is `body`: the high 16 bits of the 32-bit FNV-1a hash of
/// the text, as four lowercase hex digits. The text has one space for each run of white space,
/// no `prefix` (the `{#id}` as written) and no space at the ends. A marker stores this output, so
/// a change here makes every marker stale.
fn hash_of(body: &str, prefix: Option<&str>) -> String {
    let words = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = prefix
        .and_then(|prefix| words.strip_prefix(prefix))
        .unwrap_or(&words)
        .trim_start();
    let hash = text.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    });
    format!("{:04x}", hash >> 16)
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
            _ if rule.after_space => {
                "write the ID at the start of the item, with no other character before it."
                    .to_string()
            }
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

/// The IDs that a marker can name: each ID of each joined page. The index never joins, so its
/// IDs are unknown. An ID with a fault is known, so a marker that names it has no second fault.
pub(crate) fn known_ids(pages: &[(String, Page)]) -> FxHashSet<String> {
    pages
        .iter()
        .filter(|(stem, _)| !is_index(stem))
        .flat_map(|(_, page)| page.rules.iter().filter_map(|rule| rule.id.clone()))
        .collect()
}

/// A `##` or `###` section of a page: the lines from its heading to the next heading.
struct Section {
    start: usize,
    end: usize,
    pending: bool,
    plain: bool,
}

/// The sections of `page`, in page order. A heading of any level ends a section, but only `##` and
/// `###` start one, as on the site. One pass over the headings and the fences.
fn sections(page: &Page) -> Vec<Section> {
    let ends = page.headings.iter().skip(1).map(|&(line, _)| line);
    let mut fences = page.fences.iter().peekable();
    let mut sections = Vec::new();
    for (&(start, level), end) in page
        .headings
        .iter()
        .zip(ends.chain(std::iter::once(usize::MAX)))
    {
        // The fences before this heading are in no section.
        while fences.next_if(|&&(line, _)| line < start).is_some() {}
        let (mut pending, mut plain) = (false, false);
        while let Some(&(_, is_pending)) = fences.next_if(|&&(line, _)| line < end) {
            pending |= is_pending;
            plain |= !is_pending;
        }
        if matches!(level, 2 | 3) {
            sections.push(Section {
                start,
                end,
                pending,
                plain,
            });
        }
    }
    sections
}

/// The faults of `page`, the page `stem`, found with the `markers` of the tests, sorted by line.
/// They are the faults of `id_faults` and the faults of the proof of each rule: a rule with no
/// test, a rule whose text changed after its tests, and a section with a pending and a plain
/// example. The index and a page with no `{#` item have no proof fault. A rule whose ID has a
/// fault has none either. A rule in a section with a pending example needs no test. Its markers
/// still need the current hash.
pub(crate) fn rule_faults(stem: &str, page: &Page, markers: &[RuleMarker]) -> Vec<Fault> {
    let mut faults = id_faults(stem, page);
    if !is_index(stem) && !page.id_lines.is_empty() {
        let bad_lines: FxHashSet<usize> = faults.iter().map(|fault| fault.line).collect();
        faults.extend(proof_faults(page, markers, &bad_lines));
    }
    faults.sort_by_key(|fault| fault.line);
    faults
}

/// The faults of the proof of each rule of the joined `page`. A rule at one of the `bad_lines`,
/// where the ID of a rule has a fault, has none.
fn proof_faults(page: &Page, markers: &[RuleMarker], bad_lines: &FxHashSet<usize>) -> Vec<Fault> {
    let sections = sections(page);
    let mut by_id: FxHashMap<&str, Vec<&RuleMarker>> = FxHashMap::default();
    for marker in markers {
        by_id.entry(&marker.id).or_default().push(marker);
    }
    let mut faults = Vec::new();
    // The rules are in page order, like the sections: one cursor moves over the sections.
    let mut section = 0;
    for rule in &page.rules {
        while sections.get(section).is_some_and(|s| s.end <= rule.line) {
            section += 1;
        }
        let Some(id) = rule
            .id
            .as_deref()
            .filter(|_| !bad_lines.contains(&rule.line))
        else {
            continue;
        };
        let hash = &rule.hash;
        let named = by_id.get(id).map_or(&[][..], Vec::as_slice);
        let stale: Vec<&&RuleMarker> = named.iter().filter(|m| m.hash != *hash).collect();
        let message = if named.is_empty() {
            let in_development = sections
                .get(section)
                .is_some_and(|s| s.start <= rule.line && s.pending);
            if in_development {
                continue;
            }
            format!("{id} has no test. Add `r[verify {id}@{hash}]` to a test that proves it.")
        } else if stale.is_empty() {
            continue;
        } else {
            let list: Vec<String> = stale
                .iter()
                .map(|m| format!("{}:{} (@{})", m.file, m.line, m.hash))
                .collect();
            format!(
                "the text of {id} changed. Its hash is now @{hash}. Read these tests again, \
                 then change each marker's hash:\n  {}",
                list.join("\n  ")
            )
        };
        faults.push(Fault {
            line: rule.line,
            message,
        });
    }
    faults.extend(sections.iter().filter(|s| s.pending && s.plain).map(|s| {
        Fault {
            line: s.start,
            message:
                "this section has a pending example and a plain example. Put the pending part \
                  under its own heading."
                    .to_string(),
        }
    }));
    faults
}
