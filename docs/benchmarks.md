# Benchmarks

These numbers compare Sisu with C, Rust, Go, Java and Python on small
single-threaded programs. They show where Sisu stands, not how fast it will
be. Each program uses the same algorithm in every language, written plainly,
with no per-language tuning.

## Results

All ran at commit 6ff7970 on 2026-10-25, on an Intel Core i9-14900K under
Linux 7.0 (Ubuntu 24.04, glibc 2.39). Each figure is the median of 9 runs,
pinned to one CPU. Each table runs from fastest to slowest wall time, with Sisu
in bold. The machine was quiet: its load average was below 2, on 32 CPUs, at
the start and at the end of the run.

### fib

| Language | Wall (s) | CPU (s) | Peak RSS (MiB) | Binary (KiB) | Build (s) |
| --- | ---: | ---: | ---: | ---: | ---: |
| Rust | 0.16 | 0.16 | 1.9 | 329 | 0.103 |
| C | 0.18 | 0.18 | 1.7 | 14 | 0.054 |
| **Sisu** | **0.25** | **0.25** | **6.0** | **539** | **0.052** |
| Go | 0.27 | 0.27 | 1.8 | 1483 | 0.053 |
| Java | 0.27 | 0.26 | 39.8 | - | 0.186 |
| Python | 4.71 | 4.71 | 10.7 | - | - |

### sieve

| Language | Wall (s) | CPU (s) | Peak RSS (MiB) | Binary (KiB) | Build (s) |
| --- | ---: | ---: | ---: | ---: | ---: |
| Rust | 0.17 | 0.17 | 49.4 | 329 | 0.097 |
| **Sisu** | **0.18** | **0.18** | **53.8** | **539** | **0.055** |
| C | 0.19 | 0.19 | 49.1 | 14 | 0.061 |
| Go | 0.20 | 0.19 | 49.8 | 1483 | 0.067 |
| Java | 0.24 | 0.24 | 88.5 | - | 0.203 |
| Python | 2.27 | 2.26 | 392.2 | - | - |

### binary_trees

| Language | Wall (s) | CPU (s) | Peak RSS (MiB) | Binary (KiB) | Build (s) |
| --- | ---: | ---: | ---: | ---: | ---: |
| Java | 0.36 | 0.35 | 333.8 | - | 0.201 |
| **Sisu** | **0.49** | **0.49** | **65.8** | **543** | **0.069** |
| C | 0.62 | 0.61 | 33.7 | 14 | 0.058 |
| Rust | 0.73 | 0.72 | 33.7 | 330 | 0.100 |
| Go | 1.34 | 1.34 | 46.8 | 1487 | 0.062 |
| Python | 18.66 | 18.65 | 107.6 | - | - |

### hello

| Language | Wall (s) | CPU (s) | Peak RSS (MiB) | Binary (KiB) | Build (s) |
| --- | ---: | ---: | ---: | ---: | ---: |
| C | 0.00 | 0.00 | 1.4 | 14 | 0.066 |
| Go | 0.00 | 0.00 | 1.8 | 1483 | 0.063 |
| Python | 0.00 | 0.00 | 10.7 | - | - |
| Rust | 0.00 | 0.00 | 1.7 | 328 | 0.082 |
| **Sisu** | **0.00** | **0.00** | **5.8** | **539** | **0.052** |
| Java | 0.01 | 0.00 | 39.1 | - | 0.185 |

### quicksort

| Language | Wall (s) | CPU (s) | Peak RSS (MiB) | Binary (KiB) | Build (s) |
| --- | ---: | ---: | ---: | ---: | ---: |
| C | 0.53 | 0.53 | 77.6 | 14 | 0.066 |
| Rust | 0.54 | 0.54 | 77.9 | 330 | 0.138 |
| Go | 0.55 | 0.54 | 78.3 | 1487 | 0.079 |
| **Sisu** | **0.56** | **0.56** | **83.8** | **543** | **0.059** |
| Java | 0.64 | 0.63 | 117.5 | - | 0.195 |
| Python | 12.85 | 12.84 | 393.7 | - | - |

### nbody

