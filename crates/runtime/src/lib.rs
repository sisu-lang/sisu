//! The Sisu runtime: functions that compiled Sisu programs call over the C ABI.

pub mod args;
pub mod float;
pub mod stdin;
pub mod text;

use std::alloc::{Layout, alloc, dealloc, handle_alloc_error, realloc};
use std::fs::File;
use std::io::{IsTerminal, Write};
use std::os::fd::AsFd;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, PoisonError};

/// Number of objects `sisu_alloc` returned that `sisu_free` has not yet freed.
///
/// A Sisu program runs on one thread, so a separate load and store keep the count exact.
/// `fetch_add` and `fetch_sub` would be locked instructions, each a full memory barrier on
/// x86-64, on every allocation and free: a quarter of the `binary_trees` benchmark's time.
/// Tasks on several threads (ADR 0010) will need a count per thread.
static LIVE: AtomicU64 = AtomicU64::new(0);

/// Prints `value` and a newline to standard output. Sisu's `print` calls this for an `i64`
/// argument.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name"
)]
#[unsafe(no_mangle)]
pub extern "C" fn sisu_print_int(value: i64) {
    print_bytes(text::int_text(value, &mut [0; 20]));
}

/// Prints `value` and a newline to standard output. Sisu's `print` calls this for an `f64`
/// argument.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name"
)]
#[unsafe(no_mangle)]
pub extern "C" fn sisu_print_f64(value: f64) {
    print_bytes(float::shortest(value, &mut [0; 24]));
}

/// Prints `true` or `false` and a newline to standard output. Sisu's `print` calls this
/// for a `bool` argument.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name"
)]
#[unsafe(no_mangle)]
pub extern "C" fn sisu_print_bool(value: bool) {
    print_bytes(text::bool_text(value));
}

/// Writes `value` and a newline to standard error. Sisu's `eprint` calls this for an `i64`
/// argument.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name"
)]
#[unsafe(no_mangle)]
pub extern "C" fn sisu_eprint_int(value: i64) {
    eprint_bytes(text::int_text(value, &mut [0; 20]));
}

/// Writes `value` and a newline to standard error. Sisu's `eprint` calls this for an `f64`
/// argument.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name"
)]
#[unsafe(no_mangle)]
pub extern "C" fn sisu_eprint_f64(value: f64) {
    eprint_bytes(float::shortest(value, &mut [0; 24]));
}

/// Writes `true` or `false` and a newline to standard error. Sisu's `eprint` calls this for a
/// `bool` argument.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name"
)]
#[unsafe(no_mangle)]
pub extern "C" fn sisu_eprint_bool(value: bool) {
    eprint_bytes(text::bool_text(value));
}

/// Bytes `print` collects before it writes them out, when standard output is not a terminal.
const BUFFER_CAPACITY: usize = 8192;

/// The longest line `print` writes: `-9223372036854775808` and a newline.
const LONGEST_LINE: usize = 21;

// The same on Linux and macOS; check it when adding another target.
const EBADF: i32 = 9;

/// The buffer's size: a line that starts below `BUFFER_CAPACITY` still fits.
const RESERVED: usize = BUFFER_CAPACITY + LONGEST_LINE;

/// Where `print` output goes, fixed at the first `print`.
struct Sink {
    /// A duplicate of file descriptor 1, written without std's own line buffering. `None`
    /// when standard output is closed: output is then dropped, as Rust's `println!` does.
    file: Option<File>,
    /// Whether standard output is a terminal, which gets a write per line.
    line_buffered: bool,
}

impl Sink {
    /// Duplicates file descriptor 1. A closed standard output (`EBADF`) is not an error: its
    /// output is dropped. Any other failure, such as `EMFILE`, is one the caller reports.
    fn open() -> std::io::Result<Sink> {
        let stdout = std::io::stdout();
        let file = match stdout.as_fd().try_clone_to_owned() {
            Ok(fd) => Some(File::from(fd)),
            Err(error) if error.raw_os_error() == Some(EBADF) => None,
            Err(error) => return Err(error),
        };
        Ok(Sink {
            line_buffered: stdout.is_terminal(),
            file,
        })
    }
}

/// What `print` has collected and not yet written. Like C stdio, it writes at once to a
/// terminal and in blocks otherwise.
struct Stdout {
    buffer: Vec<u8>,
    sink: Option<Sink>,
}

impl Stdout {
    /// Adds `bytes` and a newline. Bytes that would outgrow the buffer go straight out, after
    /// what the buffer holds.
    fn append_bytes(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.open()?;
        // The buffer is below `BUFFER_CAPACITY` here, and the newline needs one byte of room.
        if bytes.len() < RESERVED - self.buffer.len() {
            self.buffer.extend_from_slice(bytes);
        } else {
            self.flush()?;
            write_to(self.sink.as_mut(), bytes)?;
        }
        self.buffer.push(b'\n');
        self.end_line()
    }

    /// Opens the sink at the first `print`.
    fn open(&mut self) -> std::io::Result<()> {
        if self.sink.is_none() {
            // One allocation, once: the buffer never outgrows this. A flush follows any
            // line that takes it to `BUFFER_CAPACITY`.
            self.buffer.reserve_exact(RESERVED);
            self.sink = Some(Sink::open()?);
        }
        Ok(())
    }

