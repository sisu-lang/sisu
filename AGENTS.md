# Sisu

Sisu is a small, statically typed language compiled to native x86-64 and ARM64 Linux and ARM64 macOS code through LLVM 23. This Cargo workspace holds four crates:

- `crates/sisuc`: the compiler (`sisu`).
- `crates/runtime`: `sisu-runtime`, the runtime's code: an `rlib` with a C ABI.
- `crates/runtime-mimalloc` and `crates/runtime-system`: `sisu-runtime-mimalloc` and `sisu-runtime-system`, the `staticlib`s that link it, on mimalloc and on the System allocator. `sisu build` links one of them into every compiled program (`--allocator`).

The workspace also holds `selfhost/`: `lexer.sisu`, Sisu's lexer written in Sisu, the growing proof program. The tests check it against `sisu build --emit tokens`, so a change to the lexer's rules changes it in the same pull request (`docs/development.md`).

## Commands

Run commands through `make`; `make help` lists every target (`docs/development.md`, Build and test). `make test` runs nextest, not `cargo test`, and skips `make form-matrix` and `make memory-safety`. Run `make form-matrix` after front-end changes, `make memory-safety` after codegen, drop, runtime or harness changes, and `make miri` after `crates/runtime` changes. Run `make coverage` on changes since `BASE` (`docs/development.md`, Coverage and performance; `CODING_STANDARDS.md`, Tests). Run `make mutation-test` only when asked; quote only `make mutation-status` for progress. Heavy targets share one clone lock (`scripts/heavy-run.sh --status`).

Wait for pull request reviews and checks only with `python3 scripts/watch_pr.py <N>`, with the sandbox off and `run_in_background`; run it again after each event (a `Monitor` watch wakes a full turn for each check). Use `python3 scripts/pr_threads.py <N>` (sandbox off) to list review threads and comment ids.

For compile speed and scaling sweeps, use `make compile-speed` and `make scaling` (`docs/benchmarks.md`). Run `make setup` once per clone to install LLVM 23 into `.llvm/23` (`.cargo/config.toml` points `LLVM_SYS_231_PREFIX` at it; do not build against system LLVM) and enable Git hooks (`docs/development.md`, Set up a clone).

## Code rules

- Lint policy lives in the root `Cargo.toml` under `[workspace.lints]`. Every crate opts in with `[lints] workspace = true`. CI fails on any warning.
- Fix code to satisfy Clippy (`#[expect]` with a reason, no `unwrap()` outside tests, `// SAFETY:` comments). For a shape lint such as `clippy::struct_excessive_bools` or `clippy::too_many_lines`, keep a right shape and add `#[expect(<lint>, reason = "...")]`. The lint table changes only with the user's agreement.
- Python: `ruff.toml` and `pyrefly.toml` hold the rules (`CODING_STANDARDS.md`, Python).
- Write and review code against `CODING_STANDARDS.md`; its Performance section covers allocation and measurement.
- Before you write, refactor or review Rust, load the `rust-best-practices` skill from [BeautifulRust](https://github.com/morganlinton/BeautifulRust). An agent without plugin support reads `skills/rust-best-practices/SKILL.md` in that repository. When that skill and this file, `CODING_STANDARDS.md` or the lint table disagree, Sisu's rules apply.
- When `.codegraph/` exists, call `codegraph_explore` (or `codegraph explore "<query>"`) before grepping to trace callers and readers.

## Writing

Write all prose in ASD-STE100 Simplified Technical English: diagnostics, comments, docs, commit messages, pull requests and GitHub issues. `CODING_STANDARDS.md` (Writing) has the rules.

When you show Sisu code to the user, fence it as `rust`, which highlighters know, and put compiler output in a plain fence. When the user must decide, ask in your reply, not only in a question dialog. Say the problem, and show example programs with today's output and the wanted output. Then give the root cause, each proposal with its output and its cost, and your recommendation. Then wait for the answer.

## Specs, plans and tests

Follow `CODING_STANDARDS.md` (Tests, Specs and plans) for Gherkin scenarios, `<subject>_should_<result>` names, and GIVEN, WHEN and THEN lines.

## Open-source facts: use GitHits, do not guess

Check every fact about an open-source dependency, tool or GitHub Action (`inkwell`, `llvm-sys`, LLVM, `cargo-nextest`, `cargo-deny`, `prek`, `.github/workflows/`) with the GitHits tools, at the version in `Cargo.lock` or the pinned SHA; the `githits-mcp` skill says which tool to use. Cite what you used and name what you could not verify. If GitHits is unavailable, say so and read the pinned source in `~/.cargo/registry/src/`.

## Agent skills

### Issue tracker

Issues are GitHub issues on `sisu-lang/sisu`, made with `gh api`. When GitHub cannot be reached, write a fallback issue under `.scratch/<feature>/issues/`. Specs, plans and review notes are private and never go into an issue. See `docs/agents/issue-tracker.md`.

### Triage labels

Default five-role vocabulary (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`), as GitHub labels, plus kind, area and severity labels. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: one `GLOSSARY.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`. `docs/register.md` holds rulings that are not built yet, and the known structural debt. Read it before you write or review a spec, a plan or a diff.

### Issues

To work through the open GitHub issues, use the `solve-issues` skill. It picks and groups the issues, finds each root cause, hands each task to an executor, and keeps the batch ledger in `.scratch/<batch>/progress.md`.

### Reviews

To have another model review a spec, a plan or a diff, use the `review-sisu` skill. This includes the review of each task in a subagent-driven plan: use the brief of `review-sisu`, not the task-reviewer template of that plan skill. The `review-sisu` skill holds the review brief, the reviewers to launch, and how to rule on their findings.

### Docs

Before you open a pull request, run the `update-docs` skill to bring the docs in line with the code. After a merge, run it only when the user asks or the pull request skipped it. Skills live in `.agents/skills/`, where Codex finds them; `.claude/skills/` symlinks each one for Claude Code. An agent without skill support follows `.agents/skills/update-docs/SKILL.md` directly.

### Comments

To audit or delete the comments of a scope, use the `delete-comment` skill.

### Retro

After the final review of a branch, or before the pull request when the branch had no review, run the `kaizen` skill (`.agents/skills/kaizen/SKILL.md`).
