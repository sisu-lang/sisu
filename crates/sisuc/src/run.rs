//! `sisu run`: compiles a program into a private temporary directory, starts it, removes the
//! directory, and exits with the program's exit code.

use std::ffi::OsString;
use std::fs::{self, DirBuilder, Permissions};
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus};

use crate::report::{Format, tool_error_after_summary};
use crate::{Backend, Report, Source, compile, fresh_names, front_end, link, read_source};

/// Compiles `source` and runs it with `args`, its stdin, stdout and stderr those of `sisu`.
/// Finishes `reporter`, which writes in `format`, before the program starts.
pub(crate) fn run(
    mut reporter: Report,
    format: &Format,
    source: &Source,
    args: &[OsString],
    backend: &Backend,
) -> ExitCode {
    let built = build(&mut reporter, source, backend);
    // The warnings and their summary come before anything the program writes.
    reporter.finish();
    let Some((dir, exe)) = built else {
        return ExitCode::FAILURE;
    };
    let spawned = Command::new(&exe).args(args).spawn();
    // `spawn` returns once `exec` has worked, and the running program keeps its inode, so the
    // directory can go before the program ends, whatever ends it.
    if let Err(e) = fs::remove_dir_all(&dir) {
        tool_error_after_summary(format, &format!("cannot remove {}: {e}", dir.display()));
    }
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            tool_error_after_summary(format, &format!("cannot run {}: {e}", exe.display()));
            return ExitCode::FAILURE;
        }
    };
    match child.wait() {
        Ok(status) => exit_code(status),
        Err(e) => {
            tool_error_after_summary(format, &format!("cannot wait for {}: {e}", exe.display()));
            ExitCode::FAILURE
        }
    }
}

/// Compiles `source` into an executable in a new private directory, and returns the directory
/// and the executable. Reports why it cannot, and then leaves nothing behind.
fn build(reporter: &mut Report, source: &Source, backend: &Backend) -> Option<(PathBuf, PathBuf)> {
    // Found here, not by `ld` after a whole compile.
    let runtime = link::runtime_library()
        .inspect_err(|message| reporter.tool_error(message))
        .ok()?;
    let (name, text) = read_source(reporter, source)?;
    let program = front_end(reporter, &name, &text)?;
    // shortcut: a Ctrl-C from here until `run` removes the directory, through the compile, the
    // link and the spawn, leaves it behind; a signal handler would close that gap, if it ever
    // matters.
    let dir = private_dir(&std::env::temp_dir(), fresh_names(""))
        .inspect_err(|message| reporter.tool_error(message))
        .ok()?;
    let exe = dir.join("program");
    match compile(&program, &name, &text, &runtime, &exe, &dir, backend) {
        Ok(()) => Some((dir, exe)),
        Err(message) => {
            reporter.tool_error(&message);
            if let Err(e) = fs::remove_dir_all(&dir) {
                reporter.tool_error(&format!("cannot remove {}: {e}", dir.display()));
            }
            None
        }
    }
}

/// Creates a directory in `parent` that only its owner can read or write, under the first of
/// `names` that is free, and returns its path. `cc` creates the executable with the umask, so
/// the directory is what keeps others from reading or replacing it.
fn private_dir(parent: &Path, names: impl IntoIterator<Item = String>) -> Result<PathBuf, String> {
    let mut builder = DirBuilder::new();
    builder.mode(0o700);
    for name in names {
        let dir = parent.join(name);
        match builder.create(&dir) {
            // The umask can take bits from `mode`, as it did 0o700 under `umask 0777`, but not
            // from `set_permissions`.
            Ok(()) => {
                return match fs::set_permissions(&dir, Permissions::from_mode(0o700)) {
                    Ok(()) => Ok(dir),
                    Err(e) => {
                        let message = format!("cannot set the mode of {}: {e}", dir.display());
                        fs::remove_dir(&dir)
                            .map_err(|e| format!("{message}; cannot remove it: {e}"))
                            .and(Err(message))
                    }
                };
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("cannot create {}: {e}", dir.display())),
        }
    }
    Err(format!(
        "cannot create a temporary directory in {}",
        parent.display()
    ))
}

/// The program's exit code, or 128 plus the number of the signal that ended it, as a shell
/// reports it.
fn exit_code(status: ExitStatus) -> ExitCode {
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .and_then(|code| u8::try_from(code).ok())
        .map_or(ExitCode::FAILURE, ExitCode::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_dir_takes_the_first_free_name_and_is_the_owners_only() {
        let parent = std::env::temp_dir().join(format!("sisu-private-dir-{}", std::process::id()));
        // It is absent unless an earlier run failed, so a failure here is expected and harmless.
        let _ = fs::remove_dir_all(&parent);
        fs::create_dir_all(parent.join("taken")).expect("creates the directories");

        let dir =
            private_dir(&parent, ["taken", "free"].map(String::from)).expect("`free` is free");
        assert_eq!(dir, parent.join("free"));
        let mode = fs::metadata(&dir)
            .expect("reads metadata")
            .permissions()
            .mode();
        // With every name taken, it gives up.
        let taken = private_dir(&parent, ["taken".to_string()]);
        fs::remove_dir_all(&parent).expect("removes the directories");
        assert_eq!(mode & 0o077, 0, "mode {mode:o}");
        assert_eq!(
            taken,
            Err(format!(
                "cannot create a temporary directory in {}",
                parent.display()
            ))
        );
    }
}
