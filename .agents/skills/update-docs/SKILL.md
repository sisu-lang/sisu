---
name: update-docs
description: Use in the Sisu repo before opening a pull request, or when one merged without a docs pass, if a change to sisu, the runtime, CLI flags, language features, the toolchain, the repository layout, or milestone status may have left the README, guides, architecture diagram, roadmap, glossary or ADRs stale.
---

# Update the docs after a change

The docs describe what `main` does. Before a pull request, bring them back
in line with its code. Change only what the diff made untrue or incomplete,
check every claim against the code or a real run, and report what you
changed and what you left alone.

Run this on the pull request's branch before you open it. Run it after a
merge only when the user asks, or when the pull request merged without it.

## 1. Find the change

Run plain git from the repository root, one command per call. Start with
`git fetch origin`, so `origin/main` is current.

Before a pull request, run `git merge-tree --write-tree --name-only origin/main HEAD`.
Exit status 1 means the branch conflicts with `main`, and the output names the
files. Rebase onto `origin/main` first, then document the rebased code: the docs
describe what merges, and a rebase can change it.

- Before a pull request: `git diff --stat origin/main...HEAD`, then
  `git diff origin/main...HEAD -- <path>` for each file that matters.
- For a branch that is not checked out: the same, with `<branch>` for
  `HEAD`. `git show <branch>:<path>` reads a file as that branch has it.
- After a merge: find the merge commit with
  `gh pr view <number> --json mergeCommit`, then
  `git diff <merge-sha>^1 <merge-sha> -- <path>`.

If the pull request exists, read its body with `gh pr view <number>`. It
names what is out of scope and what does not work yet.

## 2. Map the change to the docs

Read each target below before you decide it needs no edit.

| The diff changes | Check and update |
| --- | --- |
| Flags or usage text in `crates/sisuc/src/main.rs` | The "Look inside the compiler" table in `docs/development.md`; the Driver card in `docs/architecture/sisu-pipeline.json` |
| A stage, or what passes between stages: the lexer, parser and AST (`lexer/`, `parser.rs`, `ast.rs`), the checker (`check/`) and `tir.rs`, codegen (`codegen/`, including symbol names and LLVM passes), linking (`link.rs`), diagnostics (`diagnostic/`, `report.rs`, `front_end.rs`); also which stage lowers which sugar | The stage paragraphs in `docs/architecture.md`; the diagram (step 4); the "Compiler" terms in `GLOSSARY.md` |
| Functions exported by `crates/runtime` | The Runtime paragraph in `docs/architecture.md`; the Runtime card in the diagram JSON |
| A language feature added, removed or renamed (a case in `crates/sisuc/tests/run` added or changed) | "What works today" in `docs/roadmap.md`; "A taste of Sisu" in `README.md`, only if a headline feature is missing from its examples; `GLOSSARY.md` for the terms |
| A milestone starting or finishing | The milestone line in `README.md`; the milestone table in `docs/roadmap.md` |
| A choice the pull request body names as hard to reverse, with the alternatives it rejected | A new ADR in `docs/adr/`, numbered after the last one, following `docs/agents/domain.md`; "Decided in milestone N" in `docs/roadmap.md`. Any other choice gets no ADR. |
| Behavior that a "Decided in milestone N" bullet in `docs/roadmap.md` describes | Rewrite the bullet to the current fact and name the milestone that changed it, for example "desugared by the checker (moved from the parser in milestone 2)" |
| Diagnostic or panic text, or an exit code | The sample outputs and exit codes in `docs/development.md` |
| The toolchain or a prerequisite (`rust-toolchain.toml`, `scripts/`, `.cargo/`), a command, how tests run (`crates/sisuc/tests/*.rs`), CI (`.github/workflows/`, `prek.toml`), dependency policy (`deny.toml`) or lint policy (`[workspace.lints]` in `Cargo.toml`) | "Prerequisites" and "Build and test" in `docs/development.md`; "Commands" and "Code rules" in `AGENTS.md`; step 5 of `CONTRIBUTING.md` |
| A top-level directory or crate | "Repository layout" in `docs/development.md`; the crate list in `AGENTS.md` |
| What `sisu` prints, or the text of an explanation in `docs/diagnostics/` (each `.json` summary embeds it) | `make bless`, then read the diff under `site/programs/` |
| The build steps, prerequisites or compiler stages | "Build it" and the pipeline section on Home (`site/src/index.html`) |
| `docs/roadmap.md` | `site/data/status.toml` |
| A diagnostic code added or removed | `site/data/categories.toml` |
| An ADR added | A row on `site/src/decisions.html` |
| Closures landing | Move `site/programs/pending/orders.sisu` to `site/programs/run/`; the pending case says so |
| A stage, the runtime (`crates/runtime`) or `docs/architecture.md` | The Architecture page, `site/src/architecture.html`, and the page's hand-kept commit |
| A command, a prerequisite, CI (`.github/workflows/`), `CONTRIBUTING.md`, `AGENTS.md` or `CODING_STANDARDS.md` | The Contribute page, `site/src/contribute.html`, and "Build it" on Home, and the page's hand-kept commit |
| A language feature added or changed | The Tour's prose and its "What is not here yet" list in `site/src/tour.html`, and the page's hand-kept commit |
| A language feature added or changed | Its page in `docs/reference/`, in the same pull request |
| The prose of `docs/benchmarks.md`, or `scripts/bench.sh` | The Benchmarks prose in `site/src/benchmarks.html`, and the page's hand-kept commit |
| An ADR's status changing, such as proposed to accepted, or a planned one built | Its row's note on `site/src/decisions.html` (the `<small>` or `soon` after its title) and its label in `site/data/status.toml`, such as "ADR 0012 (proposed)", and the page's hand-kept commit |
| You reviewed a hand-written page: Home's hand-written sections, Tour, Architecture, Decisions, Contribute, or Benchmarks prose | The commit written on it: "checked against the repository at commit" at the end of Home's "Built in the open" section and in the meta lines of Tour and Contribute; "Prose checked against the repository at commit" in the Benchmarks meta line; "at commit" in the meta lines of Architecture and Decisions. Generated pages and blocks show `{{commit}}` and need no update. |
| `run/simplify.sisu`, `run/manager.sisu`, `run/shared_arrays.sisu`, `run-fail/factorial.sisu` or `diagnostics/counter.sisu` under `crates/sisuc/tests`, or their expected output | The examples in `README.md`, which must stay identical to them |

