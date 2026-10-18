# Toolbox

Commands every area uses. Shell variables do not survive between tool
calls, so begin each command with the literal paths:

```console
$ S=<repo>/target/debug/sisuc L=<repo>/.llvm/23/bin D=<RUN>/<area>/<probe>; mkdir -p $D
```

The commands below assume that line, with `p.sisu` written in `$D`. They
name every file through `$D` and never `cd`: in Claude Code, a `cd` out of
the repository moves the session's working directory, and later commands
run in the wrong checkout.

## Exit codes

| Code | From | Means |
| --- | --- | --- |
| 0 | `sisuc` or a program | success |
| 1 | `sisuc` | an `error:` diagnostic; or a usage error, an unreadable file, or an internal failure (`sisuc: ...`, such as a verifier or link error). After `--check` passes, only 0 is right |
| 101 | a program | a Sisu panic: `sisu: panic at <file>:<line>:<col>: ...` on stderr |
| 101 | `sisuc` | `sisuc` itself panicked: `panicked at` on stderr. Always a finding |
| 102 | a program built with `--rc-checks` | a failed count check, or live objects at exit: `sisu: rc check:` |
| 124 | `timeout` | over the time limit: a lead until a larger limit or a smaller input shows it hangs |
| 128+N | `sisuc` | killed by signal N, an LLVM crash. Always a finding |
| 128+N | a program | killed by signal N: 139 a segfault, 134 an abort, 141 a closed pipe. [finding.md](finding.md) says which are findings |

## Time and memory limits

- `timeout 20` on every `sisuc` call and plain program run.
- `timeout 120` under Valgrind, ASan or Miri.
- `limits` timing probes: `timeout 60`, as its guide says.
- Cargo commands get no timeout.
- Any probe that allocates in proportion to an input you grow runs under `ulimit -v 4194304` (4 GiB) in a subshell: `(ulimit -v 4194304; timeout 20 $D/p)`. Other subagents share the machine.

## Build and run

```console
$ timeout 20 $S $D/p.sisu $D/p && timeout 20 $D/p; echo $?                  # plain
$ timeout 20 $S -O2 $D/p.sisu $D/p2 && timeout 20 $D/p2; echo $?            # -O2
$ timeout 20 $S --rc-checks $D/p.sisu $D/prc && timeout 20 $D/prc; echo $?  # count checks
$ timeout 20 $S -O2 --rc-checks $D/p.sisu $D/prc2 && timeout 20 $D/prc2     # both; -O2 comes first
$ timeout 20 $S --check $D/p.sisu                                       # diagnostics only
```

`--rc-checks` misses most uses after free: freed memory may still hold a
count of 1 or more. Valgrind and ASan find those.

## Memory

Valgrind, with the flags the `cases` harness uses. The `total heap usage: N
allocs` line in its summary counts the blocks the program allocated:

```console
$ timeout 120 valgrind --leak-check=full --errors-for-leak-kinds=definite,indirect,possible --error-exitcode=1 $D/p
```

AddressSanitizer, with LeakSanitizer. `sisuc` has no flag for it, so mark
each function and instrument the raw IR with `opt`:

```console
$ timeout 20 $S --emit ir-raw $D/p.sisu > $D/p.ll
$ sed -i -E 's/^(define [^{]*\)) \{/\1 sanitize_address {/' $D/p.ll
$ $L/opt -passes=asan $D/p.ll -S -o $D/p.asan.ll                 # or -passes='asan,default<O2>'
$ $L/clang -fsanitize=address $D/p.asan.ll <repo>/target/debug/libsisu_runtime.a -o $D/p.asan
$ timeout 120 $D/p.asan
```

The "Redundant instrumentation detected" warning from `clang` is harmless.
The runtime itself is not instrumented, and its `sisu_free` frees through
plain `free()`, so neither ASan nor Valgrind sees a free with the wrong size
or alignment. The layout shim does:

```console
$ timeout 20 $S --emit ir-raw $D/p.sisu > $D/p.ll
$ $L/clang $D/p.ll <repo>/.agents/skills/audit-sisu/layout-shim.c <repo>/target/debug/libsisu_runtime.a -Wl,--wrap=sisu_alloc,--wrap=sisu_free -o $D/p.shim
$ timeout 20 $D/p.shim        # aborts with "layout-shim: ..." on a mismatch
```

