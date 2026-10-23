//! Fuzzes generated programs: each input's bytes become a well-typed Sisu program, built three
//! ways with its rewritten copy, run, and judged against properties 1 to 5 (ADR 0033).
#![no_main]
use std::path::PathBuf;
use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;

#[path = "../common/thread.rs"]
mod thread;

/// The runtime archive the builds link, from `SISU_RUNTIME`, read once.
fn runtime() -> &'static PathBuf {
    static RUNTIME: OnceLock<PathBuf> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        let path = std::env::var_os("SISU_RUNTIME")
            .map(PathBuf::from)
            .filter(|path| path.is_file());
        path.expect("SISU_RUNTIME should name the runtime archive; run this through scripts/fuzz.sh")
    })
}

fuzz_target!(|data: &[u8]| {
    let data = data.to_vec();
    let runtime = runtime();
    thread::run_on_thread(move || {
        sisuc::fuzz_program(&data, runtime);
    });
});
