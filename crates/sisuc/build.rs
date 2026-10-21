//! Sets `SISU_COMMIT`, the short hash of the commit being built, for `sisuc --version --verbose`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!(
        "cargo::rustc-env=SISU_COMMIT={}",
        commit().unwrap_or_else(|| "unknown".into())
    );
    // `HEAD` changes on a checkout, the reflog on every commit, checkout and reset, whether the
    // branch's ref is loose or packed. A watched file that is missing makes Cargo rerun on
    // every build, so only the ones that exist are watched.
    for name in ["HEAD", "logs/HEAD"] {
        if let Some(path) = git(&["rev-parse", "--git-path", name]).map(PathBuf::from)
            && path.exists()
        {
            println!("cargo::rerun-if-changed={}", path.display());
        }
    }
}

/// The output of `git args`, trimmed, or `None` if git fails.
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// The commit of the workspace being built, or `None` outside its git checkout. Git searches
/// parent directories, so a source tree unpacked inside another repository must not get that
/// repository's commit.
fn commit() -> Option<String> {
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR")?;
    let root = Path::new(&manifest).join("../..").canonicalize().ok()?;
    let top = PathBuf::from(git(&["rev-parse", "--show-toplevel"])?)
        .canonicalize()
        .ok()?;
    if top != root {
        return None;
    }
    git(&["rev-parse", "--short=7", "HEAD"])
}
