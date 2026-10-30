//! Positions in the source text: byte spans, and lines and columns for people.

/// The longest source in bytes: 1 GiB. A `Span` stores its offsets as `u32`s, and two texts
/// longer than the source get spans too:
/// - `front_end::decode`'s shown text, the valid prefix and one U+FFFD: at most
///   `MAX_SOURCE + 3` bytes;
/// - a fix applied to the source: `front_end::sound_fixes` applies each fix alone and lexes the
///   result again. Only `diagnostic::registry` builds fixes. One writes either at most 11 bytes
///   of its own text (`char(i64(` and `))` around an operand; `let _ = ` is 8; a typed literal
///   is at most `: char` and a 3-byte literal, in place of a call of at least 5 bytes), or one name
///   and at most 4 bytes: a declared field (`{field}: `), a similar name, or the array method a
///   call names (`(` and `{close}.{name}(`). That name is a built-in one or also stands
///   elsewhere in the source, in its declaration or in the call the fix rewrites, so a fix
///   inserts at most source-sized text and the fixed text is at most `2 * MAX_SOURCE` bytes.
///
/// A suggested name may also stand in the prelude, whose length is fixed and far below
/// `MAX_SOURCE`, so the prelude adds nothing to these bounds.
pub(crate) const MAX_SOURCE: usize = 1 << 30;

const _: () = assert!(
    2 * MAX_SOURCE + 3 <= u32::MAX as usize,
    "every offset of a source and its prelude should fit in a u32"
);

/// Where the prelude's offsets start: above every offset of a user's text (at most
/// `2 * MAX_SOURCE + 3`), so that a span says by its offsets which text it indexes.
pub(crate) const PRELUDE_BASE: u32 = 3 << 30;

const _: () = assert!(
    PRELUDE_BASE as usize > 2 * MAX_SOURCE + 3,
    "the prelude offsets should start above every offset of a user source"
);

/// A byte range of the source. The offsets are stored as `u32`s, which `MAX_SOURCE` keeps
/// them within; `new` is the one place that converts them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Span {
    start: u32,
    end: u32,
}

const _: () = assert!(size_of::<Span>() == 8, "a span should be 8 bytes");

impl Span {
    pub(crate) fn new(start: usize, end: usize) -> Span {
        let offset = |at: usize| {
            u32::try_from(at).expect(
                "a source is at most `MAX_SOURCE` bytes, so a span offset should fit in `u32`",
            )
        };
        Span {
            start: offset(start),
            end: offset(end),
        }
    }

    pub(crate) fn start(self) -> usize {
        self.start as usize
    }

    pub(crate) fn end(self) -> usize {
        self.end as usize
    }

    /// `start()..end()`, to slice the source with.
    pub(crate) fn range(self) -> std::ops::Range<usize> {
        self.start()..self.end()
    }

    /// The empty span where `self` ends.
    pub(crate) fn end_point(self) -> Span {
        Span {
            start: self.end,
            end: self.end,
        }
    }

    /// The span from the start of `self` to the end of `other`.
    pub(crate) fn to(self, other: Span) -> Span {
        Span::new(self.start(), other.end())
    }

    /// Whether `self` lies in the prelude's text, not the user's.
    pub(crate) fn in_prelude(self) -> bool {
        self.start >= PRELUDE_BASE
    }
}

/// One text, whose spans start at `base`: a user's file at 0, the prelude at `PRELUDE_BASE`.
/// Every slice of a text by a span goes through it, so that a span of one text never indexes
/// another.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Source<'a> {
    text: &'a str,
    base: usize,
}

impl<'a> Source<'a> {
    /// `text`, whose first byte has offset `base`.
    pub(crate) fn new(text: &'a str, base: usize) -> Source<'a> {
        Source { text, base }
    }

    /// The text that `span` covers.
    pub(crate) fn text(&self, span: Span) -> &'a str {
        self.slice(span.range())
    }

    /// The text between the offsets of `range`. The offsets of another text lie outside this
    /// one: the wrapped subtraction then leaves a range that no text has.
    pub(crate) fn slice(&self, range: std::ops::Range<usize>) -> &'a str {
        let range = range.start.wrapping_sub(self.base)..range.end.wrapping_sub(self.base);
        self.text
            .get(range)
            .expect("a span should lie in the text it indexes")
    }

    /// The offset of the first byte.
    pub(crate) fn start(&self) -> usize {
        self.base
    }

    /// The offset just past the last byte.
    pub(crate) fn end(&self) -> usize {
        self.base + self.text.len()
    }

    /// The text from offset `from` to the end.
    pub(crate) fn rest(&self, from: usize) -> &'a str {
        self.slice(from..self.end())
    }
}

/// The byte offset where each line of a source starts, so that a position costs a binary
/// search, not a scan from the start.
pub(crate) struct LineIndex(Vec<usize>);

impl LineIndex {
    pub(crate) fn new(source: &str) -> LineIndex {
        let breaks = source.match_indices('\n').map(|(i, _)| i + 1);
        LineIndex(std::iter::once(0).chain(breaks).collect())
    }

    /// The 1-based line and column of byte `offset` in the `source` that built the index; the
    /// column counts chars, not bytes.
    pub(crate) fn line_col(&self, source: &str, offset: usize) -> (usize, usize) {
        // Line starts at or before `offset`; the first is 0, so there is at least one.
        let line = self.0.partition_point(|&start| start <= offset);
        let line_start = self.0[line - 1];
        (line, source[line_start..offset].chars().count() + 1)
    }

