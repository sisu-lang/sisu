//! Links an object file with the Sisu runtime into an executable. The system C
//! compiler drives the linker, so it also adds the C startup code and libc.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::Allocator;

/// What a link needs besides the object file.
pub(crate) struct Linking<'a> {
    /// The runtime library, found before the compile.
    pub(crate) runtime: &'a Path,
    pub(crate) output: &'a Path,
    pub(crate) linker: Option<&'a str>,
}

/// Links `objects`, in order, with `cc`'s default linker, or with `ld.<linker>` through
/// `cc -fuse-ld=<linker>`.
pub(crate) fn link(objects: &[PathBuf], linking: &Linking<'_>) -> Result<(), String> {
    let Linking {
        runtime,
        output,
        linker,
    } = *linking;
    let mut cc = Command::new("cc");
    if let Some(linker) = linker {
        cc.arg(format!("-fuse-ld={linker}"));
    }
    // `output`, not `status`: cc's messages go into the tool error, so stderr stays one format.
    let out = cc
        .args(objects)
        .arg(runtime)
        // libm, after the runtime, for the C math functions. Not for `fmod`, which `frem`
        // lowers to: the runtime's compiler_builtins defines it, weakly, and is linked first.
        .arg("-lm")
        .arg("-o")
        .arg(output)
        .output()
        .map_err(|e| format!("cannot run cc: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let mut message = format!("cc failed: {}", out.status);
    // A missing `ld.lld` makes gcc say only "cannot find 'ld'", which names the wrong program.
    // `cc` also finds linkers off PATH, so the check waits until it has failed. Clang on macOS
    // looks for `ld64.<name>`.
    let prefix = if cfg!(target_os = "macos") {
        "ld64"
    } else {
        "ld"
    };
    if let Some(program) = linker.map(|linker| format!("{prefix}.{linker}"))
        && !on_path(&program)
    {
        message = format!("linker {program} not found on PATH; {message}");
    }
    match String::from_utf8_lossy(&out.stderr).trim() {
        "" => Err(message),
        detail => Err(format!("{message}\n{detail}")),
    }
}

/// Whether a directory on PATH holds `program`.
fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

/// The command in a clone that builds the runtime archive for the profile of the running `sisu`:
/// `debug` is true for a debug build.
const fn rebuild_command(_debug: bool) -> &'static str {
    "cargo build --workspace"
}

/// The runtime archive on `allocator`. It is next to `sisu`, in a Cargo target directory or in an
/// install: `libsisu_runtime_mimalloc.a` or `libsisu_runtime_system.a`.
pub(crate) fn runtime_library(allocator: Allocator) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate sisu: {e}"))?;
    let path = exe.with_file_name(match allocator {
        Allocator::Mimalloc => "libsisu_runtime_mimalloc.a",
        Allocator::System => "libsisu_runtime_system.a",
    });
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!(
            "runtime library not found at {}; run the Sisu install line again, or run \
             `{}` in a clone",
            path.display(),
            rebuild_command(cfg!(debug_assertions))
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebuild_command_should_name_the_profile_of_the_running_sisu() {
        assert_eq!(rebuild_command(true), "cargo build --workspace");
        assert_eq!(rebuild_command(false), "cargo build --workspace --release");
    }
}