| Language | Wall (s) | CPU (s) | Peak RSS (MiB) | Binary (KiB) | Build (s) |
| --- | ---: | ---: | ---: | ---: | ---: |
| Rust | 0.11 | 0.11 | 1.9 | 358 | 0.094 |
| **Sisu** | **0.13** | **0.13** | **5.8** | **543** | **0.062** |
| C | 0.14 | 0.14 | 1.7 | 14 | 0.070 |
| Java | 0.20 | 0.19 | 43.8 | - | 0.216 |
| Go | 0.23 | 0.23 | 1.8 | 1495 | 0.073 |
| Python | 11.23 | 11.23 | 11.2 | - | - |

### mandelbrot

| Language | Wall (s) | CPU (s) | Peak RSS (MiB) | Binary (KiB) | Build (s) |
| --- | ---: | ---: | ---: | ---: | ---: |
| C | 0.58 | 0.58 | 1.7 | 14 | 0.059 |
| Rust | 0.58 | 0.58 | 1.9 | 329 | 0.069 |
| **Sisu** | **0.58** | **0.58** | **5.8** | **539** | **0.059** |
| Go | 0.59 | 0.59 | 1.8 | 1483 | 0.084 |
| Java | 0.61 | 0.60 | 40.5 | - | 0.195 |
| Python | 17.18 | 17.18 | 11.0 | - | - |

### fannkuch_redux

| Language | Wall (s) | CPU (s) | Peak RSS (MiB) | Binary (KiB) | Build (s) |
| --- | ---: | ---: | ---: | ---: | ---: |
| C | 0.11 | 0.11 | 1.7 | 14 | 0.066 |
| Go | 0.12 | 0.12 | 1.8 | 1495 | 0.081 |
| Rust | 0.12 | 0.12 | 1.7 | 331 | 0.135 |
| **Sisu** | **0.12** | **0.12** | **5.8** | **547** | **0.075** |
| Java | 0.16 | 0.16 | 42.8 | - | 0.205 |
| Python | 2.76 | 2.76 | 11.0 | - | - |

Toolchains: `sisu build -O2` (LLVM 23), Ubuntu clang 18.1.3 with `-O2`, rustc
1.99.0 with `-C opt-level=3`, Go 1.27.2, OpenJDK 25.0.4.1 (Temurin), Python
3.14.5.

## The programs

