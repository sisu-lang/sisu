---
name: solve-issues
description: Use in the Sisu repo when the user asks to tackle, fix or batch open GitHub issues of sisu-lang/sisu, or to resume such a batch from its ledger in .scratch/.
---

# Solve Sisu issues

You are the controller. You choose the issues, group them, find each root
cause, and write the briefs. Executors write the code. Reviewers read it.
You rule on their findings. Two stops wait for the user.

## Roles

| Role | Who | Does |
| --- | --- | --- |
| Controller | you | picks, groups, finds root causes, writes briefs, rules on findings, keeps the ledger |
| Executor | `sonnet-high` (Agent tool, `subagent_type` only) | implements one task from its brief; never does root-cause work |
| Task review | `gpt-6.1-sol` medium in the first round only, `opus-medium` in every round | through `review-sisu` |
| Final review | `gpt-6.1-sol` medium and `opus-high` | through `review-sisu`, on the whole branch |

A task gets at most two fix rounds (`review-sisu`, step 5).

## The ledger

The batch runs for hours, and the context gets compacted. Keep its state in
`.scratch/<batch>/progress.md` ([ledger.md](ledger.md) gives the form). Put
the briefs and the root-cause notes in the same folder. Update the ledger
after each event: a stop answered, a task dispatched, a report, a ruling, a
commit. When the user asks you to resume, read the ledger first and go on from
its last line.

## Steps

### 1. Pick the issues

1. `git fetch origin`. Do all root-cause work on `origin/main`: build `sisu` at that commit, never at an older checkout.
2. List the open issues: `gh api 'repos/sisu-lang/sisu/issues?state=open&per_page=100' --paginate`, with the sandbox off. Skip each item that has a `pull_request` field.
3. Leave out each issue that an open branch or pull request fixes (`git branch --list 'fix/*'`, the ledgers in `.scratch/`, open pull requests), and each issue that the user, the memory or the ledger puts on hold.
4. Run the repro of each remaining bug report on `origin/main`. When it is fixed already, or the behaviour is what the language reference or a spec says, comment on the issue with the command and its output, and close it (`state_reason` `completed` or `not_planned`, `docs/agents/issue-tracker.md`). Do not ask. Write it in the ledger.
5. An issue that asks to change documented behaviour is not a bug report: it needs a language decision (step 2.4).

### 2. Group them

1. Put issues in one group when their fixes change the same functions. One group is one branch and one pull request.
2. Order the groups: two groups that change the same code run one after the other. A group that changes code an open branch changes waits for that branch to merge.
3. For each group, propose the full review flow (the roles above, and one mutation test per task) or a light one (`opus-medium`, `make test` and `make coverage` only). Give the reason.
4. An issue that needs a language decision gets its options here.

**Stop 1.** Show the groups, their order, the review flow of each, and each
language decision, in one message. Ask with `AskUserQuestion`. Put the risk
and a short Sisu example in the reply text, as well as in the question.

### 3. Find the root causes

For each issue of the group that comes next:

1. Reproduce it at `origin/main`.
2. Find the cause in the code: the function, the line, and why it gives this output.
3. List the neighbouring cases: the same rule in each position the language allows it. Probe each one, and keep the probes in the ledger folder.
4. A new bug with the same cause joins the group, and the pull request names it. A new bug with a different cause becomes a new issue (`docs/agents/issue-tracker.md`) and stays out of the group.
5. Choose the fix that is elegant, robust and performant, in the shared code that causes all the cases.
6. Check each claim that will go into a brief:
   - **Diagnostic text.** Write the full output for a probe whose names differ from the prelude's and the compiler's: a user type parameter `U`, a class `Node`. The text names only what the user wrote. Use only the parts a diagnostic has: a message, a label, secondary labels and a help (no note). Apply each help by hand: the result passes `sisu check`.
   - **Cost.** State the cost of each walk the fix adds, and its worst input, against `CODING_STANDARDS.md` (Performance). A walk that goes on after the first answer needs a cache or a bound.
   - **Names.** Each function, file and line that the brief names exists at `origin/main`.
7. When the fix has more than one good design, write the options with their costs, and post them on the issue (`## Options`). After the user chooses, edit the comment to name the choice. Post nothing on an issue that has no options.

**Stop 2.** Show, for each issue: the cause, the fix, its cost, and the
options when there are some. Ask with `AskUserQuestion`, as at Stop 1.

### 4. Write the briefs

Write one brief for each task from [brief.md](brief.md), and fill every slot.
A task holds the issues whose fixes touch the same code. Order the tasks so
that each one builds on the last.

### 5. Run each task

1. Set up the group's worktree: `git worktree add --no-track -b fix/<group> .claude/worktrees/<group> origin/main`, then `codegraph init -y`, the `.llvm` symlink, and `cargo fetch --locked` with the sandbox off.
2. Fill `<BASE>` with the previous task's final head, and launch the executor with `run_in_background`.
3. Check its report: the commit exists, and the last line of the mutation log names that commit.
4. Run `review-sisu` on the task's range. It writes the review brief, launches the reviewers, and says how to rule. Write each fix-round brief from the "Fix round" part of [brief.md](brief.md).
5. Before the next task: when `git merge-base --is-ancestor origin/main HEAD` fails, merge `origin/main`, and run `make test`.

### 6. Finish the group

1. Run `review-sisu` on the whole branch, with the final reviewers.
2. Run the `update-docs` skill.
3. Write the pull request body with `mattpocock-skills:pr`, with one `Closes #<N>` line for each issue.
4. The user pushes. Never merge.
5. Write the ledger's "After the merge" lines: one line, with a runnable repro, for each deferred finding and each line of `diagnostic-issues.md`.

## Common mistakes

| Mistake | Instead |
| --- | --- |
| The root-cause work runs on an old checkout | `git fetch origin`; build `origin/main` |
| The mutation command starts with `CARGO_MUTANTS_JOBS=2 …`, which the shell wrapper rewrites, so the log loses its last line | The brief gives the `env CARGO_MUTANTS_JOBS=2 …` form |
| A diagnostic text in a brief names the prelude's `T` | Write the output for a probe with the user's names first |
| A brief adds a walk with no cost bound, and review finds it exponential | State each walk's cost and worst input before Stop 2 |
| Many questions in one message, among them whether to close a fixed issue | Ask only at the two stops; close a fixed issue with its evidence |
| The executor finds the cause | The brief states the cause; the executor only implements |
| The batch state is only in the conversation | Write it in the ledger after each event |
| A feature request is closed because the reference documents today's behaviour | Give its options at Stop 1 |
| `git worktree prune` after a scratch build, which touches the worktrees of other branches | `git worktree remove <path>` for the worktree you added, and nothing more |
