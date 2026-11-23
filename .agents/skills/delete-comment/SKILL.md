---
name: delete-comment
description: Use in the Sisu repo when the user asks to audit, sort, prune or delete the comments of a scope, such as a folder, a file or the whole codebase.
---

# Delete comment

This skill sorts each comment of one scope into a kind, and does the action of that kind.
`CODING_STANDARDS.md` (Code, "A comment says what the code cannot show") defines the four
kinds and their actions. Read that rule first.

Work in a worktree from `origin/main`. Audit one scope in each pull request. `<main checkout>` is
the directory that `review-sisu` step 6 names.

## 1. List the comments

The scope is the path that the user names. List each comment line in it:

```sh
git grep -n -E '^\s*(//|#[^!\[])' -- <scope>
```

This command finds full-line comments only. Read each file of the scope for comments after code
on the same line, and add them to the list.

Remove the required comments from the list. These stay, whatever their text:

- `// SAFETY:` comments, which clippy requires;
- `shortcut:` comments and recorded exceptions;
- the GIVEN, WHEN and THEN lines of an end-to-end test;
- a unit test comment that says why its input finds the defect;
- each comment in a Sisu test program under `crates/*/tests/`. The tests pin their lines and
  columns.

Done when the list holds each comment of the scope that is not required.

## 2. Sort each comment

Write a ledger at `<main checkout>/.scratch/delete-comment/<scope>.md`, with one row for each
comment: `file:line`, kind, action and a note. Give each comment the kind that its text has
today:

- **Tautology** or **stale**: the action is delete.
- **Excuse**: the action is delete, and the note names the hack and the fix that the issue asks
  for.
- **Decision record**: the action is "ADR" when the decision governs more than the code below
  it. Otherwise the action is keep.

Check a stale claim against the code before you sort it. A comment that names a function, an
order or a printed value is stale only when the code says otherwise.

Done when each comment of the list has a row with a kind and an action.

## 3. Ask the user

Show the counts of each kind, each proposed issue and each proposed ADR. Then wait for the
user. The user can change a row before you act.

## 4. Act

- Delete each comment that has the action delete. Remove a blank line that the deletion leaves
  twice.
- Write each issue that the user accepted. `docs/agents/issue-tracker.md` and
  `docs/agents/triage-labels.md` say how.
- Write each ADR that the user accepted, with the `domain-modeling` skill. Delete its comment, or
  make the comment one line that names the ADR.

Done when each row has its action done, and the ledger gives the issue number or the ADR file of
each such row.

## 5. Check

Run `make test`. Stage the changes, then run `prek run comment-width` and `prek run text-limits`.
A doc comment can hold a doctest: when you delete one, check that a test still covers what it
showed.

Report the lines shed (`git diff --shortstat origin/main`), the count of each kind, and the
issues and ADRs.