| Benchmark | What it does | What it measures |
| --- | --- | --- |
| [fib](../bench/fib) | Computes `fib(40)` by naive recursion | Function calls and integer arithmetic |
| [sieve](../bench/sieve) | Counts the primes up to 50 000 000 with a sieve of Eratosthenes | Array indexing in a tight loop |
| [binary_trees](../bench/binary_trees) | The [binary-trees](https://benchmarksgame-team.pages.debian.net/benchmarksgame/description/binarytrees.html) program of the Benchmarks Game, depth 18 | Allocating and freeing many small objects |
| [hello](../bench/hello) | Prints `Hello, World!` | The time from source to output: build and startup |
| [quicksort](../bench/quicksort) | Sorts 10 000 000 pseudo-random integers with a textbook quicksort, then prints a checksum that depends on their order | Element writes, and in Sisu whether an array passed and returned is written in place |
| [nbody](../bench/nbody) | The [n-body](https://benchmarksgame-team.pages.debian.net/benchmarksgame/description/nbody.html) program of the Benchmarks Game, 5 000 000 steps | `f64` arithmetic and `sqrt`, and field writes through shared objects |
| [mandelbrot](../bench/mandelbrot) | Counts the points of a 4000 by 4000 grid that stay in the Mandelbrot set for 50 iterations, the test of the Benchmarks Game's [mandelbrot](https://benchmarksgame-team.pages.debian.net/benchmarksgame/description/mandelbrot.html) without its bitmap output | `f64` arithmetic in a tight loop |
| [fannkuch_redux](../bench/fannkuch_redux) | The [fannkuch-redux](https://benchmarksgame-team.pages.debian.net/benchmarksgame/description/fannkuchredux.html) program of the Benchmarks Game, n = 10 | Reads and writes of small arrays, with short loops |

## What the numbers mean

- **Wall** is elapsed time. **CPU** is user plus system time, so the work of a
  garbage collector or JIT thread counts too. Both come from `/usr/bin/time`.
- **Peak RSS** is the most memory the process held at once.
- **Binary** is the size of the stripped executable. Java and Python have none:
  they need their runtime installed. A Sisu binary links the runtime
  (`libsisu_runtime_mimalloc.a`, written in Rust, with mimalloc inside)
  statically, which accounts for most of its size.
- **Build** is the wall time to compile the one file and link it, the median
  of 9 builds, to the millisecond.
- Every process runs whole, so startup counts. Java pays for JVM startup and
  JIT warm-up, Python for interpreter startup.
- `taskset` pins each run to one CPU. Go and the JVM therefore see a single
  CPU, and the JVM picks its serial garbage collector.
- The checks differ. Sisu checks both integer overflow and array bounds. Rust,
  Go and Java check bounds but not overflow; C checks neither.
- The inputs are constants in the source. No compiler folded a program to its
  answer: the run times grow with the input.

Java is the fastest on binary_trees, because its collector allocates by
bumping a pointer and frees short-lived objects in bulk. It pays for that with
five times the memory Sisu holds.

## Where binary_trees spends its time

Sisu took 1.4 times as long as C on binary_trees at commit 7c8d29e, and 1.2
times as long at 95cc196. At 6ff7970 it takes 0.79 times as long (0.49 s
against 0.62 s in the table above), because it now allocates with mimalloc
where C calls glibc's `malloc`. A run at 7d4d6d0 on a busy machine read 0.56
times, but C's time has since fallen from 0.98 s to 0.62 s with no change to
its program, so the load caused most of that gap. Callgrind counted the
instructions in each function, and each suspect was then removed by hand from
the program's IR (`sisu build -O2 --emit ir`) and the result timed. Each
bullet names the state it measured.

- **The allocators differ.** Both programs make 68.3 million allocations and
  as many frees. Sisu allocates with mimalloc
  ([ADR 0031](adr/0031-compiled-programs-allocate-with-mimalloc.md)), and C
  calls glibc 2.39's `malloc` and `free`. At commit 7d4d6d0 and `-O2`,
  callgrind counted 16.1 billion instructions for C, 12.4 billion of them
  (77%) in glibc's allocator, 181 for each allocation and free. Sisu runs 11.2
  billion, 4.7 billion of them (42%) in mimalloc and 2.3 billion (20%) in the
  runtime's `sisu_alloc` and `sisu_free`, 102 for each allocation and free
  together. Outside the allocator, Sisu runs 4.2 billion instructions to C's
  3.8. A Sisu node is 24 bytes, its count and two fields, against C's 16.
  Peak memory is about twice C's: 65.8 MiB against 33.7 MiB. The larger node
  explains at most 1.5 times. The rest has not been measured; mimalloc is the
  likely cause, because it holds 4.4 MiB more than glibc even in `hello`
  (5.8 MiB against 1.4 MiB).
- **A locked live count, now gone.** The runtime counted live objects for
  `--rc-checks` with an atomic add and subtract. On x86-64 each is a locked
  instruction and a full memory barrier, paid on every allocation and free.
  That was a quarter of the run: Sisu took 1.10 s before the runtime switched
  to a plain load and store.
- **`self` was passed owned.** Before `self.left?.check()` the caller adds 1
  to the child's count, and `check` subtracts 1 from `self` when it returns.
  No count reaches 0 there, but each call writes two nodes and calls
  `sisu_rc.release.Node`. With `self` borrowed instead, the run took 0.75 s
  instead of 0.84 s. Skipping the add is safe here because `left` and `right` are not `mut`:
  the node holds its children for as long as the caller holds the node.
  [ADR 0029](adr/0029-parameters-are-borrowed-unless-consumed.md) borrows it,
  and binary_trees took 0.72 s with glibc's allocator. It takes 0.55 s now.
- **`?.` and `??` build an optional.** `self.left?.check() ?? 0` makes an
  `i64?` and then picks from it, where C branches once. With the counting
  removed, `check` still runs 1.8 times C's instructions, but they are
  register operations that the CPU predicts well.
- **The runtime wraps the allocator.** `sisu_alloc` validates the layout and
  calls Rust's allocator, which calls mimalloc; `sisu_free` does the same.
  With glibc, a wrapper that called `malloc` directly ran no faster, so
  link-time optimization with the runtime (roadmap) looked like a small gain.
  With mimalloc, callgrind counted 28 instructions a call in `sisu_alloc` and
  5 in `sisu_free`, 20% of the program's instructions at commit 7d4d6d0. No
  direct-call wrapper has been timed against mimalloc.

Sisu is now ahead of C on this program, because mimalloc's allocation and free
take fewer instructions than glibc's. What Sisu pays that C does not is the
counting itself: a store when an object is made, and a load, subtract and
store when it is released. ADR 0005 accepts that cost.

## quicksort on an array value

A Sisu function cannot change its caller's array (ADR 0007), so `quicksort`
takes the array and returns it, and recurses with
`a = quicksort(a, lo, i - 1)`. Last use hands the array over, so each call
holds the only reference and every element write happens in place. Peak
memory shows it: Sisu holds 83.8 MiB, against C's 77.6 MiB for its one array
of 10 000 000 `i64`. The extra 6 MiB is mimalloc's: `hello` holds 5.8 MiB
where C's holds 1.4 MiB. A copy per call would add 80 MB.

Each element write checks the index against the bounds and the array's
count against 1, and each addition checks for overflow. To find what each
check costs, each kind was removed by hand from the program's IR
(`sisu build --emit ir-raw`), and the result built with the same `default<O2>`
pipeline. hyperfine timed every build in one session, pinned to one CPU, as
the mean of 15 runs, and callgrind counted the instructions. "Before" is
commit c75ef2d; "after" is 3dbda29, which adds the change described below.

| Build | Mean (ms) | Times C | Instructions (billions) |
| --- | ---: | ---: | ---: |
| C, for comparison | 542 | 1.00 | 2.82 |
| Before | 688 | 1.27 | 6.44 |
| Before, overflow checks removed | 709 | 1.31 | 6.25 |
| Before, count checks removed | 628 | 1.16 | 5.07 |
| Before, bounds checks removed | 628 | 1.16 | 4.35 |
| Before, all three removed | 545 | 1.01 | 2.95 |
| Before, TBAA tags only | 682 | 1.26 | 6.28 |
| Before, reordered `unique` only | 685 | 1.26 | 6.68 |
| After | 622 | 1.15 | 4.82 |
| After, count checks removed | 582 | 1.07 | 3.99 |
| After, bounds checks removed | 586 | 1.08 | 3.57 |

A later session, on a busy machine, measured loop hoisting (ADR 0021). Its ms
do not compare with the rows above, so it has its own table and its own C
baseline. Each binary ran 40 times in turn, pinned to CPU 2 with `taskset`.
The mean and the median swung by 30% under the load, so the minimum is the
figure to read; the instruction counts come from callgrind and do not vary.

| Build | Min (ms) | Median (ms) | Mean (ms) | Times C (min) | Instructions (billions) |
| --- | ---: | ---: | ---: | ---: | ---: |
| C, for comparison | 610 | 760 | 958 | 1.00 | |
| Before loop hoisting (552dbac) | 690 | 870 | 1078 | 1.13 | 4.82 |
| After loop hoisting (cf07f6d) | 640 | 790 | 988 | 1.05 | 4.04 |

552dbac is main before loop hoisting, and cf07f6d is the commit that adds
it. The timings were taken on the same changes before they were rebased
onto 552dbac. The counts reproduce on the rebased branch: callgrind counts
4,822,781,453 instructions at 552dbac and 4,043,218,655 at 92bec34, with
the same checksum. 4.82 billion is also the "After" row above.

The checks are the whole gap to C. The overflow checks cost no measurable
time: without them the program ran fewer instructions but no faster.

Some checks were redundant. In the swap `a[i] = a[j]` then `a[j] = t`, the
second write checked the count and the length of the array the first write
had just made unique. LLVM kept those checks for two reasons:

- It could not tell that storing an element leaves the count and the length
  alone.
- On the copy path of `sisu_rc.unique`, the old block's count was written
  after the copy's header. LLVM could not prove that the old block was not
  the copy, so it could not prove that the copy's count was still 1.

Codegen now tags the count and the length apart from the elements (TBAA,
type-based alias analysis), and `unique` releases the old block before it
allocates the copy. Either change alone keeps those checks and gains no
time. Together they remove them: quicksort takes 622 ms instead of 688, and
runs 25% fewer instructions. Marking `sisu_alloc`'s result `noalias` in place
of the reorder removes none of them.

Two costs remained: the count checks, which removing by hand would have
taken the time from 622 to 582 ms, and the bounds checks. A loop now makes
its written arrays unique once before it starts (ADR 0021). The count check
had made the array's pointer a phi in the loop (the helper may store a new
block into the local), so LLVM could neither hoist the check nor the length
load. With the check outside the loop, the quicksort benchmark runs 4.04
billion instructions in all, not 4.82, and its fastest run takes 640 ms
instead of 690 (C: 610 ms, in the same session). In the `-O2` IR of `quicksort`, one count check is
left in each call, before the partition loop. The writes in the loop and the
writes after it run without one.

What remains are the bounds checks, which LLVM cannot remove: `lo` may be
negative, so nothing shows `j` is below the length. Removing them by hand
took the time from 622 to 586 ms before the change above. Proving a range
fact about the loop is a roadmap entry.

## Where a hello build spends its time

The Build column of hello is the time from source to a program, and every
program then starts in under a millisecond. hyperfine timed the steps on
commit 4690caa, pinned to one CPU, as the mean of 50 runs after 5 warm-up
runs (`taskset -c 2 hyperfine -N -w 5 -r 50 ...`):

| Command | Mean (ms) |
| --- | ---: |
| `sisu check main.sisu` | 1.4 |
| `sisu build -O2 main.sisu -o main` | 27.6 |
| `cc main.o libsisu_runtime.a -lm -o main`, the link step alone | 21.9 |
| `./main` built by `sisu` | 0.2 |
| `./main` built by `clang` | 0.2 |
| `python3.14 main.py` | 5.3 |

- Lexing, parsing and checking take 1.4 ms.
- The link through `cc`, which `sisu` runs as its last step, takes about
  22 of the 28 ms. A faster edit-to-output loop starts there, not in the
  compiler.
- `scripts/bench.sh` does not pin or warm up its builds, so its Build column
  reads higher.

## Benchmarks for CodSpeed only

`bench/features/` holds Sisu-only programs sized for CodSpeed's simulation.
They have no ports to other languages and no place in the tables above. Each
must print values that depend on all of its work, and must scale with its
input.

### What CodSpeed covers

This table lists each feature or path and the benchmark that measures it:

| Feature or path | Benchmark |
| --- | --- |
| Calls, integer arithmetic | fib |
| Array indexing in a loop | sieve |
| Class allocation and release, `?.` and `??` | binary_trees |
| Writes to an array with one holder, passed and returned | quicksort |
| Copy-on-write of an array with a live copy, and `push` onto an array and onto one with a live copy | shared_arrays |
| String `==` on array elements and class fields that stay live | compare |
| `f64` arithmetic | nbody, mandelbrot |
| Short loops over small arrays | fannkuch_redux |
| String `+=`, interpolation with format specs, `words`, `chars`, `bytes`, non-ASCII text | strings |
| Enum payloads under `match`: inline in an array of optionals, whose `None` is a spare tag, and boxed in a recursive tree | enum_match |
| A match that takes boxes apart and builds new ones of the same enum | tree_rewrite |
| Text scanned from `bytes()` into a boxed tree, rewritten and evaluated | interp |
| Calls through a bound, static at two conforming classes, and an associated type read through a bound | interfaces |
| Printing compound values; `==` through a written `Eq` | show |
| Lexing as a real program does it: `read_line`, `chars()`, an enum of tokens with a generated `Display`, `Stack<T>` over a linked class, an interface bound, and `push` onto arrays, on a fixed corpus of `.sisu` files | selfhost_lexer |
| The front end | check_binary_trees, check_large |
| An `-O2` build | build_hello, build_binary_trees, build_selfhost_lexer, build_large_20; build_large on `main` only |

## Running them

```sh
make bench                       # all benchmarks
make bench BENCH='fib sieve'     # some of them
RUNS=10 BENCH_CPU=4 make bench
```

The script builds `sisu` in release mode and then runs each program in every
language whose toolchain is on `PATH`. It stops if a program prints something
other than the Sisu version, and it prints the tables above with the commit,
date and machine. It needs `clang`, `rustc`, `go`, `javac` and `java`, and
`python3.14` (or the interpreter `PYTHON` names); any that is missing is
skipped. If mise installs Go and Java, put them on `PATH` with
`mise exec go java -- make bench`. Build outputs go to `target/bench/`.

The numbers depend on the machine. Rerun the script and replace the tables when
a change to `sisu` or the runtime should move them.

To add a benchmark, add a directory under `bench/` with `main.sisu` and, for
each language, `main.c`, `main.rs`, `main.go`, `Main.java` and `main.py`, all
printing the same output. Then add its name to the default list in
`scripts/bench.sh`.

CodSpeed tracks Sisu against itself on every pull request that touches the
code; see [Coverage and performance](development.md#coverage-and-performance).
