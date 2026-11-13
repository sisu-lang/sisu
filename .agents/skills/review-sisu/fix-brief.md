# Fix-round brief

Write each fix-round brief from this template. Step 5 of [SKILL.md](SKILL.md) gives the rules.
The brief is ready only when each item has its "Neighbours" table. A table with one row
fixes one shape, and the next round finds the others.

```markdown
# <What is under review>, fix round <N>

Base: <sha>. Review report: <RUN>/<stem>-<round>.md. Rulings: <RUN>/rulings-<round>.md.
Do only these items.

Failure class: <the class of the round's findings, such as an incomplete fix or a wrong ruling>.

Test first: one commit with the new tests, each red at the base, then the fix commits.

1. **<Title> (<stem> <n>).** Cause: <the function and line, and why it gives the wrong result>.
   Fix: <the change, in the shared code that each neighbour reaches>.
   Neighbours: each row is one shape of the same rule, in each position the language allows it
   (`crates/sisuc/tests/neighbours.txt`).

   | Shape | Output at the base | Wanted output | Probe |
   | --- | --- | --- | --- |
   | <the finding's repro> | <...> | <...> | RUN/probes/<round>/<name>.sisu |
   | <each neighbour> | <...> | <...> | <...> |

   Tests: <the fixture or the run case that holds each row, and the mutation each new test kills>.

After the fix: `make test`, `prek run --all-files`, clippy clean. Compare the probes with
`python3 scripts/diag_diff.py BASE_SISU HEAD_SISU RUN/probes`. Report each row's output at the head.
```
