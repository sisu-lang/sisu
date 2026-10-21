//! Fuzzes `sisu check` and its text and JSON reports with arbitrary bytes, and compiles each
//! input that checks clean.
#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../common/thread.rs"]
mod thread;

fuzz_target!(|data: &[u8]| {
    let data = data.to_vec();
    thread::run_on_thread(move || {
        sisuc::fuzz_front_end(&data);
    });
});
