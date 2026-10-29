//! The text sink that `print` writes through. Generated `show` helpers write the text of a
//! value into a sink piece by piece: scalars, the quoted text of a string or a `char`, and the
//! events of a group (`open`, `sep` and `close`), so that a pretty sink can lay out the same
//! events later. A flat sink writes `sep` as `, ` and nothing more.
//!
//! A sink is a stack slot of `SINK_SIZE` bytes that generated code owns. One kind writes to the
//! runtime's standard output buffer, so `print` needs no buffer for the text. Another builds a
//! block that is the result string: `header` bytes that the sink never touches, then the text.
//! The third, the pretty sink, records the pieces and the events of a value, and
//! `sisu_show_pretty_finish` lays them out to fit 80 columns in the sink that it wraps.
//! The runtime neither builds nor reads a Sisu object (ADR 0009) and does not know the string
//! layout (ADR 0014); generated code writes the header of the block in place after
//! `sisu_show_finish`.

use std::io::Write;
use std::ptr;

use crate::text::sisu_pad;
use crate::{float, sisu_alloc, sisu_realloc, stdout_panic, text, with_stdout};

/// The size in bytes of a [`Sink`]. Generated code allocates this much for a sink, and
/// `codegen/abi.rs` checks that its own type for a sink measures the same.
pub const SINK_SIZE: usize = 48;
/// The offset in bytes of a sink's text length, which generated code reads after
/// `sisu_show_finish`.
pub const SINK_LEN: usize = 24;
/// The offset in bytes of a sink's text capacity, which generated code reads after
/// `sisu_show_finish`.
pub const SINK_CAP: usize = 32;

/// The sink writes to the standard output buffer.
const STDOUT: usize = 0;
/// The sink writes to a block it owns.
const BUFFER: usize = 1;
/// The sink records pieces and group events for a layout.
const PRETTY: usize = 2;
/// The columns a line may fill before a group breaks.
const WIDTH: usize = 80;
/// The columns that a broken group indents its children by.
const INDENT: usize = 4;
/// The text capacity of a new buffer sink, in bytes. It doubles when the text outgrows it.
const FIRST_CAPACITY: usize = 64;
/// The alignment of a buffer sink's block, the string header's.
const BLOCK_ALIGN: usize = 8;

/// A sink for the text of a value. Generated code reads `len` and `cap` after
/// `sisu_show_finish`, at `SINK_LEN` and `SINK_CAP`.
#[derive(Debug)]
#[repr(C)]
pub struct Sink {
    /// `STDOUT`, `BUFFER` or `PRETTY`.
    kind: usize,
    /// The block of a buffer sink.
    block: *mut u8,
    /// The bytes at the start of the block that the sink never touches.
    header: usize,
    /// The bytes of text written, after the header.
    len: usize,
    /// The bytes of text that the block has room for, after the header.
    cap: usize,
    /// What a pretty sink has recorded, and the sink it lays out into; `None` for the others.
    pretty: Option<Box<Recorder>>,
}

/// The size of a block with `header` bytes and `cap` bytes of text. A size that overflows is a
/// layout `sisu_alloc` refuses, so the process aborts as it does there.
fn block_size(header: usize, cap: usize) -> usize {
    header
        .checked_add(cap)
        .unwrap_or_else(|| std::process::abort())
}

impl Sink {
    /// Writes `bytes` to the sink.
    fn put(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if let Some(recorder) = self.pretty.as_deref_mut() {
            recorder.record(Event::Text, bytes);
            return;
        }
        if self.kind == STDOUT {
            // The lock is released before the panic, which flushes through it.
            let result = with_stdout(|out| out.append_piece(bytes));
            if let Err(error) = result {
                stdout_panic(&error);
            }
            return;
        }
        self.reserve(bytes.len());
        let end = self.block.wrapping_add(self.header + self.len);
        #[expect(
            unsafe_code,
            reason = "copies into the block that `reserve` has made room in"
        )]
        // SAFETY: `reserve` made the block hold `header + len + bytes.len()` bytes, so `end`
        // is inside it with room for `bytes`, which is not part of the block.
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), end, bytes.len());
        }
        self.len += bytes.len();
    }

    /// Makes room for `extra` more bytes of text, doubling the capacity until they fit.
    fn reserve(&mut self, extra: usize) {
        let needed = self
            .len
            .checked_add(extra)
            .unwrap_or_else(|| std::process::abort());
        let mut cap = self.cap;
        while cap < needed {
            cap = cap.checked_mul(2).unwrap_or_else(|| std::process::abort());
        }
        if cap == self.cap {
            return;
        }
        let old_size = block_size(self.header, self.cap);
        let new_size = block_size(self.header, cap);
        #[expect(unsafe_code, reason = "grows the block this sink owns")]
        // SAFETY: the block came from `sisu_alloc` or `sisu_realloc` with `old_size` bytes and
        // `BLOCK_ALIGN`, and nothing else keeps a pointer to it.
        let grown = unsafe { sisu_realloc(self.block, old_size, BLOCK_ALIGN, new_size) };
        self.block = grown;
        self.cap = cap;
    }

    /// Pads the text written since `mark` to `width` code points, in place, as `sisu_pad` pads a
    /// part. Interpolation pads a part of a buffer sink: standard output has taken its text
    /// already, so a standard output sink cannot pad.
    fn pad(&mut self, mark: usize, width: usize, align: u8) {
        if self.kind != BUFFER || mark > self.len {
            std::process::abort();
        }
        // `sisu_pad` writes at most `width` bytes after the text, so make room for as many.
        self.reserve(width);
        let text = self.block.wrapping_add(self.header + mark);
        #[expect(
            unsafe_code,
            reason = "pads the text in the block that `reserve` has grown"
        )]
        // SAFETY: the block holds the `len - mark` bytes of text at `text`, and `reserve` made
        // room for `width` more after `len`, so `len - mark + width` bytes are writable there.
        let padded = unsafe { sisu_pad(text, self.len - mark, width, align) };
        self.len = mark + padded;
    }

    /// Writes `count` spaces.
    fn spaces(&mut self, count: usize) {
        const BLANKS: [u8; 64] = [b' '; 64];
        for _ in 0..count / BLANKS.len() {
            self.put(&BLANKS);
        }
        self.put(&BLANKS[..count % BLANKS.len()]);
    }

    /// Writes the text of a group event: a pretty sink records it, another sink writes it.
    fn group_text(&mut self, event: fn(Span) -> Event, bytes: &[u8]) {
        match self.pretty.as_deref_mut() {
            Some(recorder) => recorder.record(event, bytes),
            None => self.put(bytes),
        }
    }

    /// Ends the line: a standard output sink writes the newline and the line's end, a buffer
    /// sink writes the newline.
    fn line_end(&mut self) {
        if self.kind == STDOUT {
            let result = with_stdout(crate::Stdout::finish_line);
            if let Err(error) = result {
                stdout_panic(&error);
            }
        } else {
            self.put(b"\n");
        }
    }
}

/// The escape of `c` in a literal quoted with the ASCII `quote`, as Sisu's literals write it:
/// `\n`, `\t`, `\r`, `\0`, `\\`, the quote, and `\u{X}` in uppercase hex for any other control
/// character and for U+2028 and U+2029, so a nested string never holds a line break. `None` for
/// a `char` that a literal holds as it is. The escape is in `buffer`.
fn escape(c: char, quote: char, buffer: &mut [u8; 10]) -> Option<&[u8]> {
    let letter = match c {
        '\n' => b'n',
        '\t' => b't',
        '\r' => b'r',
        '\0' => b'0',
        '\\' => b'\\',
        c if c == quote => u8::try_from(quote).expect("a quote should be ASCII"),
        c if c.is_control() || c == '\u{2028}' || c == '\u{2029}' => {
            let total = buffer.len();
            let mut rest = &mut buffer[..];
            write!(rest, "\\u{{{:X}}}", u32::from(c)).expect("10 bytes should hold every escape");
            let len = total - rest.len();
            return Some(&buffer[..len]);
        }
        _ => return None,
    };
    buffer[..2].copy_from_slice(&[b'\\', letter]);
    Some(&buffer[..2])
}

