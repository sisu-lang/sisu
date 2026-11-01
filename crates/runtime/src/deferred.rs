//! The queue of drops that wait past the drop depth limit (ADR 0038).
//!
//! A generated drop function counts its depth. At the limit it does not recurse: it puts its
//! block in this queue and returns. The drop that brings the depth back to 0 empties the queue.
//! A deferred block has count 0 and keeps its fields, so its drop function frees them when it
//! runs. The generated code calls [`sisu_drop_drain`] only at drop depth 0. A Sisu program runs
//! on one thread, and the queue is only used at the limit, so a thread-local is cheap enough.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

/// A block and the drop function that frees it.
type Entry = (*mut u8, unsafe extern "C" fn(*mut u8));

thread_local! {
    /// The deferred drops, oldest first. The capacity stays after the queue is empty.
    static QUEUE: RefCell<VecDeque<Entry>> = const { RefCell::new(VecDeque::new()) };
    /// Whether a call of [`sisu_drop_drain`] is running its loop.
    static DRAINING: Cell<bool> = const { Cell::new(false) };
}

/// Puts `(block, drop)` at the back of the queue. `drop(block)` runs when [`sisu_drop_drain`]
/// reaches it. A failed allocation aborts the process, as `VecDeque` does.
///
/// # Safety
///
/// `drop(block)` is sound to call once, at any later time on this thread, and `block` stays
/// valid until then.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sisu_drop_defer(block: *mut u8, drop: unsafe extern "C" fn(*mut u8)) {
    QUEUE.with_borrow_mut(|queue| queue.push_back((block, drop)));
}

/// Takes the front entry of the queue and calls its drop function, until the queue is empty.
/// A drop function can add entries, and this loop runs them too. When a loop is already
/// running, a call returns at once, so the stack does not grow with the chain.
#[expect(
    unsafe_code,
    reason = "generated code calls this function by its unmangled symbol name"
)]
#[unsafe(no_mangle)]
pub extern "C" fn sisu_drop_drain() {
    if DRAINING.replace(true) {
        return;
    }
    // The entry leaves the queue before the call, so the call can defer again.
    while let Some((block, drop)) = QUEUE.with_borrow_mut(VecDeque::pop_front) {
        // SAFETY: `sisu_drop_defer`'s caller promised that this call is sound.
        unsafe { drop(block) };
    }
    DRAINING.set(false);
}

#[cfg(test)]
#[expect(
    unsafe_code,
    reason = "the test calls the runtime's unsafe C ABI functions"
)]
mod tests {
    use std::cell::RefCell;
    use std::ptr;

    use super::{sisu_drop_defer, sisu_drop_drain};

    thread_local! {
        /// The tags of the drops that ran, in order.
        static RECORD: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    }

    /// The tag that a test passes as a block: the block's address, which nothing reads through.
    fn tag(n: u8) -> *mut u8 {
        ptr::without_provenance_mut(usize::from(n))
    }

    fn record(block: *mut u8) {
        RECORD.with_borrow_mut(|record| record.push(u8::try_from(block.addr()).unwrap()));
    }

    unsafe extern "C" fn record_1(block: *mut u8) {
        record(block);
    }

    unsafe extern "C" fn record_2(block: *mut u8) {
        record(block);
        // SAFETY: `record_4` only records the tag, and the queue is not borrowed here.
        unsafe { sisu_drop_defer(tag(4), record_4) };
        // A loop is already running, so this call returns at once.
        sisu_drop_drain();
        // A second loop would have run 3 and 4 here.
        assert_eq!(recorded(), [1, 2]);
    }

    unsafe extern "C" fn record_3(block: *mut u8) {
        record(block);
    }

    unsafe extern "C" fn record_4(block: *mut u8) {
        record(block);
    }

    unsafe extern "C" fn record_5(block: *mut u8) {
        record(block);
    }

    fn recorded() -> Vec<u8> {
        RECORD.with_borrow(Clone::clone)
    }

    /// GIVEN the runtime's deferred queue and three deferred drops whose drop functions
    ///   record their order
    /// AND the second drop function adds a fourth drop and then calls the function that
    ///   empties the queue
    /// WHEN the queue is emptied, in the runtime's unit tests and under `make miri`
    /// THEN the drop functions run in the order 1, 2, 3, 4
    /// AND at most one loop that empties the queue runs at a time
    /// AND a later drop is run by a later call that empties the queue
    #[test]
    fn deferred_queue_should_run_drops_in_order_in_one_loop() {
        // SAFETY: `record_1` only records the tag it gets.
        unsafe { sisu_drop_defer(tag(1), record_1) };
        // SAFETY: `record_2` only records the tag it gets.
        unsafe { sisu_drop_defer(tag(2), record_2) };
        // SAFETY: `record_3` only records the tag it gets.
        unsafe { sisu_drop_defer(tag(3), record_3) };
        sisu_drop_drain();
        assert_eq!(recorded(), [1, 2, 3, 4]);
        sisu_drop_drain();
        assert_eq!(
            recorded(),
            [1, 2, 3, 4],
            "a second drain should find nothing"
        );
        // SAFETY: `record_5` only records the tag it gets.
        unsafe { sisu_drop_defer(tag(5), record_5) };
        sisu_drop_drain();
        assert_eq!(
            recorded(),
            [1, 2, 3, 4, 5],
            "a drain should run after the first loop ended"
        );
    }
}
