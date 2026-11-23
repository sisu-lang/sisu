//! Links an object file with the Sisu runtime into an executable. The system C
//! compiler drives the linker, so it also adds the C startup code and libc.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use crate::{Allocator, Sanitizer};

/// What a link needs besides the object file.
pub(crate) struct Linking<'a> {
    /// The runtime library, found before the compile.
    pub(crate) runtime: &'a Path,
    pub(crate) output: &'a Path,
    pub(crate) linker: Option<&'a str>,
    /// The sanitizer that the objects are built for; its runtime is linked in.
    pub(crate) sanitize: Option<Sanitizer>,
}

/// Links `objects`, in order, with `cc`'s default linker, or with `ld.<linker>` through
/// `cc -fuse-ld=<linker>`.
pub(crate) fn link(objects: &[PathBuf], linking: &Linking<'_>) -> Result<(), String> {
    let Linking {
        runtime,
        output,
        linker,
        sanitize,
    } = *linking;
    let mut cc = Command::new("cc");
    if sanitize.is_some() {
        cc.arg("-fsanitize=address");
    }
    if let Some(linker) = linker {
        cc.arg(format!("-fuse-ld={linker}"));
    }
    // The runtime archive holds the DWARF of Rust's standard library, and Sisu emits no debug info
    // of its own. `-S` drops it, and the symbol table stays, so Valgrind keeps the names. Linux
    // only: `ld64` does not copy DWARF, and nobody has checked `-S` there. When Sisu emits its own
    // debug info, this flag must go or become conditional.
    if cfg!(target_os = "linux") {
        cc.arg("-Wl,-S");
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

/// The file name of the runtime archive on `allocator`.
const fn archive_name(allocator: Allocator) -> &'static str {
    match allocator {
        Allocator::Mimalloc => "libsisu_runtime_mimalloc.a",
        Allocator::System => "libsisu_runtime_system.a",
    }
}

/// The runtime archive on `allocator`. It is next to `sisu`, in a Cargo target directory or in an
/// install: `libsisu_runtime_mimalloc.a` or `libsisu_runtime_system.a`.
pub(crate) fn runtime_library(allocator: Allocator) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate sisu: {e}"))?;
    let path = exe.with_file_name(archive_name(allocator));
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

/// The runtime archive on `allocator` for a unit test.
///
/// Nextest's setup script writes `SISU_TEST_RUNTIME_DIR`. Without it, search ancestors above the
/// binary's folder, then fall back two levels up.
#[cfg(test)]
pub(crate) fn test_runtime(allocator: Allocator) -> PathBuf {
    let exe = std::env::current_exe().expect("the test binary should know its path");
    test_runtime_in(
        std::env::var_os("SISU_TEST_RUNTIME_DIR").as_deref(),
        &exe,
        allocator,
    )
}

#[cfg(test)]
fn test_runtime_in(env_dir: Option<&std::ffi::OsStr>, exe: &Path, allocator: Allocator) -> PathBuf {
    let archive = archive_name(allocator);
    if let Some(dir) = env_dir.filter(|d| !d.is_empty()) {
        let path = Path::new(dir).join(archive);
        if path.is_file() {
            return path;
        }
    }
    // Skip the binary and its immediate folder (such as `deps/`) so an instrumented copy never
    // wins over `target/debug/`.
    if let Some(dir) = exe.ancestors().skip(2).find(|d| d.join(archive).is_file()) {
        return dir.join(archive);
    }
    exe.parent()
        .and_then(Path::parent)
        .expect("the test binary should be two levels inside the target directory")
        .join(archive)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebuild_command_should_name_the_profile_of_the_running_sisu() {
        assert_eq!(rebuild_command(true), "cargo build --workspace");
        assert_eq!(rebuild_command(false), "cargo build --workspace --release");
    }

    #[test]
    fn test_runtime_in_should_prefer_env_dir_and_search_ancestors_for_archive() {
        struct TempDir(PathBuf);
        impl Drop for TempDir {
            fn drop(&mut self) {
                drop(std::fs::remove_dir_all(&self.0));
            }
        }

        let root =
            TempDir(std::env::temp_dir().join(format!("sisu-test-runtime-{}", std::process::id())));
        drop(std::fs::remove_dir_all(&root.0));

        let env_dir = root.0.join("env");
        let empty_dir = root.0.join("empty");
        let debug_dir = root.0.join("target/debug");
        let deps_dir = debug_dir.join("deps");
        let nightly_dir = debug_dir.join("build/sisuc/abc123");
        let nowhere_exe = root.0.join("nowhere/deps/sisuc-abc");
        for dir in [&env_dir, &empty_dir, &deps_dir, &nightly_dir] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::create_dir_all(nowhere_exe.parent().unwrap()).unwrap();

        for allocator in [Allocator::Mimalloc, Allocator::System] {
            let name = archive_name(allocator);
            std::fs::write(env_dir.join(name), b"").unwrap();
            std::fs::write(deps_dir.join(name), b"").unwrap();
            std::fs::write(debug_dir.join(name), b"").unwrap();

            let stable_exe = deps_dir.join("sisuc-abc");
            let nightly_exe = nightly_dir.join("sisuc-abc");

            // 1. `Some(dir)` holding the archive wins even when `exe` is elsewhere.
            assert_eq!(
                test_runtime_in(Some(env_dir.as_os_str()), &nowhere_exe, allocator),
                env_dir.join(name),
            );
            // 2. `Some(empty_dir)` falls back to ancestor search under `exe`.
            assert_eq!(
                test_runtime_in(Some(empty_dir.as_os_str()), &stable_exe, allocator),
                debug_dir.join(name),
            );
            // 3. `Some("")` is ignored and falls back to ancestor search.
            assert_eq!(
                test_runtime_in(Some(std::ffi::OsStr::new("")), &stable_exe, allocator),
                debug_dir.join(name),
            );
            // 4. Stable layout skips `deps/` so `debug/` wins over `deps/`.
            assert_eq!(
                test_runtime_in(None, &stable_exe, allocator),
                debug_dir.join(name),
            );
            // 5. Nightly layout walks ancestors up to `target/debug/`.
            assert_eq!(
                test_runtime_in(None, &nightly_exe, allocator),
                debug_dir.join(name),
            );
            // 6. When no ancestor has the archive, fall back two levels up.
            assert_eq!(
                test_runtime_in(None, &nowhere_exe, allocator),
                root.0.join("nowhere").join(name),
            );
        }
    }
}
