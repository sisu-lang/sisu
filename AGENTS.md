# Sisu

Sisu is a small, statically typed language compiled to native x86-64 and ARM64 Linux and ARM64 macOS code through LLVM 23. This Cargo workspace holds four crates:

- `crates/sisuc`: the compiler (`sisu`).
- `crates/runtime`: `sisu-runtime`, the runtime's code: an `rlib` with a C ABI.
- `crates/runtime-mimalloc` and `crates/runtime-system`: `sisu-runtime-mimalloc` and `sisu-runtime-system`, the `staticlib`s that link it, on mimalloc and on the System allocator. `sisu build` links one of them into every compiled program (`--allocator`).

## Commands

Run commands through `make`; `make help` lists the targets. `make test` runs nextest, not `cargo test`: nextest's setup script builds the runtime. `make coverage` and `make mutation-test` measure the changes since `BASE`, by default where HEAD left main; on a stacked branch, pass the stage's base. `make mutation-test BASE=<base>` mutates each changed line and runs the whole test suite once for each mutant. A run can take an hour or more and loads every core. Run it only when the user or your instructions ask for it; it is not a default step. Its last line is the summary; to keep that line in a file, pass `LOG=<file>`, never a shell redirect (`docs/development.md`). `make miri` runs the runtime's unit tests under Miri on nightly, as CI does; run it after a change to `crates/runtime`. Every nextest run writes `target/nextest/<profile>/junit.xml`; CI and the pre-commit hook read it with `scripts/slow_tests.py`, which warns about each test over its time budget (`CODING_STANDARDS.md`, Tests); the hook lists only the tests your change names. Neither fails: wall-clock time depends on the machine. After the tests, `make test` fails on a filter in `.config/nextest.toml` that matches no test. A CI warning for a test you added or changed is a finding. The budget is for a CI runner, so a local time is only a hint: see one with `python3 scripts/slow_tests.py target/nextest/default/junit.xml --diff <base>`.

Once per clone, run `make setup`. It installs LLVM 23 into `.llvm/23`, which `.cargo/config.toml` points `LLVM_SYS_231_PREFIX` at; do not build against a system LLVM. It also enables the Git pre-commit and commit-msg hooks. The toolchain is pinned in `rust-toolchain.toml`.

## Code rules

- Lint policy lives in the root `Cargo.toml` under `[workspace.lints]`. Every crate opts in with `[lints] workspace = true`. CI fails on any warning.
- Clippy enforces the code rules (`#[expect]` with a reason, no `unwrap()` outside tests, `// SAFETY:` comments). Fix the code to satisfy a lint; the lint table changes only with the user's agreement.
- Reviews: apply `CODING_STANDARDS.md`.
- Write code to the rules in `CODING_STANDARDS.md` as well as reviewing against them; its Performance section covers allocation and measurement.
- Before you write, refactor or review Rust, load the `rust-best-practices` skill from [BeautifulRust](https://github.com/morganlinton/BeautifulRust) and follow it. Claude Code installs it with `/plugin marketplace add morganlinton/BeautifulRust`, then `/plugin install beautiful-rust@beautiful-rust`. An agent without plugin support reads `skills/rust-best-practices/SKILL.md` in that repository. When the skill and this file, `CODING_STANDARDS.md` or the lint table do not agree, the Sisu rules apply.

## Writing

Write all prose in ASD-STE100 Simplified Technical English: diagnostics, comments, docs, commit messages, pull requests and GitHub issues. `CODING_STANDARDS.md` (Writing) has the rules.

## Specs, plans and tests

A spec states its acceptance tests as Gherkin scenarios with IDs, and a plan maps each scenario to a task and a test. A test name says what should hold: `<subject>_should_<result>`. An end-to-end test has GIVEN, WHEN and THEN lines. `CODING_STANDARDS.md` (Tests, Specs and plans) has the rules.

## Open-source facts: use GitHits, do not guess

Check every fact about an open-source dependency, tool or GitHub Action (`inkwell`, `llvm-sys`, LLVM, `cargo-nextest`, `cargo-deny`, `prek`, `.github/workflows/`) with the GitHits tools, at the version in `Cargo.lock` or the pinned SHA; the `githits-mcp` skill says which tool to use. Cite what you used and name what you could not verify. If GitHits is unavailable, say so and read the pinned source in `~/.cargo/registry/src/`.

## Agent skills

### Issue tracker

Issues are GitHub issues on `sisu-lang/sisu`, made with `gh api`. When GitHub cannot be reached, write a fallback issue under `.scratch/<feature>/issues/`. Specs, plans and review notes are private and never go into an issue. See `docs/agents/issue-tracker.md`.

### Triage labels

Default five-role vocabulary (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`), as GitHub labels, plus kind, area and severity labels. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: one `GLOSSARY.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.

### Reviews

To have another model review a spec, a plan or a diff, use the `review-sisu` skill. It holds the review brief, the reviewers to launch, and how to rule on their findings.

### Docs

Before you open a pull request, run the `update-docs` skill to bring the docs in line with the code. After a merge, run it only when the user asks or the pull request skipped it. Skills live in `.agents/skills/`, where Codex finds them; `.claude/skills/` symlinks each one for Claude Code. An agent without skill support follows `.agents/skills/update-docs/SKILL.md` directly.
