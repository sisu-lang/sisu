//! Stack growth for the generated show and `==` helpers (ADR 0047).
//!
//! A helper that can call itself through an array or an `any` box takes frames for each link of
//! a value. It runs through [`sisu_stack_grow`], so a deep value goes on to a new stack segment.

use std::ffi::c_void;

/// The stack that must be left for a helper to stay on the current segment. It holds the frames
/// between two checks. The type fixes how many frames those are, and `sisuc` has the same value
/// for its own walks over a type. Keep it below the 256 KiB stack of the small-stack tests.
const RED_ZONE: usize = 100 * 1024;

/// The size of each new stack segment. A larger size makes fewer system calls, and a deep value
/// holds about one segment for each few thousand links.
const SEGMENT: usize = 1024 * 1024;

/// Runs `body(env)` on this stack, or on a new stack segment when less than `RED_ZONE` is left.
///
/// # Safety
///
/// `body` must be safe to call with `env`.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_stack_grow(
    body: unsafe extern "C" fn(*mut c_void),
    env: *mut c_void,
) {
    stacker::maybe_grow(RED_ZONE, SEGMENT, || {
        // SAFETY: the caller promised that this call is sound.
        unsafe { body(env) }
    });
}

#[cfg(test)]
#[expect(
    unsafe_code,
    reason = "the test calls the runtime's unsafe C ABI functions"
)]
mod tests {
    use super::sisu_stack_grow;
    use std::ffi::c_void;
    use std::hint::black_box;

    /// Subtracts 1 from the `i64` at `env`, and calls itself until that `i64` is 0. Each call holds
    /// 16 KiB of stack while the next call runs.
    unsafe extern "C" fn count_down(env: *mut c_void) {
        // SAFETY: each caller passes the address of an `i64`.
        let left = unsafe { &mut *env.cast::<i64>() };
        *left -= 1;
        let frame = black_box([0_u8; 16 * 1024]);
        if *left > 0 {
            // SAFETY: `count_down` takes the address of an `i64`, and `env` is one.
            unsafe { sisu_stack_grow(count_down, env) };
        }
        black_box(frame);
    }

    #[test]
    fn stack_grow_should_call_the_body_with_the_env() {
        let mut left = 1_i64;
        // SAFETY: `count_down` takes the address of an `i64`, and `left` is one.
        unsafe { sisu_stack_grow(count_down, (&raw mut left).cast()) };
        assert_eq!(left, 0);
    }

    /// 2000 levels of 16 KiB take 32 MiB, more than the stack of a test thread. So a run that ends
    /// has grown the stack.
    #[test]
    #[cfg_attr(miri, ignore = "the levels take too long under Miri")]
    fn stack_grow_should_run_deeper_than_the_stack() {
        let mut left = 2000_i64;
        // SAFETY: `count_down` takes the address of an `i64`, and `left` is one.
        unsafe { sisu_stack_grow(count_down, (&raw mut left).cast()) };
        assert_eq!(left, 0);
    }
}
