# Area: resource limits

Sisu documents no limits. Find where each one is, what happens when a
program crosses it, and whether that is a clean failure (a diagnostic, or a
Sisu panic with a message) or a crash, a hang or a wrong answer.

Run every probe that allocates in proportion to its input under the
`ulimit` in the toolbox: other areas run on the same machine.

## Probes

Grow each until something changes, doubling the size, and report the size
where it did:

- recursion depth in a compiled program, plain and `-O2`: report the depth, and whether `-O2` turns the recursion into a loop;
- recursion depth inside `sisu`: deeply nested expressions, blocks, `if` chains, parentheses, array literals (`[[[[...]]]]`), `?.` chains, comparison chains;
- program size: thousands of functions, locals, classes and fields; one function of 100 000 statements;
- data size: `[0; n]` growing towards the `ulimit`, and an `n` that overflows `n * element size` (it must panic or fail cleanly, never allocate a short block);
- long lists and deep trees dropped at exit, along a field of the class's own type and along a field of another class (see known-findings);
- integer edges in every place an integer becomes a size or an index.

For `sisu`'s time, run with `timeout 60`, and record the first size at
which doubling the input more than triples the time: quadratic or worse
growth.

## Severity

A crash or a hang in `sisu` is `crash`. For a compiled program,
[finding.md](../finding.md) says which signals are findings: user recursion
that overflows the stack is not, a short block from an overflowed size is
`wrong`. A limit that is clean but low, such as a diagnostic at nesting
depth 50, is `gap`.
