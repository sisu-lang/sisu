# Area: crashes

Two targets. `sisu` must end every input with exit 0, or exit 1 and an
`error:` diagnostic; never a Rust panic, a signal, a hang, or an internal
failure (`sisu: ...`) on a program its checker accepted. A compiled program
must end with 0, 101 (a Sisu panic) or, under `--rc-checks`, 102; the
signals that are not crashes are listed in [finding.md](../finding.md).

## Crash sisu

Oracle, for every input:

- `sisu check p.sisu` exits 0 or 1. Exit 1 prints at least one `error:`; any other exit, or `panicked at` on stderr, is `crash`.
- When `sisu check` exits 0, each of `sisu build p.sisu`, `sisu build -O2 p.sisu -o p2`, `sisu build --rc-checks p.sisu -o prc` and `sisu build --emit ir-raw p.sisu` exits 0. Any other exit is `crash`.

Mutate real programs and feed each mutant through that oracle. Seeds:
every `.sisu` under `crates/sisuc/tests`. Mutations, each applied at many
positions:

- delete a token, a line, or the rest of the file (truncation);
- duplicate a token or a line; swap two adjacent tokens;
- replace a token with each keyword, each operator, each bracket;
- insert stray bytes: `\0`, a lone `\r`, a tab, invalid UTF-8, a BOM, a non-ASCII identifier, a 10 000-character identifier or literal;
- unbalance brackets; nest brackets 10 000 deep;
- an empty file, a file of only comments, a file with no trailing newline.

Run thousands of mutants: write a generator (toolbox, "Program generation")
and loop over its output. Record every oracle failure and every timeout.
Shrink each one by deleting lines and tokens while it still fails.

The likeliest crash is a program the checker accepts that codegen did not
expect. Search `crates/sisuc/src/codegen` for `panic!`, `expect(`,
`unreachable!` and indexing; for each one, find the checker rule it relies
on and write a program that slips past that rule.

## Crash a program

A checked program must never die by a signal finding.md does not excuse.
Probe out-of-bounds indexes of every sign and size (`-1`, `len`, `i64::MAX`),
`[x; n]` with a negative or huge `n` (under `ulimit`, toolbox), `None`
reaching every operator that accepts an optional, `u8(...)` of out-of-range
values, division and remainder by 0 and of `i64::MIN` by `-1`, and empty
arrays through `sum` and `contains`.

## Not a finding

Stack overflow from deep recursion goes to `limits`. A Sisu panic (101)
with a correct message is the intended result.
