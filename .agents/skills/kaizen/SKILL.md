---
name: kaizen
description: Use in the Sisu repo after the final review of a branch, before the push, or when the user asks for a retro, to find what went wrong and propose how to work better tomorrow.
---

# Kaizen

Kaizen is a short look back at one branch, while its details are still in
context. It finds the lessons and proposes changes.

Run it once for each branch: after the final review, before the push. A
branch with no review runs it before the pull request. In a plan, run it
before you delete the plan workspace, because the ledger is there.

`<main checkout>` is the directory that `review-sisu` step 6 names.

## 1. Read the record

Take the facts from these sources, which hold more than the context after a compaction.

- The ledger: `<main checkout>/.scratch/<feature>/progress.md`, or the plan workspace's `progress.md`.
  A branch with no ledger uses `<main checkout>/.scratch/<branch name>/` as its `<feature>` folder:
  create it for `kaizen.md`, and take the facts from the other sources.
- Each review run: the rulings, the fix briefs, the reports and `RUN/retro.md`.
  Kept runs are in `<main checkout>/.scratch/reviews/`.
- When the work has review runs: `python3 .agents/skills/review-sisu/scripts/review_round.py stats <runs>`, on the runs of this work only.
- `git log --oneline <base>..HEAD`, where `<base>` is `origin/main`, or the stage's base on a
  stacked branch.
- The session itself: each user correction, each rejected proposal, and each
  tool call that was blocked, failed or ran again.

## 2. Answer four questions

Give each answer its evidence: a commit, a ruling, a report line or the
user's words.

1. **What went wrong?** One line for each fix round, each rejected proposal,
   each user correction, each repeated call and each `retro.md` line. Put
   the lines of one `retro.md` class together.
2. **What was difficult?** Where the time went, and why.
3. **How do we do better tomorrow?** For each lesson, take the first kind
   that can hold it:
   1. an automated check: a hook in `prek.toml`, a test or a script check;
   2. a rule in `CODING_STANDARDS.md`, for a judgement that no check can make;
   3. a step or a pointer in a skill, or a line in `AGENTS.md`;
   4. a memory of the agent, for how the agent works with this user;
   5. a GitHub issue, for a defect in `sisu` or in a script.
4. **What can we leave in better shape?** Cleanups in the files that this
   work touched: a stale comment, a wrong docs line, dead code, a
   `shortcut:` comment that is now due.

Before you propose a change, search for a check, a rule, a memory or an
issue that already holds it. When one exists but did not catch the problem,
that is the lesson: say why.

The step is done when each line of question 1 names its lesson in
question 3, or says `no lesson`.

## 3. Write and propose

Write the answers to `<main checkout>/.scratch/<feature>/kaizen.md`, or to
`<ledger folder>/<group>-kaizen.md` in a `solve-issues` batch. Save
each lesson of kind 4 as a memory, and name it to the user. An agent with no
memories proposes it as a line in `AGENTS.md`.

Then give the user each proposal of kinds 1, 2, 3 and 5, and each cleanup,
the most severe first. For each one, give the change, the files and the
cost. The user picks which ones to make. A picked change goes on its own
branch and pull request, or into the current branch when the user says so.
When a picked change in the current branch gets a new review, the next run
adds a section with the date to the same `kaizen.md`.

The step is done when each lesson of question 3 is a proposal or a memory.
