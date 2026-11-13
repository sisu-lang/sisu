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

For the final diff of a branch, also follow [final-diff.md](final-diff.md).

## Steps

### 1. Set up

1. The repository root is `git rev-parse --show-toplevel` in the checkout that holds the target, which may be a worktree.
2. `mktemp -d "<scratch>/sisu-review-<name>-XXXX"` makes the run directory, `RUN`. `<scratch>` is the session's scratchpad directory. When the harness gives none, it is `${TMPDIR:-/tmp}`. `$TMPDIR` changes with the sandbox mode. So a sandboxed reviewer cannot write in a directory that an unsandboxed call made under `/tmp`. Then `mkdir RUN/opus-scratch RUN/outside-scratch` makes the reviewers' scratch directories, and `mkdir RUN/probes` makes the directory that keeps the probe programs of every round. `<name>` says what is under review, such as `m8p5-task3` or `attributes-final`. Prompts and reports stay there, outside the repository. Shell variables do not survive between tool calls, so write `RUN` as a literal path.
3. Delete the build directories that have not changed for a day, because a run that never reaches section 6 (Report) keeps its builds: `find <scratch>/sisu-review-*/ -mindepth 1 -maxdepth 1 -type d ! -name probes -mtime +0 -exec rm -rf {} +`.
4. Run `git fetch origin` before every review: the brief and its script come from `origin/main`, so a branch that started before a change to the criteria gets the current ones.
5. For the final diff of a branch, do the steps of [final-diff.md](final-diff.md) (Before the launch).

### 2. Write the brief

Write the Focus to `RUN/focus.md`: what the caller knows that the target
does not say, such as a task's done condition, a risk, a question to
answer, or earlier rulings. List the deferred findings and open issues
that the target touches, so that no reviewer reports them again. Do not
summarize the target; the reviewer reads it.

Then let `review_round.py` do the fixed steps, for every kind. `<skill base>` is the directory of this file. Run it with the sandbox off, because it runs `git fetch`. Write `RUN` and each file path as an absolute path. Write `<target>` just after `RUN`:

```
python3 <skill base>/scripts/review_round.py prepare <repository root> RUN <target> --kind <spec|plan|diff> --doc <spec or plan>
```

It takes `build_sisu.sh` and `diff_brief.py` from `origin/main`, because a branch's own copy can be older than the template. It builds `sisu` at both ends of a diff, so that no reviewer builds them; earlier builds stay in `RUN/bin`. It writes `RUN/brief.md`, adds `RUN/focus.md`, and records the checkout in `RUN/tree-1.txt`. Run it just before the launch.

- `<target>` is the path of a spec or a plan, or `<base>..<head>` for a diff. Write a branch as `origin/main..<branch>`. The brief writes a diff's range as full SHAs, and copies a spec or a plan to `RUN/target-<round>.md`, which the reviewer reads.
- Repeat `--doc` for each document the reviewer reads with the target; leave it out when there is none.
- Pass a pull request body as `--pr-body`, not `--doc`. The script checks each of its lines against the text limits, then gives it to the reviewer as a `--doc`.
- The script stops when a diff makes an "in N fields" count in `docs/register.md` larger, and neither the Focus nor the body names the row. Name the row and the reason, then run it again.
- The brief lists each build in `RUN/bin`, and each line that the diff removes from the tests.
- For a diff, pass the `LOG=` file of each `make mutation-test` run as `--mutation-log`. Repeat the option for more than one log; Focus gets one line for each, and the script checks each one. Pass the output of `make coverage BASE=<base> > RUN/coverage.txt` as `--coverage`. Leave out each one that did not run. The script stops when the log measured uncommitted changes or a commit outside the range. It also stops when the coverage output measured uncommitted changes or a commit other than the head. A stage or a group runs mutation tests once, on its final head after the task reviews, so a task review has no log; the final review gets it. A large fix round of the final review runs one more ([final-diff.md](final-diff.md)). A log from an earlier commit of the range is still accepted, and the brief lists the commits after it.
- The script drops a skill line it cannot find. Tell the user in the report.

### 3. Launch the reviewers

Each review has a Claude seat. Round 1 can also have an outside seat.

| Review | Claude seat | Outside seat |
| --- | --- | --- |
| spec or plan | `opus-high` | round 1, when chosen |
| the diff of one task | `opus-medium` | round 1, when chosen |
| the diff of one task in the light flow of `solve-issues` | `opus-medium` | none |
| the final diff of a branch or a milestone | `opus-high` | round 1, when chosen |

The Claude seat reviews every round. In a later round it reviews alone, and
it also checks the fixes for the outside seat's findings.

The outside seat is a model that is not the caller's: Sol, Fable 5.2, or
another model the user names. Its model and its CLI change with the user's
token limits, so this skill fixes no model or CLI. Take the model and the CLI from the
first of these that gives them:

1. what the user said for this review;
2. the reviewers row of the plan's `## Execution` section;
3. the user's answer: ask before the launch.

