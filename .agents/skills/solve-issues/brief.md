# Executor brief

Copy this file to `.scratch/<batch>/<group>-task-<N>-brief.md`. Fill every
`<slot>`. Keep each section, even when it is short. Write `<author>` as the
git identity that the user's rules give for this repository. Write
`<ledger folder>` as an absolute path: `<repo>/.scratch/<batch>`.

For a group on the light flow, delete step 5 of "Your job", and the mutation
line of step 6.

```markdown
# Group <group>, task <N>: <what the task fixes> (<#issues>)

You are the executor. The controller found the root causes; this brief states them. Implement exactly this, with tests. Do not widen the scope. When the code shows that the brief is wrong, stop. Report BLOCKED with the evidence.

Worktree: `<absolute worktree path>` (branch `<branch>`). Task base: `<BASE>`. Never `cd` at the top level. Use absolute paths or `(cd DIR && cmd)`.

Read first: `AGENTS.md` and `CODING_STANDARDS.md` (Writing, Tests, Diagnostics, Performance). Load the `beautiful-rust:rust-best-practices` skill. Write all prose in ASD-STE100 Simplified Technical English. Test names: `<subject>_should_<result>`.

## <#issue>: <title>

Repro: <Sisu program, command, actual output>.

Cause: <function, file and line at the base, and why it gives this output>.

Fix: <what to change, and where>.

Cost: <each walk the fix adds, its worst input, and the step counter that guards it>.

Diagnostic text: <the message, label, secondary labels and help; each fix with its message, edit, and safe or unsafe; as the controller wrote them for a probe with user names>.

Neighbours: <the table of neighbouring cases, with today's output and the wanted output>.

<Repeat this section for each issue of the task.>

## Tests

<The fixtures and unit tests to add, and the docs to update.>

## Your job

1. Implement the fixes and tests above.
2. Run `(cd <worktree> && nice -n 19 make test)`. It must pass. Clippy and fmt must be clean.
3. Commit as `<author>`. Use `git -C <worktree> commit -F <file>`, one commit per Bash call. Write the message in STE. Add no `Claude-Session` or session-URL line. Then run `git log -1` to check that it landed.
4. Run `(cd <worktree> && nice -n 19 make coverage BASE=<BASE> > <ledger folder>/<group>-task-<N>-coverage.txt 2>&1)`. Cover or explain each uncovered changed region.
5. Run no mutation test: the controller runs one for the whole group, after the reviews.
6. Stop every background job you started. Then report once:
   - the status: DONE, DONE_WITH_CONCERNS or BLOCKED;
   - each commit;
   - the coverage result for the changed lines;
   - the neighbour outputs before and after;
   - anything you could not do.

Do not push, do not open a pull request, do not merge.
```

## Fix round

A fix-round brief has these parts, in this order:

1. The group, the task and its round base. A pointer to the task brief, whose rules apply.
2. The rulings file and the reports.
3. Each accepted finding: its severity, the cause you checked, the probe that shows it, and the fix.
4. "Fix the class": the neighbouring cases to list and probe, the probe folder (`review-sisu` names it), and the `scripts/diag_diff.py` command to run.
5. The job: a test that fails without each fix; `make test`; one commit; `make coverage BASE=<round base>` into `<ledger folder>/<group>-task-<N>-fix<round>-coverage.txt`. No mutation test. One report.