/// Writes `text` between two `quote`s, with the escapes of [`escape`]. A run of `char`s that
/// need none is one piece.
fn put_literal(sink: &mut Sink, text: &str, quote: char) {
    let mut quote_bytes = [0; 4];
    let quote_text = quote.encode_utf8(&mut quote_bytes).as_bytes();
    sink.put(quote_text);
    let mut from = 0;
    for (at, c) in text.char_indices() {
        if let Some(escaped) = escape(c, quote, &mut [0; 10]) {
            sink.put(&text.as_bytes()[from..at]);
            sink.put(escaped);
            from = at + c.len_utf8();
        }
    }
    sink.put(&text.as_bytes()[from..]);
    sink.put(quote_text);
}

/// A run of recorded text: `len` bytes at `at` in the recorder's text.
#[derive(Clone, Copy, Debug)]
struct Span {
    at: usize,
    len: usize,
}

/// What a pretty sink records, in the order of the calls.
#[derive(Clone, Copy, Debug)]
enum Event {
    /// Text that is part of a value.
    Text(Span),
    /// The text that starts a group.
    Open(Span),
    /// The place between two parts of a group.
    Sep,
    /// The text that ends a group.
    Close(Span),
}

/// The events of a pretty sink, the bytes of their text, and the sink to lay them out into. The
/// text is copied at once, since the caller may free a piece when its call returns.
#[derive(Debug)]
struct Recorder {
    /// The sink that `sisu_show_pretty_finish` lays the text out into.
    inner: *mut Sink,
    text: Vec<u8>,
    events: Vec<Event>,
    /// The events read through `event`, which a test checks against the layout's two passes.
    #[cfg(test)]
    reads: std::cell::Cell<usize>,
}

impl Recorder {
    fn new(inner: *mut Sink) -> Self {
        Self {
            inner,
            text: Vec::new(),
            events: Vec::new(),
            #[cfg(test)]
            reads: std::cell::Cell::new(0),
        }
    }

    /// Records `bytes`, a copy of them, as the event that `event` makes.
    fn record(&mut self, event: fn(Span) -> Event, bytes: &[u8]) {
        let span = Span {
            at: self.text.len(),
            len: bytes.len(),
        };
        self.text.extend_from_slice(bytes);
        self.events.push(event(span));
    }

    /// The event `i`. Both passes of the layout read an event only through this function.
    fn event(&self, i: usize) -> Event {
        #[cfg(test)]
        self.reads.set(self.reads.get() + 1);
        self.events[i]
    }

    /// The recorded text of `span`.
    fn bytes(&self, span: Span) -> &[u8] {
        &self.text[span.at..span.at + span.len]
    }

    /// The width that each group needs on one line, at the index of its `Open`. It is its flat
    /// width, and 1 more when a `,` follows it. The pass goes from the last event to the first.
    /// It keeps a stack of the widths of the groups still open, since a group can hold 1,000,000
    /// levels. A text piece that holds a line break has no flat width, so it is `usize::MAX`. Then
    /// every group around it needs more than a line.
    fn group_needs(&self) -> Vec<usize> {
        let mut needs = vec![0; self.events.len()];
        // The width so far of the group that the pass is inside, and of each group around it,
        // with whether a `Sep` follows that group.
        let mut width = 0usize;
        let mut outer: Vec<(usize, bool)> = Vec::new();
        let mut sep_follows = false;
        for i in (0..self.events.len()).rev() {
            let event = self.event(i);
            match event {
                Event::Text(span) => width = width.saturating_add(flat_width(self.bytes(span))),
                Event::Sep => width = width.saturating_add(2),
                Event::Close(span) => {
                    outer.push((width, sep_follows));
                    width = flat_width(self.bytes(span));
                }
                Event::Open(span) => {
                    width = width.saturating_add(flat_width(self.bytes(span)));
                    let (around, comma) = outer.pop().expect("an open should have a close");
                    needs[i] = width.saturating_add(usize::from(comma));
                    width = width.saturating_add(around);
                }
            }
            sep_follows = matches!(event, Event::Sep);
        }
        needs
    }
}

/// The Unicode scalar values in `bytes`; a byte that continues a code point is not one.
fn chars(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&byte| byte & 0xC0 != 0x80).count()
}

/// The width of `bytes` on one line, or `usize::MAX` when they hold a line break.
fn flat_width(bytes: &[u8]) -> usize {
    if bytes.contains(&b'\n') {
        usize::MAX
    } else {
        chars(bytes)
    }
}

/// The column after `bytes` written at column `col`: counted from the last line break in them.
fn column_after(col: usize, bytes: &[u8]) -> usize {
    match bytes.iter().rposition(|&byte| byte == b'\n') {
        Some(at) => chars(&bytes[at + 1..]),
        None => col.saturating_add(chars(bytes)),
    }
}

/// Where a layout writes.
trait Output {
    fn text(&mut self, bytes: &[u8]);
    fn spaces(&mut self, count: usize);
}

impl Output for Sink {
    fn text(&mut self, bytes: &[u8]) {
        self.put(bytes);
    }

    fn spaces(&mut self, count: usize) {
        Sink::spaces(self, count);
    }
}

/// A group that the layout has opened and not closed.
struct Frame {
    /// The group has a line for each child.
    broken: bool,
    /// The line break before the first child is written.
    started: bool,
    /// The columns before the group's own lines: four per enclosing broken group.
    indent: usize,
}

/// The second pass of the layout. It goes over the events from the first to the last. It keeps a
/// stack of the groups that are open, and the column of the text written so far. The layout
/// starts at column 0, wherever the text lands.
struct Layout<'a, O: Output> {
    out: &'a mut O,
    col: usize,
    frames: Vec<Frame>,
}

impl<O: Output> Layout<'_, O> {
    fn text(&mut self, bytes: &[u8]) {
        self.out.text(bytes);
        self.col = column_after(self.col, bytes);
    }

    fn line_break(&mut self, indent: usize) {
        self.out.text(b"\n");
        self.out.spaces(indent);
        self.col = indent;
    }

    /// Starts the line of the next child of the group that is open, if it is broken. A group with
    /// no child writes no line break.
    fn start_child(&mut self) {
        if let Some(frame) = self.frames.last_mut()
            && frame.broken
            && !frame.started
        {
            frame.started = true;
            let indent = frame.indent + INDENT;
            self.line_break(indent);
        }
    }

    /// Opens a group that needs `need` columns on one line. It stays flat when it fits from the
    /// column it starts at, or when the group around it is flat.
    fn open(&mut self, bytes: &[u8], need: usize) {
        self.start_child();
        let (inside_flat, indent) = self.frames.last().map_or((false, 0), |frame| {
            let step = if frame.broken { INDENT } else { 0 };
            (!frame.broken, frame.indent + step)
        });
        let broken = !inside_flat && self.col.saturating_add(need) > WIDTH;
        self.frames.push(Frame {
            broken,
            started: false,
            indent,
        });
        self.text(bytes);
    }

    fn sep(&mut self) {
        let broken_child_indent = self
            .frames
            .last()
            .filter(|frame| frame.broken)
            .map(|frame| frame.indent + INDENT);
        if let Some(indent) = broken_child_indent {
            self.start_child();
            self.text(b",");
            self.line_break(indent);
        } else {
            self.text(b", ");
        }
    }

    fn close(&mut self, bytes: &[u8]) {
        let frame = self.frames.pop().expect("a close should have an open");
        if frame.broken && frame.started {
            self.line_break(frame.indent);
        }
        self.text(bytes);
    }
}

