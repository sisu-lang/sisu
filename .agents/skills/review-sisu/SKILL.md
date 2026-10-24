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
2. `mktemp -d "${TMPDIR:-/tmp}/sisu-review-XXXXXXXX"` makes the run directory, `RUN`, and `mkdir RUN/opus-scratch` makes the Opus reviewer's scratch directory. Prompts and reports stay there, outside the repository. Shell variables do not survive between tool calls, so write `RUN` as a literal path.
3. Find the two skills the brief cites: `test-audit` at `~/.claude/skills/test-audit/SKILL.md` and `codebase-design` at `~/.agents/skills/codebase-design/SKILL.md`. When one is not there, `find ~/.claude ~/.agents -path '*/<name>/SKILL.md'`. When neither place has it, delete its line from the brief and tell the user in the report.
4. Run `git fetch origin` before every review: the brief comes from `origin/main`, so a branch that started before a change to the criteria gets the current ones.
5. For the final diff of a branch, review what will merge. When `git merge-base --is-ancestor origin/main <head>` fails, `main` has moved: bring it in, run `make test` and `make site`, and review the range from `origin/main` to the new head. Rebase onto `origin/main` when the branch's commits replay cleanly. Merge `origin/main` instead when a rebase conflicts in commits that later fix rounds rewrote: one merge resolves the conflict against the final code. A new check on `main`, such as a warning, can fail the branch's own tests with no conflict, so `make test` is part of the merge.

### 2. Write the brief

Write `origin/main`'s copy of the template to `RUN/brief.md`: `git show origin/main:.agents/skills/review-sisu/brief.md > RUN/brief.md`. A branch's own [brief.md](brief.md) can be older than the criteria. Fill every `<...>` slot above
"# Report"; the slots under it are the reviewer's. Keep the common
sections and the section for the target's kind; delete the other two kind
sections, and the "Read first" lines that are only for a diff. Under "Focus", write what the caller knows that
the target does not say: a task's done condition, a risk, a question to
answer, earlier rulings. Write "none" when there is nothing. Do not
summarize the target; the reviewer reads it.

For a diff, including one task's diff in a plan run, write the Focus to
`RUN/focus.md` and let the script fill the rest:
`python3 .agents/skills/review-sisu/scripts/diff_brief.py <repository root> <base>..<head> RUN/brief.md --doc <spec or plan> --focus RUN/focus.md`.
Repeat `--doc` for each document; leave it out when there is none.

### 3. Launch the reviewers

| Review | Reviewers |
| --- | --- |
| spec, plan, or the diff of one task | `gpt-6.1-sol` at medium effort |
| the final diff of a branch or a milestone | `gpt-6.1-sol` at medium effort, and an `opus-high` subagent |

The user or the caller's plan may change an effort or add a reviewer.
Launch the reviewers together.

- **Sol:** `sol-review <effort> RUN/brief.md RUN/sol.md <repository root>`, through Bash with `run_in_background` and the sandbox off: codex needs the network and writes under `~/.codex`. The completion notification ends the wait; do not poll. When it exits 1, show the user the error it prints and ask how to go on. Do not put another model in Sol's place without asking.
- **Opus:** the Agent tool with `subagent_type: opus-high`, and the prompt `Read RUN/brief.md and follow it. Your scratch directory is RUN/opus-scratch. Write your report to RUN/opus.md; your final reply is that report.` with `RUN` written out. Pass no `model` or `effort`.

### 4. Rule on each finding

Give every finding one ruling:

- **Accept:** you will fix it.
- **Reject:** with a reason that cites the target, the code, or `CODING_STANDARDS.md`.
- **Defer:** with where it goes: a later task, a roadmap entry, or a GitHub issue per `docs/agents/issue-tracker.md`.

Rules for ruling:

1. Check a finding before you rule. Run its repro or read the source it cites. When the claim does not hold, reject it and say what you saw.
2. Severity follows "Reviewing" in `CODING_STANDARDS.md`. A blocking finding is fixed or deferred with the user's agreement; a nit may be rejected.
3. A ruling must not break a rule in `CODING_STANDARDS.md`.
4. When two reviewers disagree, rule on the evidence and record the disagreement.
5. Ask the user instead of ruling when the fix would change the spec's intent, syntax, a diagnostic code, a public interface or the scope, or when you would reject a blocking finding.

### 5. Fix and review again

Fix the accepted findings in the target. A fix to code keeps `make test`
passing, and a fix that changes behavior adds a test that fails without
it.

When you fixed a blocking finding, review again. Write `RUN/brief-2.md`
from the same template, with the findings, your rulings and the fixes
under "Focus", and send it to the reviewers that found the blocking
findings; their reports go to `RUN/sol-2.md` and `RUN/opus-2.md`. Allow
at most two fix rounds. After the second round, rule on what is left and
defer it.

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
