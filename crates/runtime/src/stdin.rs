//! Reading standard input a line at a time, for Sisu's `read_line`.

use std::borrow::Cow;
use std::io::BufRead;
use std::sync::{Mutex, PoisonError};

use crate::text::repair;

/// The lines of an input, read one at a time into one reused buffer. It holds no reader, so it
/// can live in a `static`; each call passes the reader in.
#[derive(Debug, Default)]
pub struct Lines {
    /// The last line read, without its line ending and repaired to valid UTF-8.
    line: Vec<u8>,
    /// Whether the input has ended: end of input is sticky, so no later call reads again. A
    /// last line without `\n` ends it too.
    ended: bool,
}

impl Lines {
    pub const fn new() -> Lines {
        Lines {
            line: Vec::new(),
            ended: false,
        }
    }

    /// The next line of `reader`, or `None` at the end of input and at every call after it. A
    /// line cut short by the end of input is the last line, even if the reader could give more.
    /// A final `\n` is removed, and a `\r` before it; a lone `\r` stays. Invalid UTF-8 is
    /// repaired over the whole line, so a character split across two reads stays whole.
    ///
    /// # Errors
    ///
    /// A failed read of `reader`, other than `Interrupted`, which `read_until` retries.
    pub fn next<R: BufRead>(&mut self, reader: &mut R) -> std::io::Result<Option<&[u8]>> {
        if self.ended {
            return Ok(None);
        }
        self.line.clear();
        if reader.read_until(b'\n', &mut self.line)? == 0 {
            self.ended = true;
            return Ok(None);
        }
        if self.line.last() == Some(&b'\n') {
            self.line.pop();
            if self.line.last() == Some(&b'\r') {
                self.line.pop();
            }
        } else {
            // `read_until` stops short of `\n` only at end of input.
            self.ended = true;
        }
        if let Cow::Owned(repaired) = repair(&self.line) {
            self.line = repaired;
        }
        Ok(Some(&self.line))
    }
}

/// The lines of standard input. `std::io::stdin()` keeps one global buffer, so locking it
/// again on each call loses nothing it read ahead.
static LINES: Mutex<Lines> = Mutex::new(Lines::new());