    /// Writes out the buffer when the terminal wants each line, or when it is full enough.
    fn end_line(&mut self) -> std::io::Result<()> {
        let line_buffered = self.sink.as_ref().is_some_and(|sink| sink.line_buffered);
        if line_buffered || self.buffer.len() >= BUFFER_CAPACITY {
            self.flush()
        } else {
            Ok(())
        }
    }

    /// Writes out the buffer. The buffer is emptied even when the write fails, so nothing
    /// tries the same bytes again, and the panic that a failure causes is the only error.
    fn flush(&mut self) -> std::io::Result<()> {
        let result = write_to(self.sink.as_mut(), &self.buffer);
        self.buffer.clear();
        result
    }
}

/// Writes `bytes` to the sink's file; with no file, standard output is closed and they are dropped.
fn write_to(sink: Option<&mut Sink>, bytes: &[u8]) -> std::io::Result<()> {
    match sink.and_then(|sink| sink.file.as_mut()) {
        Some(file) => file.write_all(bytes),
        None => Ok(()),
    }
}

static STDOUT: Mutex<Stdout> = Mutex::new(Stdout {
    buffer: Vec::new(),
    sink: None,
});

/// Runs `f` on the output buffer. Nothing panics while it holds the lock (`panic = "abort"`),
/// so a poisoned lock cannot happen; the error arm only avoids an `unwrap`.
fn with_stdout<R>(f: impl FnOnce(&mut Stdout) -> R) -> R {
    f(&mut STDOUT.lock().unwrap_or_else(PoisonError::into_inner))
}

/// Adds `bytes` and a newline to the output. A failed write is a Sisu panic: `println!`
/// would panic and, with `panic = "abort"`, kill the program with SIGABRT. A pipe with no
/// reader still kills the program with SIGPIPE, as it does a C program.
fn print_bytes(bytes: &[u8]) {
    // The lock is released here: the panic below flushes the buffer through it again.
    let result = with_stdout(|out| out.append_bytes(bytes));
    if let Err(error) = result {
        stdout_panic(&error);
    }
}

/// Writes `bytes` and a newline to standard error with one `write_all`, so a line up to
/// `PIPE_BUF` is not split by another writer. Nothing waits in a buffer between calls, and the
/// result is ignored: a closed or failing standard error drops the line, as it does a panic's.
/// A pipe with no reader still kills the program with SIGPIPE.
fn eprint_bytes(bytes: &[u8]) {
    let mut line = Vec::with_capacity(bytes.len() + 1);
    line.extend_from_slice(bytes);
    line.push(b'\n');
    let _ = std::io::stderr().write_all(&line);
}

fn stdout_panic(error: &std::io::Error) -> ! {
    exit_with_panic(format_args!(
        "sisu: panic: standard output could not be written: {error}"
    ))
}

/// Writes out what `print` has collected; a failed write is the panic of `print_bytes`.
fn flush_or_panic() {
    let result = with_stdout(Stdout::flush);
    if let Err(error) = result {
        stdout_panic(&error);
    }
}

/// Writes out what `print` has collected. The generated C `main` calls this when `sisu.main`
/// returns, so a program that ends normally delivers all its output.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name"
)]
#[unsafe(no_mangle)]
pub extern "C" fn sisu_flush_stdout() {
    flush_or_panic();
}

/// Prints `sisu: panic at <msg>` to standard error and exits with code 101.
///
/// Lines printed before the panic are written to standard output first, so they come out
/// before the panic line.
///
/// # Safety
///
/// - `msg` is non-null, even when `len` is 0.
/// - The `len` bytes starting at `msg` are readable and lie within one allocation.
/// - Nothing writes to those bytes for the duration of the call.
/// - `len` is at most `isize::MAX`.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name and passes a raw pointer"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_panic(msg: *const u8, len: usize) -> ! {
    // SAFETY: the caller upholds the `# Safety` contract above, which is `raw_text`'s.
    let msg = unsafe { raw_text(msg, len) };
    exit_with_panic(format_args!("sisu: panic at {msg}"))
}

/// The `len` bytes at `msg` as text, each ill-formed sequence replaced by `U+FFFD`, for a
/// panic line. The panic functions exit, so their tests run in a child process, out of Miri's
/// reach; this function is what Miri checks of their reads.
///
/// # Safety
///
/// `msg` and `len` meet the contract of [`sisu_panic`].
#[expect(unsafe_code, reason = "reads the raw pointer generated code passed")]
unsafe fn raw_text<'a>(msg: *const u8, len: usize) -> std::borrow::Cow<'a, str> {
    // SAFETY: the caller upholds the contract above, which is exactly what `from_raw_parts`
    // requires for a `u8` slice.
    String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(msg, len) })
}

/// Prints `sisu: panic at <at>: <msg>` to standard error and exits with code 101. Generated
/// code calls this for a `panic(msg)` call; `at` is its source location.
///
/// # Safety
///
/// `at` and `at_len`, and `msg` and `msg_len`, each meet the contract of `msg` and `len` for
/// [`sisu_panic`].
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name and passes raw pointers"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_panic_message(
    at: *const u8,
    at_len: usize,
    msg: *const u8,
    msg_len: usize,
) -> ! {
    // SAFETY: the caller upholds the `# Safety` contract above, which is `raw_text`'s for
    // each pair.
    let (at, msg) = unsafe { (raw_text(at, at_len), raw_text(msg, msg_len)) };
    exit_with_panic(format_args!("sisu: panic at {at}: {msg}"))
}

