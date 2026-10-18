# Writing a finding

A finding is a defect you reproduced, not a suspicion. A doubt you could
not turn into a failing run is a lead: one line in a `## Leads` list at the
end of the findings file.

## Severity

Ranked from worst; the report orders findings this way.

| Severity | Means |
| --- | --- |
| `wrong` | A program the checker accepts prints the wrong thing or exits with the wrong code; `-O0` and `-O2` disagree; the checker accepts a program the implemented language forbids or rejects one it allows; memory is corrupted (use after free, double free, out-of-bounds access, a free with the wrong layout); a size overflows into a short allocation |
| `crash` | `sisuc` panics, dies by a signal, hangs, or fails (exit 1 with `sisuc: ...`) on a program `--check` accepts; a compiled program dies by a signal, except the cases below |
| `leak` | An object outlives its last reference in a program with no reference cycle, at exit or while the program runs |
| `unsound` | IR the verifier rejects, or undefined behavior on a path a Sisu program can reach, that has not yet misbehaved |
| `slow` | Sound code with avoidable, measured cost: a redundant retain and release pair, a check `-O2` cannot remove, a missed attribute, dead code emitted |
| `gap` | A confusing or misplaced diagnostic, a low but clean limit, or behavior no test would notice breaking |

Signals that are not `crash`:

- 134 with Rust's `memory allocation of N bytes failed`, for a size the program really asked for (`largest_array_reaches_the_allocator` in `crates/sisuc/tests/programs.rs` expects it). From a size that overflowed, it is `wrong`.
- 139 from user recursion that runs out of stack, and from the drop shapes [known-findings.md](known-findings.md) lists. A stack overflow while dropping along a class's chain field (its last own-type field, which drops in a loop) is `crash`.
- 141 when stdout's reader closes early (`$D/p | head -c 0`): the program keeps the default `SIGPIPE` action, as a C program does. Report it as `gap` until `known-findings.md` rules on it.

A lint warning is a lead until you show the operation is reachable from a
Sisu program; LLVM's lint also flags blocks that never run.

## Template

````markdown
### <area>-<N>: <one-line claim>

Severity: <severity>

Repro:

```console
$ S=<repo>/target/debug/sisuc L=<repo>/.llvm/23/bin D=<a fresh directory>; mkdir -p $D
$ cat > $D/p.sisu <<'EOF'
<the shrunk program>
EOF
$ <every command that builds and runs it, with its timeout, naming files through $D>
```

Observed: <exact output and exit code, trimmed to the decisive lines>
Expected: <what should happen, and the source that says so: docs/roadmap.md "What works today", an implemented ADR, docs/development.md, or Rust or LLVM semantics>

Root cause: <file:line and function>, <confirmed by reading the code | guess>
Suggested fix: <what to change, where; small enough to judge>
Suggested test: <path, such as crates/sisuc/tests/run/<name>.sisu, and its full text with the expected .out or .err; or a unit test in the module's tests>
Test checked: <how you showed the test behaves as required below>
````

The Repro block rebuilds everything from the program text, so it runs in
any fresh directory. For a generated program, add the generator and its seed
below the block.

Shrink each program to the smallest one that still fails. The suggested
test for a defect must fail on today's code. The suggested test for a
coverage gap must pass on today's code and fail with the surviving mutant
or the change you name applied.