/// Lays the events of `recorder` out into `out` to fit 80 columns. Each group is its open text,
/// its children and its close text. Flat, the children are on one line, joined by `, `. Broken,
/// each child has a line, four columns in from the group, and all but the last end in `,`. The
/// close text has a line at the group's own indentation. Groups are decided from the outermost.
/// A group stays flat when its column, its flat width and a following `,` fit in 80 columns. The
/// children of a flat group are flat. A group with no child is always flat. Widths count Unicode
/// scalar values. There are two passes, each a loop over the events. So the work is linear in
/// the events, the text recorded and the text written.
fn lay_out(recorder: &Recorder, out: &mut impl Output) {
    let needs = recorder.group_needs();
    let mut layout = Layout {
        out,
        col: 0,
        frames: Vec::new(),
    };
    for (i, &need) in needs.iter().enumerate() {
        match recorder.event(i) {
            Event::Text(span) => {
                layout.start_child();
                layout.text(recorder.bytes(span));
            }
            Event::Open(span) => layout.open(recorder.bytes(span), need),
            Event::Sep => layout.sep(),
            Event::Close(span) => layout.close(recorder.bytes(span)),
        }
    }
}

/// The sink at `sink`.
///
/// # Safety
///
/// `sink` points to a sink that a `sisu_show_stdout`, `sisu_show_buffer` or `sisu_show_pretty`
/// call started, and nothing else uses it for `'a`.
#[expect(
    unsafe_code,
    reason = "turns the pointer generated code passes into a reference"
)]
unsafe fn sink_at<'a>(sink: *mut Sink) -> &'a mut Sink {
    // SAFETY: the caller promises that `sink` is started and not shared.
    unsafe { &mut *sink }
}

/// The `len` bytes at `bytes`.
///
/// # Safety
///
/// `bytes` is not null and `len` bytes at it are readable for `'a`.
#[expect(
    unsafe_code,
    reason = "turns the pointer generated code passes into a slice"
)]
unsafe fn bytes_at<'a>(bytes: *const u8, len: usize) -> &'a [u8] {
    // SAFETY: the caller promises `len` readable bytes at `bytes`.
    unsafe { std::slice::from_raw_parts(bytes, len) }
}

/// Starts a sink over the standard output buffer at `sink`. Text goes to the buffer as the sink
/// gets it, and `sisu_show_line_end` ends the line. No lock is held between two calls, so a
/// written `to_string` that prints writes where the line has got to.
///
/// # Safety
///
/// `sink` is writable for `SINK_SIZE` bytes, aligned to 8.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name and passes a raw pointer"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_show_stdout(sink: *mut Sink) {
    // SAFETY: the caller promises room for a sink at `sink`.
    unsafe {
        sink.write(Sink {
            kind: STDOUT,
            block: ptr::null_mut(),
            header: 0,
            len: 0,
            cap: 0,
            pretty: None,
        });
    }
}

/// Starts a sink at `sink` that builds a block of `header` bytes, which the sink never touches,
/// and then 64 bytes of text room. The room doubles when the text outgrows it. The block is
/// aligned to 8, and `sisu_free` frees it with `header` plus the capacity that the sink ends
/// with.
///
/// # Safety
///
/// `sink` is writable for `SINK_SIZE` bytes, aligned to 8.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name and passes a raw pointer"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_show_buffer(sink: *mut Sink, header: usize) {
    let block = sisu_alloc(block_size(header, FIRST_CAPACITY), BLOCK_ALIGN);
    // SAFETY: the caller promises room for a sink at `sink`.
    unsafe {
        sink.write(Sink {
            kind: BUFFER,
            block,
            header,
            len: 0,
            cap: FIRST_CAPACITY,
            pretty: None,
        });
    }
}

/// Ends a buffer sink and returns its block. The sink keeps its text length (`len`) and its text
/// capacity (`cap`), which generated code reads to write the string header in place.
///
/// # Safety
///
/// `sink` is a started buffer sink.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name and passes a raw pointer"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_show_finish(sink: *mut Sink) -> *mut u8 {
    // SAFETY: the caller promises that `sink` is started.
    unsafe { sink_at(sink) }.block
}

/// Ends the line of `sink`.
///
/// # Safety
///
/// `sink` is a started sink.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name and passes a raw pointer"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_show_line_end(sink: *mut Sink) {
    // SAFETY: the caller promises that `sink` is started.
    let sink = unsafe { sink_at(sink) };
    debug_assert_ne!(sink.kind, PRETTY, "a pretty sink ends no line");
    sink.line_end();
}

/// The length of the text of `sink` so far, for `sisu_show_pad` to pad from. A buffer sink
/// only: standard output has taken its text already, and `#` takes no width.
///
/// # Safety
///
/// `sink` is a started sink.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name and passes a raw pointer"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_show_mark(sink: *mut Sink) -> usize {
    // SAFETY: the caller promises that `sink` is started.
    let sink = unsafe { sink_at(sink) };
    debug_assert_ne!(sink.kind, PRETTY, "a pretty sink takes no width");
    sink.len
}

/// Pads the text written since `mark`, a result of `sisu_show_mark` on the same buffer sink, to
/// `width` code points, in place, as `sisu_pad` does: `align` 0 puts the text on the left, 1 on
/// the right and 2 in the centre. The sink grows if the padding needs room.
///
/// # Safety
///
/// `sink` is a started buffer sink. `mark` is at most its text length.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name and passes a raw pointer"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_show_pad(sink: *mut Sink, mark: usize, width: usize, align: u8) {
    // SAFETY: the caller promises that `sink` is started.
    unsafe { sink_at(sink) }.pad(mark, width, align);
}

/// Starts a pretty sink at `sink` over `inner`. The sink records the pieces and the group events
/// that it gets, as a flat sink would write them. It copies the bytes of each piece before the
/// call returns. `sisu_show_pretty_finish` lays them out into `inner`. Until then `inner` has
/// taken no text from the pretty sink. A pretty sink takes no `sisu_show_mark` and
/// `sisu_show_pad`, as `#` takes no width.
///
/// # Safety
///
/// `sink` is writable for `SINK_SIZE` bytes, aligned to 8. `inner` is a started standard output
/// or buffer sink that is not `sink`.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name and passes raw pointers"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_show_pretty(sink: *mut Sink, inner: *mut Sink) {
    // SAFETY: the caller promises room for a sink at `sink`.
    unsafe {
        sink.write(Sink {
            kind: PRETTY,
            block: ptr::null_mut(),
            header: 0,
            len: 0,
            cap: 0,
            pretty: Some(Box::new(Recorder::new(inner))),
        });
    }
}

/// Ends a pretty sink: lays out what it recorded into its `inner` sink, then frees what it
/// recorded. The layout starts at column 0, wherever the text in `inner` has got to.
///
/// # Safety
///
/// `sink` is a started pretty sink, and its `inner` sink is still started and used by nothing
/// else.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name and passes a raw pointer"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_show_pretty_finish(sink: *mut Sink) {
    // SAFETY: the caller promises that `sink` is started.
    let recorder = unsafe { sink_at(sink) }
        .pretty
        .take()
        .expect("a pretty sink should be finished once");
    // SAFETY: the caller promises that `inner` is started and not shared.
    lay_out(&recorder, unsafe { sink_at(recorder.inner) });
}

/// Writes the `len` bytes at `bytes` as they are.
///
/// # Safety
///
/// `sink` is a started sink. `bytes` is not null and `len` bytes at it are readable; valid
/// UTF-8 is not required.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name and passes raw pointers"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_show_str(sink: *mut Sink, bytes: *const u8, len: usize) {
    // SAFETY: the caller promises that `sink` is started.
    let sink = unsafe { sink_at(sink) };
    // SAFETY: the caller promises `len` readable bytes at `bytes`.
    sink.put(unsafe { bytes_at(bytes, len) });
}

