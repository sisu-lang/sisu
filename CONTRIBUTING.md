# Contributing to Sisu

Sisu is a solo learning project, and most changes are written with an AI
agent. [AGENTS.md](AGENTS.md) holds the rules for changing this tree, for
people and agents alike. This page is the route through them.

1. Set up a clone as [Developing Sisu](docs/development.md) describes. The
   pre-commit hook runs the same checks as CI.
2. Read [AGENTS.md](AGENTS.md): the commands, the lint policy, and how to
   check facts about dependencies.
3. Read [CODING_STANDARDS.md](CODING_STANDARDS.md): the judgement calls that
   reviewers check and no linter can.
4. Use the words in [GLOSSARY.md](GLOSSARY.md), and read the decisions in
   [docs/adr](docs/adr) that touch your change.
5. Finish with `make check` passing, as CI requires, and `make miri` too
   when you change `crates/runtime`. Then run
   `make verify` on your diff, as [AGENTS.md](AGENTS.md) shows, and cover
   or explain each uncovered region and kill or explain each surviving
   mutant.

Work is tracked as [GitHub issues](https://github.com/sisu-lang/sisu/issues),
as [docs/agents/issue-tracker.md](docs/agents/issue-tracker.md) describes.

## Pull requests

A pull request says what changed and why, then what a reviewer cannot see
from the diff: which contract each new test proves, what was left out of
scope, and what is known not to work yet.

Before you open one, run the `update-docs` skill
([.agents/skills/update-docs/SKILL.md](.agents/skills/update-docs/SKILL.md))
so the docs match the change.