## IR

```console
$ timeout 20 $S --emit ir-raw $D/p.sisu > $D/p.ll; echo $?          # check the exit code first
$ $L/opt -passes=lint -disable-output $D/p.ll                     # undefined-behavior patterns
$ $L/opt -passes=simplifycfg $D/p.ll -S | $L/opt -passes=lint -disable-output
$ timeout 20 $S -O2 --emit ir $D/p.sisu > $D/p.o2.txt                # before and after default<O2>
$ $L/opt -passes='default<O2>' -pass-remarks-missed='licm|loop-vectorize|inline|gvn' -disable-output $D/p.ll
$ timeout 120 valgrind --tool=callgrind --callgrind-out-file=$D/cg.out $D/p2   # instruction count: the "Ir" total
```

`--emit ir-raw` runs LLVM's verifier before it prints: a module the verifier
rejects gives exit 1 and `sisuc: ...` on stderr, not IR. Neither `--emit`
mode takes `--rc-checks`, so the IR of the count checks can only be tested
by running `--rc-checks` builds.

The optimized half of `--emit ir` starts after the line `; after default<O2>`.
Each Sisu function `f` is `@sisu.f`, each method `@sisu.<Class>.<method>`, and
the count helpers are `@sisu_rc.retain`, `@sisu_rc.release.<T>`,
`@sisu_rc.drop.<T>`, `@sisu_rc.eq.<T>` and `@sisu_rc.unique.<T>`. Names
with brackets print quoted: `@"sisu_rc.release.[i64]"`.

## Runtime under Miri

Miri runs on the nightly toolchain and cannot spawn processes, so the tests
that run a child process fail under it. Run the in-process tests, then each
child case directly:

```console
$ cargo +nightly miri test -p sisu-runtime -- alloc_and_free_round_trip alloc_honours
$ SISU_RUNTIME_RC_CASE=leak MIRIFLAGS=-Zmiri-disable-isolation cargo +nightly miri test -p sisu-runtime -- --exact tests::rc_child
```

The child cases and their env vars are in the `tests` module of
`crates/runtime/src/lib.rs`. An exit code of 102 or 101 from a child is its
intended result; a Miri `Undefined Behavior` error is a finding.

## Mutants

From the repository root. cargo-mutants copies the tree without `.llvm/`, so
the LLVM prefix must be absolute, and `-o` keeps its output out of the
repository:

```console
$ LLVM_SYS_231_PREFIX=$PWD/.llvm/23 cargo mutants -f crates/sisuc/src/last_use.rs --list
$ LLVM_SYS_231_PREFIX=$PWD/.llvm/23 cargo mutants -f crates/sisuc/src/last_use.rs -j 4 -o <RUN>/coverage/last_use
```

Each mutant runs the whole test suite, Valgrind cases included, so a file
takes a long time. `scripts/mutants.sh` mutates a branch's diff from `main`;
on `main` itself it has nothing to mutate. A killed run leaves
`$TMPDIR/cargo-mutants-*.tmp` behind; delete those when you stop one.

## Coverage

```console
$ make coverage BASE=$(git rev-list --max-parents=0 HEAD)
```

It runs the whole suite under instrumentation, so it takes minutes. The first line gives the totals. "Touching changed code" lists the uncovered regions inside the diff; "Other" lists the rest of the changed files. With the first commit as base, every file counts as changed.

## Program generation

Write small generators into `$D` that print Sisu programs rather than typing
programs by hand; run Python with `python3 -I`. Seeds come from
`crates/sisuc/tests/run`, `run-fail` and `diagnostics`. Keep the generator
and its seed with each finding.

A reference implementation (Python or Rust) used as an oracle must keep
Sisu's semantics: `i64` and `u8` arithmetic that panics on overflow,
division and remainder that panic on 0 and on `i64::MIN / -1`, bounds
checks, and left-to-right evaluation. On a panic it prints the same stdout
first and exits 101. `generated_arithmetic_matches_rust` in
`crates/sisuc/tests/programs.rs` does this with `checked_*` operations.
