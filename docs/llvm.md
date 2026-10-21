# Upgrading LLVM

Sisu builds against one pinned LLVM major. This page explains what decides
which major we can use, and lists every place a bump touches.

## The chain

`sisu` reaches LLVM through two crates, and each LLVM major needs a new
release of both:

1. **LLVM** releases a new major about every six months, around March and
   September, then patch releases (`X.1.1`, `X.1.2`, ...) every two weeks or
   so. The `main` branch already carries the next major's number, with a
   `git` suffix: `24.0.0git` is unreleased.
2. **llvm-sys** publishes one crate version line per LLVM major and minor:
   `221.x` binds LLVM 22.1, `231.x` binds LLVM 23.1. It finds LLVM through
   `LLVM_SYS_<major><minor>_PREFIX`, for example `LLVM_SYS_221_PREFIX`.
3. **inkwell** wraps llvm-sys and selects the version with a Cargo feature,
   `llvm<major>-1`, for example `llvm22-1`. A feature lands on inkwell's
   `master` first and reaches crates.io in a later release.

Sisu moves to a major only when all three exist for it. llvm-sys can build
against a newer LLVM than it binds, but nothing checks that the bindings still
match, so we do not rely on it. A development LLVM (`main`) has no bindings
and no release binaries.

## Policy

Run the newest LLVM major that a crates.io release of inkwell supports. Bump
once that release is out, in one pull request. Use an inkwell git dependency
pinned to a commit only when a newer LLVM gives us something we need, and go
back to crates.io when inkwell releases.

LLVM 23 is such a case. The pinned Rust toolchain's `rustc` is on LLVM 23.1,
so with LLVM 23 `sisu` can read the bitcode that toolchain writes, which
linking the runtime as bitcode needs (Link-time optimization with the runtime
in [Later and stretch goals](roadmap.md#later-and-stretch-goals)). inkwell's
`master` has `llvm23-1`, so `crates/sisuc/Cargo.toml` pins a commit of it, and
`deny.toml` allows the repository. Both go back to a crates.io version once an
inkwell release has `llvm23-1`.

## Status

Checked on 2026-10-18:

| Component | Newest | Notes |
| --- | --- | --- |
| LLVM release | 23.1.3 | `main` is `24.0.0git` |
| llvm-sys | `231.0.1` | no `241` yet |
| inkwell on crates.io | `0.10.0`, up to `llvm22-1` | |
| inkwell `master` | adds `llvm23-1` | unreleased; what Sisu uses |

Sisu is on LLVM 23.1.3, inkwell `master` at `c9d9734a` with `llvm23-1`, and
`llvm-sys 231`.

## Checking for a new version

Use the GitHits tools, as `AGENTS.md` requires; do not rely on memory or on
this table.

- New LLVM major: `cmake/Modules/LLVMVersion.cmake` in `github:llvm/llvm-project`
  shows the version on `main`; a tag `llvmorg-<major>.1.0` means it is released.
- llvm-sys: `pkg_changelog` on `crates:llvm-sys`, looking for a new
  `<major><minor>.x` line.
- inkwell: `pkg_changelog` on `crates:inkwell` lists releases, but changelogs
  do not say which LLVM a release supports. `grep` for `llvm<major>-1` in
  `Cargo.toml` of `crates:inkwell@<version>` to confirm a release has the
  feature and its llvm-sys dependency. The same `grep` on
  `github:TheDan64/inkwell` shows whether `master` has it; `master` can carry
  the released version number with unreleased features.
- inkwell API changes: `code_diff` on `crates:inkwell` between our version and
  the target. Read the changes to the types and methods `codegen/` uses.

## Bump checklist

Every file that names the LLVM major:

- `crates/sisuc/Cargo.toml`: the inkwell version (or git commit) and its
  `llvm<major>-1` feature, and the comment above it.
- `deny.toml`: the `allow-git` entry, while inkwell is a git dependency.
- `.cargo/config.toml`: the `LLVM_SYS_<major><minor>_PREFIX` name and the
  `.llvm/<major>` path.
- `scripts/install-llvm.sh`: `version`, the three `sha256` values (of the
  release's `LLVM-<version>-Linux-X64.tar.xz`, `-Linux-ARM64.tar.xz` and
  `-macOS-ARM64.tar.xz`) and `dest`. Check that the tar paths still
  exist in the new archive, and that `bin/llc` is still in the macOS one.
- `.github/actions/install-llvm/action.yml`: the cache `path` and the
  description. The cache key hashes the script, so it changes on its own.
- `.cargo/mutants.toml`: the comment naming the prefix variable.
- Docs: `AGENTS.md`, `README.md`, `docs/development.md`, `docs/architecture.md`
  and its pipeline diagrams in `docs/architecture/`, `docs/roadmap.md`,
  `.agents/skills/update-docs/SKILL.md`, and the status table above.

This command finds them; `Cargo.lock` changes through Cargo:

```console
$ rg -l 'llvm2[0-9]|LLVM 2[0-9]|LLVM_SYS_[0-9]+|\.llvm/[0-9]+' --hidden -g '!.git' -g '!target' -g '!.llvm'
```

Then:

1. Run `scripts/install-llvm.sh` and delete the old `.llvm/<major>`.
2. Fix the compile errors from inkwell API changes.
3. Run the checks in `docs/development.md`: `make build`, then
   `make check`.
4. Check the tests that look at optimized IR, such as the `default<O2>` tests in
   `crates/sisuc/tests/cli.rs`. A new LLVM can optimize differently; change
   such a test only once the new output is understood.
5. Read the LLVM release notes for changes to the C API, to the pass pipeline
   names that `codegen/mod.rs` passes to `run_passes`, and to the target defaults.
