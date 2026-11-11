# The final diff of a branch

Do these steps of `SKILL.md` for the final diff of a branch, and only for it.

## Before the launch

Do these after step 1.4 of `SKILL.md`.

1. For the final diff of a branch, review what will merge. When `git merge-base --is-ancestor origin/main <head>` fails, `main` has moved: bring it in, run `make test` and `make site`, and review the range from `origin/main` to the new head. Rebase onto `origin/main` when the branch's commits replay cleanly. Merge `origin/main` instead when a rebase conflicts in commits that later fix rounds rewrote: run `git rebase --abort` first, then merge, because one merge resolves the conflict against the final code. A new check on `main`, such as a warning, can fail the branch's own tests with no conflict, so `make test` is part of the merge. `review_round.py prepare` stops before it builds when a diff starts on `main` and `origin/main` is not an ancestor of its head. Pass `--old-main` only for the diff of one task.
2. Run the fuzz target when the final diff of a branch changes `crates/sisuc/src/parser.rs` or `crates/sisuc/src/lexer/`. Run `make fuzz SECS=900` (the `front_end` target) on the head, in the branch's worktree, with Bash `run_in_background`. Read its exit status and `fuzz/artifacts/summary.md`. When `make fuzz` succeeds but the summary warns of few new inputs or says that no job finished, run it again with twice the seconds. Each run writes the summary anew. Do not list `fuzz/artifacts/front_end/`: it keeps the inputs of earlier runs. When the summary lists a crash, a timeout or an out-of-memory input, put it in the first round. Give its input path and, for a crash, its replay at the end of `fuzz/artifacts/fuzz.log`. The target needs cargo-fuzz (`make setup`). `docs/development.md` (Fuzz the front end) says how to replay an input.
3. Run the generator gap check when the final diff of a branch changes `crates/sisuc/src`, `crates/sisuc/tests/run` or `crates/sisuc/tests/run-fail`. Run `make fuzz-gaps` on the head, in the branch's worktree, with Bash `run_in_background`, after step 2. Read the section `## Generator gaps` that it prints. Its exit status tells only pass from fail, because `make` returns 2 for each failure. A section that says valid programs reach functions that no seed reaches names a new gap. Add its generator rule or its known key before you launch the reviewers (`docs/development.md`, Add a fixed form for a generator gap). A section that says the check did not finish, or no section, means that the measurement failed, so fix that first. A warning names a stale key, so remove the key from `fuzz/known_gaps.txt`.
4. When the branch has a pull request body, pass its file as one more `--doc` in step 2 of `SKILL.md`. The Focus asks the reviewers to check the title and the body against `CODING_STANDARDS.md` (Writing).

## A large fix round

Some flows run mutation tests. In the final review of such a flow, a fix round can change over 50 Rust lines outside the tests, such as a new algorithm. Then it runs one mutation test on its head before the re-review. The stage's or group's run came before that round, so no line is mutated twice. `git diff --shortstat <round base> HEAD -- '*.rs' ':!**/tests/**' ':!**/tests.rs'` gives the count. Run this with Bash `run_in_background`:

```
(cd <worktree> && env CARGO_MUTANTS_JOBS=2 nice -n 19 make mutation-test BASE=<round base> LOG=RUN/mutation-<round>.log)
```

Put each surviving mutant into the round, with a test that kills it or a reason it is equivalent. Pass the log as another `--mutation-log`.
