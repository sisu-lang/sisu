# Fix-round brief

Write each fix-round brief from this template. Step 5 of [SKILL.md](SKILL.md) gives the rules.
A `solve-issues` batch uses the "Fix round" part of `solve-issues/brief.md` instead.
The brief is ready only when each item has its "Neighbours" table. A table with one row
fixes one shape, and the next round finds the others.

```markdown
# <What is under review>, fix round <N>

Base: <sha>. Review report: <RUN>/<stem>.md in round 1, <RUN>/<stem>-<round>.md later.
Rulings: <RUN>/rulings-<round>.md. Do only these items.

Failure class: <the class of the round's findings, such as an incomplete fix or a wrong ruling>.

Test first: the tests of each change in behaviour go in one commit titled "Add failing tests",
each red at the base. Then the fix commits. A test that kills a surviving mutant, or that holds
the rule of code a ruling kept, is green at the base: commit it on its own and say why.

1. **<Title> (<stem> <n>).** Cause: <the function and line, and why it gives the wrong result>.
   Fix: <the change, in the shared code that each neighbour reaches>.
   Neighbours: each row is one shape of the same rule, in each position the language allows it
   (`crates/sisuc/tests/neighbours.txt`). For a finding in a script, a hook or a doc, each row is
   one input of the same rule, and the Probe column holds the command that shows the output.

   | Shape | Output at the base | Wanted output | Probe |
   | --- | --- | --- | --- |
   | <the finding's repro> | <...> | <...> | RUN/probes/<round>/<name>.sisu |
   | <each neighbour> | <...> | <...> | <...> |

   Tests: <the fixture or the run case that holds each row, and the mutation each new test kills>.

Report, do not add: when a check (a hook, a lint, a test table) seems to need a new exemption,
report the hit and the reason, and add nothing.

After the fix: `make test`, `prek run --all-files`, clippy clean. Compare the probes with
`python3 scripts/diag_diff.py BASE_SISU HEAD_SISU RUN/probes`. Report each row's output at the head.
No mutation test. Do not push, open a pull request or merge.
```
