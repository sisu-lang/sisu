# Compiled programs allocate with mimalloc

Status: accepted, 2026-10-22 (#130).

CI moved to Ubuntu 26.04, and its glibc 2.43 made compiled programs slower.
Once a size's tcache bin is full, `free` now coalesces the block
(`_int_free_chunk`, `_int_free_merge_chunk`, `unlink_chunk`), where glibc 2.39
on 24.04 kept small blocks for reuse. CodSpeed showed binary_trees at -25.6%
and enum_match at -11.4%, and users pay too: full binary_trees takes 1.08 s on
26.04 against 0.73 s on 24.04. Only `GLIBC_TUNABLES=glibc.malloc.tcache_count=65535`
fixes it inside glibc, and a program cannot raise its own tcache count.

Compiled programs allocate through mimalloc v3 by default:

- **The allocator.** The `mimalloc` crate, version 0.1.52 with its default
  features (v3), is the global allocator of the runtime a program links. It is
  pinned exactly (`=0.1.52`), so that an update, which changes every program's
  speed and memory, is its own pull request. `sisu_alloc`, `sisu_free` and
  `sisu_realloc` are unchanged: they call `std::alloc`, which the archive's
  global allocator serves.
- **The flag.** `--allocator <mimalloc|system>` on `build` and `run` picks the
  runtime when the program is built. Without it, the build uses mimalloc. The
  program never checks the choice, so a default build pays nothing per
  allocation for it.
- **Two archives.** `sisu-runtime` keeps all the runtime's code and becomes an
  `rlib`. `sisu-runtime-mimalloc` (`libsisu_runtime_mimalloc.a`) links it and
  declares `MiMalloc` as the global allocator; `sisu-runtime-system`
  (`libsisu_runtime_system.a`) links it and declares nothing, so it uses the
  System allocator. `cargo build --workspace` builds both next to `sisu`.
- **Why `system` exists.** Valgrind and ASan track `malloc`. They see no block
  that mimalloc hands out, so a leak or an overflow in a Sisu object goes
  unreported. A program under either tool is built with `--allocator system`.
- **The link.** `libmimalloc-sys` builds `libmimalloc.a` with `cc`, rustc
  bundles it into `libsisu_runtime_mimalloc.a`, and the existing
  `cc obj <runtime> -lm` needs nothing new.

## Considered options

Medians of 9 runs pinned to one CPU, Ubuntu 26.04 in Docker, `-O2`. Every
variant printed the same output as glibc.

| | glibc | mimalloc v2 | **mimalloc v3** | jemalloc | own free list, cap 1024 | own free list, no cap |
|---|---|---|---|---|---|---|
| binary_trees (full), time | 1.09 s | 0.51 s | **0.58 s** | 0.81 s | 0.80 s | 0.42 s |
| binary_trees, peak RSS | 33 MB | 51 MB | **66 MB** | 35 MB | 33 MB | 33 MB |
| phase program, time | 0.29 s | 0.20 s | **0.22 s** | 0.28 s | 0.29 s | 0.26 s |
| phase program, peak RSS | 116 MB | 129 MB | **120 MB** | 168 MB | 116 MB | 244 MB |
| strings, per run | 4.20 ms | 3.60 ms | **3.80 ms** | 4.17 ms | 4.27 ms | 4.19 ms |
| hello, instructions | 188k | 276k | **297k** | 313k | 188k | 188k |
| hello, stripped binary | 362 KB | 511 KB | **552 KB** | 883 KB | 362 KB | 362 KB |

The phase program builds and drops a tree of 24-byte objects, then a tree of
56-byte objects, then a million strings, twice. On the 24.04 host, mimalloc v3
took binary_trees from 0.74 s to 0.52 s and the phase program from 0.33 s to
0.21 s.

- **An own per-size free list in the runtime.** It is fastest only without a
  cap. binary_trees frees whole trees at once, so a cap of 1024 or 4096 blocks
  per size keeps little of the gain. Without a cap, it never reuses memory
  across sizes and doubled peak RSS on the phase program. Reusing memory across
  sizes means writing a real allocator. CodSpeed's reduced binary_trees fits
  under a cap of 4096, so CodSpeed alone would have hidden this.
- **jemalloc** (`tikv-jemallocator` 0.7.0, jemalloc 5.3.1): slower than
  mimalloc everywhere except binary_trees memory, no gain on the phase program
  or strings, and the largest binary.
- **Doing nothing**: 26.04 users keep paying about 1.5 times the wall time on
  allocation-heavy programs.
- **mimalloc v2**: slightly faster and less binary_trees RSS, but v3 is the
  crate's default, and Sisu takes the default.
- **Choosing at run time with `SISU_ALLOCATOR=system`**: a global allocator
  that read the variable at the first allocation and forwarded each call to
  mimalloc or to the System allocator. It was measured on the reduced
  binary_trees (135,854 allocations) against plain `MiMalloc`: 12.9% more
  instructions as first written, and 2.99% with the check inlined and the
  decision moved to a cold function. That is 3 instructions (load, compare,
  branch) on every `alloc` and every `free`, the floor for any check made per
  call, and over the 2% budget for the cost of the choice. It also needed an allocation-free
  panic path and a Valgrind suppression for mimalloc's startup read. The flag
  at build time replaces it.
- **One runtime on the System allocator plus a mimalloc archive that replaces
  `malloc` for the whole program**: a static replacement of `malloc` does not
  take over libSystem's allocations on macOS, which needs dynamic interposition.
- **Detecting Valgrind at run time** (`RUNNING_ON_VALGRIND`): it is true under
  callgrind too, so CodSpeed and our own callgrind measurements would measure
  glibc instead of the allocator we ship.

## Prior art

Rust programs choose their global allocator per binary with
`#[global_allocator]`, as each archive does here. Rust and Go programs on Linux
overcommit the same way, and the kernel's out-of-memory killer ends them when
they touch memory the machine cannot back.

## Consequences

- **A request the machine cannot back may exit 137.** With glibc, a request
  larger than RAM plus swap is refused at once: the program exits with 134 and
  prints "memory allocation of N bytes failed". mimalloc maps memory with
  `MAP_NORESERVE` in some cases, so such a request can succeed, and on Linux
  the out-of-memory killer may then end the program with 137 once it touches
  the memory. A request far beyond RAM plus swap can still be refused at once
  with 134: a 2^50-byte request (1125899906842640 bytes) prints "memory
  allocation of 1125899906842640 bytes failed" under mimalloc. An 800 GB array
  at `-O0` on a 62 GB host exits 134 under glibc and 137 under mimalloc. The
  largest array the size checks allow (about 2^63 bytes) still exits 134 with
  Rust's message.
- **Startup and size cost more.** `hello` takes 297k instructions to start
  against 188k, and a stripped binary grows from 362 to 552 KB. The 297k is
  for an empty environment: at startup mimalloc scans `environ` once per
  option, about 3.8k instructions per environment variable against about 640
  for glibc (callgrind on `-O2` hello under `env -i` plus N variables: N=0
  288,534 against 188,613; N=100 665,464 against 252,343; N=200 1,043,164
  against 316,843). `hello`'s peak RSS grows from 1.46 MB to 6.15 MB
  (`/usr/bin/time -f %M`: 6152 KB in a default build, 1456 KB in a system
  build). No CodSpeed benchmark runs a plain `hello`, so it does not show
  there.
- **The user's `MIMALLOC_*` variables apply.** mimalloc reads them from the
  environment, as in any Rust program that uses it, and the runtime sets none.
  `MIMALLOC_VERBOSE=1` makes a program write to stderr outside the runtime,
  which [ADR 0009](0009-runtime-owns-io.md) otherwise reserves for the runtime.
- **A double free no longer aborts a default build,** as glibc's tcache check
  made it do. The Linux harness's plain runs at `-O0` and `-O2` are system
  builds and still abort on one; the `--rc-checks` runs, the Linux `-O2` run on
  mimalloc and every run on macOS use mimalloc and do not.
- **Valgrind on a default build fails.** mimalloc v3 runs `mi_process_init`
  from an ELF constructor before `main`. In libmimalloc-sys 0.1.49 (mimalloc
  v3.3.2), `unix_detect_thp` reads `/sys/kernel/mm/transparent_hugepage/enabled`
  into a `char buf[32]`, gets a short read, and searches all 32 bytes. Memcheck
  reports "Conditional jump or move depends on uninitialised value(s)". Upstream's
  `dev3` branch, as of commit b5d94fe8, reads and searches only the bytes it
  got; no release after v3.3.2 carries that yet. A system build contains no mimalloc and
  never meets it. A user who runs Valgrind on a default build sees this error
  and no Sisu allocations, and the guides tell them to build with
  `--allocator system`. There is no suppression file.
- [ADR 0010](0010-tasks-share-no-objects.md) needs nothing for the allocator:
  mimalloc is thread-safe.
- [ADR 0005](0005-reference-counting.md)'s claim that Valgrind can prove a
  program frees everything holds for a program built with `--allocator system`.
- The compiler itself keeps glibc, and so do glibc's own allocations, such as
  stdio buffers.