If no row matches, report "no doc changes needed" and why. Do not edit to
show activity.

Never write to `docs/superpowers/`. It holds internal specs and plans, is
gitignored, and the docs do not link to it.

## 3. Verify before you write

- Every command and output in the docs comes from a run of the commit you
  document. Before a pull request, that is the branch's checkout. After a
  merge, make a worktree with `git worktree add $TMPDIR/sisu-main <merge-sha>`
  and set `LLVM_SYS_231_PREFIX` to an existing `.llvm/23`, because a new
  worktree has none. Build with `cargo build --workspace` and run that
  checkout's `target/debug/sisu`; a binary from another checkout may come
  from another branch. Never write output from memory or from the diff.
- Write the executables you compile to `$TMPDIR`, not the repository root.
  Keep source paths as the docs show them, because panic messages quote them.
- Confirm each path, flag and function name in the code. Use CodeGraph when
  `.codegraph/` exists, otherwise grep.
- Link to a file that exists on `main` or that this change adds. Write a path
  that only another unmerged branch has as code, not as a link.
- Use the terms in `GLOSSARY.md` as spelled there, for example `tir` and
  poison type.
- A change that contradicts an ADR gets a new ADR that supersedes it. Do not
  edit the old ADR's decision.

## 4. Write

- If the `clarity` skill is available, use it in rewrite mode on each
  paragraph you add or rewrite.
- Match the surrounding text: short sentences, present tense, the same line
  width.
- Describe what the code does now. Status words such as "in progress" or
  "planned" belong only in the README milestone line and the roadmap.
- Keep the README short. A new capability is one bullet, not a section.
- The diagram: edit `docs/architecture/sisu-pipeline.json`, then regenerate
  both SVGs as "Updating the diagram" in `docs/architecture.md` describes.
  The SVGs hold no `cards` text, so an edit to cards alone is done once the
  JSON is.
  The `archify` skill ships the command-line tool at
  `~/.agents/skills/archify/bin/archify.mjs`. Chrome comes from Playwright's
  cache: `ARCHIFY_CHROME=$(printf '%s\n' ~/.cache/ms-playwright/chromium-*/chrome-linux64/chrome | tail -1)`.
  Run `finalize` and `export-svgs.mjs` with the sandbox off, because the
  sandbox blocks Chrome's sockets. Never edit the SVGs by hand. If you cannot
  run archify, update the JSON and report that the SVGs are stale.

## 5. Check

```console
$ python3 .agents/skills/update-docs/scripts/check_links.py README.md CONTRIBUTING.md AGENTS.md GLOSSARY.md CODING_STANDARDS.md docs/*.md docs/adr/*.md docs/agents/*.md
broken: 0
$ for case in 1:run/simplify.sisu 5:run/manager.sisu 7:run/shared_arrays.sisu 9:run-fail/factorial.sisu 13:diagnostics/counter.sisu; do awk -v k="${case%%:*}" '/^```/{n++;next} n==k' README.md | diff - "crates/sisuc/tests/${case#*:}"; done
$ awk '/^```/{n++;next} n==15' README.md | tail -n +2 | diff - crates/sisuc/tests/diagnostics/counter.err
$ make site
$ prek run --files <each changed file>
```

The two `awk` lines print nothing when the README's examples match their
cases. The number before each case is its code block's position: block `k`
is `n==2k-1`. Fix the numbers if a code block is added or moved. The panic
output under the factorial example comes from `run-fail/factorial.out` and
`factorial.err`, with lines 3 to 19 shown as `...`; compare it by eye.

## 6. Report and commit

Report, in this order:

1. Each file you changed, with one line on why.
2. How you verified each new claim: the command you ran, or the code you read.
3. Each row from step 2 you checked and left alone, with the reason.
4. Anything still stale, such as SVGs you could not regenerate.

Before a pull request, commit the doc changes on its branch. After a merge,
put them on a new `docs/<topic>` branch, open a pull request, and remove the
worktree. Never merge it; the user reviews and merges.
