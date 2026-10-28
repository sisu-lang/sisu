//! The text sink that `print` writes through. Generated `show` helpers write the text of a
//! value into a sink piece by piece: scalars, the quoted text of a string or a `char`, and the
//! events of a group (`open`, `sep` and `close`), so that a pretty sink can lay out the same
//! events later. A flat sink writes `sep` as `, ` and nothing more.
//!
//! A sink is a stack slot of `SINK_SIZE` bytes that generated code owns. One kind writes to the
//! runtime's standard output buffer, so `print` of a value allocates nothing. The other builds a
//! block that is the result string: `header` bytes that the sink never touches, then the text.
//! The runtime neither builds nor reads a Sisu object (ADR 0009) and does not know the string
//! layout (ADR 0014); generated code writes the header of the block in place after
//! `sisu_show_finish`.

use std::io::Write;
use std::ptr;

use crate::{float, sisu_alloc, sisu_realloc, stdout_panic, text, with_stdout};

/// The size in bytes of a [`Sink`]. Generated code allocates this much for a sink, and
/// `codegen/abi.rs` checks that its own type for a sink measures the same.
pub const SINK_SIZE: usize = 40;
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
/// The text capacity of a new buffer sink, in bytes. It doubles when the text outgrows it.
const FIRST_CAPACITY: usize = 64;
/// The alignment of a buffer sink's block, the string header's.
const BLOCK_ALIGN: usize = 8;

/// A sink for the text of a value. Generated code reads `len` and `cap` after
/// `sisu_show_finish`, at `SINK_LEN` and `SINK_CAP`.
#[derive(Debug)]
#[repr(C)]
pub struct Sink {
    /// `STDOUT` or `BUFFER`.
    kind: usize,
    /// The block of a buffer sink.
    block: *mut u8,
    /// The bytes at the start of the block that the sink never touches.
    header: usize,
    /// The bytes of text written, after the header.
    len: usize,
    /// The bytes of text that the block has room for, after the header.
    cap: usize,
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

/// The sink at `sink`.
///
/// # Safety
///
/// `sink` points to a sink that a `sisu_show_stdout` or `sisu_show_buffer` call started, and
/// nothing else uses it for `'a`.
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
    unsafe { sink_at(sink) }.line_end();
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
/// starts a part that may break after them.
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
    // SAFETY: the same promises as `sisu_show_str`.
    unsafe { sisu_show_str(sink, bytes, len) };
}

/// Separates two parts of a group: a flat sink writes `, `.
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
    unsafe { sink_at(sink) }.put(b", ");
}

/// Ends a group: writes the `len` bytes at `bytes`, such as `)` or `]`.
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
    // SAFETY: the same promises as `sisu_show_str`.
    unsafe { sisu_show_str(sink, bytes, len) };
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

    #[expect(unsafe_code, reason = "declares libc `dup2`")]
    unsafe extern "C" {
        fn dup2(old: i32, new: i32) -> i32;
    }

    /// The child of `stdout_sink_should_panic_when_standard_output_is_full`: standard output is
    /// `/dev/full`, and the case in the variable writes to it through a sink. It does nothing
    /// unless the variable is set.
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
