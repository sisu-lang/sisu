//! Scratch directories for the test binaries that run `sisu` with a custom `PATH`.

use std::fs;
use std::path::{Path, PathBuf};

/// An empty directory under the target tmp dir, for `sisu` to use as `TMPDIR`.
pub(crate) fn empty_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    // It is absent on the first run, so a failure here is expected and harmless.
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir(&dir).expect("creates the directory");
    dir
}

/// A directory that holds only a link to `cc`, for a `PATH` with no linker on it.
pub(crate) fn only_cc(name: &str) -> PathBuf {
    let dir = empty_dir(name);
    let cc = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join("cc"))
        .find(|cc| cc.is_file())
        .expect("cc is on PATH");
    std::os::unix::fs::symlink(cc, dir.join("cc")).expect("links cc");
    dir
}
