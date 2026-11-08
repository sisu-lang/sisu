//! Links an object file with the Sisu runtime into an executable. The system C
//! compiler drives the linker, so it also adds the C startup code and libc.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

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
    Err(cc_failure(
        out.status,
        &String::from_utf8_lossy(&out.stderr),
        linker,
    ))
}

/// Checks, without linking, that `cc` finds `linker`: `cc -fuse-ld=<linker> -Wl,--version` runs
/// the linker and prints its version. Found before the compile, not by the link after it. Always
/// `Ok` without a linker.
pub(crate) fn probe_linker(linker: Option<&str>) -> Result<(), String> {
    // shortcut: Linux only. Apple's clang looks for `ld64.<name>`, and nobody has checked that it
    // accepts `--version`; macOS keeps the late check in `link`. Probe on macOS after a check on
    // a Mac.
    if !cfg!(target_os = "linux") {
        return Ok(());
    }
    let Some(linker) = linker else {
        return Ok(());
    };
    let out = Command::new("cc")
        .arg(format!("-fuse-ld={linker}"))
        .arg("-Wl,--version")
        .output()
        .map_err(|e| format!("cannot run cc: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    // gcc's `collect2` reads `--version` as verbose: it prints its version and then the whole
    // linker command line, which is not part of the failure.
    let stderr = String::from_utf8_lossy(&out.stderr);
    let kept = if stderr.starts_with("collect2 version ") {
        stderr
            .split_once('\n')
            .and_then(|(_, rest)| rest.split_once('\n'))
            .map_or("", |(_, rest)| rest)
    } else {
        &stderr
    };
    Err(cc_failure(out.status, kept, Some(linker)))
}

/// The message for a `cc` that failed with `status` and `stderr`: its status, a note when
/// `linker` is not on PATH, and its stderr.
fn cc_failure(status: ExitStatus, stderr: &str, linker: Option<&str>) -> String {
    let mut message = format!("cc failed: {status}");
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
    match stderr.trim() {
        "" => message,
        detail => format!("{message}\n{detail}"),
    }
}

/// Whether a directory on PATH holds `program`.
fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

/// The command in a clone that builds the runtime archive where the running `sisu` looks for it:
/// `cargo build` fills `target/debug` for a debug `sisu` and `target/release` for a release one.
const fn rebuild_command(debug: bool) -> &'static str {
    if debug {
        "cargo build --workspace"
    } else {
        "cargo build --workspace --release"
    }
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