/// Prints `sisu: panic at <msg>: index <index> out of bounds for length <length>` to
/// standard error and exits with code 101. Generated code calls this for a failed array
/// index check; `msg` is the source location of the index expression.
///
/// # Safety
///
/// `msg` and `len` meet the same contract as for [`sisu_panic`].
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name and passes a raw pointer"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_panic_bounds(
    msg: *const u8,
    len: usize,
    index: i64,
    length: i64,
) -> ! {
    // SAFETY: the caller upholds the `# Safety` contract above, which is `raw_text`'s.
    let msg = unsafe { raw_text(msg, len) };
    exit_with_panic(format_args!(
        "sisu: panic at {msg}: index {index} out of bounds for length {length}"
    ))
}

// `signal(2)`, declared here to avoid a dependency. `handler` is a function address or
// `SIG_IGN`; the previous handler is not needed.
#[expect(
    unsafe_code,
    reason = "declares a libc function; the runtime is linked against libc"
)]
unsafe extern "C" {
    fn signal(signum: i32, handler: usize) -> usize;
}

// The same on Linux (x86-64, aarch64) and macOS; check them when adding another target.
const SIGPIPE: i32 = 13;
const SIG_IGN: usize = 1;

/// Writes out the output `print` collected before a panic or fault report. A Sisu program
/// keeps the default SIGPIPE action for standard output, but a report must reach its own
/// exit code even when nothing reads a pipe. So SIGPIPE is ignored first, and a failed
/// flush is not reported: it emptied the buffer, so the exit does not try again.
fn flush_and_ignore_sigpipe() {
    // SAFETY: `signal` gets a valid signal number and `SIG_IGN`; it only changes the
    // process's signal disposition.
    #[expect(unsafe_code, reason = "calls libc `signal`")]
    unsafe {
        signal(SIGPIPE, SIG_IGN);
    }
    let _ = with_stdout(Stdout::flush);
}

fn exit_with_panic(message: std::fmt::Arguments<'_>) -> ! {
    // Earlier lines come out before the panic line.
    flush_and_ignore_sigpipe();
    // `eprintln!` panics on a failed write, which would abort before exit code 101.
    let _ = writeln!(std::io::stderr(), "{message}");
    std::process::exit(101)
}

/// Allocates `size` bytes aligned to `align` for a Sisu object. Generated code calls this
/// to construct a class instance.
///
/// Aborts when `size` is 0 or `align` is not a power of two, or when the layout overflows
/// `isize`; codegen never asks for any of these. Calls `handle_alloc_error` when the
/// allocator returns null, so the result is never null.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name"
)]
#[unsafe(no_mangle)]
pub extern "C" fn sisu_alloc(size: usize, align: usize) -> *mut u8 {
    let Ok(layout) = Layout::from_size_align(size, align) else {
        std::process::abort()
    };
    if size == 0 {
        std::process::abort()
    }
    // SAFETY: `layout` has a non-zero size, which `alloc` requires.
    let ptr = unsafe { alloc(layout) };
    if ptr.is_null() {
        handle_alloc_error(layout)
    }
    LIVE.store(LIVE.load(Relaxed).wrapping_add(1), Relaxed);
    ptr
}

/// Frees an object that `sisu_alloc` returned. Generated code calls this when an object's
/// reference count reaches zero.
///
/// # Safety
///
/// - `ptr` came from `sisu_alloc(size, align)` with these `size` and `align`.
/// - `ptr` is not used after this call.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name and passes a raw pointer"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_free(ptr: *mut u8, size: usize, align: usize) {
    // SAFETY: `sisu_alloc` accepted `size` and `align` as a valid layout, and the caller
    // passes the same two values.
    let layout = unsafe { Layout::from_size_align_unchecked(size, align) };
    // SAFETY: the caller upholds the `# Safety` contract above: `ptr` was allocated with
    // `layout` and is not used again.
    unsafe { dealloc(ptr, layout) };
    LIVE.store(LIVE.load(Relaxed).wrapping_sub(1), Relaxed);
}

/// Grows or shrinks an object that `sisu_alloc` or `sisu_realloc` returned to `new_size`
/// bytes, keeping its first `min(old_size, new_size)` bytes. Generated code calls this to grow
/// a string's block. The object stays one object, so the live count does not change.
///
/// Aborts when `new_size` is 0 or the new layout overflows `isize`, as `sisu_alloc` does.
/// Calls `handle_alloc_error` when the allocator returns null, so the result is never null.
///
/// # Safety
///
/// - `ptr` came from `sisu_alloc(old_size, align)` or `sisu_realloc` with these sizes.
/// - `ptr` is not used after this call.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name and passes a raw pointer"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_realloc(
    ptr: *mut u8,
    old_size: usize,
    align: usize,
    new_size: usize,
) -> *mut u8 {
    let Ok(new_layout) = Layout::from_size_align(new_size, align) else {
        std::process::abort()
    };
    if new_size == 0 {
        std::process::abort()
    }
    // SAFETY: the block was allocated with `old_size` and `align`, which `sisu_alloc` or an
    // earlier `sisu_realloc` accepted as a valid layout.
    let layout = unsafe { Layout::from_size_align_unchecked(old_size, align) };
    // SAFETY: the caller upholds the `# Safety` contract above, and `new_size` is non-zero and
    // valid with `align`, as `new_layout` proves.
    let grown = unsafe { realloc(ptr, layout, new_size) };
    if grown.is_null() {
        handle_alloc_error(new_layout)
    }
    grown
}

