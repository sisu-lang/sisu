# A large program splits into codegen units by default

Status: accepted, 2026-11-04.

Almost all of the time of `sisu build` on a large program is LLVM, and LLVM
runs on one thread. On a 14k-line program at `-O2`, the optimizer and the
object writer take 3.6 s of 4.0 s. Sisu now splits a large program into
**codegen units**: modules that LLVM optimizes and writes on their own
threads. `sisu` links their objects into one executable.

## Decisions

1. **A split by default, at every level.** A plain `sisu build`, `sisu build
   -O2`, `sisu build -O3` and `sisu run` split a large program. One size rule
   applies at `-O0` too, although `-O0` could split smaller programs at no
   run-time cost: one rule needs one set of tests.
2. **`--codegen-units 1` gives one module**, as before this decision. That
   build optimizes the whole program and gives the fastest program. The flag
   takes 1 to 64, and it is a maximum: a program with fewer functions gets one
   unit for each function.
3. **The count comes only from the program**, never from the core count. A
   machine with fewer cores runs the same units on fewer threads, and the
   executable is the same bytes. There is no `auto` value and no environment
   variable that sets the count.
4. **The assignment is greedy and reads only the program.** The largest
   function goes first, each one to the unit with the least total size, ties
   broken by function index and unit index.
5. **Small callees are copied at `-O2` and `-O3`.** A unit that calls a small
   function of another unit gets an `available_externally` copy of it, and of
   the small functions that the copy calls, so LLVM can inline it.
6. **No LTO now.** Sisu builds the whole program from source into one LLVM
   module, so the one-module build already optimizes across the whole program.
   There is no `--lto` flag.

## Calibrations kept in the code

These numbers live in the `units` module of codegen. A change to them
needs no new ADR:

- `UNIT_SIZE`: the mir size that matches 25k raw IR instructions. A program
  gets one unit for each `UNIT_SIZE`.
- The 50k limit: a program under `2 * UNIT_SIZE` stays one module. Every bench
  program and `selfhost/lexer.sisu` stay one module.
- `MAX_UNITS = 8`: the spike showed little gain past 8 units, even at 143k
  lines.
- `SMALL`: the mir size under which a callee is copied, about 40 raw IR
  instructions.

## Consequences

- **External linkage costs some speed in a split `-O2` build.** Each function
  that a unit defines is external (with hidden visibility), so other units can
  call it. LLVM cannot delete a function that it inlined into every caller,
  and dead-argument elimination, IPSCCP and argument promotion no longer apply
  to such a function. The copies of small callees get back most of the loss:
  with them, the bench programs forced into 8 units ran at most 6% slower in
  the spike, against up to 19% with no copies.
- **Users must find it easy to change the unit count**, and LTO once it
  exists. When projects arrive, `sisu.toml` gets a setting for both
  (`docs/register.md`).
- **What will need LTO later:** separate compilation or a build cache for
  packages (the most likely), ThinLTO across codegen units, inlining the
  runtime's functions into Sisu code, and C libraries through FFI.
- **An object cache needs a stable assignment.** A cache that reuses the
  object of an unchanged unit needs each function to stay in its unit when
  another function changes, for example a unit chosen from the function's
  name or module. The greedy rule moves functions on every edit, so such a
  cache would replace it.

## Considered options

Measured on a 14k-line program at `-O2`, 8 units, LLVM time plus link; the
run-time loss is the worst bench program forced into 8 units:

| Option | Build | Run-time loss | Verdict |
| --- | --- | --- | --- |
| One module | 2.52 s | none | Kept as `--codegen-units 1` |
| Split, nothing copied | 0.40 s | up to +19% | Rejected: too slow at run time |
| Split, small callees copied | 0.71 s | up to +6% | **Chosen** |
| Split and ThinLTO through `clang` and `ld.lld` | 1.05 s | up to +12% | Rejected for now: lld for every link, or a C++ shim; the C API writes no ThinLTO summary |
| Split and ThinLTO through libLTO | not measured | | Rejected for now: `.llvm/23` has no `libLTO.so` |
| Split and fat LTO | 1.82 s | up to +17% | Rejected: the link is single-threaded and gives back most of the gain |

At 143k lines, one module took 27.5 s and 16 units with nothing copied took
2.8 s. A codegen pass once, then a bitcode round trip into each thread, was
also rejected: the round trip costs about 0.09 s on each thread at 14k lines,
which is most of the `-O0` gain at that size, and codegen itself stays on one
thread.