A source can name a model but no CLI, or a CLI that
[launchers.md](launchers.md) does not list. Then ask the user for the CLI and its command.

Ask the user for the effort of the outside seat every time, unless the user
named it for this review. "None" is a correct answer: then the Claude seat
reviews alone.

Launch the seats together. Each launcher must let its reviewer read `RUN`
and the skill paths in the brief.

- **Claude seat:** the Agent tool with the `subagent_type` from the table, and the prompt `Read RUN/brief.md and follow it. Your scratch directory is RUN/opus-scratch. Your final reply is the whole report.` with `RUN` written out, and `brief-<round>.md` in a later round. Pass no `model` or `effort`. A read-only reviewer such as `opus-medium` cannot write a file. So save each reply word for word with `python3 <skill base>/scripts/review_round.py save-report RUN opus --agent <agent ID> --round <round>`, which writes `RUN/opus.md` (`opus-<round>.md` in a later round) from the agent's transcript.
- **Outside seat:** the command for its CLI in [launchers.md](launchers.md). Its report stem is the model's short name, such as `sol` or `fable`. When it fails, hits a usage limit or leaves no report, show the user the error it prints and ask how to go on. Do not put another model in its place without asking.

After the last reviewer ends and you saved each report, run `python3 <skill base>/scripts/review_round.py verify <repository root> RUN --round <round> --report <stem>`,
with one `--report` for each reviewer, such as `--report sol --report opus`. Run it before a fix commit.
It fails when a report is missing or has no `Model:` first line, or when the checkout differs from the record of `prepare`. A
missing report means that the reviewer failed or that you did not save its reply. Check that the `Model:` line names the model you launched. A difference
means that a reviewer wrote in the repository, so the round is a failed run: show the user the difference and
ask how to go on. An outside seat's launch can write anywhere, so this check is the guard. It does not see a
write to an ignored file. When only HEAD moved, `verify`
names the new commits and still fails, because a reviewer can commit too. When you made each of
those commits, the round is good.

### 4. Rule on each finding

A report without a `Verdict:` line, or one that says it could not read
the brief, is a failed run, not a review: tell the user and ask how to go
on.

Give every finding one ruling, and write each ruling as one line of
`RUN/rulings-<round>.md`:
`<stem> <n>: Accept | Reject | Defer to <where>: <reason>`, where `<stem>` is
the report's stem. When two reviewers report one defect, give both lines the
same ruling and name the other line. Then run `python3 <skill base>/scripts/review_round.py rulings RUN --round <round> --report <stem>`,
with the same `--report` options as `verify`. It fails on a finding that has
no ruling, and its counts go into the report to the user.

- **Accept:** you will fix it.
- **Reject:** with a reason that cites the target, the code, or `CODING_STANDARDS.md`.
- **Defer:** with where it goes: a later task, a roadmap entry, or a GitHub issue per `docs/agents/issue-tracker.md`.

A nit that a report lists under `Other nits:` needs no ruling. When one of
them is blocking, rule on it as your own blocking finding, and name it in
the report to the user. A nit that names a surviving mutant is never in
that list, because the brief makes it a block.

For each blocking finding that you accept, add one line to `RUN/retro.md`:
`<report> <n>: <class>: <the check that could have caught it before the review>`.
`<report>` is the file stem of the report, such as `sol` or `opus-2`. The
class is one of `incomplete fix`, `missed neighbour`, `missing test`,
`wrong text`, `regression` or `design`. When none fits, write a new class
of a few words. The check is a hook, a test, a lint, a line of the brief,
or `none`.

A diagnostic improvement (`CODING_STANDARDS.md`, Reviewing) is deferred without a fix round, also one under `Other nits:`. Add one line with its repro to the plan workspace's `diagnostic-issues.md`, or to `RUN/diagnostic-issues.md` outside a plan. After the pull request merges, run each repro again on `main` and file one issue per diagnostic code.

Rules for ruling:

1. Check a finding before you rule. Run its repro or read the source it cites. When the claim does not hold, reject it and say what you saw. Save each repro program you run as `RUN/probes/<round>/<reviewer>-<n>.sisu`.
   A fix that a reviewer suggests, or that you write yourself, is a claim too. Check its text, such as the wording of a comment, against the code before it goes into a brief or a commit.
2. Severity follows "Reviewing" in `CODING_STANDARDS.md`. A blocking finding is fixed or deferred with the user's agreement. Accept a nit only in round 1, because its fix is code that the next round must review. A nit that names a surviving mutant and the test that kills it is the exception (`solve-issues`, Scope).
3. A ruling must not break a rule in `CODING_STANDARDS.md`.
4. When two reviewers disagree, rule on the evidence and record the disagreement.
5. Ask the user instead of ruling when the fix would change the spec's intent, syntax, a diagnostic code, a public interface or the scope, or when you would reject a blocking finding.
6. A ruling that would delete code as dead needs more than its evidence. The evidence can be a surviving mutant, a probe with no change in output, or a reviewer's claim. It shows only that no test holds the code. Name the rule of the language that the code implements. The next fix round adds a run case or a fixture that holds that rule. It deletes the code only when the new test passes without it. When you cannot name the rule, rule `Accept: add a test and keep the code`.