/// Exit code for a reference-count check failure under `--rc-checks`.
const RC_CHECK_EXIT: i32 = 102;

/// Exits with code 102 when any object is still live. Programs compiled with
/// `--rc-checks` call this on a normal exit to detect leaks.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name"
)]
#[unsafe(no_mangle)]
pub extern "C" fn sisu_rc_check_exit() {
    let live = LIVE.load(Relaxed);
    if live != 0 {
        // The flush is a no-op here (`main` already flushed); the call ignores SIGPIPE.
        flush_and_ignore_sigpipe();
        // Write errors are ignored as in `exit_with_panic`: `eprintln!` would panic.
        let _ = writeln!(
            std::io::stderr(),
            "sisu: rc check: live objects at exit: {live}"
        );
        std::process::exit(RC_CHECK_EXIT);
    }
}

/// Reports a reference-count fault and exits with code 102. Kind 0 is a retain of an
/// object whose count is 0, kind 1 a release of one, kind 2 a read of a local after it was
/// moved. Codegen names the kinds `RC_FAULT_RETAIN`, `RC_FAULT_RELEASE` and `RC_FAULT_MOVED`
/// in `crates/sisuc/src/codegen/abi.rs`.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name"
)]
#[unsafe(no_mangle)]
pub extern "C" fn sisu_rc_fault(kind: u32) -> ! {
    flush_and_ignore_sigpipe();
    let _ = match kind {
        0 => writeln!(
            std::io::stderr(),
            "sisu: rc check: retain of an object whose count is 0"
        ),
        1 => writeln!(
            std::io::stderr(),
            "sisu: rc check: release of an object whose count is 0"
        ),
        2 => writeln!(
            std::io::stderr(),
            "sisu: rc check: read of a local after it was moved"
        ),
        other => writeln!(std::io::stderr(), "sisu: rc check: fault {other}"),
    };
    std::process::exit(RC_CHECK_EXIT)
}

#[cfg(test)]
mod tests {
    use super::{
        BUFFER_CAPACITY, RESERVED, SIGPIPE, Sink, Stdout, raw_text, signal, sisu_alloc,
        sisu_eprint_bool, sisu_eprint_f64, sisu_eprint_int, sisu_flush_stdout, sisu_free,
        sisu_panic_bounds, sisu_print_bool, sisu_print_f64, sisu_print_int, sisu_rc_check_exit,
        sisu_rc_fault, sisu_realloc, with_stdout,
    };
    use crate::text::{sisu_eprint_char, sisu_eprint_str};
    use std::os::fd::AsRawFd;
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command;

    const ABORT_CASE_VAR: &str = "SISU_RUNTIME_ABORT_CASE";
    const PANIC_BOUNDS_VAR: &str = "SISU_RUNTIME_PANIC_BOUNDS";
    const EPRINT_VAR: &str = "SISU_RUNTIME_EPRINT";
    const RC_CASE_VAR: &str = "SISU_RUNTIME_RC_CASE";
    const SIGABRT: i32 = 6;

