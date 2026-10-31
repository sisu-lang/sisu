# Executor brief

Copy this file to `.scratch/<batch>/task-<N>-brief.md` and fill every
`<slot>`. Keep each section, even when it is short. For a group on the light
flow, delete steps 5 and 6 of "Your job", and the mutation line of step 7.

```markdown
# Task <N>: <what the task fixes> (<#issues>)

You are the executor. The controller found the root causes; this brief states them. Implement exactly this, with tests. Do not widen the scope. When the code shows that the brief is wrong, stop and report BLOCKED with the evidence.

Worktree: `<absolute worktree path>` (branch `<branch>`). Task base: `<BASE>`. Never `cd` at the top level: use absolute paths or `(cd DIR && cmd)`.

Read first: `AGENTS.md`, `CODING_STANDARDS.md` (Writing, Tests, Diagnostics, Performance), and load the `beautiful-rust:rust-best-practices` skill. Write all prose (diagnostic text, comments, docs, commit message) in ASD-STE100 Simplified Technical English. Test names: `<subject>_should_<result>`.

## <#issue>: <title>

Repro: <Sisu program, command, actual output>.

Cause: <function, file and line at the base, and why it gives this output>.

Fix: <what to change, and where>. Cost: <the cost of each walk it adds, and its worst input>.

Diagnostic text: <the exact message, label, secondary labels and help, as the controller wrote them for a probe with user names>.

Neighbours: <the table of neighbouring cases, with today's output and the wanted output>.

<Repeat this section for each issue of the task.>

## Tests

<The fixtures and unit tests to add, and the docs to update.>

## Your job

1. Implement the fixes and tests above.
2. `nice -n 19 make test` passes. Clippy and fmt are clean.
3. Commit as `Mikko Leppänen <mleppan23@gmail.com>` (`git -C <worktree> -c user.email=mleppan23@gmail.com commit -F <file>`), one commit per Bash call, message in STE. No `Claude-Session` or session-URL line. Then `git log -1` to check that it landed.
4. `nice -n 19 make coverage BASE=<BASE>`: cover or explain each uncovered changed region.
5. After the commit, run exactly one `(cd <worktree> && env CARGO_MUTANTS_JOBS=2 nice -n 19 make mutation-test BASE=<BASE> LOG=<ledger folder>/task-<N>-mutation.log)` with Bash `run_in_background: true`. Write it exactly so, with `env`. Wait for its completion notification; never poll with sleep. Never start a second run. If it is killed for low memory, report DONE_WITH_CONCERNS "mutation test not run".
6. Kill (with a test) or explain each surviving mutant.
7. Stop every background job you started, then report once: DONE, DONE_WITH_CONCERNS or BLOCKED; the commit; the last line of the mutation log; the coverage result for the changed lines; the neighbour outputs before and after; anything you could not do.

Do not push, do not open a pull request, do not merge.
```

## Fix round

A fix-round brief has these parts, in this order:

1. The task and its round base, and a pointer to the task brief, whose rules apply.
2. The rulings file and the reports.
3. Each accepted finding: its severity, the cause you checked, the probe that shows it, and the fix.
4. "Fix the class": the neighbouring cases to list and probe, the probe folder (`review-sisu` names it), and the `scripts/diag_diff.py` command to run.
5. The job: a test that fails without each fix; `make test`; one commit; `make coverage BASE=<round base>`. No mutation test. One report.
