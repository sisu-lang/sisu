---
name: solve-issues
description: Use in the Sisu repo when the user asks to tackle, fix or batch open GitHub issues of sisu-lang/sisu, or to resume such a batch from its ledger in .scratch/.
---

# Solve Sisu issues

You are the controller. You choose the issues, group them, find each root
cause, and write the briefs. Executors write the code. Reviewers read it.
You rule on their findings. Two planned stops wait for the user, and
`review-sisu` adds its own stops.

## Roles

| Role | Who | Does |
| --- | --- | --- |
| Controller | you | picks, groups, finds root causes, writes briefs, rules on findings, keeps the ledger |
| Executor | `sonnet-high` (Agent tool, `subagent_type` only) | implements one task brief or fix-round brief; never does root-cause work |
| Task review | `gpt-6.1-sol` medium in the first round only, `opus-medium` in every round | through `review-sisu` |
| Final review | `gpt-6.1-sol` medium and `opus-high` | through `review-sisu`, on the whole branch |

These are the reviewers of the full flow. When this table and the table in
`review-sisu` (step 3) differ, `review-sisu` wins. The light flow
(step 2.3) replaces both tables for its group.

## The ledger

The batch runs for hours, and the context gets compacted. Keep its state in
`.scratch/<batch>/progress.md` ([ledger.md](ledger.md) gives the form). Put
the briefs, the logs and the root-cause notes in the same folder. Update the
ledger after each event: a stop answered, a dispatch, a report, a ruling, a
commit. When the user asks you to resume, read the ledger first. Go on from
its last event.

## Steps

### 1. Pick the issues

1. Run `git fetch origin`. Do all root-cause work on `origin/main`.
2. Build `sisu` at `origin/main` once for the batch:
   `git -C <repo> show origin/main:.agents/skills/review-sisu/scripts/build_sisu.sh | bash -s -- <repo> ${TMPDIR:-/tmp}/sisu-issues-<batch> origin/main`.
   Use the `sisu` path that it prints. It exports the commit. Do not use `git worktree add` for this build (`CODING_STANDARDS.md`, Reviewing).
3. When the user names the issues, take those issues and go to step 1.6.
4. Otherwise, list the open issues: `gh api 'repos/sisu-lang/sisu/issues?state=open&per_page=100' --paginate`, with the sandbox off. Skip each item that has a `pull_request` field.
5. Leave out each issue that a branch or a pull request fixes, and each issue on hold. Look in these places:
   - the branches: `git branch -a`;
   - the open pull requests: `gh api 'repos/sisu-lang/sisu/pulls?state=open'`;
   - the ledgers in `.scratch/`;
   - the comments on the issue, the memory, and what the user said.
6. Run the repro of each bug report on `origin/main`. Close it without a question in two cases only:
   - It is fixed on `origin/main`. Close it with `state_reason` `completed`.
   - The committed reference (`docs/reference/`) or an ADR states this exact behaviour. The issue does not argue that the rule is wrong. Close it as intended (below).

   Before you close, comment with the command and its output (`docs/agents/issue-tracker.md`). Write the command as `sisu run <file>.sisu`, and give the program text in the comment: issue text has no `/tmp` or `.scratch` paths. Write the close in the ledger.

   To close an issue as intended, replace its triage label with `wontfix`. Keep its other labels: `PUT` replaces the whole list. Then close it with `not_planned`.
7. Each other issue that disagrees with documented behaviour needs a language decision (step 2.4).

### 2. Group them

1. Put issues in one group when their fixes change the same functions. One group is one branch and one pull request.
2. Order the groups. Two groups that change the same code run one after the other. A group that changes code an open branch changes waits for that branch to merge.
3. For each group, propose a review flow, with the reason:
   - **Full:** the roles above, and one mutation test for the group, which you run after the last task review (step 7.1).
   - **Light:** `opus-medium` for each task and for the final review. `make test` and `make coverage` only; no mutation test.
4. For each language decision, write the options and their costs. Post them on the issue, as in step 3.8.

**Stop 1.** Show these in the reply text, in one message:

- the groups, their order, and the review flow of each group;
- the issues left out, and the issues closed;
- each language decision, with its risk and a short Sisu example.

Then ask with `AskUserQuestion`. One call takes at most four questions. Ask
one question that approves the table or asks for changes. Ask one question
for each language decision. With more than three decisions, use more than
one call. Write the answers in the ledger.

After a language decision at Stop 1:

- The user keeps today's rule: close the issue as intended now (step 1.6). When the reference does not state the rule yet, add the rule to the first task of the first group. Name the issue in that pull request body without `Closes`.
- The user changes the rule: the issue joins a group. The first task of that group records the decision in `docs/reference/` or in an ADR.

### 3. Find the root causes

Take the next group in the order. Do one group at a time. Run
`git fetch origin` first. When `origin/main` moved since the build, build it
again (step 1.2) and write the new path in the ledger. For each issue of the
group:

