# Executor brief

Copy this file to `.scratch/<batch>/<group>-task-<N>-brief.md`. Fill every
`<slot>`. Keep each section, even when it is short. Write
`<ledger folder>` as an absolute path: `<repo>/.scratch/<batch>`.

```markdown
# Group <group>, task <N>: <what the task fixes> (<#issues>)

You are the executor. The controller found the root causes; this brief states them. Implement exactly this, with tests. Do not widen the scope. When the code shows that the brief is wrong, stop. Report BLOCKED with the evidence.

Worktree: `<absolute worktree path>` (branch `<branch>`). Task base: `<BASE>`. Never `cd` at the top level. Use absolute paths or `(cd DIR && cmd)`.

Read first: `AGENTS.md` and `CODING_STANDARDS.md` (Writing, Tests, Diagnostics, Performance). Load the `beautiful-rust:rust-best-practices` skill. Write all prose in ASD-STE100 Simplified Technical English. Test names: `<subject>_should_<result>`.

## <#issue>: <title>

Repro: <Sisu program, command, actual output>.

Cause: <function, file and line at the base, and why it gives this output>.

Fix: <what to change, and where>.

Cost: <each walk the fix adds, its worst input, and the step counter that guards it>. The counter ticks in the one accessor that reads the data, so a scan anywhere ticks. Its proof is an un-ticked mutation (`CODING_STANDARDS.md`, Tests): put back the old scan, add no tick, and show that the test fails. A fix that adds or changes a walk names a counter test for each shape of `SHAPES` (`scripts/scaling.py`) that reaches it. Use shapes such as "else-if chain with a result type" or "alias chain".

Diagnostic text: <the message, label, secondary labels and help; each fix with its message, edit, and safe or unsafe; as the controller wrote them for a probe with user names>.

Neighbours: <the table of neighbouring cases, with today's output and the wanted output>.

<Repeat this section for each issue of the task.>

## Tests

<The test list. One row for each case of the "Neighbours" tables, in both orders and at each depth, with its wanted output, and the fixture or unit test that holds it. One unit test that pins each cause. The docs to update. A fuzz input that panicked is a row of `front_end_should_pass_on_an_input_it_failed_on`, not a new test (`docs/development.md`, Fuzz the front end). Probe each kind of member that a new doc rule sentence lists (a literal, `None`, an array, a call, a struct, an enum value), in both orders, with and without an expected type. Add a test of the members in each order, with a generic struct leaf and an enum leaf.>

The wanted outputs come from this list. Do not change one to fit the code. Keep failing rows and must-pass rows in separate tables or loops so the red-at-base check stays clear; each must-pass row keeps its `, unchanged` label. A task that changes only docs or the text of a diagnostic has no test commit: its fixture diff is the test. When a test in a behaviour task wants new diagnostic text, a bless at the base writes the old text, and the fixture is green. So bless, then edit each wanted line of the `.err` and the `.json` by hand, and check that the fixture fails at the base on those lines only. The test commit of a performance fix can add the `cfg(test)` counter and its ticks.

Unit tests assert diagnostic codes, spans and fix edits, never wording (`test-wording` hook). Pin exact wording in `.err` and `.json` fixtures. Keep each rule in its own test function or table (`CODING_STANDARDS.md`, Tests).

## Your job

1. Write the tests of the "Tests" list first. Run only these tests at the task base (`cargo nextest run -E 'test(<name>)'`; for a `memory::` test, add `--profile memory`, see `docs/development.md`, Build and test). List each test that is red, and why. Write the tests as their own commit (step 4 says how). Its title starts with "Add failing tests", and its body gives the red list. `make test` fails at this commit. A task whose tests are in `scripts/test_*.py` has no separate test commit, because the `python-tests` pre-commit hook rejects a red test. Its tests go into the fix commit, and that message gives the red list. A task with no test commit ("Tests" above) skips this step.
2. Implement the fixes. Make each red test green with the smallest change at the cause. When the fix does not make a test green, that is a finding: report it. Do not change the test or its wanted output.
3. Run `(cd <worktree> && nice -n 19 make test)`. It must pass. Clippy and fmt must be clean. After a change to the front end or the form matrix files (`docs/development.md`, Build and test), also run `(cd <worktree> && nice -n 19 make form-matrix)`: `make test` leaves it out. When it prints `heavy-run: waiting for this run to end: make mutation-test ...`, stop it and run `(cd <worktree> && SISU_HEAVY_LOCK=skip nice -n 19 make test)`. Say so in the report.
4. Write the fix as a commit, after the test commit when there is one. Before you save the patch of commit 1, run `(cd <worktree> && make lint && git ls-files -z --others --exclude-standard | xargs -0 -r prek run --files)`. `text-limits` and `test-names` see a new file only when the controller commits it. Your sandbox cannot write the git index, so the controller commits. For commit `<n>`, write the message in STE to `<ledger folder>/<group>-task-<N>-commit-<n>.txt`. Add no `Claude-Session` or session-URL line. When the changes of the commit are complete, write a patch of all the changes since `HEAD`, new files included:

   ```
   (cd <worktree> && { git diff --binary HEAD; git ls-files --others --exclude-standard -z | xargs -0 -r -n1 git diff --binary --no-index /dev/null || true; } > <ledger folder>/<group>-task-<N>-commit-<n>.patch)
   ```

   Each patch holds the changes of the earlier commits too, so a test and its fix can be in one file. Leave all the changes in the worktree.
5. Run no `make coverage` and no `make mutation-test`. The controller runs `make coverage` on the clean `HEAD` after committing and checks uncovered lines.
6. Stop every background job you started. Then report once:
   - the status: DONE, DONE_WITH_CONCERNS or BLOCKED;
   - each commit: its message file and its patch;
   - each test: red at the base, green at the head;
   - the neighbour outputs before and after;
   - anything you could not do.

Do not push, do not open a pull request, do not merge.
```

## Fix round

A fix-round brief has these parts, in this order:

1. The group, the task and its round base. A pointer to the task brief, whose rules apply.
2. The rulings file and the reports.
3. Each accepted finding: its severity, the cause you checked, the probe that shows it, the fix, and the test list with the wanted outputs, as in "Tests" above.
4. "Fix the class": the neighbouring cases to list and probe, the probe folder (`review-sisu` names it), and the `scripts/diag_diff.py` command to run. When the fix gives a sentence for a doc, each example in it is one case with its probe; a probe that contradicts the sentence changes the sentence.
5. "Report, do not add": when a check (a hook, a lint, a test table) seems to need a new exemption or opt-out, the executor reports the hit and the reason, and adds nothing. The controller rules on it.
6. The job: the tests first, red at the round base, as one commit titled "Add failing tests"; then the fix as a second commit. Steps 1 and 4 of the task brief say how to write each commit, and when a fix has no separate test commit. Then run `make test`. Run no `make coverage` and no `make mutation-test`: the controller runs `make coverage` on the clean `HEAD` and checks uncovered lines. One report, with each test red before and green after. A test that kills a surviving mutant, or that holds the rule of code a ruling kept (`review-sisu`, rule 6), is the exception: it is green at the round base and needs no fix. Write it as its own commit, and name its mutant or its rule in the report. A surviving mutant whose only effect is extra work that stays linear is not pinned. It gets the note `accepted: extra work only: <the work>` (`CODING_STANDARDS.md`, Tests).
