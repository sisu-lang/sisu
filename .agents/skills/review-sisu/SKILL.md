---
name: review-sisu
description: Use in the Sisu repo when a spec, an implementation plan or a code diff, such as a task's diff or a branch's whole diff before a pull request, needs a review from another model, or when the user asks for a cross-model, Sol or final review.
---

# Review Sisu work

Another model reviews one target against Sisu's criteria. You, the caller,
write its brief from [brief.md](brief.md), launch it, rule on each finding,
fix the findings you accept, and report to the user. The reviewer only
reads.

## Target

The caller names one target. Infer its kind; ask when it is unclear.

| Kind | Given as | The reviewer also reads |
| --- | --- | --- |
| spec | the path of a design document | nothing more |
| plan | the path of an implementation plan, a document of numbered tasks | the spec it implements, which the plan names |
| diff | a range `<base>..<head>` or a branch, against `origin/main` by default | the spec and plan, when the caller names them |

## Steps

### 1. Set up

1. The repository root is `git rev-parse --show-toplevel` in the checkout that holds the target, which may be a worktree.
2. `mktemp -d "${TMPDIR:-/tmp}/sisu-review-<name>-XXXX"` makes the run directory, `RUN`, and `mkdir RUN/opus-scratch` makes the Opus reviewer's scratch directory. `<name>` says what is under review, such as `m8p5-task3` or `attributes-final`. Prompts and reports stay there, outside the repository. Shell variables do not survive between tool calls, so write `RUN` as a literal path.
3. Delete the build directories that have not changed for a day, because a run that never reaches step 6 keeps its builds: `find "${TMPDIR:-/tmp}"/sisu-review-*/ -mindepth 1 -maxdepth 1 -type d -mtime +0 -exec rm -rf {} +`.
4. Run `git fetch origin` before every review: the brief and its script come from `origin/main`, so a branch that started before a change to the criteria gets the current ones.
5. For the final diff of a branch, review what will merge. When `git merge-base --is-ancestor origin/main <head>` fails, `main` has moved: bring it in, run `make test` and `make site`, and review the range from `origin/main` to the new head. Rebase onto `origin/main` when the branch's commits replay cleanly. Merge `origin/main` instead when a rebase conflicts in commits that later fix rounds rewrote: one merge resolves the conflict against the final code. A new check on `main`, such as a warning, can fail the branch's own tests with no conflict, so `make test` is part of the merge.

### 2. Write the brief

Write the Focus to `RUN/focus.md`: what the caller knows that the target
does not say, such as a task's done condition, a risk, a question to
answer, or earlier rulings. List the deferred findings and open issues
that the target touches, so that no reviewer reports them again. Do not
summarize the target; the reviewer reads it.

Then let the script write the brief, for every kind. Run `origin/main`'s
copy of it, because a branch's own copy can be older than the template:

```
git -C <repository root> show origin/main:.agents/skills/review-sisu/scripts/diff_brief.py |
  python3 - <repository root> <target> RUN/brief.md --kind <spec|plan|diff> --doc <spec or plan> --focus RUN/focus.md
```

- `<target>` is the path of a spec or a plan, or `<base>..<head>` for a diff. Write a branch as `origin/main..<branch>`. The script writes a diff's range as full SHAs, and copies a spec or a plan to `RUN/target-<round>.md`, which the reviewer reads.
- Repeat `--doc` for each document the reviewer reads with the target; leave it out when there is none.
- For a diff, pass the `LOG=` file of a `make mutation-test` run as `--mutation-log`, and the output of `make coverage BASE=<base> > RUN/coverage.txt` as `--coverage`. Leave out each one that did not run. The script stops when the log measured uncommitted changes or a commit outside the range. A log from an earlier commit of the range is correct: a task runs mutation tests once, and the brief lists the commits after it.
- The script drops a skill line it cannot find. Tell the user in the report.

### 3. Launch the reviewers

| Review | First round | Later rounds |
| --- | --- | --- |
| spec or plan | `gpt-6.1-sol` medium, `opus-high` | both |
| the diff of one task | `gpt-6.1-sol` medium, `opus-medium` | `opus-medium`; it also checks the fixes for Sol's findings |
| the final diff of a branch or a milestone | `gpt-6.1-sol` medium, `opus-high` | both |

