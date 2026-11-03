//! The `sisu` binary: runs the compiler's command line on a thread with a large stack.

use std::panic;
use std::process::ExitCode;
use std::thread;

use mimalloc::MiMalloc;

// The compiler and LLVM make and free many small nodes, and mimalloc does that faster than
// glibc's malloc. Its `override` feature also replaces `malloc`, so LLVM uses it too.
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

fn main() -> ExitCode {
    let compiler = thread::Builder::new()
        .name("main".to_string())
        .stack_size(sisuc::STACK_SIZE)
        .spawn(sisuc::cli)
        .expect("the OS should start a thread with a 64 MiB stack");
    // `panic = "abort"` makes a panic in the compiler abort the process (SIGABRT, exit 134) on
    // this thread as it did on the main one, so `join` fails only in a build that unwinds.
    compiler.join().unwrap_or_else(|p| panic::resume_unwind(p))
}
