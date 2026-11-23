---
name: delete-comment
description: Use in the Sisu repo when the user asks to audit, sort or delete the comments of a scope, such as a folder, a file or the whole codebase.
---

# Delete comment

This skill sorts each comment of one scope into a kind, and does the action of that kind.
`CODING_STANDARDS.md` (Code, "A comment says what the code cannot show") defines the four
kinds and their actions. Read that rule first.

Work in a worktree from `origin/main`. Audit one scope in each pull request. `<main checkout>` is
the directory that `review-sisu` step 6 names. `<slug>` is the scope with each `/` changed to
`-`, or `all` for the whole codebase.

## 1. List the comments

The scope is the path that the user names. List the comment lines of its code files, with one
command for each comment mark:

```sh
git grep -n -E '^\s*//' -- '<scope>/*.rs' '<scope>/*.sisu'
git grep -n -E '^\s*#($|[^!\[])' -- '<scope>/*.py' '<scope>/*.sh'
```

For a file, use the file as the pathspec, with the command of its comment mark, such as
`git grep -n -E '^\s*//' -- <file>`.

A line in a multi-line string can match. Read each hit, and remove each one that is not a
comment. Read each file of the scope for comments after code on the same line, and add them.

Remove the required comments from the list. A lint, a tool, a test or a rule of
`CODING_STANDARDS.md` asks for each of these. They stay, whatever their text:

- `// SAFETY:` comments, which clippy requires;
- `r[verify ...]` rule markers, which the `reference` tests read;
- tool directives, such as `# noqa:`, `# pyrefly: ignore`, `# shellcheck disable` and
  `spellchecker:`;
- `shortcut:` comments and recorded exceptions;
- the GIVEN, WHEN, THEN and AND lines of a test;
- the test comments that the Tests rules ask for: why an input finds the defect, a size margin,
  a seed criterion, and a timeout test's times;
- each comment in a Sisu program under `crates/*/tests/` or `site/programs/`. Tests pin their
  lines and output.

Done when the list holds each comment of the scope that is not required.

## 2. Sort each comment

Write a ledger at `<main checkout>/.scratch/delete-comment/<slug>.md`, with one row for each
comment: `file:line`, kind, action and a note. Give each comment the kind that its text has
today:

- **Tautology**: the action is delete.
- **Stale**: the action is delete. When the comment holds a reason, the action is "make true".
- **Excuse**: the action is delete, and the note names the hack and the fix for the issue. When
  the limit stays on purpose, the action is "shortcut", and the note gives the limit and when to
  upgrade.
- **Reason**: the action is keep. When the reason governs more than the code below it, the
  action is "move", and the note names the doc that owns the rule.

Check a stale claim before you sort it. Read the code for a claim about code. Run the program
for a claim about a printed value or an order.

Done when each comment of the list has a row with a kind and an action.

## 3. Ask the user

Show the count of each kind, each proposed issue and each proposed move. Then wait for the
user. The user can change a row before you act.

Done when the user accepts the ledger.

## 4. Act

- Delete each comment that has the action delete. Remove a blank line that the deletion leaves
  twice.
- Rewrite each comment that has the action "make true" or "shortcut".
- Write each issue that the user accepted. `docs/agents/issue-tracker.md` and
  `docs/agents/triage-labels.md` say how.
- Move each reason that the user accepted. A new ADR follows `docs/agents/domain.md`. Delete the
  comment, or make it one line that names the doc.

Done when each row has its action done, and the ledger gives the issue number or the doc of each
such row.

## 5. Check

A deletion in a test that `scripts/test_names_baseline.txt` lists changes the test's body. Then
the `test-names` hook asks for a new name: rename the test, or keep the comment. A deleted tool
directive can make Ruff, Pyrefly or shellcheck fail.

Run `make test`. Stage the changes, then run `prek run`, which runs each hook on them. A doc
comment can hold a doctest: when you delete one, check that a test still covers what it showed.

Report the lines removed (`git diff --shortstat origin/main...HEAD`), the count of each kind, and
the issues and moves.

Done when `make test` and `prek run` pass, and the report is given.
