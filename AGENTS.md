# Sisu

Sisu is a small, statically typed language compiled to native x86-64 and ARM64 Linux and ARM64 macOS code through LLVM 23. This Cargo workspace holds four crates:

- `crates/sisuc`: the compiler (`sisu`).
- `crates/runtime`: `sisu-runtime`, the runtime's code: an `rlib` with a C ABI.
- `crates/runtime-mimalloc` and `crates/runtime-system`: `sisu-runtime-mimalloc` and `sisu-runtime-system`, the `staticlib`s that link it, on mimalloc and on the System allocator. `sisu build` links one of them into every compiled program (`--allocator`).

## Commands

Run commands through `make`; `make help` lists the targets. `make test` runs nextest, not `cargo test`: nextest's setup script builds the runtime. `make coverage`, `make mutants` and `make verify` measure the changes since `BASE`, by default where HEAD left main; on a stacked branch, pass the stage's base. `make verify` runs coverage, then mutants, and its last line is the verdict; to keep that line in a file, pass `LOG=<file>`, never a shell redirect (`docs/development.md`). `make miri` runs the runtime's unit tests under Miri on nightly, as CI does; run it after a change to `crates/runtime`. Every nextest run writes `target/nextest/<profile>/junit.xml`; CI and the pre-commit hook read it with `scripts/slow_tests.py`, which warns about each test over its time budget (`CODING_STANDARDS.md`, Tests); the hook lists only the tests your change names. A warning for a test you added or changed is a finding: check one with `python3 scripts/slow_tests.py target/nextest/default/junit.xml --diff <base>`.

Once per clone, run `make setup`. It installs LLVM 23 into `.llvm/23`, which `.cargo/config.toml` points `LLVM_SYS_231_PREFIX` at; do not build against a system LLVM. It also enables the Git pre-commit hook. The toolchain is pinned in `rust-toolchain.toml`.

## Code rules

- Lint policy lives in the root `Cargo.toml` under `[workspace.lints]`. Every crate opts in with `[lints] workspace = true`. CI fails on any warning.
- Clippy enforces the code rules (`#[expect]` with a reason, no `unwrap()` outside tests, `// SAFETY:` comments). Fix the code to satisfy a lint; the lint table changes only with the user's agreement.
- Reviews: apply `CODING_STANDARDS.md`.
- Write code to the rules in `CODING_STANDARDS.md` as well as reviewing against them; its Performance section covers allocation and measurement.

## Open-source facts: use GitHits, do not guess

Check every fact about an open-source dependency, tool or GitHub Action (`inkwell`, `llvm-sys`, LLVM, `cargo-nextest`, `cargo-deny`, `prek`, `.github/workflows/`) with the GitHits tools, at the version in `Cargo.lock` or the pinned SHA; the `githits-mcp` skill says which tool to use. Cite what you used and name what you could not verify. If GitHits is unavailable, say so and read the pinned source in `~/.cargo/registry/src/`.

## Agent skills

### Issue tracker

Issues live as local markdown files under `.scratch/<feature>/`. See `docs/agents/issue-tracker.md`.

### Triage labels

Default five-role vocabulary (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`), recorded as `Status:` lines. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: one `GLOSSARY.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.

### Reviews

To have another model review a spec, a plan or a diff, use the `review-sisu` skill. It holds the review brief, the reviewers to launch, and how to rule on their findings.

### Docs

Before you open a pull request, run the `update-docs` skill to bring the docs in line with the code. After a merge, run it only when the user asks or the pull request skipped it. Skills live in `.agents/skills/`, where Codex finds them; `.claude/skills/` symlinks each one for Claude Code. An agent without skill support follows `.agents/skills/update-docs/SKILL.md` directly.
