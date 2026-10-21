//! Sets `SISU_COMMIT`, the short hash of the commit being built, for `sisuc --version --verbose`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!(
        "cargo::rustc-env=SISU_COMMIT={}",
        commit().unwrap_or_else(|| "unknown".into())
    );
    // `HEAD` changes on a checkout. The HEAD reflog gets a line on every commit, checkout and
    // reset, whether the branch's ref is loose or packed, but it is missing when
    // `core.logAllRefUpdates` is false; then the current branch's ref and `packed-refs` stand in.
    // A watched file that is missing makes Cargo rerun on every build, so only existing paths
    // are watched.
    let git_path = |name: &str| git(&["rev-parse", "--git-path", name]).map(PathBuf::from);
    let mut watched = vec![git_path("HEAD"), git_path("logs/HEAD")];
    if !watched[1].as_ref().is_some_and(|path| path.exists()) {
        let branch = git(&["rev-parse", "--symbolic-full-name", "HEAD"])
            .filter(|name| name != "HEAD")
            .and_then(|name| git_path(&name));
        // A packed branch has no ref file until the next commit writes one, so until then the
        // nearest directory that exists is watched: Cargo sees the new file in it.
        watched.push(
            branch.and_then(|path| path.ancestors().find(|p| p.exists()).map(Path::to_path_buf)),
        );
        watched.push(git_path("packed-refs"));
    }
    for path in watched.into_iter().flatten().filter(|path| path.exists()) {
        println!("cargo::rerun-if-changed={}", path.display());
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