/// Writes the `len` bytes at `bytes` between double quotes, escaped as a string literal writes
/// them. A byte that is not valid UTF-8 becomes U+FFFD, as `read_line` repairs its input.
///
/// # Safety
///
/// `sink` is a started sink. `bytes` is not null and `len` bytes at it are readable.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name and passes raw pointers"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_show_quoted(sink: *mut Sink, bytes: *const u8, len: usize) {
    // SAFETY: the caller promises that `sink` is started.
    let sink = unsafe { sink_at(sink) };
    // SAFETY: the caller promises `len` readable bytes at `bytes`.
    let text = String::from_utf8_lossy(unsafe { bytes_at(bytes, len) });
    put_literal(sink, &text, '"');
}

/// Writes the `char` with scalar value `c`: as it is, or when `quoted`, between single quotes
/// and escaped as a `char` literal writes it. Generated code passes only scalar values.
///
/// # Safety
///
/// `sink` is a started sink.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name and passes a raw pointer"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_show_char(sink: *mut Sink, c: u32, quoted: bool) {
    // SAFETY: the caller promises that `sink` is started.
    let sink = unsafe { sink_at(sink) };
    let Some(c) = char::from_u32(c) else {
        std::process::abort()
    };
    if quoted {
        put_literal(sink, c.encode_utf8(&mut [0; 4]), '\'');
    } else {
        sink.put(c.encode_utf8(&mut [0; 4]).as_bytes());
    }
}

/// Writes `value` in decimal, as `print` writes it.
///
/// # Safety
///
/// `sink` is a started sink.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name and passes a raw pointer"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_show_int(sink: *mut Sink, value: i64) {
    // SAFETY: the caller promises that `sink` is started.
    unsafe { sink_at(sink) }.put(text::int_text(value, &mut [0; 20]));
}

/// Writes `x` as `print` writes it.
///
/// # Safety
///
/// `sink` is a started sink.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name and passes a raw pointer"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_show_f64(sink: *mut Sink, x: f64) {
    // SAFETY: the caller promises that `sink` is started.
    unsafe { sink_at(sink) }.put(float::shortest(x, &mut [0; 24]));
}

/// Writes `x` with `precision` decimals, as interpolation writes it (`sisu_write_f64_fixed`).
///
/// # Safety
///
/// `sink` is a started sink. `precision` is at most 30.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name and passes a raw pointer"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_show_f64_fixed(sink: *mut Sink, x: f64, precision: u8) {
    // SAFETY: the caller promises that `sink` is started.
    let sink = unsafe { sink_at(sink) };
    // The longest text at precision 30 is 341 bytes (`sisu_write_f64_fixed`).
    let mut buffer = [0; 341];
    #[expect(unsafe_code, reason = "calls the runtime's C ABI function")]
    // SAFETY: `buffer` holds the 311 bytes and the precision that the text takes at most, and
    // the caller promises a precision of at most 30.
    let len = unsafe { float::sisu_write_f64_fixed(x, precision, buffer.as_mut_ptr()) };
    sink.put(&buffer[..len]);
}

/// Writes `true` or `false`.
///
/// # Safety
///
/// `sink` is a started sink.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name and passes a raw pointer"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_show_bool(sink: *mut Sink, value: bool) {
    // SAFETY: the caller promises that `sink` is started.
    unsafe { sink_at(sink) }.put(text::bool_text(value));
}

/// Starts a group: writes the `len` bytes at `bytes`, such as `Add(` or `[`. A pretty sink
/// records a group that may break after them.
///
/// # Safety
///
/// `sink` is a started sink. `bytes` is not null and `len` bytes at it are readable.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name and passes raw pointers"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_show_open(sink: *mut Sink, bytes: *const u8, len: usize) {
    // SAFETY: the caller promises that `sink` is started.
    let sink = unsafe { sink_at(sink) };
    // SAFETY: the caller promises `len` readable bytes at `bytes`.
    sink.group_text(Event::Open, unsafe { bytes_at(bytes, len) });
}

/// Separates two parts of a group: a flat sink writes `, `, and a pretty sink records the place.
///
/// # Safety
///
/// `sink` is a started sink.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name and passes a raw pointer"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_show_sep(sink: *mut Sink) {
    // SAFETY: the caller promises that `sink` is started.
    let sink = unsafe { sink_at(sink) };
    match sink.pretty.as_deref_mut() {
        Some(recorder) => recorder.events.push(Event::Sep),
        None => sink.put(b", "),
    }
}