    /// The byte offset where 1-based `line` starts, or `None` past the last line; `line` is at
    /// least 1.
    pub(crate) fn line_start(&self, line: usize) -> Option<usize> {
        self.0.get(line - 1).copied()
    }

    /// The 1-based line that `annotate-snippets` puts byte `offset` on. A text that ends in a
    /// line break has no empty last line there, so an offset at its end sits on the line before.
    /// `source.len()` is the only offset that can lie past the last rendered line, so it is the
    /// only one clamped.
    pub(crate) fn render_line(&self, source: &str, offset: usize) -> usize {
        let ends_in_break = !source.is_empty() && self.0.last() == Some(&source.len());
        let lines = self.0.len() - usize::from(ends_in_break);
        self.0.partition_point(|&start| start <= offset).min(lines)
    }
}

/// The 1-based line and column of byte `offset`; the column counts chars, not bytes.
#[cfg(test)]
pub(crate) fn line_col(source: &str, offset: usize) -> (usize, usize) {
    LineIndex::new(source).line_col(source, offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_offsets_reach_u32_max() {
        let max = usize::try_from(u32::MAX).expect("`u32::MAX` should fit in `usize` here");
        let empty = Span::new(max, max);
        assert_eq!(empty.start(), max);
        assert_eq!(empty.end(), max);
        assert_eq!(empty.range(), max..max);
        assert_eq!(
            Span::new(0, 5).to(Span::new(max - 1, max)),
            Span::new(0, max)
        );
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    #[should_panic(expected = "should fit in `u32`")]
    fn span_offset_past_u32_max_panics() {
        let max = usize::try_from(u32::MAX).expect("`u32::MAX` should fit in `usize` here");
        Span::new(0, max + 1);
    }

    #[test]
    fn in_prelude_should_hold_from_the_base_on() {
        let base = PRELUDE_BASE as usize;
        assert!(!Span::new(0, 1).in_prelude());
        assert!(!Span::new(base - 1, base - 1).in_prelude());
        assert!(Span::new(base, base).in_prelude());
        assert!(Span::new(base + 5, base + 6).in_prelude());
    }

    /// The text is non-ASCII, so a byte offset that is not a char boundary would panic.
    #[test]
    fn source_should_slice_the_text_at_offsets_above_its_base() {
        let base = 100;
        let source = Source::new("äb cd", base);
        assert_eq!(source.text(Span::new(base, base + 2)), "ä");
        assert_eq!(source.slice(base + 4..base + 6), "cd");
        assert_eq!(source.rest(base + 3), " cd");
        assert_eq!(source.rest(source.end()), "");
        assert_eq!(source.end(), base + 6);
    }

    #[test]
    #[should_panic(expected = "a span should lie in the text it indexes")]
    fn source_should_refuse_a_span_of_another_text() {
        let prelude = Source::new("enum P { A }", PRELUDE_BASE as usize);
        prelude.text(Span::new(0, 4));
    }

    #[test]
    fn line_col_counts_chars_not_bytes() {
        assert_eq!(line_col("ää x", 5), (1, 4));
        assert_eq!(line_col("ää\nx", 5), (2, 1));
        assert_eq!(line_col("", 0), (1, 1));
    }

    #[test]
    fn line_index_agrees_with_a_scan_at_every_offset() {
        // The reference counts from the start of the text, as `line_col` did before the index.
        fn scan(source: &str, offset: usize) -> (usize, usize) {
            let before = &source[..offset];
            let line = before.matches('\n').count() + 1;
            let line_start = before.rfind('\n').map_or(0, |i| i + 1);
            (line, before[line_start..].chars().count() + 1)
        }
        for source in [
            "",
            "ää x",
            "ää\nx",
            "\n",
            "a\n\nöö€b\n",
            "x\nyy\nä",
            "\n\nä\n",
        ] {
            let index = LineIndex::new(source);
            for offset in (0..=source.len()).filter(|&i| source.is_char_boundary(i)) {
                assert_eq!(
                    index.line_col(source, offset),
                    scan(source, offset),
                    "{source:?} at {offset}"
                );
            }
        }
    }

    #[test]
    fn line_index_pins_first_last_and_post_break_positions() {
        let source = "ä\nbö€\nc";
        let index = LineIndex::new(source);
        assert_eq!(index.line_col(source, 0), (1, 1));
        // Right before and right after a line break.
        assert_eq!(index.line_col(source, 2), (1, 2));
        assert_eq!(index.line_col(source, 3), (2, 1));
        // After the multi-byte chars of line 2, then the break.
        assert_eq!(index.line_col(source, 9), (2, 4));
        assert_eq!(index.line_col(source, 10), (3, 1));
        // The end of the text, after the last character.
        assert_eq!(index.line_col(source, source.len()), (3, 2));
    }

    #[test]
    fn render_line_clamps_the_end_of_a_text_that_ends_in_a_break() {
        let index = LineIndex::new("a\nb\n");
        // The end sits on the line before the empty one that the index counts.
        assert_eq!(index.render_line("a\nb\n", 4), 2);
        assert_eq!(index.render_line("a\nb\n", 2), 2);
        assert_eq!(LineIndex::new("a\nb").render_line("a\nb", 3), 2);
        assert_eq!(LineIndex::new("").render_line("", 0), 1);
    }

    #[test]
    fn line_start_is_none_past_the_last_line() {
        let index = LineIndex::new("a\nb");
        assert_eq!(index.line_start(1), Some(0));
        assert_eq!(index.line_start(2), Some(2));
        assert_eq!(index.line_start(3), None);
    }
}
