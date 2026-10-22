# Area: compile time

How fast `sisu` itself is: work the compiler does that it could skip. The
cost of the code it emits belongs to `ir`, and growth that runs into a
timeout belongs to `limits`.

## Setup

The debug `sisu` runs unoptimized and certifies every mir function, so
measure a release build. Build one with line tables, in a target directory
of its own so that `target/release` stays as it is. From the repository
root:

```console
$ CARGO_PROFILE_RELEASE_DEBUG=line-tables-only CARGO_PROFILE_RELEASE_STRIP=none cargo build --release --locked -p sisuc -p sisu-runtime-mimalloc --target-dir <RUN>/compile-time/target
```

In this area, `P=<RUN>/compile-time/target/release/sisu` takes the place of
`S`, in probes and in Repro blocks. `sisu` finds the runtime next to
itself, and links `libsisu_runtime_mimalloc.a` by default, which is why the build
names `sisu-runtime-mimalloc` too.

Generate the inputs. `large_input` in `scripts/codspeed-prep.sh` is the
program CodSpeed's `check_large` and `build_large` benchmarks compile: copy
its section into a generator in `$D` and grow the section count. Also use
`bench/*/main.sisu` and the hello-world program, whose build is mostly fixed
cost.

## Phase split

A callgrind run of one build attributes every instruction to a phase. Run
callgrind under `timeout 600`: a `-O2` build of 20 sections is about 1.9
billion instructions.

Give each run a fresh `$D`, so its `cg.*` files do not mix with another
run's.

```console
$ timeout 600 valgrind --tool=callgrind --trace-children=yes --callgrind-out-file=$D/cg.%p $P build -O2 $D/p.sisu -o $D/p
$ grep -H '^cmd:' $D/cg.*                    # one file per process; its cmd: line names it
$ callgrind_annotate --inclusive=yes --threshold=100 $D/cg.<sisu pid> 2>/dev/null | grep -E 'PROGRAM TOTALS|=> .*sisuc::(front_end::front_end|mir::build|codegen::compile) \(|^ *[0-9,]+ \([ 0-9.]+%\)  \?\?\?:LLVM(RunPasses|TargetMachineEmitToMemoryBuffer|VerifyModule) '
$ callgrind_annotate --inclusive=yes --threshold=100 $D/cg.<sisu pid> 2>/dev/null | grep -E 'PassModel<.*>::run' | grep -vE 'PassManager<|Adaptor|WrapperPass|RepeatedPass' | head -20
```

The phase command prints one line per phase. `sisu` owns the front end,
`mir::build` and `codegen::compile`; their lines are calls in the annotated
source. LLVM owns the passes (`LLVMRunPasses`), object emission
(`LLVMTargetMachineEmitToMemoryBuffer`) and the verifier; their lines are
totals over every call, so the verifier's line counts each time it runs.
`codegen::optimize` holds both the passes and one verifier run: leave it
out of a sum. The `ld` process is the link. `--threshold=100` keeps every
function in the list; the default cuts it after the first under 1%, which
can drop the verifier.

The `PassModel` command lists LLVM's passes. Its counts are inclusive and
nest, so it drops the pass managers and adaptors that wrap the rest.

A function inlined into its caller has no line. Its callees still show,
and `--tree=caller` gives each callee's callers and their share.

Record each split in `probes.md` as instruction counts, at the default
level (no `-O` flag) and at `-O2`, for at least two sizes of the large
program. A cost claim needs a count, not a wall time.

## Probes

- **Repeat work on the clean path.** A step that runs twice when its result is the same: LLVM's verifier, a walk over the whole program, a lex or parse of the source again, a type lookup that a pass recomputes per use. Count the calls in the callgrind output (`--tree=caller`).
- **IR volume.** Bytes of `--emit ir-raw` per byte of source, and what `-O2` deletes: helpers nothing calls, blocks no path reaches, copies a later pass removes. LLVM spends its passes on all of it. Which pass costs most (the `PassModel` command above) tells where the volume hurts: the inliner on many small functions, InstCombine on long instruction lists. Code that `-O2` deletes belongs here; code that survives `-O2` belongs to `ir`.
- **Allocations in `sisu`'s own phases.** Run DHAT on `check` and on `build --emit mir`: `timeout 600 valgrind --tool=dhat --fullpath-after= --dhat-out-file=$D/dhat.json $P check $D/p.sisu` (without `--fullpath-after=`, frames name only a file's basename). Its summary gives the total blocks. In the JSON, each entry of `pps` is an allocation point, with `tbk` (blocks), `tb` (bytes) and `fs` (indexes into the frame table `ftbl`); sort by `tbk` with a short `python3 -I` script, and print each point's first frame under `crates/sisuc` (the frames above it are the allocator's). An allocation per token, per node or per lookup is a lead.
- **The Performance rules of `CODING_STANDARDS.md`.** A `clone()`, `to_string()`, `to_vec()` or `format!` on a per-token, per-node or per-lookup path with no reason given; a std `HashMap` or `HashSet` in `crates/sisuc`. Each becomes a finding only when callgrind shows its function's cost.
- **Growth by phase.** Double the section count three times and divide each phase's count by the one before. Over 2.2 is a lead even when the whole build stays under its timeout: `limits` sees only the total.
- **Fixed cost.** The split of the hello-world build: target machine setup, runtime declarations, the link. Every build pays it once.
- **Failing late.** Work done before an error that could have come first: a missing runtime library, an output path that cannot be written, a `--linker` that is not installed. Count the instructions spent before the error.
- **The link.** Its share of the build, and the wall time with each `--linker` that is installed. A faster linker is a lead for the user, not a finding.

## Severity

Avoidable compile-time work is `slow`. The finding gives the instruction
counts of the phase before and after the suggested fix, and its suggested
test is structural, never a time limit: a unit test that the step runs once,
or that a helper is not emitted. A `sisu` that hangs is `crash`.