1. Reproduce it with the `origin/main` build.
2. Find the cause in the code: the function, the line, and why it gives this output.
3. List the neighbouring cases: the same rule in each position the language allows it. Probe each case. Keep the probes in the ledger folder.
4. A new bug can have the same cause. Then it joins the group, and the pull request names it.
5. A new bug can have a different cause. Then file a new issue (`docs/agents/issue-tracker.md`). Keep it out of the group.
6. Fix the cause in the shared code, so that all the cases are right. Rank the options as ADR 0006 does: correctness, simplicity, readability, performance.
7. Check each claim that will go into a brief:
   - **Diagnostic text.** Write the full output for a probe with user names: a type parameter `U`, a class `Node`. The text names only what the user wrote. A diagnostic has a message, a label, secondary labels, a help and fixes. It has no note. For each fix, write its message, its edit, and safe or unsafe. Apply each fix by hand. The result must pass `sisu check`.
   - **Cost.** Each new walk is linear in the program (`CODING_STANDARDS.md`, Performance). State its worst input. Name the step counter that guards it.
   - **Names.** Each function, file and line in the brief exists at `origin/main`.
8. A fix can have more than one good design. Then write the options with their costs. Post them on the issue under `## Options`. After the user chooses, edit the comment to name the choice. Post nothing on an issue that has no options.

**Stop 2.** Show, for each issue: the cause, the fix, its cost, and the
options when there are some. Ask with `AskUserQuestion`, as at Stop 1.

### 4. Write the briefs

1. Put the issues whose fixes touch the same code into one task.
2. Order the tasks so that each one builds on the last.
3. Write one brief for each task from [brief.md](brief.md). Fill every slot.

### 5. Set up the group

Do this once for each group. On resume, use the worktree in the ledger.

1. Run, in the repository root: `git worktree add --no-track -b fix/<batch>-<group> .claude/worktrees/<batch>-<group> origin/main`.
2. Run `codegraph init -y` in the new worktree.
3. Link the LLVM: `ln -s <repo>/.llvm .claude/worktrees/<batch>-<group>/.llvm`.
4. Run `cargo fetch --locked` in the new worktree, with the sandbox off.

### 6. Run each task

1. Run `git fetch origin`. When `git merge-base --is-ancestor origin/main HEAD` fails in the worktree, merge `origin/main` and run `make test`.
2. Set `<BASE>` to the worktree's `HEAD` now. Write it in the brief and in the ledger.
3. Launch the executor with `run_in_background`.
4. Read its report, and act on its status:
   - **DONE:** go to step 6.5.
   - **BLOCKED:** the stated cause is wrong. Do step 3 again for that issue, then Stop 2 again.
   - **DONE_WITH_CONCERNS:** check each concern.
5. Run `review-sisu` on `<BASE>..HEAD`. It writes the review brief, launches the reviewers, and says how to rule and when to ask the user.
6. Write each fix-round brief from the "Fix round" part of [brief.md](brief.md). Give it to a `sonnet-high` executor.

### 7. Finish the group

1. On the full flow, run one mutation test on the group's head with Bash `run_in_background: true`: `(cd <worktree> && env CARGO_MUTANTS_JOBS=2 nice -n 19 make mutation-test BASE=<group base> LOG=<ledger folder>/<group>-mutation.log)`. Check that its last line names the head. Put each surviving mutant into one fix round, with a test that kills it or a reason it is equivalent. An executor never runs a mutation test: it would block the task for an hour.
2. Run the `update-docs` skill.
3. Run `review-sisu` on the whole branch, with `--mutation-log` set to that log and the final reviewers of the group's flow.
4. Write the pull request body with `mattpocock-skills:pr`, with one `Closes #<N>` line for each issue.
5. The user pushes. Then create the pull request with `gh api repos/sisu-lang/sisu/pulls`, with the sandbox off.
6. Never merge on your own. When the user says to merge, run `python3 scripts/merge_pr.py <N>` in the main checkout, with the sandbox off and `run_in_background`. It waits for the checks, stops when one fails or the head does not contain `main`, and merges. Then it waits for each `Closes` issue to close, and removes the group's worktree and branch when they are clean and at the merged head. Write each line it prints in the ledger.
7. Write the ledger's "After the merge" lines. Write one line with a runnable repro for each deferred finding. Do the same for each line of each `diagnostic-issues.md` in the group's review runs.
8. Go to step 3 for the next group that does not wait for a merge. When all groups wait, tell the user and stop.
9. When the batch ends, delete the batch build: `rm -rf ${TMPDIR:-/tmp}/sisu-issues-<batch>`.

## Common mistakes

| Mistake | Instead |
| --- | --- |
| The root-cause work runs on an old checkout | `git fetch origin`; build `origin/main` with `build_sisu.sh` |
| A scratch build in a worktree, then `git worktree prune` | Export the commit (`build_sisu.sh`); never prune other worktrees |
| The mutation command starts with `CARGO_MUTANTS_JOBS=2 …` | Use the `env CARGO_MUTANTS_JOBS=2 …` form; a shell wrapper rewrites the bare form, and the log loses its last line |
| A diagnostic text in a brief names the prelude's `T` | Write the output for a probe with the user's names first |
| A brief adds a walk with no cost bound | State the walk's worst input and its guard before Stop 2 |
| A question to the user between the stops, about a fixed issue | Close a fixed issue with its evidence; ask at the stops and where `review-sisu` says |
| A feature request is closed because the reference documents today's behaviour | Give its options at Stop 1 |
| The executor finds the cause | The brief states the cause; the executor only implements |
| The batch state is only in the conversation | Write it in the ledger after each event |