    #[expect(
        unsafe_code,
        reason = "the test writes through the raw pointer and frees it"
    )]
    #[test]
    fn alloc_and_free_round_trip() {
        let ptr = sisu_alloc(24, 8);
        assert!(!ptr.is_null());
        assert_eq!(ptr.addr() % 8, 0, "pointer must be 8-aligned");
        for i in 0..24 {
            // SAFETY: `ptr` points to 24 allocated bytes and `i` is below 24.
            unsafe { ptr.add(i).write(u8::try_from(i).expect("i is below 24")) };
        }
        for i in 0..24 {
            // SAFETY: the same 24 bytes were written above.
            let byte = unsafe { ptr.add(i).read() };
            assert_eq!(usize::from(byte), i);
        }
        // SAFETY: `ptr` came from `sisu_alloc(24, 8)` and is not used afterwards.
        unsafe { sisu_free(ptr, 24, 8) };
    }

    #[expect(unsafe_code, reason = "the test frees the pointer")]
    #[test]
    fn alloc_honours_an_alignment_above_the_allocator_default() {
        let ptr = sisu_alloc(24, 4096);
        assert!(!ptr.is_null());
        assert_eq!(ptr.addr() % 4096, 0, "pointer must be 4096-aligned");
        // SAFETY: `ptr` came from `sisu_alloc(24, 4096)` and is not used afterwards.
        unsafe { sisu_free(ptr, 24, 4096) };
    }

    #[expect(
        unsafe_code,
        reason = "the test writes through the raw pointer, grows it and frees it"
    )]
    #[test]
    fn realloc_keeps_bytes() {
        let ptr = sisu_alloc(24, 8);
        for i in 0..24 {
            // SAFETY: `ptr` points to 24 allocated bytes and `i` is below 24.
            unsafe { ptr.add(i).write(u8::try_from(i).expect("i is below 24")) };
        }
        // SAFETY: `ptr` came from `sisu_alloc(24, 8)` and is not used afterwards.
        let grown = unsafe { sisu_realloc(ptr, 24, 8, 4096) };
        assert_eq!(grown.addr() % 8, 0, "pointer must stay 8-aligned");
        for i in 0..24 {
            // SAFETY: the grown block keeps the 24 bytes written above.
            let byte = unsafe { grown.add(i).read() };
            assert_eq!(usize::from(byte), i);
        }
        // SAFETY: `grown` came from `sisu_realloc` with the new size 4096 and alignment 8.
        unsafe { sisu_free(grown, 4096, 8) };
    }

    /// Child entry point for the `alloc_aborts_*` and `realloc_aborts_*` tests. It returns at
    /// once unless the parent set the env var, so a normal test run ignores it.
    #[expect(
        unsafe_code,
        reason = "the `realloc` cases call an unsafe extern function"
    )]
    #[test]
    fn abort_child() {
        let Ok(case) = std::env::var(ABORT_CASE_VAR) else {
            return;
        };
        let (size, align) = match case.as_str() {
            "zero" => (0, 8),
            "invalid_align" => (8, 3),
            "overflow" => (usize::MAX - 8, 4096),
            "utf8_bytes" => {
                // SAFETY: one readable byte; it is not UTF-8, so the call aborts.
                unsafe { crate::text::sisu_utf8_count([0xFF].as_ptr(), 1) };
                return;
            }
            "utf8_scalar" => {
                // SAFETY: one readable `u32`; it is a surrogate, so the call aborts.
                unsafe { crate::text::sisu_utf8_len([0xD800].as_ptr(), 1) };
                return;
            }
            realloc_case => {
                let new_size = match realloc_case {
                    "realloc_zero" => 0,
                    "realloc_overflow" => usize::MAX - 8,
                    other => panic!("unknown abort case {other}"),
                };
                // SAFETY: the block came from `sisu_alloc(24, 8)`; the call aborts before
                // anything could use it again.
                unsafe { sisu_realloc(sisu_alloc(24, 8), 24, 8, new_size) };
                return;
            }
        };
        sisu_alloc(size, align);
        // Reached only if `sisu_alloc` returned: exit normally so the parent sees no signal.
    }

    /// Child entry point for `panic_bounds_exits_101_when_stderr_fails`; a no-op unless the
    /// parent set the env var.
    #[expect(unsafe_code, reason = "the test calls an unsafe extern function")]
    #[test]
    fn panic_bounds_child() {
        if std::env::var(PANIC_BOUNDS_VAR).is_err() {
            return;
        }
        let msg = "f.sisu:3:7";
        // SAFETY: `msg` is non-null, readable for `msg.len()` bytes, and not written to.
        unsafe { sisu_panic_bounds(msg.as_ptr(), msg.len(), 7, 5) };
    }

    /// Child entry point for `eprint_writes_each_type_and_a_newline`; a no-op unless the parent
    /// set the env var, so the parent can read the child's standard error.
    #[expect(unsafe_code, reason = "the test calls an unsafe extern function")]
    #[test]
    fn eprint_child() {
        if std::env::var(EPRINT_VAR).is_err() {
            return;
        }
        sisu_eprint_int(-42);
        sisu_eprint_f64(-0.5);
        sisu_eprint_bool(true);
        sisu_eprint_char(u32::from('ä'));
        let text = b"a\0b";
        // SAFETY: `text` is non-null and readable for `text.len()` bytes.
        unsafe { sisu_eprint_str(text.as_ptr(), text.len()) };
        // SAFETY: an empty slice's pointer is non-null and no byte of it is read.
        unsafe { sisu_eprint_str([].as_ptr(), 0) };
    }

    /// Each `sisu_eprint_*` writes its text and a newline to standard error, a zero byte
    /// included.
    #[test]
    #[cfg_attr(miri, ignore = "spawns a child process, which Miri cannot")]
    fn eprint_writes_each_type_and_a_newline() {
        let exe = std::env::current_exe().expect("test binary path");
        let out = Command::new(exe)
            .args(["--exact", "tests::eprint_child", "--nocapture"])
            .env(EPRINT_VAR, "1")
            .output()
            .expect("spawn child test");
        assert_eq!(out.status.code(), Some(0), "{:?}", out.status);
        assert_eq!(out.stderr, "-42\n-0.5\ntrue\nä\na\0b\n\n".as_bytes());
    }

    /// `sisu_print_str` and `sisu_eprint_str` read their `len` bytes, a zero byte included, for
    /// a nonempty and an empty string. The test runs in process, so Miri checks that neither
    /// reads past `len`, which the child tests above do out of its reach. The buffered stdout
    /// bytes are checked here; the stderr bytes only by `eprint_writes_each_type_and_a_newline`.
    #[expect(unsafe_code, reason = "the test calls unsafe extern functions")]
    #[test]
    fn str_output_reads_len_bytes() {
        let text = b"a\0b";
        // SAFETY: each slice's pointer is non-null and readable for its length.
        unsafe {
            crate::text::sisu_print_str(text.as_ptr(), text.len());
            crate::text::sisu_print_str([].as_ptr(), 0);
            sisu_eprint_str(text.as_ptr(), text.len());
            sisu_eprint_str([].as_ptr(), 0);
        }
        // A terminal gets each line at once; otherwise both lines wait in the buffer.
        with_stdout(|out| {
            let line_buffered = out.sink.as_ref().is_some_and(|sink| sink.line_buffered);
            assert!(
                line_buffered || out.buffer.ends_with(b"a\0b\n\n"),
                "{:?}",
                out.buffer
            );
        });
    }

    /// `raw_text` reads exactly `len` bytes, for a nonempty and an empty message, and
    /// replaces an ill-formed byte. Under Miri this checks the panic functions' reads.
    #[expect(unsafe_code, reason = "the test calls an unsafe function")]
    #[test]
    fn raw_text_reads_len_bytes() {
        let msg = b"f.sisu:3:7\xFF";
        // SAFETY: each slice's pointer is non-null and readable for its length.
        let (text, empty) =
            unsafe { (raw_text(msg.as_ptr(), msg.len()), raw_text([].as_ptr(), 0)) };
        assert_eq!(text, "f.sisu:3:7\u{FFFD}");
        assert_eq!(empty, "");
    }

    #[test]
    #[cfg_attr(miri, ignore = "spawns a child process, which Miri cannot")]
    #[cfg_attr(
        not(target_os = "linux"),
        ignore = "writes to /dev/full, which only Linux has"
    )]
    fn panic_bounds_exits_101_when_stderr_fails() {
        let exe = std::env::current_exe().expect("test binary path");
        let full = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .expect("/dev/full should exist on Linux");
        let out = Command::new(exe)
            .args(["--exact", "tests::panic_bounds_child", "--nocapture"])
            .env(PANIC_BOUNDS_VAR, "1")
            .stderr(full)
            .output()
            .expect("spawn child test");
        assert_eq!(
            out.status.code(),
            Some(101),
            "child exited with {:?}",
            out.status
        );
        // libtest also exits 101 for a failed test, but then it prints its summary; the
        // runtime's own exit leaves before the test finishes.
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            !stdout.contains("test result:"),
            "libtest finished the child test, so the runtime did not exit: {stdout}"
        );
    }

    fn assert_child_aborts(case: &str) {
        let exe = std::env::current_exe().expect("test binary path");
        let status = Command::new(exe)
            .args(["--exact", "tests::abort_child", "--nocapture"])
            .env(ABORT_CASE_VAR, case)
            .status()
            .expect("spawn child test");
        assert_eq!(
            status.signal(),
            Some(SIGABRT),
            "child exited with {status:?}"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "spawns a child process, which Miri cannot")]
    fn alloc_aborts_on_zero_size() {
        assert_child_aborts("zero");
    }

    #[test]
    #[cfg_attr(miri, ignore = "spawns a child process, which Miri cannot")]
    fn alloc_aborts_on_invalid_alignment() {
        assert_child_aborts("invalid_align");
    }

    #[test]
    #[cfg_attr(miri, ignore = "spawns a child process, which Miri cannot")]
    fn alloc_aborts_when_layout_overflows() {
        assert_child_aborts("overflow");
    }

    /// A compiler bug that breaks the string invariant stops the program.
    #[test]
    #[cfg_attr(miri, ignore = "spawns a child process, which Miri cannot")]
    fn utf8_aborts_on_invalid_bytes() {
        assert_child_aborts("utf8_bytes");
    }

    /// So does a `[char]` holding a value that is no scalar value.
    #[test]
    #[cfg_attr(miri, ignore = "spawns a child process, which Miri cannot")]
    fn utf8_aborts_on_invalid_scalar() {
        assert_child_aborts("utf8_scalar");
    }

    #[test]
    #[cfg_attr(miri, ignore = "spawns a child process, which Miri cannot")]
    fn realloc_aborts_on_zero_size() {
        assert_child_aborts("realloc_zero");
    }

    #[test]
    #[cfg_attr(miri, ignore = "spawns a child process, which Miri cannot")]
    fn realloc_aborts_when_layout_overflows() {
        assert_child_aborts("realloc_overflow");
    }

    /// Child entry point for the `check_exit_*` and `rc_fault_*` tests. `LIVE` is
    /// process-global, so each case runs in its own process; a no-op unless the parent set
    /// the env var.
    #[expect(unsafe_code, reason = "the test frees raw pointers")]
    #[test]
    fn rc_child() {
        let Ok(case) = std::env::var(RC_CASE_VAR) else {
            return;
        };
        let a = sisu_alloc(8, 8);
        match case.as_str() {
            "balanced" => {
                // SAFETY: `a` came from `sisu_alloc(8, 8)` and is not used afterwards.
                unsafe { sisu_free(a, 8, 8) };
                sisu_rc_check_exit();
            }
            "leak" => {
                let _leaked = sisu_alloc(8, 8);
                // SAFETY: `a` came from `sisu_alloc(8, 8)` and is not used afterwards.
                unsafe { sisu_free(a, 8, 8) };
                sisu_rc_check_exit();
            }
            "fault1" => sisu_rc_fault(1),
            "fault0" => sisu_rc_fault(0),
            "fault2" => sisu_rc_fault(2),
            "fault7" => sisu_rc_fault(7),
            "print_then_fault" => {
                sisu_print_int(5);
                sisu_print_f64(2.5);
                sisu_rc_fault(1);
            }
            "dup_fails" => {
                unsafe extern "C" {
                    fn setrlimit(resource: i32, limit: *const [u64; 2]) -> i32;
                    fn dup(fd: i32) -> i32;
                }
                // SAFETY: `limit` points to a valid `rlimit` (two `u64`s); `dup` takes any
                // integer. Lowers the descriptor limit to 32 and uses every free slot, so
                // the first `print` cannot duplicate the open standard output.
                #[expect(unsafe_code, reason = "calls libc `setrlimit` and `dup`")]
                unsafe {
                    const RLIMIT_NOFILE: i32 = if cfg!(target_os = "macos") { 8 } else { 7 };
                    assert_eq!(setrlimit(RLIMIT_NOFILE, &[32, 32]), 0);
                    // `dup(0)` takes the lowest free slot, so it fills the table without a second
                    // open file. Descriptor 0 is open here: std's startup reopens a closed one on
                    // /dev/null. Were it closed just before the loop, `dup` would fail at once, the
                    // table would stay unfilled, the `print` would succeed and the parent's
                    // expected exit 101 would fail.
                    while dup(0) >= 0 {}
                }
                sisu_print_int(5);
            }
            "fault_full_stdout" => {
                let full = std::fs::OpenOptions::new()
                    .write(true)
                    .open("/dev/full")
                    .expect("/dev/full should exist on Linux");
                point_fd_at(full.as_raw_fd(), 1);
                sisu_print_int(5);
                sisu_rc_fault(1);
            }
            "fault_closed_stdout_pipe" => {
                let (reader, writer) = std::io::pipe().expect("creates a pipe");
                drop(reader);
                point_fd_at(writer.as_raw_fd(), 1);
                // libtest starts with SIGPIPE ignored; a Sisu program does not.
                // SAFETY: a valid signal number and `SIG_DFL` (0).
                #[expect(unsafe_code, reason = "calls libc `signal`")]
                unsafe {
                    signal(SIGPIPE, 0);
                }
                sisu_print_int(5);
                sisu_rc_fault(1);
            }
            "leak_closed_stderr_pipe" => {
                let (reader, writer) = std::io::pipe().expect("creates a pipe");
                drop(reader);
                point_fd_at(writer.as_raw_fd(), 2);
                // libtest starts with SIGPIPE ignored; a Sisu program does not.
                // SAFETY: a valid signal number and `SIG_DFL` (0).
                #[expect(unsafe_code, reason = "calls libc `signal`")]
                unsafe {
                    signal(SIGPIPE, 0);
                }
                let _leaked = sisu_alloc(8, 8);
                // SAFETY: `a` came from `sisu_alloc(8, 8)` and is not used afterwards.
                unsafe { sisu_free(a, 8, 8) };
                sisu_rc_check_exit();
            }
            "print_char" => {
                crate::text::sisu_print_char(0x1F600);
                sisu_flush_stdout();
            }
            "print_str" => {
                let long = [b'x'; RESERVED];
                // SAFETY: each slice is readable for its length.
                #[expect(unsafe_code, reason = "calls the runtime's C ABI function")]
                unsafe {
                    crate::text::sisu_print_str(b"a\0b".as_ptr(), 3);
                    crate::text::sisu_print_str(long.as_ptr(), long.len());
                }
                sisu_print_int(5);
                sisu_print_bool(false);
                sisu_flush_stdout();
            }
            other => panic!("unknown rc case {other}"),
        }
    }

    fn run_rc_child(case: &str) -> std::process::Output {
        let exe = std::env::current_exe().expect("test binary path");
        Command::new(exe)
            .args(["--exact", "tests::rc_child", "--nocapture"])
            .env(RC_CASE_VAR, case)
            .output()
            .expect("spawn child test")
    }

    fn assert_rc_exit(case: &str, code: Option<i32>, stderr: &str) {
        let out = run_rc_child(case);
        assert_eq!(
            out.status.code(),
            code,
            "child exited with {:?}",
            out.status
        );
        assert_eq!(String::from_utf8_lossy(&out.stderr), stderr);
    }

    /// Makes file descriptor `target` refer to the same file as `fd`.
    #[expect(unsafe_code, reason = "declares and calls libc `dup2`")]
    fn point_fd_at(fd: i32, target: i32) {
        unsafe extern "C" {
            fn dup2(old: i32, new: i32) -> i32;
        }
        // SAFETY: `fd` is an open descriptor and `target` is a valid target.
        let result = unsafe { dup2(fd, target) };
        assert_eq!(result, target, "dup2 should succeed");
    }

    /// The leak report reaches its exit code 102 even when standard error is a pipe with no
    /// reader, as a fault's report does.
    #[test]
    #[cfg_attr(miri, ignore = "spawns a child process, which Miri cannot")]
    fn check_exit_reports_when_stderr_is_a_closed_pipe() {
        assert_rc_exit("leak_closed_stderr_pipe", Some(102), "");
    }

    /// `sisu_print_char` writes the character as UTF-8, then a newline.
    #[test]
    #[cfg_attr(miri, ignore = "spawns a child process, which Miri cannot")]
    fn print_char_writes_utf8() {
        let out = run_rc_child("print_char");
        assert!(out.status.success(), "child exited with {:?}", out.status);
        assert!(String::from_utf8_lossy(&out.stdout).contains("\u{1F600}\n"));
    }

    /// `sisu_print_str` writes the bytes as they are, zero bytes included, then a newline; a
    /// string longer than the buffer still comes out in order, before the lines after it.
    #[test]
    #[cfg_attr(miri, ignore = "spawns a child process, which Miri cannot")]
    fn print_str_writes_bytes_in_order() {
        let out = run_rc_child("print_str");
        assert!(out.status.success(), "child exited with {:?}", out.status);
        let mut want = b"a\0b\n".to_vec();
        want.extend([b'x'; RESERVED]);
        want.extend(b"\n5\nfalse\n");
        // The child is a test binary, so libtest's own lines surround the output.
        assert!(out.stdout.windows(want.len()).any(|w| w == want));
    }

    /// Bytes that fit, with their newline, join the buffer; longer ones go straight out, so
    /// the buffer keeps its one allocation.
    #[test]
    fn append_bytes_never_outgrows_the_buffer() {
        let mut out = Stdout {
            buffer: Vec::with_capacity(RESERVED),
            sink: Some(Sink {
                file: None,
                line_buffered: false,
            }),
        };
        out.append_bytes(b"ab").expect("a closed sink drops output");
        assert_eq!(out.buffer, b"ab\n");
        // One byte too many to fit beside the newline.
        out.append_bytes(&[b'x'; RESERVED - 3])
            .expect("a closed sink drops output");
        assert_eq!(out.buffer, b"\n");
        assert_eq!(out.buffer.capacity(), RESERVED);
        // Fits, and takes the buffer past `BUFFER_CAPACITY`, which writes it out.
        out.append_bytes(&[b'x'; BUFFER_CAPACITY])
            .expect("a closed sink drops output");
        assert_eq!(out.buffer, b"");
        assert_eq!(out.buffer.capacity(), RESERVED);
    }

    /// A standard output that is open but cannot be duplicated (EMFILE) is a failed write,
    /// not a closed standard output.
    #[test]
    #[cfg_attr(miri, ignore = "spawns a child process, which Miri cannot")]
    fn print_panics_when_standard_output_cannot_be_duplicated() {
        assert_rc_exit(
            "dup_fails",
            Some(101),
            "sisu: panic: standard output could not be written: Too many open files (os error 24)\n",
        );
    }

    /// A fault whose stdout flush fails still reports itself and exits 102, never 101 or a
    /// signal, whether stdout is full or a pipe with no reader.
    #[test]
    #[cfg_attr(miri, ignore = "spawns a child process, which Miri cannot")]
    fn rc_fault_reports_when_the_stdout_flush_fails() {
        // Only Linux has /dev/full.
        let full = cfg!(target_os = "linux").then_some("fault_full_stdout");
        for case in full.into_iter().chain(["fault_closed_stdout_pipe"]) {
            assert_rc_exit(
                case,
                Some(102),
                "sisu: rc check: release of an object whose count is 0\n",
            );
        }
    }

    /// An `rc_fault` ends the program, so the lines printed before it must come out first.
    #[test]
    #[cfg_attr(miri, ignore = "spawns a child process, which Miri cannot")]
    fn rc_fault_flushes_buffered_output() {
        let out = run_rc_child("print_then_fault");
        assert_eq!(out.status.code(), Some(102), "{:?}", out.status);
        // The libtest child also prints to stdout; the line it buffered is among it.
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.lines().any(|line| line == "5"), "{stdout}");
        assert!(stdout.lines().any(|line| line == "2.5"), "{stdout}");
    }

    #[test]
    #[cfg_attr(miri, ignore = "spawns a child process, which Miri cannot")]
    fn check_exit_passes_when_balanced() {
        assert_rc_exit("balanced", Some(0), "");
    }

    #[test]
    #[cfg_attr(miri, ignore = "spawns a child process, which Miri cannot")]
    fn check_exit_reports_live_objects() {
        assert_rc_exit(
            "leak",
            Some(102),
            "sisu: rc check: live objects at exit: 1\n",
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "spawns a child process, which Miri cannot")]
    fn rc_fault_exits_102() {
        assert_rc_exit(
            "fault1",
            Some(102),
            "sisu: rc check: release of an object whose count is 0\n",
        );
        assert_rc_exit(
            "fault0",
            Some(102),
            "sisu: rc check: retain of an object whose count is 0\n",
        );
        assert_rc_exit(
            "fault2",
            Some(102),
            "sisu: rc check: read of a local after it was moved\n",
        );
        assert_rc_exit("fault7", Some(102), "sisu: rc check: fault 7\n");
    }
}
