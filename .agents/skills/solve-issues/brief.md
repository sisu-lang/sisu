# Executor brief

Copy this file to `.scratch/<batch>/<group>-task-<N>-brief.md`. Fill every
`<slot>`. Keep each section, even when it is short. Write `<author>` as the
git identity that the user's rules give for this repository. Write
`<ledger folder>` as an absolute path: `<repo>/.scratch/<batch>`.

For a group on the light flow, delete step 6 of "Your job", and the mutation
line of step 7.

```markdown
# Group <group>, task <N>: <what the task fixes> (<#issues>)

You are the executor. The controller found the root causes; this brief states them. Implement exactly this, with tests. Do not widen the scope. When the code shows that the brief is wrong, stop. Report BLOCKED with the evidence.

Worktree: `<absolute worktree path>` (branch `<branch>`). Task base: `<BASE>`. Never `cd` at the top level. Use absolute paths or `(cd DIR && cmd)`.

Read first: `AGENTS.md` and `CODING_STANDARDS.md` (Writing, Tests, Diagnostics, Performance). Load the `beautiful-rust:rust-best-practices` skill. Write all prose in ASD-STE100 Simplified Technical English. Test names: `<subject>_should_<result>`.

## <#issue>: <title>

Repro: <Sisu program, command, actual output>.

Cause: <function, file and line at the base, and why it gives this output>.

Fix: <what to change, and where>.

Cost: <each walk the fix adds, its worst input, and the step counter that guards it>. The counter ticks in the one accessor that reads the data, so a scan anywhere ticks. Its proof is an un-ticked mutation (`CODING_STANDARDS.md`, Tests): put back the old scan, add no tick, and show that the test fails.

Diagnostic text: <the message, label, secondary labels and help; each fix with its message, edit, and safe or unsafe; as the controller wrote them for a probe with user names>.

Neighbours: <the table of neighbouring cases, with today's output and the wanted output>.

<Repeat this section for each issue of the task.>

## Tests

<The test list. One row for each case of the "Neighbours" tables, in both orders and at each depth, with its wanted output, and the fixture or unit test that holds it. One unit test that pins each cause. The docs to update.>

The wanted outputs come from this list. Do not change one to fit the code. A task that changes only docs or the text of a diagnostic has no test commit: its fixture diff is the test. When a test in a behaviour task wants new diagnostic text, a bless at the base writes the old text, and the fixture is green. So bless, then edit each wanted line of the `.err` and the `.json` by hand, and check that the fixture fails at the base on those lines only. The test commit of a performance fix can add the `cfg(test)` counter and its ticks.

## Your job

1. Write the tests of the "Tests" list first. Run only these tests at the task base (`cargo nextest run -E 'test(<name>)'`). List each test that is red, and why. Commit the tests alone. The title starts with "Add failing tests", and the body gives the red list. `make test` fails at this commit. A task with no test commit ("Tests" above) skips this step.
2. Implement the fixes. Make each red test green with the smallest change at the cause. When the fix does not make a test green, that is a finding: report it. Do not change the test or its wanted output.
3. Run `(cd <worktree> && nice -n 19 make test)`. It must pass. Clippy and fmt must be clean. When it prints `heavy-run: waiting for this run to end: make mutation-test ...`, stop it and run `(cd <worktree> && SISU_HEAVY_LOCK=skip nice -n 19 make test)`, and the same for `make coverage`. Say so in the report.
4. Commit the fix, after the test commit when there is one. Commit as `<author>`. Use `git -C <worktree> commit -F <file>`, one commit per Bash call. Write the message in STE. Add no `Claude-Session` or session-URL line. Then run `git log -1` to check that it landed.
5. Run `(cd <worktree> && nice -n 19 make coverage BASE=<BASE> > <ledger folder>/<group>-task-<N>-coverage.txt 2>&1)`. Cover or explain each uncovered changed region.
6. Run no mutation test: the controller runs one for the whole group, after the reviews.
7. Stop every background job you started. Then report once:
   - the status: DONE, DONE_WITH_CONCERNS or BLOCKED;
   - each commit;
   - each test: red at the base, green at the head;
   - the coverage result for the changed lines;
   - the neighbour outputs before and after;
   - anything you could not do.

Do not push, do not open a pull request, do not merge.
```

## Fix round

A fix-round brief has these parts, in this order:

1. The group, the task and its round base. A pointer to the task brief, whose rules apply.
2. The rulings file and the reports.
3. Each accepted finding: its severity, the cause you checked, the probe that shows it, the fix, and the test list with the wanted outputs, as in "Tests" above.
4. "Fix the class": the neighbouring cases to list and probe, the probe folder (`review-sisu` names it), and the `scripts/diag_diff.py` command to run.
5. "Report, do not add": when a check (a hook, a lint, a test table) seems to need a new exemption or opt-out, the executor reports the hit and the reason, and adds nothing. The controller rules on it.
6. The job: the tests first, red at the round base, in one commit titled "Add failing tests"; then the fix in a second commit; `make test`; `make coverage BASE=<round base>` into `<ledger folder>/<group>-task-<N>-fix<round>-coverage.txt`. No mutation test. One report, with each test red before and green after. A test that kills a surviving mutant is the exception: it is green at the round base and needs no fix. Commit it on its own, and name its mutant in the report.
