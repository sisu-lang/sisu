---
name: kaizen
description: Use in the Sisu repo after the last review round of a branch, at the end of a plan or a solve-issues batch, or when the user asks for a retro, to find what went wrong and propose how to work better tomorrow.
---

# Kaizen

Kaizen is a short look back at one piece of work, while its details are
still in context. It finds the lessons and proposes changes. The user picks
which changes to make. Run it once for each branch, after the last review
round and before the merge.

## 1. Read the record

Take the facts from these sources, which hold more than the context after a compaction.

- The ledger: `.scratch/<feature>/progress.md`, or the plan workspace's `progress.md`.
- Each review run: the rulings, the fix briefs, the reports and `RUN/retro.md`.
  Kept runs are in `.scratch/reviews/`.
- `python3 .agents/skills/review-sisu/scripts/review_round.py stats <runs>`, on the runs of this work only.
- `git log --oneline origin/main..HEAD` and the pull request's checks.
- The session itself: each user correction, each rejected proposal, and each
  tool call that was blocked, failed or ran again.

## 2. Answer four questions

Give each answer its evidence: a commit, a ruling, a report line or the
user's words.

1. **What went wrong?** One line for each fix round, each rejected proposal,
   each user correction and each repeated call. Each `retro.md` class that
   two or more findings share is one more line.
2. **What was difficult?** Where the time went, and why.
3. **How do we do better tomorrow?** For each lesson, take the first kind
   that can hold it:
   1. an automated check: a hook in `prek.toml`, a test or a script check;
   2. a rule in `CODING_STANDARDS.md`, for a judgement that no check can make;
   3. a step or a pointer in a skill, or a line in `AGENTS.md`;
   4. a memory of the agent, for how the agent works with this user.
4. **What can we leave in better shape?** Cleanups in the files that this
   work touched: a stale comment, a wrong docs line, dead code, a
   `shortcut:` comment that is now due.

Before you propose a change, search for a check, a rule, a memory or an
issue that already holds it. When one exists but did not fire, that is
the lesson: say why it did not fire.

The step is done when every fix round, every user correction and every
`retro.md` line of this work has a line in the answers.

## 3. Write and propose

Write the answers to `.scratch/<feature>/kaizen.md`. Save the lessons of
kind 4 as memories, if your agent keeps memories.

Then give the user the proposals of kinds 1 to 3 and the cleanups, the most
severe first. For each one, give the change, the files and the cost. The
user picks which ones to make. A picked change goes on its own branch and
pull request, or into the current branch when the user says so. A new
GitHub issue also waits for the user's word.