### 5. Fix and review again

Fix the accepted findings in the target. Fix the class, not only the repro. For each finding, list the neighbouring cases: the same rule in each position the language allows it. `crates/sisuc/tests/neighbours.txt` lists the known values and positions, and each new one goes there. Try each case, and fix the cause so that all of them are right. Write the brief for a fix round from [fix-brief.md](fix-brief.md). A `solve-issues` batch uses the "Fix round" part of its own `brief.md` instead. It asks for that list and its results. It also asks for the command and the output behind each claim about `sisu`. The executor writes each probe program to `RUN/probes/<round>/`.
The executor compares the probes of every round with `python3 scripts/diag_diff.py BASE_SISU HEAD_SISU RUN/probes`, where `BASE_SISU` is the build before the round. The script compares the codes, the positions, the text and the fixes of each diagnostic; `--codes` compares only codes, positions and crashes, for a round that rewords many diagnostics on purpose. It runs `sisu check`, and prints each program that crashes the head as `crash`. So it finds a panic in the front end with no second run. A panic in codegen or at run time needs a `sisu run` of the probe. A fix to code keeps `make test`
passing. A fix that changes behavior adds its tests in a commit before the
fix, red at the round base. The reviewer runs those tests at that commit:
each must fail, and for the stated cause.

A large fix round of a final diff runs one more mutation test ([final-diff.md](final-diff.md), A large fix round).

When you fixed a blocking finding, review again. In a diff, a fix range
that changes only Markdown files, and none under `docs/reference/`,
`docs/diagnostics/` or `.agents/skills/`, gets no second review. `sisu` or
its tests read the first two, and an agent follows a skill. Read the fix
diff yourself, and name it in the report. A spec or a plan always gets its
second review. Otherwise, write `RUN/focus-<round>.md`
with the fixes and what you probed. Run `review_round.py prepare` with
the round's target (for a diff, the fix range) and `--round <round>`. It writes
`RUN/brief-<round>.md` and takes `RUN/rulings-<round - 1>.md` and each earlier report from `RUN`.
Its "Re-review" section asks for a verdict on each accepted finding, the
neighbours of each fix, and regressions against the base of the whole
work. Send it to the Claude seat; its report goes to `RUN/opus-<round>.md`. Allow at most two
fix rounds. After the review of the second fix round, do the redo below when
it applies. Otherwise ask the user about
each blocking finding that is still open: a third round, or a deferral.
With the question, say whether the open findings have one cause in the
approach. When they do, name the other approach.
A blocking finding in a diff can still be open after the review of the
second fix round. It can have a small fix, such as a pattern in a hook:
fewer than 10 lines, and no change to `sisu` output. It needs no question and
no reviewer: make the fix, read its diff yourself, and run its tests. A spec
or a plan gets no such fix. Defer the nits.

Two fix rounds in a row can each add a blocking regression, or the second
fix of one finding can fail. Then stop patching and do a redo. Give it to a fresh `opus-high` executor.
It first commits failing tests for
each open finding and its neighbours, then fixes the code until they pass.
The Claude seat reviews the redo as one more round. When that review finds a blocking
defect, or the failed rounds already ran on `opus-high`, ask the user.

In a `solve-issues` batch, the rules of `solve-issues` (Scope) replace
the fix-round rules of this step.

### 6. Report

Tell the user:

1. which reviewers ran, with model, CLI and effort, and the `RUN` path;
2. the counts of `rulings`, then one line per finding: its title, its severity, your ruling, and the reason;
3. the fixes you made, and where each deferred finding went;
4. the questions that wait for the user.

After the last round, keep the record of the review in the main checkout,
so that a later review of this skill can measure it. The main checkout is the
directory above `git rev-parse --path-format=absolute --git-common-dir`.
`.scratch/` is git-ignored.

```
mkdir -p <main checkout>/.scratch/reviews/<RUN name> && cp RUN/*.md <main checkout>/.scratch/reviews/<RUN name>/ && cp -r RUN/probes <main checkout>/.scratch/reviews/<RUN name>/
```

Measure the kept runs with `python3 <skill base>/scripts/review_round.py stats <main checkout>/.scratch/reviews/*`.
It counts the rounds of each run, and the rulings by round and severity.
It also counts the rulings of each reviewer, the origin of each later
blocking finding, and the classes of `retro.md`.

- A pull request that changes this skill, its brief or a seat gives the output in its body, so that the next change can compare.
- At the end of a plan or a `solve-issues` batch, run it on the runs of that work only, such as `.scratch/reviews/sisu-review-m9p2-*`. Give the user each retro class that two or more findings share, with the hook or the test that would catch it.

Then delete everything in `RUN` except the briefs, the reports and `probes`:
`find RUN -mindepth 1 -maxdepth 1 -type d ! -name probes -exec rm -rf {} +`.
Reviewers build there, about 7 GB a build, and nothing else removes it.
