//! The program's arguments, for Sisu's `fn main(args: [string])`.

use std::borrow::Cow;
use std::ffi::{CStr, c_char};
use std::sync::{Mutex, PoisonError};

use crate::text::repair;

/// The last argument `sisu_arg` had to repair.
static REPAIRED: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// The number of arguments after the program name: `argc` less one, and 0 when `argc` is 0 or
/// below, as for a process started with no `argv[0]`.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name"
)]
#[unsafe(no_mangle)]
pub extern "C" fn sisu_args_len(argc: i32) -> i64 {
    i64::from(argc).max(1) - 1
}

/// Argument `index` after the program name, repaired to valid UTF-8 as `read_line`'s lines
/// are: writes its length to `len` and returns its bytes, which are valid until the next call.
/// A valid argument is returned in place, in `argv`; an invalid one from a buffer that the next
/// repair overwrites.
///
/// # Safety
///
/// - `argv` holds `index + 2` readable pointers or more, each to a readable C string, as the C
///   `main`'s `argv` does for an `index` below `sisu_args_len(argc)`. `index` is not negative.
/// - `len` is non-null and writable for one `usize`.
/// - The returned bytes are read before the next call, which may overwrite them.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name and passes raw pointers"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_arg(
    argv: *const *const c_char,
    index: i64,
    len: *mut usize,
) -> *const u8 {
    let index = usize::try_from(index).expect("generated code should pass an index from 0 up");
    // SAFETY: the caller promises that `argv` holds an entry after the program name at
    // `index`, so the offset stays inside the array.
    let slot = unsafe { argv.add(index + 1) };
    // SAFETY: that entry is readable, as the caller promises.
    let entry = unsafe { *slot };
    // SAFETY: the caller promises that the entry is a readable C string.
    let arg = unsafe { CStr::from_ptr(entry) }.to_bytes();
    let (bytes, n) = match repair(arg) {
        Cow::Borrowed(arg) => (arg.as_ptr(), arg.len()),
        Cow::Owned(repaired) => {
            // Nothing panics while the lock is held (`panic = "abort"`), so it is never
            // poisoned.
            let mut buffer = REPAIRED.lock().unwrap_or_else(PoisonError::into_inner);
            *buffer = repaired;
            // SAFETY of the caller's later read: the bytes live in `REPAIRED`, a `static`,
            // after the guard is dropped. A Sisu program runs on one thread, so nothing
            // changes them until the next call, before which the caller has copied them.
            (buffer.as_ptr(), buffer.len())
        }
    };
    // SAFETY: the caller promises that `len` is writable.
    unsafe { len.write(n) };
    bytes
}

#[cfg(test)]
mod tests {
    use std::ffi::{CString, c_char};

    use super::{sisu_arg, sisu_args_len};

    /// The program name is not an argument, and a process started with no `argv[0]` has none.
    #[test]
    fn args_len() {
        for (argc, want) in [(-1, 0), (0, 0), (1, 0), (3, 2)] {
            assert_eq!(sisu_args_len(argc), want, "argc {argc}");
        }
    }

    /// A valid argument comes back in place, from `argv`; an invalid one comes back repaired.
    #[test]
    fn arg_repairs() {
        let owned = [&b"prog"[..], "häst".as_bytes(), b"x\xFFy"]
            .map(|arg| CString::new(arg).expect("a test argument should hold no zero byte"));
        let argv: Vec<*const c_char> = owned.iter().map(|arg| arg.as_ptr()).collect();
        let arg = |index| {
            let mut len = 0;
            // SAFETY: `argv` holds 3 C strings, so indexes 0 and 1 are below its length less
            // the program name; `len` is writable.
            #[expect(unsafe_code, reason = "calls the runtime's C ABI function")]
            let bytes = unsafe { sisu_arg(argv.as_ptr(), index, &raw mut len) };
            // SAFETY: `sisu_arg` returned `len` readable bytes, valid until its next call.
            #[expect(unsafe_code, reason = "reads the bytes the runtime returned")]
            let text = unsafe { std::slice::from_raw_parts(bytes, len) }.to_vec();
            (bytes, text)
        };
        let (bytes, text) = arg(0);
        assert_eq!(bytes, argv[1].cast());
        assert_eq!(text, "häst".as_bytes());
        assert_eq!(arg(1).1, "x\u{FFFD}y".as_bytes());
    }
}
