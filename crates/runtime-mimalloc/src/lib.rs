//! `libsisu_runtime_mimalloc.a`: the Sisu runtime, allocating through mimalloc.

use mimalloc::MiMalloc;
use sisu_runtime as _;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;
