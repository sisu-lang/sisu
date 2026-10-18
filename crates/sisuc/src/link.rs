//! Links an object file with the Sisu runtime into an executable. The system C
//! compiler drives the linker, so it also adds the C startup code and libc.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Links with `cc`'s default linker, or with `ld.<linker>` through `cc -fuse-ld=<linker>`.
pub(crate) fn link(object: &Path, output: &Path, linker: Option<&str>) -> Result<(), String> {
    let runtime = runtime_library()?;
    let mut cc = Command::new("cc");
    if let Some(linker) = linker {
        cc.arg(format!("-fuse-ld={linker}"));
    }
    // `output`, not `status`: cc's messages go into the tool error, so stderr stays one format.
    let out = cc
        .arg(object)
        .arg(&runtime)
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
    // A missing `ld.lld` makes gcc say only "cannot find 'ld'", which names the wrong program.
    // `cc` also finds linkers off PATH, so the check waits until it has failed.
    if let Some(program) = linker.map(|linker| format!("ld.{linker}"))
        && !on_path(&program)
    {
        return Err(format!("linker {program} not found on PATH"));
    }
    let message = format!("cc failed: {}", out.status);
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

/// Cargo builds `libsisu_runtime.a` into the same directory as `sisuc`.
// ponytail: only works from a Cargo target directory; add a search path once sisuc is installed elsewhere.
fn runtime_library() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate sisuc: {e}"))?;
    let path = exe.with_file_name("libsisu_runtime.a");
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!(
            "runtime library not found at {}; build it with `cargo build --workspace`",
            path.display()
        ))
    }
}