/// Ends a group: writes the `len` bytes at `bytes`, such as `)` or `]`. A pretty sink records them
/// as the end of its group.
///
/// # Safety
///
/// `sink` is a started sink. `bytes` is not null and `len` bytes at it are readable.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name and passes raw pointers"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_show_close(sink: *mut Sink, bytes: *const u8, len: usize) {
    // SAFETY: the caller promises that `sink` is started.
    let sink = unsafe { sink_at(sink) };
    // SAFETY: the caller promises `len` readable bytes at `bytes`.
    sink.group_text(Event::Close, unsafe { bytes_at(bytes, len) });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Sink as FileSink, Stdout, sisu_free};
    use std::mem::{MaybeUninit, offset_of, size_of};

    /// The header bytes that a buffer sink must leave alone.
    const HEADER: usize = 24;
    const SENTINEL: u8 = 0xAB;

    /// Runs `write` on a buffer sink with a header of `HEADER` bytes, and gives the text it
    /// wrote and the capacity of its text. Checks that the header kept its bytes.
    #[expect(unsafe_code, reason = "the test calls the runtime's C ABI")]
    fn buffered(write: impl FnOnce(*mut Sink)) -> (String, usize) {
        let mut slot = MaybeUninit::<Sink>::uninit();
        let sink = slot.as_mut_ptr();
        // SAFETY: `sink` is writable for one `Sink`.
        unsafe { sisu_show_buffer(sink, HEADER) };
        // SAFETY: `sisu_show_buffer` has started the sink, so its block is allocated.
        let block = unsafe { (*sink).block };
        // SAFETY: the block holds `HEADER` bytes before the text.
        unsafe { block.write_bytes(SENTINEL, HEADER) };
        write(sink);
        // SAFETY: the sink is a started buffer sink.
        let block = unsafe { sisu_show_finish(sink) };
        // SAFETY: `sisu_show_finish` leaves the sink's fields as they were.
        let len = unsafe { (*sink).len };
        // SAFETY: as above.
        let cap = unsafe { (*sink).cap };
        // SAFETY: the block holds `HEADER + cap` bytes, and the header ones are written above.
        let all = unsafe { std::slice::from_raw_parts(block, HEADER + cap) };
        assert_eq!(
            all[..HEADER],
            [SENTINEL; HEADER],
            "the header should keep its bytes"
        );
        let text = String::from_utf8_lossy(&all[HEADER..HEADER + len]).into_owned();
        // SAFETY: the block came from `sisu_alloc` with this size and alignment.
        unsafe { sisu_free(block, HEADER + cap, 8) };
        (text, cap)
    }

    /// The text of `write` on a buffer sink.
    fn text_of(write: impl FnOnce(*mut Sink)) -> String {
        buffered(write).0
    }

    #[expect(unsafe_code, reason = "the test calls the runtime's C ABI")]
    fn quoted(sink: *mut Sink, text: &[u8]) {
        // SAFETY: `sink` is started, and `text` is readable for its length.
        unsafe { sisu_show_quoted(sink, text.as_ptr(), text.len()) };
    }

    #[expect(unsafe_code, reason = "the test calls the runtime's C ABI")]
    fn raw(sink: *mut Sink, text: &str) {
        // SAFETY: `sink` is started, and `text` is readable for its length.
        unsafe { sisu_show_str(sink, text.as_ptr(), text.len()) };
    }

    #[expect(unsafe_code, reason = "the test calls the runtime's C ABI")]
    fn character(sink: *mut Sink, c: char, quoted: bool) {
        // SAFETY: `sink` is started.
        unsafe { sisu_show_char(sink, u32::from(c), quoted) };
    }

    /// Each escape that a string literal writes, and the characters it leaves alone: `'` is
    /// bare in a string, and `ä` is not escaped.
    #[test]
    fn quoted_should_escape_as_a_literal_writes_it() {
        for (text, want) in [
            ("hi", r#""hi""#),
            ("", r#""""#),
            ("a\"b", r#""a\"b""#),
            ("it's", r#""it's""#),
            ("a\\b", r#""a\\b""#),
            ("a\nb", r#""a\nb""#),
            ("a\tb", r#""a\tb""#),
            ("a\rb", r#""a\rb""#),
            ("a\0b", r#""a\0b""#),
            ("\u{1}", r#""\u{1}""#),
            ("\u{1B}", r#""\u{1B}""#),
            ("\u{7F}", r#""\u{7F}""#),
            ("\u{85}", r#""\u{85}""#),
            ("\u{2028}", r#""\u{2028}""#),
            ("\u{2029}", r#""\u{2029}""#),
            ("ä", "\"ä\""),
            ("\u{FFFD}", "\"\u{FFFD}\""),
        ] {
            assert_eq!(text_of(|s| quoted(s, text.as_bytes())), want, "{text:?}");
        }
    }

    /// `0xFF` is not UTF-8, so it must become U+FFFD and not reach the output as it is.
    #[test]
    fn quoted_should_repair_bytes_that_are_not_utf8() {
        assert_eq!(text_of(|s| quoted(s, b"a\xFFb")), "\"a\u{FFFD}b\"");
    }

    /// A `char` literal escapes `'` and not `"`, the reverse of a string's; unquoted, the `char`
    /// is its UTF-8 bytes as they are, a line break included.
    #[test]
    fn char_should_escape_as_a_literal_writes_it() {
        for (c, quoted, want) in [
            ('a', true, "'a'"),
            ('\'', true, r"'\''"),
            ('"', true, "'\"'"),
            ('\\', true, r"'\\'"),
            ('\n', true, r"'\n'"),
            ('\t', true, r"'\t'"),
            ('\r', true, r"'\r'"),
            ('\0', true, r"'\0'"),
            ('\u{1}', true, r"'\u{1}'"),
            ('\u{1B}', true, r"'\u{1B}'"),
            ('\u{7F}', true, r"'\u{7F}'"),
            ('\u{2028}', true, r"'\u{2028}'"),
            ('\u{2029}', true, r"'\u{2029}'"),
            ('ä', true, "'ä'"),
            ('a', false, "a"),
            ('ä', false, "ä"),
            ('"', false, "\""),
            ('\'', false, "'"),
            ('\n', false, "\n"),
        ] {
            assert_eq!(
                text_of(|s| character(s, c, quoted)),
                want,
                "{c:?}, quoted {quoted}"
            );
        }
    }

    #[expect(unsafe_code, reason = "the test calls the runtime's C ABI")]
    fn group(sink: *mut Sink, (open, close): (&str, &str), parts: &[i64]) {
        // SAFETY: `sink` is started, and the texts are readable for their lengths.
        unsafe { sisu_show_open(sink, open.as_ptr(), open.len()) };
        for (i, &part) in parts.iter().enumerate() {
            if i > 0 {
                // SAFETY: `sink` is started.
                unsafe { sisu_show_sep(sink) };
            }
            // SAFETY: `sink` is started.
            unsafe { sisu_show_int(sink, part) };
        }
        // SAFETY: `sink` is started, and `close` is readable for its length.
        unsafe { sisu_show_close(sink, close.as_ptr(), close.len()) };
    }

    /// A flat sink writes the events of `Add(Num(1), Num(2))` on one line.
    #[test]
    fn groups_should_write_open_sep_and_close_on_a_flat_sink() {
        let text = text_of(|s| {
            raw(s, "Add(");
            group(s, ("Num(", ")"), &[1]);
            sep(s);
            group(s, ("Num(", ")"), &[2]);
            raw(s, ")");
        });
        assert_eq!(text, "Add(Num(1), Num(2))");
        assert_eq!(text_of(|s| group(s, ("[", "]"), &[1, 2, 3])), "[1, 2, 3]");
    }

    #[expect(unsafe_code, reason = "the test calls the runtime's C ABI")]
    fn sep(sink: *mut Sink) {
        // SAFETY: `sink` is started.
        unsafe { sisu_show_sep(sink) };
    }

    /// Each scalar writes the text that `print` writes for it.
    #[expect(unsafe_code, reason = "the test calls the runtime's C ABI")]
    #[test]
    fn scalars_should_write_the_text_print_writes() {
        let text = text_of(|s| {
            // SAFETY: `s` is a started sink; each call is one unsafe operation.
            let int = |value| unsafe { sisu_show_int(s, value) };
            // SAFETY: as above.
            let float = |x| unsafe { sisu_show_f64(s, x) };
            // SAFETY: as above, and 2 is below the largest precision.
            let fixed = |x, precision| unsafe { sisu_show_f64_fixed(s, x, precision) };
            // SAFETY: as above.
            let boolean = |value| unsafe { sisu_show_bool(s, value) };
            int(i64::MIN);
            sep(s);
            float(0.5);
            sep(s);
            float(f64::NAN);
            sep(s);
            fixed(2.0, 2);
            sep(s);
            boolean(true);
            sep(s);
            boolean(false);
        });
        assert_eq!(text, "-9223372036854775808, 0.5, nan, 2.00, true, false");
    }

    /// A new buffer has room for 64 bytes. The 65th byte doubles the room, then the 129th does,
    /// and the header bytes keep their value through each growth (`buffered` checks them).
    #[test]
    fn buffer_should_start_with_64_bytes_and_double() {
        for (len, cap) in [
            (0, 64),
            (1, 64),
            (64, 64),
            (65, 128),
            (128, 128),
            (129, 256),
        ] {
            let (text, got) = buffered(|s| {
                for i in 0..len {
                    raw(s, &(i % 10).to_string());
                }
            });
            assert_eq!(got, cap, "{len} bytes");
            assert_eq!(text.len(), len, "{len} bytes");
        }
        // One piece that is longer than the room doubles it until the piece fits.
        let long = "x".repeat(1000);
        let (text, cap) = buffered(|s| raw(s, &long));
        assert_eq!((text, cap), (long, 1024));
    }

    /// `text` after `before`, padded by `sisu_show_pad` from the mark that `sisu_show_mark` gave
    /// before `text`; `before` stays as it is.
    #[expect(unsafe_code, reason = "the test calls the runtime's C ABI")]
    fn padded_after(before: &str, text: &str, width: usize, align: u8) -> String {
        text_of(|s| {
            raw(s, before);
            // SAFETY: `s` is a started buffer sink.
            let mark = unsafe { sisu_show_mark(s) };
            raw(s, text);
            // SAFETY: as above, and `mark` came from the same sink.
            unsafe { sisu_show_pad(s, mark, width, align) };
        })
    }

    /// `sisu_show_pad` pads the text since the mark as `sisu_pad` pads a part, for each
    /// alignment, and never the text before the mark.
    #[test]
    fn pad_should_pad_the_text_since_the_mark_as_sisu_pad_does() {
        for (text, width, align, want) in [
            ("é", 5, 1, "    é"),
            ("日本", 7, 2, "  日本   "),
            ("abc", 2, 0, "abc"),
            ("abc", 3, 1, "abc"),
            ("ab", 4, 0, "ab  "),
            ("ab", 4, 1, "  ab"),
            ("ab", 4, 2, " ab "),
            ("ab", 5, 2, " ab  "),
            ("", 3, 0, "   "),
        ] {
            let got = padded_after("xy", text, width, align);
            assert_eq!(got, format!("xy{want}"), "{text:?} {width} {align}");
        }
    }

    /// The room for the padding is made when the text is at the end of the room, and the
    /// text before the mark stays.
    #[test]
    fn pad_should_grow_the_buffer_when_the_padding_does_not_fit() {
        let before = "b".repeat(60);
        let got = padded_after(&before, "abc", 40, 1);
        assert_eq!(got, format!("{before}{}abc", " ".repeat(37)));
    }

    /// A mark is the length of the text so far.
    #[expect(unsafe_code, reason = "the test calls the runtime's C ABI")]
    #[test]
    fn mark_should_be_the_length_of_the_text_so_far() {
        let marks = std::cell::RefCell::new(Vec::new());
        let text = text_of(|s| {
            // SAFETY: `s` is a started buffer sink.
            marks.borrow_mut().push(unsafe { sisu_show_mark(s) });
            raw(s, "abc");
            // SAFETY: as above.
            marks.borrow_mut().push(unsafe { sisu_show_mark(s) });
        });
        assert_eq!(text, "abc");
        assert_eq!(*marks.borrow(), [0, 3]);
    }

    /// An event of a pretty sink, as a test plays it.
    enum Ev {
        Text(String),
        Open(String),
        Sep,
        Close(String),
    }

    fn t(text: &str) -> Ev {
        Ev::Text(text.to_owned())
    }

    fn o(text: &str) -> Ev {
        Ev::Open(text.to_owned())
    }

    fn c(text: &str) -> Ev {
        Ev::Close(text.to_owned())
    }

    #[expect(unsafe_code, reason = "the test calls the runtime's C ABI")]
    fn play(sink: *mut Sink, events: &[Ev]) {
        for event in events {
            match event {
                Ev::Text(text) => raw(sink, text),
                // SAFETY: `sink` is started, and the text is readable for its length.
                Ev::Open(text) => unsafe { sisu_show_open(sink, text.as_ptr(), text.len()) },
                // SAFETY: `sink` is started.
                Ev::Sep => unsafe { sisu_show_sep(sink) },
                // SAFETY: `sink` is started, and the text is readable for its length.
                Ev::Close(text) => unsafe { sisu_show_close(sink, text.as_ptr(), text.len()) },
            }
        }
    }

    /// The text that a pretty sink over a buffer sink with `before` in it lays out for `play`,
    /// without `before`.
    #[expect(unsafe_code, reason = "the test calls the runtime's C ABI")]
    fn laid_out(before: &str, play: impl FnOnce(*mut Sink)) -> String {
        let all = text_of(|inner| {
            raw(inner, before);
            let mut slot = MaybeUninit::<Sink>::uninit();
            let sink = slot.as_mut_ptr();
            // SAFETY: `sink` is writable for one sink, and `inner` is a started buffer sink.
            unsafe { sisu_show_pretty(sink, inner) };
            play(sink);
            // SAFETY: `sink` is a started pretty sink over `inner`.
            unsafe { sisu_show_pretty_finish(sink) };
        });
        all.strip_prefix(before)
            .expect("the layout should keep the text before it")
            .to_owned()
    }

    /// `[`, the children `texts`, and `]`, one text piece each.
    fn list(texts: &[&str]) -> Vec<Ev> {
        let mut events = vec![o("[")];
        for (i, text) in texts.iter().enumerate() {
            if i > 0 {
                events.push(Ev::Sep);
            }
            events.push(t(text));
        }
        events.push(c("]"));
        events
    }

    /// A group `open`, `children` joined by separators, and `close`.
    fn grouped(open: &str, children: Vec<Vec<Ev>>, close: &str) -> Vec<Ev> {
        let mut events = vec![o(open)];
        for (i, child) in children.into_iter().enumerate() {
            if i > 0 {
                events.push(Ev::Sep);
            }
            events.extend(child);
        }
        events.push(c(close));
        events
    }

    /// A row of the layout table: its name, the events, and the text they lay out to.
    type Row = (&'static str, Vec<Ev>, String);

    /// The rows for the 80-column boundary, at each nesting.
    fn boundary_rows() -> Vec<Row> {
        let x78 = "x".repeat(78);
        let x79 = "x".repeat(79);
        let ae78 = "ä".repeat(78);
        let z40 = "z".repeat(40);
        let z60 = "z".repeat(60);
        let w73 = "w".repeat(73);
        let w74 = "w".repeat(74);
        let a79 = "a".repeat(79);
        // `[w]` is 2 columns wider than `w`. At column 4 it fits with a `,` after it up to 73
        // `w`s, and with nothing after it up to 74. The sibling `yy` makes the parent too wide.
        let with_comma = |w: &str| grouped("(", vec![list(&[w]), vec![t("yy")]], ")");
        let without_comma = |w: &str| grouped("(", vec![vec![t("yy")], list(&[w])], ")");
        let paren = |z: &str| grouped("(", vec![vec![t(z)]], ")");
        vec![
            (
                "a short list is flat",
                list(&["1", "2", "3"]),
                "[1, 2, 3]".to_owned(),
            ),
            (
                "the groups in a flat group are flat",
                grouped("[", vec![paren("1"), paren("2")], "]"),
                "[(1), (2)]".to_owned(),
            ),
            ("80 columns is flat", list(&[&x78]), format!("[{x78}]")),
            (
                "81 columns breaks",
                list(&[&x79]),
                format!("[\n    {x79}\n]"),
            ),
            (
                "a child at column 4 ends at 80 with its comma: flat",
                with_comma(&w73),
                format!("(\n    [{w73}],\n    yy\n)"),
            ),
            (
                "a child at column 4 ends at 81 with its comma: broken",
                with_comma(&w74),
                format!("(\n    [\n        {w74}\n    ],\n    yy\n)"),
            ),
            (
                "the last child at column 4 ends at 80 with no comma: flat",
                without_comma(&w74),
                format!("(\n    yy,\n    [{w74}]\n)"),
            ),
            (
                "a nested group that fits at its indentation stays flat",
                grouped("[", vec![paren(&z40), paren(&z40)], "]"),
                format!("[\n    ({z40}),\n    ({z40})\n]"),
            ),
            (
                "three levels: the inner groups fit at column 8",
                grouped(
                    "[",
                    vec![grouped("{", vec![paren(&z60), paren(&z60)], "}")],
                    "]",
                ),
                format!("[\n    {{\n        ({z60}),\n        ({z60})\n    }}\n]"),
            ),
            (
                "an empty group at column 79 stays one line",
                vec![t(&a79), o("["), c("]")],
                format!("{a79}[]"),
            ),
            (
                "78 two-byte chars and the brackets are 80 columns: flat",
                list(&[&ae78]),
                format!("[{ae78}]"),
            ),
        ]
    }

    /// The rows for the column a group starts at: after a label, and after a line break.
    fn column_rows() -> Vec<Row> {
        let y40 = "y".repeat(40);
        let z68 = "z".repeat(68);
        let z69 = "z".repeat(69);
        let w67 = "w".repeat(67);
        let w68 = "w".repeat(68);
        // A child group after the label `name: `, so at column 10, with a `,` after it.
        let labelled = |w: &str| {
            let child = vec![t("name: "), o("["), t(w), c("]")];
            grouped("(", vec![child, vec![t("yy")]], ")")
        };
        // `[`, a piece that ends a line with 10 `q`s, and a group of `z`s.
        let after_break = |z: &str| {
            let piece = format!("{y40}\nqqqqqqqqqq");
            vec![o("["), t(&piece), o("("), t(z), c(")"), c("]")]
        };
        vec![
            (
                "a piece with a line break breaks both groups; the column counts from the break",
                after_break(&z68),
                format!("[\n    {y40}\nqqqqqqqqqq({z68})\n]"),
            ),
            (
                "a group one column too wide for the column after a line break breaks",
                after_break(&z69),
                format!("[\n    {y40}\nqqqqqqqqqq(\n        {z69}\n    )\n]"),
            ),
            (
                "text before a group on its line counts toward its column: flat at 80",
                labelled(&w67),
                format!("(\n    name: [{w67}],\n    yy\n)"),
            ),
            (
                "text before a group on its line counts toward its column: broken at 81",
                labelled(&w68),
                format!("(\n    name: [\n        {w68}\n    ],\n    yy\n)"),
            ),
            (
                "a piece with a line break inside two groups breaks both",
                grouped("[", vec![grouped("(", vec![vec![t("a\nb")]], ")")], "]"),
                "[\n    (\n        a\nb\n    )\n]".to_owned(),
            ),
        ]
    }

    /// The layout breaks a group exactly when its column, its flat width and a following `,` pass
    /// column 80. It does so at each nesting. It counts `char`s and not bytes. It counts a column
    /// from the last line break of a piece. It ignores the text before it in the inner sink.
    /// Each row is also laid out after 40 bytes of earlier text, and must give the same text.
    #[test]
    fn layout_should_break_a_group_only_past_column_80() {
        for (name, events, want) in boundary_rows().into_iter().chain(column_rows()) {
            assert_eq!(laid_out("", |s| play(s, &events)), want, "{name}");
            let earlier = "e".repeat(40);
            assert_eq!(
                laid_out(&earlier, |s| play(s, &events)),
                want,
                "{name}, after 40 bytes of text"
            );
        }
    }

    /// The recorder copies each piece: a buffer that is overwritten and freed after its call and
    /// before the finish does not change the text. Miri flags a read of the freed buffer.
    #[test]
    fn layout_should_keep_a_copy_of_each_piece() {
        let got = laid_out("", |s| {
            let mut piece = b"abc".to_vec();
            play(s, &[o("[")]);
            // SAFETY: `s` is started, and `piece` is readable for its length.
            #[expect(unsafe_code, reason = "the test calls the runtime's C ABI")]
            unsafe {
                sisu_show_str(s, piece.as_ptr(), piece.len());
            }
            piece.fill(b'Z');
            drop(piece);
            play(s, &[c("]")]);
        });
        assert_eq!(got, "[abc]");
    }

    /// Where a layout of 1,000,000 groups writes. It counts the line breaks and drops the text.
    /// All the groups break, and the indentation alone is terabytes.
    #[derive(Default)]
    struct Counted {
        line_breaks: usize,
    }

    impl Output for Counted {
        fn text(&mut self, bytes: &[u8]) {
            self.line_breaks += usize::from(bytes == b"\n");
        }

        fn spaces(&mut self, _count: usize) {}
    }

    /// `Sink::spaces` writes exactly `count` blanks, across the 64-blank chunks it writes them in.
    #[test]
    fn spaces_should_write_exactly_count_blanks() {
        for count in [0, 1, 63, 64, 65, 128, 200] {
            let got = text_of(|s| {
                // SAFETY: `s` is a started sink that nothing else uses.
                #[expect(unsafe_code, reason = "the test turns the pointer into a reference")]
                unsafe { sink_at(s) }.spaces(count);
            });
            assert_eq!(got, " ".repeat(count), "{count} blanks");
        }
    }

    /// The events read and the line breaks written when `recorder` is laid out.
    fn lay_out_counted(recorder: &Recorder) -> (usize, usize) {
        let mut out = Counted::default();
        lay_out(recorder, &mut out);
        (recorder.reads.get(), out.line_breaks)
    }

    /// The two passes read each event exactly twice, through `Recorder::event`, whatever the
    /// shape. A small nesting and a wide group come first. A rescan of a group's children shows
    /// at once there, and not as a hang at a million levels. Then nested groups lay out with no
    /// recursion: a thread with a small stack lays out a million levels.
    #[test]
    fn layout_should_read_each_event_twice() {
        const LEVELS: usize = if cfg!(miri) { 2_000 } else { 1_000_000 };
        let mut wide = Recorder::new(ptr::null_mut());
        wide.record(Event::Open, b"[");
        for i in 0..300 {
            if i > 0 {
                wide.events.push(Event::Sep);
            }
            wide.record(Event::Open, b"(");
            wide.record(Event::Text, b"z");
            wide.record(Event::Close, b")");
        }
        wide.record(Event::Close, b"]");
        let mut nested = Recorder::new(ptr::null_mut());
        for _ in 0..300 {
            nested.record(Event::Open, b"[");
        }
        nested.record(Event::Text, b"x");
        for _ in 0..300 {
            nested.record(Event::Close, b"]");
        }
        for (name, recorder) in [("wide", &wide), ("nested", &nested)] {
            let (reads, _) = lay_out_counted(recorder);
            assert_eq!(reads, 2 * recorder.events.len(), "{name}");
        }
        let (events, reads, line_breaks) = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                let mut recorder = Recorder::new(ptr::null_mut());
                for _ in 0..LEVELS {
                    recorder.record(Event::Open, b"[");
                }
                recorder.record(Event::Text, b"x");
                for _ in 0..LEVELS {
                    recorder.record(Event::Close, b"]");
                }
                let (reads, line_breaks) = lay_out_counted(&recorder);
                (recorder.events.len(), reads, line_breaks)
            })
            .expect("the thread should start")
            .join()
            .expect("the layout should not overflow the stack");
        assert_eq!(events, 2 * LEVELS + 1);
        assert_eq!(reads, 2 * events, "each event is read once per pass");
        assert_eq!(line_breaks, 2 * LEVELS, "every group should break");
    }

    /// The sink is `SINK_SIZE` bytes and keeps `len` and `cap` where generated code reads them,
    /// at `SINK_LEN` and `SINK_CAP` (`codegen/abi.rs` has the same three numbers).
    #[test]
    fn sink_fields_should_sit_where_codegen_reads_them() {
        assert_eq!(size_of::<Sink>(), SINK_SIZE);
        assert_eq!(offset_of!(Sink, len), SINK_LEN);
        assert_eq!(offset_of!(Sink, cap), SINK_CAP);
    }

    /// Pieces of a line join in the buffer, and the line end adds the newline. The sink here is
    /// closed, so no byte leaves the process.
    #[test]
    fn stdout_pieces_should_join_one_line() {
        let mut out = Stdout {
            buffer: Vec::with_capacity(crate::RESERVED),
            sink: Some(FileSink {
                file: None,
                line_buffered: false,
            }),
        };
        out.append_piece(b"[1").expect("a closed sink drops output");
        out.append_piece(b", 2]")
            .expect("a closed sink drops output");
        assert_eq!(out.buffer, b"[1, 2]");
        out.finish_line().expect("a closed sink drops output");
        assert_eq!(out.buffer, b"[1, 2]\n");
        // A piece longer than the room goes out after the line start, which the flush empties.
        out.append_piece(&[b'x'; crate::RESERVED])
            .expect("a closed sink drops output");
        assert_eq!(out.buffer, b"");
        assert_eq!(out.buffer.capacity(), crate::RESERVED);
        // The start of a line stays in the buffer past `BUFFER_CAPACITY`, and the newline fits.
        out.append_piece(&[b'y'; crate::BUFFER_CAPACITY + 1])
            .expect("a closed sink drops output");
        assert_eq!(out.buffer.len(), crate::BUFFER_CAPACITY + 1);
        out.finish_line().expect("a closed sink drops output");
        assert_eq!(out.buffer, b"");
        assert_eq!(out.buffer.capacity(), crate::RESERVED);
    }

    /// A line that has no piece still opens standard output: a newline with no sink to flush
    /// to would be lost at the end of the program.
    #[test]
    fn finish_line_should_open_standard_output_when_the_line_has_no_piece() {
        let mut out = Stdout {
            buffer: Vec::new(),
            sink: None,
        };
        out.finish_line().expect("an empty line should be written");
        assert!(out.sink.is_some(), "the line end should open the sink");
    }

    /// A buffer sink ends a line with a newline.
    #[test]
    fn line_end_should_write_a_newline_on_a_buffer_sink() {
        #[expect(unsafe_code, reason = "the test calls the runtime's C ABI")]
        let write = |s: *mut Sink| {
            raw(s, "a");
            // SAFETY: `s` is a started sink.
            unsafe { sisu_show_line_end(s) };
        };
        assert_eq!(text_of(write), "a\n");
    }

    /// The variable that makes `stdout_child_should_write_a_line_in_pieces_when_asked` run.
    const CASE_VAR: &str = "SISU_RUNTIME_SHOW_CASE";
    /// The variable that makes `full_child_should_fail_a_write_when_asked` run, and its case.
    const FULL_VAR: &str = "SISU_RUNTIME_SHOW_FULL";

    /// The child of `stdout_sink_should_write_pieces_where_the_line_has_got_to`: a line written
    /// in pieces through a sink over standard output, with a `print` and a piece as long as the
    /// buffer between them. It does nothing unless the variable is set.
    #[expect(unsafe_code, reason = "the test calls the runtime's C ABI")]
    #[test]
    fn stdout_child_should_write_a_line_in_pieces_when_asked() {
        if std::env::var(CASE_VAR).is_err() {
            return;
        }
        let mut slot = MaybeUninit::<Sink>::uninit();
        let sink = slot.as_mut_ptr();
        // SAFETY: `sink` is writable for one sink.
        unsafe { sisu_show_stdout(sink) };
        raw(sink, "[x");
        // A `print` inside a written `to_string` writes where the line has got to.
        crate::sisu_print_int(7);
        raw(sink, &"x".repeat(crate::RESERVED));
        raw(sink, "y]");
        // SAFETY: the sink is started.
        unsafe { sisu_show_line_end(sink) };
        crate::sisu_flush_stdout();
    }

    /// A sink over standard output writes its pieces in order with whatever a `print` between
    /// them writes, and a piece longer than the buffer goes straight out.
    #[test]
    #[cfg_attr(miri, ignore = "spawns a child process, which Miri cannot")]
    fn stdout_sink_should_write_pieces_where_the_line_has_got_to() {
        let exe = std::env::current_exe().expect("the test binary should have a path");
        let out = std::process::Command::new(exe)
            .args([
                "--exact",
                "show::tests::stdout_child_should_write_a_line_in_pieces_when_asked",
                "--nocapture",
            ])
            .env(CASE_VAR, "1")
            .output()
            .expect("the child test should start");
        assert_eq!(out.status.code(), Some(0), "{:?}", out.status);
        let mut want = b"[x7\n".to_vec();
        want.extend_from_slice(&vec![b'x'; crate::RESERVED]);
        want.extend_from_slice(b"y]\n");
        // The child is a test binary, so libtest's own lines surround the output.
        assert!(out.stdout.windows(want.len()).any(|w| w == want));
    }

    /// The variable that makes `pad_child_should_abort_when_asked` run, and its case.
    const PAD_VAR: &str = "SISU_RUNTIME_SHOW_PAD";

    /// The child of `pad_should_abort_when_it_cannot_pad`: a pad that has no text to pad, because
    /// the sink writes to standard output or the mark is past the text. It does nothing unless
    /// the variable is set.
    #[expect(unsafe_code, reason = "the test calls the runtime's C ABI")]
    #[test]
    fn pad_child_should_abort_when_asked() {
        let Ok(case) = std::env::var(PAD_VAR) else {
            return;
        };
        let mut slot = MaybeUninit::<Sink>::uninit();
        let sink = slot.as_mut_ptr();
        let mark = if case == "stdout" {
            // SAFETY: `sink` is writable for one sink.
            unsafe { sisu_show_stdout(sink) };
            0
        } else {
            // SAFETY: `sink` is writable for one sink.
            unsafe { sisu_show_buffer(sink, 0) };
            1
        };
        // SAFETY: the sink is started.
        unsafe { sisu_show_pad(sink, mark, 4, 0) };
    }

    /// A standard output sink has no text to pad, and a mark past the text is no mark: the
    /// process aborts, as the runtime does for a call that generated code cannot make. A child
    /// that does not end is a failure of the test, not a hang.
    #[test]
    #[cfg_attr(miri, ignore = "spawns a child process, which Miri cannot")]
    fn pad_should_abort_when_it_cannot_pad() {
        use std::os::unix::process::ExitStatusExt;
        use std::process::Stdio;
        use std::time::{Duration, Instant};

        for case in ["stdout", "mark"] {
            let exe = std::env::current_exe().expect("the test binary should have a path");
            let mut child = std::process::Command::new(exe)
                .args([
                    "--exact",
                    "show::tests::pad_child_should_abort_when_asked",
                    "--nocapture",
                ])
                .env(PAD_VAR, case)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("the child test should start");
            let deadline = Instant::now() + Duration::from_secs(20);
            let status = loop {
                if let Some(status) = child.try_wait().expect("the child should be waited for") {
                    break status;
                }
                if Instant::now() > deadline {
                    child.kill().expect("the hung child should be killed");
                    child.wait().expect("the killed child should be waited for");
                    panic!("{case}: the child should abort, not run on");
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            assert_eq!(status.signal(), Some(6), "{case}: {status:?}");
        }
    }

    #[expect(unsafe_code, reason = "declares libc `dup2`")]
    unsafe extern "C" {
        fn dup2(old: i32, new: i32) -> i32;
    }

    /// The child of `stdout_sink_should_panic_when_standard_output_is_full`: standard output is
    /// `/dev/full`, and the case in the variable writes to it through a sink. It does nothing
    /// unless the variable is set, so it also passes on macOS, which has no `/dev/full`.
    #[expect(
        unsafe_code,
        reason = "the test calls the runtime's C ABI and libc `dup2`"
    )]
    #[test]
    fn full_child_should_fail_a_write_when_asked() {
        let Ok(case) = std::env::var(FULL_VAR) else {
            return;
        };
        let full = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .expect("/dev/full should exist on Linux");
        // SAFETY: `full` is an open descriptor, and 1 is a valid target.
        let moved = unsafe { dup2(std::os::fd::AsRawFd::as_raw_fd(&full), 1) };
        assert_eq!(moved, 1, "dup2 should succeed");
        let mut slot = MaybeUninit::<Sink>::uninit();
        let sink = slot.as_mut_ptr();
        // SAFETY: `sink` is writable for one sink.
        unsafe { sisu_show_stdout(sink) };
        if case == "piece" {
            // A piece as long as the buffer goes straight out.
            raw(sink, &"x".repeat(crate::RESERVED));
        } else {
            // Pieces that fill the buffer past its capacity, and the line end writes it out.
            raw(sink, &"x".repeat(crate::BUFFER_CAPACITY + 1));
            // SAFETY: the sink is started.
            unsafe { sisu_show_line_end(sink) };
        }
    }

    /// A failed write to standard output through a sink is a Sisu panic, as a failed `print` is:
    /// exit code 101 and the message on standard error. The write fails in a piece or at the end
    /// of the line.
    #[test]
    #[cfg_attr(miri, ignore = "spawns a child process, which Miri cannot")]
    #[cfg_attr(
        not(target_os = "linux"),
        ignore = "writes to /dev/full, which only Linux has"
    )]
    fn stdout_sink_should_panic_when_standard_output_is_full() {
        for case in ["piece", "line"] {
            let exe = std::env::current_exe().expect("the test binary should have a path");
            let out = std::process::Command::new(exe)
                .args([
                    "--exact",
                    "show::tests::full_child_should_fail_a_write_when_asked",
                    "--nocapture",
                ])
                .env(FULL_VAR, case)
                .output()
                .expect("the child test should start");
            assert_eq!(out.status.code(), Some(101), "{case}: {:?}", out.status);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(
                stderr.contains("standard output could not be written"),
                "{case}: {stderr}"
            );
        }
    }
}