/// Reads the next line of standard input. Sisu's `read_line` calls this. Writes the line's
/// length to `len` and returns a pointer to its bytes, valid UTF-8 without the line ending; or
/// returns null, and writes nothing, at the end of input. A closed standard input is the end of
/// input. Any other failed read is a Sisu panic with no location.
///
/// # Safety
///
/// - `len` is non-null and writable for one `usize`.
/// - The returned bytes are read before the next call, which overwrites them.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name and passes a raw pointer"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_read_line(len: *mut usize) -> *const u8 {
    // Nothing panics while the lock is held (`panic = "abort"`), so it is never poisoned.
    let mut lines = LINES.lock().unwrap_or_else(PoisonError::into_inner);
    match lines.next(&mut std::io::stdin().lock()) {
        Ok(Some(line)) => {
            // SAFETY: the caller promises that `len` is writable.
            unsafe { len.write(line.len()) };
            // SAFETY of the caller's later read: the bytes live in `LINES`, a `static`, after
            // the guard is dropped. A Sisu program runs on one thread, so nothing changes
            // them until the next call, before which the caller has copied them.
            line.as_ptr()
        }
        Ok(None) => std::ptr::null(),
        Err(error) => crate::exit_with_panic(format_args!(
            "sisu: panic: standard input could not be read: {error}"
        )),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::io::{BufReader, Read};
    use std::rc::Rc;

    use super::Lines;

    /// A reader that gives its chunks in order: each `read` copies what fits from the current
    /// chunk and keeps the rest, and moves on only when it is used up. An empty chunk is one
    /// end-of-input result. It counts its `read` calls.
    struct Script {
        chunks: VecDeque<Vec<u8>>,
        reads: Rc<Cell<usize>>,
    }

    impl Read for Script {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.reads.set(self.reads.get() + 1);
            let Some(chunk) = self.chunks.front_mut() else {
                return Ok(0);
            };
            let n = chunk.len().min(buf.len());
            buf[..n].copy_from_slice(&chunk[..n]);
            chunk.drain(..n);
            if chunk.is_empty() {
                self.chunks.pop_front();
            }
            Ok(n)
        }
    }

    /// A 4-byte `BufReader` over `chunks`, and its `read` call count.
    fn script(chunks: &[&[u8]]) -> (BufReader<Script>, Rc<Cell<usize>>) {
        let reads = Rc::default();
        let script = Script {
            chunks: chunks.iter().map(|chunk| chunk.to_vec()).collect(),
            reads: Rc::clone(&reads),
        };
        (BufReader::with_capacity(4, script), reads)
    }

    /// The lines `next` gives over `chunks` before its first `None`, which must come within
    /// 100 calls.
    fn lines(chunks: &[&[u8]]) -> Vec<String> {
        let (mut reader, _) = script(chunks);
        let mut lines = Lines::new();
        let mut results = Vec::new();
        for _ in 0..100 {
            let Some(line) = lines
                .next(&mut reader)
                .expect("a script read should not fail")
            else {
                return results;
            };
            results.push(String::from_utf8(line.to_vec()).expect("a line should be valid UTF-8"));
        }
        panic!("`next` should reach the end of input within 100 calls")
    }

    #[test]
    fn lines_strip_endings() {
        assert_eq!(lines(&[b"a\nb\r\nc\rd\n\ne"]), ["a", "b", "c\rd", "", "e"]);
    }

    #[test]
    fn lines_keep_a_final_lone_cr() {
        assert_eq!(lines(&[b"tail\r"]), ["tail\r"]);
    }

    /// A `\r\n` and a `€` each split across two reads.
    #[test]
    fn lines_join_split_reads() {
        assert_eq!(
            lines(&[b"x\r", b"\ny\xE2", b"\x82\xAC", b"\n"]),
            ["x", "y€"]
        );
    }

    /// The repair runs over the whole line, so a character split across reads is not replaced.
    #[test]
    fn lines_repair_whole_line() {
        assert_eq!(lines(&[b"a", b"\xE2\x82", b"\xACb\n"]), ["a€b"]);
    }

    /// After the first `None`, `next` gives `None` again without reading, even though the
    /// reader has more.
    #[test]
    fn lines_end_is_sticky() {
        let (mut reader, reads) = script(&[b"a\n", b"", b"b\n"]);
        let mut lines = Lines::new();
        assert_eq!(
            lines.next(&mut reader).expect("reads"),
            Some(b"a".as_slice())
        );
        assert_eq!(lines.next(&mut reader).expect("reads"), None);
        let after_end = reads.get();
        assert_eq!(lines.next(&mut reader).expect("reads"), None);
        assert_eq!(reads.get(), after_end);
    }

    /// A read that stops short of `\n` is the end of input, so the partial line is the last
    /// one, as when Ctrl-D is pressed twice in a terminal.
    #[test]
    fn lines_end_after_a_partial_line_is_sticky() {
        let (mut reader, reads) = script(&[b"abc", b"", b"b\n"]);
        let mut lines = Lines::new();
        assert_eq!(
            lines.next(&mut reader).expect("reads"),
            Some(b"abc".as_slice())
        );
        assert_eq!(lines.next(&mut reader).expect("reads"), None);
        let after_end = reads.get();
        assert_eq!(lines.next(&mut reader).expect("reads"), None);
        assert_eq!(reads.get(), after_end);
    }

    #[test]
    fn lines_keep_zero_bytes() {
        assert_eq!(lines(&[b"a\0b\n"]), ["a\0b"]);
    }

    #[test]
    fn lines_repair_an_invalid_byte() {
        assert_eq!(lines(&[b"a\xFFb\n", b"c\n"]), ["a\u{FFFD}b", "c"]);
    }

    /// A reader whose every read fails.
    struct Failing;

    impl Read for Failing {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::IsADirectory.into())
        }
    }

    #[test]
    fn lines_pass_on_a_failed_read() {
        let error = Lines::new()
            .next(&mut BufReader::new(Failing))
            .expect_err("a failed read should be an error");
        assert_eq!(error.kind(), std::io::ErrorKind::IsADirectory);
    }
}
