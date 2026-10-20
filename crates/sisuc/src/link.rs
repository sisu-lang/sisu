//! Links an object file with the Sisu runtime into an executable. The system C
//! compiler drives the linker, so it also adds the C startup code and libc.

use std::path::{Path, PathBuf};
use std::process::Command;

/// What a link needs besides the object file.
pub(crate) struct Linking<'a> {
    /// The runtime library, found before the compile.
    pub(crate) runtime: &'a Path,
    pub(crate) output: &'a Path,
    pub(crate) linker: Option<&'a str>,
}

/// Links with `cc`'s default linker, or with `ld.<linker>` through `cc -fuse-ld=<linker>`.
pub(crate) fn link(object: &Path, linking: &Linking) -> Result<(), String> {
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
        .arg(object)
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
    // `cc` also finds linkers off PATH, so the check waits until it has failed.
    if let Some(program) = linker.map(|linker| format!("ld.{linker}"))
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

/// Cargo builds `libsisu_runtime.a` into the same directory as `sisuc`.
// shortcut: Cargo target directories only; add a search path once sisuc installs elsewhere.
pub(crate) fn runtime_library() -> Result<PathBuf, String> {
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