The user or the caller's plan may change an effort or add a reviewer.
Launch the reviewers together. Each launcher must let its reviewer read
`RUN` and the skill paths in the brief.

- **Sol:** `sol-review <effort> RUN/brief.md RUN/sol.md <repository root>`, through Bash with `run_in_background` and the sandbox off: codex needs the network and writes under `~/.codex`. The completion notification ends the wait; do not poll. When it exits 1, show the user the error it prints and ask how to go on. Do not put another model in Sol's place without asking.
- **Opus:** the Agent tool with the `subagent_type` from the table, and the prompt `Read RUN/brief.md and follow it. Your scratch directory is RUN/opus-scratch. Write your report to RUN/opus.md; your final reply is that report.` with `RUN` written out, and `opus-<round>.md` in a later round. Pass no `model` or `effort`.

### 4. Rule on each finding

A report without a `Verdict:` line, or one that says it could not read
the brief, is a failed run, not a review: tell the user and ask how to go
on.

Give every finding one ruling, and write each ruling as one line of
`RUN/rulings-<round>.md`:
`<reviewer> <n>: Accept | Reject | Defer to <where>: <reason>`. When two
reviewers report one defect, give both lines the same ruling and name the
other line.

- **Accept:** you will fix it.
- **Reject:** with a reason that cites the target, the code, or `CODING_STANDARDS.md`.
- **Defer:** with where it goes: a later task, a roadmap entry, or a GitHub issue per `docs/agents/issue-tracker.md`.

A diagnostic improvement (`CODING_STANDARDS.md`, Reviewing) is deferred without a fix round: add one line with its repro to the plan workspace's `diagnostic-issues.md`, or to `RUN/diagnostic-issues.md` outside a plan. After the pull request merges, run each repro again on `main` and file one issue per diagnostic code.

Rules for ruling:

1. Check a finding before you rule. Run its repro or read the source it cites. When the claim does not hold, reject it and say what you saw.
2. Severity follows "Reviewing" in `CODING_STANDARDS.md`. A blocking finding is fixed or deferred with the user's agreement; a nit may be rejected.
3. A ruling must not break a rule in `CODING_STANDARDS.md`.
4. When two reviewers disagree, rule on the evidence and record the disagreement.
5. Ask the user instead of ruling when the fix would change the spec's intent, syntax, a diagnostic code, a public interface or the scope, or when you would reject a blocking finding.

### 5. Fix and review again

Fix the accepted findings in the target. Fix the class, not only the repro: for each finding, list the neighbouring cases (the same rule in each position the language allows it), try each one, and fix the cause so that all of them are right. A brief for a fix round asks for that list and its results. A fix to code keeps `make test`
passing, and a fix that changes behavior adds a test that fails without
it.

When you fixed a blocking finding, review again. Write `RUN/focus-<round>.md`
with the fixes and what you probed, and run the script with
`RUN/brief-<round>.md`, the round's target (for a diff, the fix range),
`--round <round> --rulings RUN/rulings-<round - 1>.md`, and `--prior <report>` once for each earlier report.
Its "Re-review" section asks for a verdict on each accepted finding, the
neighbours of each fix, and regressions against the base of the whole
work. Send it to the later-round reviewers in the table; their reports
go to `RUN/sol-<round>.md` and `RUN/opus-<round>.md`. Allow at most two
fix rounds. After the review of the second fix round, ask the user about
each blocking finding that is still open: a third round, or a deferral.
Defer the nits.

### 6. Report

Tell the user:

1. which reviewers ran, with model and effort, and the `RUN` path;
2. one line per finding: its title, its severity, your ruling, and the reason;
3. the fixes you made, and where each deferred finding went;
4. the questions that wait for the user.

Then delete everything in `RUN` except the briefs and reports, after the
last round: `find RUN -mindepth 1 -maxdepth 1 -type d -exec rm -rf {} +`.
Reviewers build there, about 7 GB a build, and nothing else removes it.

## Common mistakes

| Mistake | Instead |
| --- | --- |
| The brief restates the target, so the reviewer reviews your summary | Name the target; the reviewer reads it |
| Every finding is accepted, so wrong claims become code | Check each claim before you rule |
| A reviewer that failed is replaced by another model in silence | Tell the user and ask |
| A blocking finding is deferred because a fix round costs time | Fix it, or ask the user to agree to defer it |
