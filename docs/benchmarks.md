# Benchmarks

These numbers compare Sisu with C, Rust, Go, Java and Python on small
single-threaded programs. They show where Sisu stands, not how fast it will
be. Each program uses the same algorithm in every language, written plainly,
with no per-language tuning.

## Results

fib and sieve ran at commit 9cbd44e and binary_trees at 7c8d29e (the change
between them is the runtime's live count), on 2026-10-12. hello ran at
commit 4690caa and quicksort at 3dbda29, on 2026-10-15; mandelbrot and
fannkuch_redux at d8a9603, and nbody at 10bb08f, on 2026-10-16.

All ran on an Intel Core i9-14900K under
Linux 7.0. Each figure is the median of 5 runs, pinned to one CPU. Each table
runs from fastest to slowest wall time, with Sisu in bold.

### fib

| Language | Wall (s) | CPU (s) | Peak RSS (MiB) | Binary (KiB) | Build (s) |
| --- | ---: | ---: | ---: | ---: | ---: |
| Rust | 0.17 | 0.17 | 1.9 | 329 | 0.06 |
| C | 0.18 | 0.18 | 1.4 | 14 | 0.04 |
| **Sisu** | **0.26** | **0.25** | **1.4** | **317** | **0.03** |
| Java | 0.27 | 0.26 | 40.3 | - | 0.19 |
| Go | 0.28 | 0.28 | 1.8 | 1483 | 1.88 |
| Python | 5.18 | 5.16 | 11.0 | - | - |

### sieve

| Language | Wall (s) | CPU (s) | Peak RSS (MiB) | Binary (KiB) | Build (s) |
| --- | ---: | ---: | ---: | ---: | ---: |
| Rust | 0.18 | 0.18 | 49.4 | 329 | 0.14 |
| C | 0.20 | 0.20 | 48.6 | 14 | 0.05 |
| Go | 0.21 | 0.21 | 49.8 | 1483 | 0.19 |
| **Sisu** | **0.23** | **0.23** | **48.9** | **317** | **0.04** |
| Java | 0.24 | 0.23 | 89.1 | - | 0.20 |
| Python | 2.29 | 2.29 | 392.5 | - | - |

### binary_trees

| Language | Wall (s) | CPU (s) | Peak RSS (MiB) | Binary (KiB) | Build (s) |
| --- | ---: | ---: | ---: | ---: | ---: |
| Java | 0.37 | 0.36 | 334.0 | - | 0.19 |
| C | 0.62 | 0.61 | 33.4 | 14 | 0.05 |
| Rust | 0.72 | 0.71 | 33.9 | 330 | 0.13 |
| **Sisu** | **0.85** | **0.85** | **33.4** | **317** | **0.05** |
| Go | 1.33 | 1.32 | 46.3 | 1487 | 0.08 |
| Python | 19.22 | 19.19 | 107.2 | - | - |

### hello

| Language | Wall (s) | CPU (s) | Peak RSS (MiB) | Binary (KiB) | Build (s) |
| --- | ---: | ---: | ---: | ---: | ---: |
| C | 0.00 | 0.00 | 1.1 | 14 | 0.053 |
| Go | 0.00 | 0.00 | 1.9 | 1483 | 0.037 |
| Python | 0.00 | 0.00 | 10.5 | - | - |
| Rust | 0.00 | 0.00 | 1.9 | 328 | 0.073 |
| **Sisu** | **0.00** | **0.00** | **1.4** | **353** | **0.058** |
| Java | 0.01 | 0.00 | 39.2 | - | 0.196 |

### quicksort

| Language | Wall (s) | CPU (s) | Peak RSS (MiB) | Binary (KiB) | Build (s) |
| --- | ---: | ---: | ---: | ---: | ---: |
| C | 0.53 | 0.52 | 77.3 | 14 | 0.054 |
| Go | 0.55 | 0.55 | 78.3 | 1487 | 0.069 |
| Rust | 0.55 | 0.54 | 77.8 | 330 | 0.152 |
| **Sisu** | **0.62** | **0.61** | **77.6** | **353** | **0.056** |
| Java | 0.64 | 0.63 | 117.6 | - | 0.196 |
| Python | 12.66 | 12.65 | 393.7 | - | - |

### nbody

| Language | Wall (s) | CPU (s) | Peak RSS (MiB) | Binary (KiB) | Build (s) |
| --- | ---: | ---: | ---: | ---: | ---: |
| Rust | 0.12 | 0.12 | 1.9 | 358 | 0.094 |
| C | 0.14 | 0.14 | 1.7 | 14 | 0.067 |
| **Sisu** | **0.14** | **0.14** | **1.4** | **357** | **0.067** |
| Java | 0.20 | 0.19 | 43.8 | - | 0.226 |
| Go | 0.23 | 0.23 | 1.8 | 1495 | 0.029 |
| Python | 12.16 | 12.16 | 11.2 | - | - |

### mandelbrot

| Language | Wall (s) | CPU (s) | Peak RSS (MiB) | Binary (KiB) | Build (s) |
| --- | ---: | ---: | ---: | ---: | ---: |
| C | 0.58 | 0.58 | 1.7 | 14 | 0.053 |
| **Sisu** | **0.58** | **0.58** | **1.4** | **353** | **0.042** |
| Go | 0.59 | 0.59 | 1.8 | 1483 | 0.078 |
| Rust | 0.59 | 0.58 | 1.9 | 329 | 0.059 |
| Java | 0.60 | 0.60 | 40.6 | - | 0.202 |
| Python | 17.15 | 17.15 | 11.1 | - | - |

### fannkuch_redux

| Language | Wall (s) | CPU (s) | Peak RSS (MiB) | Binary (KiB) | Build (s) |
| --- | ---: | ---: | ---: | ---: | ---: |
| C | 0.11 | 0.11 | 1.7 | 14 | 0.065 |
| Go | 0.12 | 0.12 | 1.8 | 1495 | 0.065 |
| Rust | 0.12 | 0.12 | 1.9 | 331 | 0.098 |
| **Sisu** | **0.13** | **0.13** | **1.7** | **357** | **0.061** |
| Java | 0.17 | 0.16 | 42.9 | - | 0.210 |
| Python | 2.76 | 2.76 | 11.1 | - | - |

Toolchains: `sisuc -O2` (LLVM 22), Ubuntu clang 18.1.3 with `-O2`, rustc
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
  (`libsisu_runtime.a`, written in Rust) statically, which accounts for most of
  its size.
- **Build** is the wall time to compile the one file and link it. The hello,
  quicksort, nbody, mandelbrot and fannkuch_redux tables give the median of
  5 builds, to the millisecond. The older tables time one build, so Go's
  1.88 s for fib is a cold build cache.
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
ten times the memory.

## Where binary_trees spends its time

Sisu takes 1.4 times as long as C on binary_trees. Callgrind counted the
instructions in each function, and each suspect was then removed by hand from
the program's IR (`sisuc -O2 --emit ir`) and the result timed.

- **The allocator costs the same.** `malloc` and `free` run 12.4 billion
  instructions in both programs. A Sisu node is 24 bytes, its count and two
  fields, against C's 16, but glibc rounds both up to a 32-byte chunk, so peak
  memory is the same too.
- **A locked live count, now gone.** The runtime counted live objects for
  `--rc-checks` with an atomic add and subtract. On x86-64 each is a locked
  instruction and a full memory barrier, paid on every allocation and free.
  That was a quarter of the run: Sisu took 1.10 s before the runtime switched
  to a plain load and store.
- **`self` is passed owned.** Before `self.left?.check()` the caller adds 1
  to the child's count, and `check` subtracts 1 from `self` when it returns.
  No count reaches 0 there, but each call writes two nodes and calls
  `sisu_rc.release.Node`. With `self` borrowed instead, the run took 0.75 s
  instead of 0.84 s. Skipping the add is safe here because `left` and `right` are not `mut`:
  the node holds its children for as long as the caller holds the node.
  [ADR 0012](adr/0012-methods-borrow-self.md) proposes this convention.
- **`?.` and `??` build an optional.** `self.left?.check() ?? 0` makes an
  `i64?` and then picks from it, where C branches once. With the counting
  removed, `check` still runs 1.8 times C's instructions, but they are
  register operations that the CPU predicts well.
- **The runtime wraps `malloc`, at no measurable cost.** `sisu_alloc`
  validates the layout and calls Rust's allocator, which calls `malloc`: about
  34 instructions a call. A wrapper that calls `malloc` directly ran no faster,
  so link-time optimization with the runtime (roadmap) would gain little here.

What remains is the counting itself: a store when an object is made, and a
load, subtract and store when it is released. ADR 0005 accepts that cost.

## quicksort on an array value

A Sisu function cannot change its caller's array (ADR 0007), so `quicksort`
takes the array and returns it, and recurses with
`a = quicksort(a, lo, i - 1)`. Last use hands the array over, so each call
holds the only reference and every element write happens in place. Peak
memory shows it: Sisu holds 77.6 MiB, as C does for its one array of
10 000 000 `i64`.

Each element write checks the index against the bounds and the array's
count against 1, and each addition checks for overflow. To find what each
check costs, each kind was removed by hand from the program's IR
(`sisuc --emit ir-raw`), and the result built with the same `default<O2>`
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
| `sisuc --check main.sisu` | 1.4 |
| `sisuc -O2 main.sisu main` | 27.6 |
| `cc main.o libsisu_runtime.a -lm -o main`, the link step alone | 21.9 |
| `./main` built by `sisuc` | 0.2 |
| `./main` built by `clang` | 0.2 |
| `python3.14 main.py` | 5.3 |

- Lexing, parsing and checking take 1.4 ms.
- The link through `cc`, which `sisuc` runs as its last step, takes about
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
| Copy-on-write of an array with a live copy | shared_arrays |
| String `==` on array elements and class fields that stay live | compare |
| `f64` arithmetic | nbody, mandelbrot |
| Short loops over small arrays | fannkuch_redux |
| String `+=`, interpolation with format specs, `words`, `chars`, `bytes`, non-ASCII text | strings |
| Enum payloads under `match`: inline in an array, and boxed in a recursive tree | enum_match |
| The front end | check_binary_trees, check_large |
| An `-O2` build | build_hello, build_binary_trees, build_large_20; build_large on `main` only |

## Running them

```sh
make bench                       # all benchmarks
make bench BENCH='fib sieve'     # some of them
RUNS=10 BENCH_CPU=4 make bench
```

The script builds `sisuc` in release mode and then runs each program in every
language whose toolchain is on `PATH`. It stops if a program prints something
other than the Sisu version, and it prints the tables above with the commit,
date and machine. It needs `clang`, `rustc`, `go`, `javac` and `java`, and
`python3.14` (or the interpreter `PYTHON` names); any that is missing is
skipped. Build outputs go to `target/bench/`.

The numbers depend on the machine. Rerun the script and replace the tables when
a change to `sisuc` or the runtime should move them.

To add a benchmark, add a directory under `bench/` with `main.sisu` and, for
each language, `main.c`, `main.rs`, `main.go`, `Main.java` and `main.py`, all
printing the same output. Then add its name to the default list in
`scripts/bench.sh`.

CodSpeed tracks Sisu against itself on every pull request that touches the
code; see [Coverage and performance](development.md#coverage-and-performance).
