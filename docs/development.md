# Developing Sisu

This guide covers building `sisuc`, compiling a Sisu program with it, and
running the same checks as CI. Sisu builds and runs on x86-64 Linux.

## Prerequisites

- The Rust toolchain pinned in `rust-toolchain.toml`. rustup installs it on
  first use.
- GNU Make, which runs the commands below.
- A C compiler on `PATH` as `cc`. `sisuc` uses it to link programs.
- On Ubuntu: `libzstd-dev`, `libxml2-dev` and `zlib1g-dev`, which LLVM links
  against.
- [prek](https://prek.j178.dev) for the Git hooks, with
  [typos](https://github.com/crate-ci/typos) and `shellcheck`, which two of
  the hooks run.
- [cargo-nextest](https://nexte.st). Run the tests with it, not
  `cargo test`: its setup script builds the runtime library that the
  end-to-end tests link against.
- [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) and
  [cargo-mutants](https://mutants.rs) for the commands below.
- [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov) for
  `scripts/coverage.sh`.
- Valgrind. The end-to-end tests run each program under it to catch leaks
  and memory errors.
- For `make miri`, a nightly toolchain with Miri:
  `rustup toolchain install nightly --component miri --component rust-src`.
- [uv](https://docs.astral.sh/uv/), for `make site`, which builds the website.

LLVM 23 lives inside the repository. `scripts/install-llvm.sh` downloads the
official release (2.0 GB) and keeps only what the build needs in `.llvm/23`
(about 570 MB). `.cargo/config.toml` points the build at it, and any system
LLVM is left alone. [Upgrading LLVM](llvm.md) covers moving to a new major.
`make audit-setup` adds `opt` and `clang`, with the ASan runtime, for the
audit-sisu skill's probes; `.llvm/23` then takes about 1.2 GB.

## Set up a clone

Run this once per clone:

```console
$ make setup   # LLVM 23 into .llvm/23, the pre-commit hook, cargo-mutants, cargo-llvm-cov, cargo-fuzz
```

## Build and test

```console
$ make help    # every target
$ make build   # sisuc and the runtime library
$ make test    # unit tests and end-to-end cases
$ make lint    # fmt, clippy, expect messages, typos, file hygiene, shell and workflow lint, as in CI
$ make deny    # advisories, licenses, sources
$ make miri    # the runtime's unit tests under Miri, which stops at undefined behaviour
$ make check   # lint, test and deny; CI also runs make miri
```

Every nextest run writes its JUnit report to
`target/nextest/<profile>/junit.xml`. `scripts/slow_tests.py` reads it and
warns about each test over its time budget: 1 s for a unit test and 4 s for an
end-to-end test, on a GitHub-hosted runner. CI runs it after the tests. The
pre-commit hook runs it on the report of your last `make test` with
`--diff HEAD`, so it lists only the slow tests your change names and counts the
rest. It only warns.

Each target is one line in the `Makefile`, a script in `scripts/` or a cargo
command. Run a script directly for the flags no target passes, such as
`scripts/fuzz.sh --replay`.

The end-to-end cases are files. The `cases` test binary runs one test per
`.sisu` file in three directories under `crates/sisuc/tests`:

| Directory | The program must | Expected output |
| --- | --- | --- |
| `run/` | exit 0 with empty stderr unless there is a `<name>.err`, in a plain and an `-O2` build, both also under Valgrind, and in a `--rc-checks` build | `<name>.out`: stdout; optional `<name>.err`: stderr |
| `run-fail/` | panic with exit code 101, the same in a plain and an `-O2` build | `<name>.out`: stdout; `<name>.err`: stderr |
| `diagnostics/` | report diagnostics under `sisuc --check`, exiting 1 if one is an error and 0 if all are warnings; a case named `<CODE>.sisu` or `<CODE>-<case>.sisu` reports that code | `<name>.err`: the diagnostics; `<name>.json`: the same under `--format json` |

To add a case, write the `.sisu` file and an empty expected file beside it,
then fill the expected file with `make bless`. Read what it wrote before
you commit it:

```console
$ touch crates/sisuc/tests/run/new_case.out
$ make bless   # SISU_BLESS=1 cargo nextest run -E 'binary(cases)'
```

`SISU_BLESS=1` rewrites the expected files of cases that pass every other
check, and only those. It never creates a file. Any other file in these
directories fails the run: a `.out`, `.err`, `.in` or `.json` without its
`.sisu`, an expected file that the directory's mode does not read, or a hidden
file.

A `run/` or `run-fail/` case that reads standard input has a `<name>.in` beside
it, which every run of the case gets on its standard input: plain, `-O2`, both
Valgrind runs and `--rc-checks`. Without the file, standard input is empty.
Write the file with `printf` or a script, byte for byte: the formatting hooks
leave the `.in`, `.out` and `.err` files of these directories alone, so a
`\r\n`, a lone `\r` or a last line without a newline stays as written. Input
that is large or generated, such as a line longer than the runtime's input
buffer, goes in a `programs.rs` test instead.

A `run/` or `run-fail/` case that takes arguments gives them in a marker line,
`// args: <arg> <arg> ...`, at the start of a line. Every run of the case gets
them, as it gets the `.in` file. The harness splits the line on ASCII spaces
and drops empty pieces, with no quoting, so `// args:` alone gives none, as
no marker does. A case has one such line at most; a second fails the case. An
argument that holds a space, or that is not valid UTF-8, goes in a
`programs.rs` test instead, which can pass any bytes.

A `run/` case that writes to standard error on purpose, with `eprint`, has a
`<name>.err` beside it. The plain, `-O2` and `--rc-checks` runs must all write
exactly that to stderr. Without the file, stderr must stay empty. The Valgrind
runs' stderr holds Valgrind's own log, so they check only stdout and the exit
code. To add the file to a case, `touch` it like any expected file.

The runs go in order: plain and `-O2` without Valgrind, then both under
Valgrind, then `--rc-checks`, and the first failure ends the case. A double
free usually aborts in the plain run, so Valgrind never reports it there. To
show what Valgrind says about a broken build, run its binary under
`valgrind --error-exitcode=1` yourself.

A `run/` case whose output depends on a value crossing a real call says so
with a marker line, `// o2-calls: <caller> <callee> <count>`. The test then
checks that the `-O2` IR of `<caller>` holds exactly `<count>` calls to
`<callee>`, because the inliner can fold a call away and keep the output
the same. A count of 0 is rejected: it holds when every call is gone. Every function except the C `main` is internal, so `-O2` would
inline a caller with one call site and the caller would vanish from the IR.
The check therefore runs `sisuc -O2 --emit ir` once for each distinct
`<caller>`, with the environment variable `SISU_KEEP_FUNCTIONS` set to that
caller alone. That marks it `noinline` in the module and changes nothing
else; `sisuc` reads it only for `--emit ir`, and the usage text does not
mention it. One caller per build, so that keeping one caller does not also
keep a callee that another marker names as its caller. The check counts the
calls to `<callee>` in the body of `<caller>` after `-O2`. It fails with "no
function named" if `<caller>` or `<callee>` does not exist, and with
"takes parameters" if `<caller>` has any: kept as a function of its own, such
a caller could hold a call that its real call sites decide away, so mark a
caller without parameters, such as `main`. `main(args)` is the exception:
only the C `main` calls it, with arguments known at run time. The check
measures the marked caller as a standalone function. In a real build the
caller is inlined, and a call to a callee that LLVM infers as `memory(none)`
or `readonly` can then be merged with an identical call at the call site. So
mark calls to callees that write memory, as every current marker does. The
old external-linkage check measured the standalone caller the same way.
Interprocedural constant propagation also runs on internal functions, so a
callee that keeps its calls recurses to a depth that the call sites do not
fix; see `handover.sisu`.
`crates/sisuc/tests/programs` holds the few programs that the small-stack
tests in `programs.rs` run.

Mutation testing changes the code that a branch touched, one mutant at a
time, and reports each mutant the tests still pass. The script mutates the
diff from where the branch left `main`. On a branch stacked on another stage,
pass that stage's tip as the base, so earlier stages stay out. Arguments after
the base go to `cargo mutants`:

```console
$ make mutants                        # changes since main
$ make mutants BASE=feat/m3-writes    # changes since the stage below
$ scripts/mutants.sh HEAD~1 --list    # list the mutants, run nothing
```

The diff includes uncommitted edits. A new file counts once `git add` tracks
it.

## Coverage and performance

`make coverage [BASE=<base>]` runs the tests under `cargo-llvm-cov` and prints
a `Coverage:` line, then two lists. "Uncovered regions touching changed code"
names regions that a changed line touches. "Other uncovered regions in changed
files (advisory)" names the rest of those files. Test code is left out:
`crates/*/tests/`, and unit-test modules in their own file, `src/**/tests.rs`
or under `src/**/tests/`. It never fails on low
coverage. Agents run it before the mutants. Commit new files, or
`git add -N` them, so the diff sees them.

`make verify [BASE=<base>]` runs coverage, then mutants, and always ends with
one line: `verify: PASS`, or `verify: FAIL` and why. PASS needs no uncovered
region touching the changes and no surviving mutant. A region you explain
rather than cover still fails it, so read the reasons, not only the word. A
diff that changes no `.rs` file passes at once, with nothing run.

Programs that the tests compile link an uninstrumented runtime, built in
`target/llvm-cov-runtime` and copied next to the instrumented `sisuc`. Runtime
coverage therefore comes from the runtime's own unit tests. One of them,
`print_panics_when_standard_output_cannot_be_duplicated`, is excluded under
coverage: it exhausts file descriptors, and the profiler's error breaks its
exact stderr check.

The CI job "Coverage (informational)" runs the script. It writes the report to
the job summary and keeps one pull request comment up to date (same-repository
pull requests only). On `main` it reports totals only. It never turns a check
red.

CodSpeed measures Sisu against itself in simulation mode. The benchmarks are
in `codspeed.yml`: the `fib`, `sieve`, `binary_trees`, `quicksort`, `nbody`,
`mandelbrot` and `fannkuch_redux` programs at reduced inputs, the `strings`,
`enum_match`, `shared_arrays` and `compare` programs from `bench/features/`,
and the compiler on `bench/hello` (an `-O2` build), on `bench/binary_trees`
(`--check` and an `-O2` build) and on a generated 200-section input that
uses most of the language. The `-O2` build of that input comes in two sizes:
20 sections on every run (`build_large_20`) and 200 sections on pushes to
`main` only (`build_large`, in `codspeed-main.yml`). Pull request reports show
`build_large` as skipped, with its result from the baseline run.
`check_large` and `make callgrind` keep the 200-section input.
`scripts/codspeed-prep.sh` builds them and checks every program's output
first. The `CodSpeed` workflow runs on pushes to `main` and on pull requests
that touch the code, the benchmarks or their tooling. It never turns a pull
request's check red. On a push to `main`, a step that fails, such as a prep
that no longer builds, fails the run, so the benchmarks cannot stop running
unnoticed. The bench step's time is its duration in the GitHub Actions run of
the `CodSpeed` workflow, or in its job log. Agents read results through the
CodSpeed MCP tools and do not run CodSpeed locally. To compare a change before
pushing, count the instructions of
`target/release/sisuc --check target/codspeed/large/main.sisu` under callgrind,
as `make callgrind` does.

Set these once in the CodSpeed UI: "Informational Status Checks on Failure"
on, PR comment mode "On Change", threshold 10%. Do not make the check
required.

CodSpeed comments on a pull request only when a change passes the threshold.
A pull request that changes `codegen`, the runtime or ownership is merged
only after someone has read its CodSpeed report, including changes below the
threshold. Any regression that is kept is explained in the description.

## Fuzz the front end

The fuzz target, `fuzz/fuzz_targets/front_end.rs`, feeds arbitrary bytes to
`sisuc --check`. It renders the report as text and as JSON, and asserts that
every span and every fix edit is well formed, and that no fix shown leaves the
source lexing or parsing worse. It also applies each safe fix
alone and checks the result: the fix deletes no comment, removes its
diagnostic, and keeps every diagnostic of another code. An input that checks
clean is printed as a typed tree and compiled, without and with `--rc-checks`,
and LLVM must verify both modules; nothing is optimized or linked. A panic, a
stack overflow, a timeout or running out of memory is a failure. The target lives in `fuzz/`,
its own Cargo workspace, and runs under cargo-fuzz without a sanitizer.
Install cargo-fuzz once:

```console
$ cargo install --locked cargo-fuzz@0.13.2   # or make setup
```

`scripts/fuzz.sh` holds every libFuzzer flag:

```console
$ make fuzz [SECS=<seconds>]         # fuzz for 1800 s by default
$ scripts/fuzz.sh --replay <input>   # run one input
$ scripts/fuzz.sh --tmin <input>     # minimize one failing input
$ scripts/fuzz.sh --cmin             # shrink fuzz/corpus/front_end in place
```

A fuzz run starts from the corpus in `fuzz/corpus/front_end` and the `.sisu`
files under `crates/sisuc/tests`, with `fuzz/front_end.dict` as its
dictionary. It runs in fork mode with at least two jobs. It writes
`fuzz/artifacts/fuzz.log`, `run.txt` (the commit and tool versions) and
`summary.md`, and puts each failing input in `fuzz/artifacts/front_end/`,
named `crash-`, `timeout-` or `oom-` and its hash. The script exits 1 when
this run left such a file, and otherwise with cargo-fuzz's status. Files
from an earlier run do not count. `--tmin` writes a `minimized-from-` file
to the same directory.

Three limits of fork mode:

- The log keeps no detail of a timeout or an out-of-memory failure, because
  fork mode drops the child's report. The input is still saved, and
  `scripts/fuzz.sh --replay <input>` reproduces it.
- A crash on a corpus input, during fork mode's first pass over the corpus,
  leaves no panic message in the log: that pass discards the children's
  output. So after the run the script replays the first 3 new `crash-`
  inputs once, each for up to 130 s (`timeout -k 10 120`), and appends their
  output, with the panic message and backtrace, to `fuzz.log`. A run with
  more crashes says in the log and the summary how many it left out; replay
  those with `--replay`. The replays change neither the exit status nor
  which inputs count.
- A stack overflow leaves a `crash-` file only on a best-effort basis:
  libFuzzer's crash handler is not async-signal-safe. It dies by a fatal
  signal, with no panic message and no Rust backtrace. A crash with no panic
  message is a stack overflow or another fatal signal; replay it to confirm.

### The nightly job

The `Fuzz` workflow (`.github/workflows/fuzz.yml`) runs every night at
02:23 UTC, and by hand with `workflow_dispatch`. It restores the corpus from
the newest `fuzz-corpus-` cache entry, fuzzes for 30 minutes and saves the
corpus under a new key. The first run starts from the test programs alone.

When the fuzz step fails, the job:

1. Uploads `fuzz/artifacts/` as the artifact `fuzz-failure`: the raw input,
   the log, the run record and the summary.
2. Minimizes the first failing input, preferring a crash to a timeout and a
   timeout to an OOM, for at most 15 minutes. Each step that shrinks the
   input writes a new `minimized-from-` file. All of them are uploaded as
   `fuzz-minimized`, and the summary names the newest, which is the
   smallest.

Every run then writes `summary.md` to the run's page and shrinks the corpus
with `--cmin`, for at most 10 minutes, before it saves it. A failed or
killed shrink leaves the corpus as it was. Only a failed merge logs
`Failed to minimize corpus`. A shrink stopped at the time limit shows as
exit code 124 in the log of the "Minimize the corpus" step, or 137 if it
ignored the first signal. After setup, only the fuzz step decides whether
the job fails.

The summary gives the seconds fuzzed, the executions in processed jobs and
each failing input with its replay command. Its paths are those of the CI
run's checkout.

GitHub disables a `schedule` trigger in a public repository after 60 days
without activity in it. That stop sends no failure email: the only sign is a
missing nightly run. Re-enable the workflow on the Actions tab.

### Fix a fuzz failure

Download `fuzz-minimized`, or `fuzz-failure` when minimizing failed, and
replay the input with `scripts/fuzz.sh --replay <input>`. The artifacts
unpack without the `fuzz/artifacts/` prefix: `fuzz-failure` holds
`front_end/crash-…` next to `fuzz.log`, and `fuzz-minimized` holds a bare
`minimized-from-…`. Pass the downloaded file by its own path; `--replay`
resolves it from your directory before it moves to the repository root.
Then:

- Fix the root cause in the compiler, never in the fuzz entry point.
- If the input now reports a diagnostic, add it as a
  `crates/sisuc/tests/diagnostics` case.
- If the input now compiles, add a checker or parser unit test.
- Either way, the minimized input becomes a test that fails before the fix.

## Compile and run a program

`sisuc` takes a source file and the name of the executable to write:

```console
$ cargo build --workspace
$ target/debug/sisuc crates/sisuc/tests/run/fib.sisu fib
$ ./fib
832040
```

`sisuc` links every program with `libsisu_runtime.a`, which it looks for
next to its own executable. `cargo build --workspace` puts both files there;
`cargo build -p sisuc` does not build the runtime.

When a program breaks a rule, `sisuc` prints a diagnostic to stderr and exits
with code 1. Take this `bad.sisu`:

```
fn main() {
    let x: bool = 1
}
```

```console
$ target/debug/sisuc bad.sisu bad
error[E0032]: expected `bool`, found `i64`
 --> bad.sisu:2:19
  |
2 |     let x: bool = 1
  |                   ^

sisuc: 1 error
sisuc: for more on a code, run `sisuc --explain <code>`: E0032
```

When a compiled program fails at runtime, for example by dividing by zero,
it prints a panic to stderr and exits with code 101:

```console
$ target/debug/sisuc crates/sisuc/tests/run-fail/div_zero.sisu div_zero
$ ./div_zero
sisu: panic at crates/sisuc/tests/run-fail/div_zero.sisu:2:5: division by zero
```

`print` writes into a buffer that the runtime owns, as C stdio does: a line
at a time when standard output is a terminal, in blocks of 8 KiB otherwise.
The runtime flushes the buffer when `main` returns, before every Sisu panic
message, and before the `--rc-checks` exit message, so a panic's stdout lines
come out before its stderr line. A program that an outside signal kills, such
as a stack overflow, loses what is still in the buffer.

A `print` or the final flush that cannot write to standard output is a panic
too, for example when the disk is full:

```console
$ ./program >/dev/full
sisu: panic: standard output could not be written: No space left on device (os error 28)
```

A pipe with no reader is the exception, as in C: writing to it kills the
program with SIGPIPE. A panic ignores SIGPIPE on its way out, so it exits with
code 101 even when nothing reads standard error.

## Build the site

The website is built from `site/` and the docs by one script. Its one
prerequisite is [uv](https://docs.astral.sh/uv/); it needs no Rust and no LLVM.

```console
$ make site   # tests the build, then builds site/_site
```

CI runs the same target. The `Pages` workflow deploys the site to GitHub Pages
and runs only when you start it by hand.

## Look inside the compiler

These flags run the pipeline up to one stage and stop there.
[Architecture](architecture.md) describes the stages.

| Command | Prints |
| --- | --- |
| `sisuc --emit tokens file.sisu` | The lexer's tokens |
| `sisuc --emit ast file.sisu` | The parser's syntax tree |
| `sisuc --emit tir file.sisu` | The checker's typed tree, `tir` |
| `sisuc --emit mir file.sisu` | The mid-level IR, `mir`, after the ownership pass, the hoist, elaboration and pair removal: every copy, move and release is a statement |
| `sisuc --emit ir file.sisu` | The LLVM IR, before and after `mem2reg` |
| `sisuc -O2 --emit ir file.sisu` | The LLVM IR, before and after LLVM's `default<O2>` pipeline |
| `sisuc -O3 --emit ir file.sisu` | The same, with the `default<O3>` pipeline |
| `sisuc --emit ir-raw file.sisu` | The LLVM IR before any pass, as one module that LLVM's `opt` reads |
| `sisuc --rc-checks --emit ir-raw file.sisu` | The same, with the count checks of `--rc-checks` built in |
| `sisuc --check file.sisu` | Only diagnostics; exits with code 1 if any is an error |
| `sisuc --explain E0001` | The explanation of a diagnostic code, in any case; unknown codes exit with code 1 |

`--format json` writes diagnostics, tool errors and a closing summary to
stderr as JSON Lines. It must be the first argument, and `--explain` does not
take it. [Diagnostic codes](diagnostics/README.md#json-output) describes the
objects.

`-O2` or `-O3` must come first, or right after `--format`. It also works when
building an executable: `sisuc -O2 file.sisu out`.

`sisuc --linker lld file.sisu out` links with `ld.lld` instead of `cc`'s
default linker; `bfd`, `gold` and `mold` work the same way. `sisuc` passes the
name to `cc -fuse-ld=`. When the link fails and `ld.<name>` is not on `PATH`,
the error names `ld.<name>`, where gcc names only `ld`. `--linker` comes after
`-O2`, `-O3` and `--rc-checks`, and works only when building an executable.
`scripts/install-llvm.sh` puts `ld.lld` in `.llvm/23/bin`; the tests put that
directory on `PATH`, or in gcc's `COMPILER_PATH`, for the `--linker lld` tests.

`sisuc --rc-checks file.sisu out` builds an executable that checks its
reference counts. Before each retain or release, it checks that the count is
at least 1. Before each read of a string, class or array local, it checks that
the local still holds a reference, which a compiler bug could have moved out.
At a normal exit, it checks that no object is still allocated.
A failed check prints a line that starts with `sisu: rc check:` and exits
with code 102. `--rc-checks` comes after `-O2` or `-O3`, if given, and works
only when building an executable or with `--emit ir` and `--emit ir-raw`,
which then print the module with the checks built in. A use after free often goes unseen: the
freed memory may still hold a count of 1 or more. Valgrind finds those.

## Add a diagnostic code

Every code ships complete in one change. A new code takes the next free
number in its series, `E` for errors and `W` for warnings.

1. Write `docs/diagnostics/<CODE>.md` in the shape that
   [the README](diagnostics/README.md#explanations) describes, and add its
   row to the README table. The `codes!` table `include_str!`s the file, so
   the crate does not build without it.
2. In `crates/sisuc/src/diagnostic/registry.rs`, add a row to the `codes!`
   table, which is where the number is chosen. `codes!` generates
   `DiagnosticKind::code`; a variant that reports an existing code joins that
   code's row, as `Mismatch | PatternMismatch` does. Add a `DiagnosticKind`
   variant and its arm in `From<DiagnosticKind> for Diagnostic`, with the
   message, labels, help and fixes. A fix is safe only when applying it always
   removes the diagnostic and leaves source that lexes and parses; every other
   fix is an unsafe fix. An idiom fix is safe
   only if the edited source also checks cleanly. When in doubt, it is unsafe.
   A rule need not look for comments or stray characters inside the text its
   fix deletes: `front_end::sound_fixes` makes that fix unsafe for it.
3. Report the variant from the lexer, parser or checker.
4. Add the fixture `crates/sisuc/tests/diagnostics/<CODE>.sisu`, and a
   `<CODE>-<case>.sisu` for each other shape. Create the `.err` and `.json`
   files empty, then fill them with `SISU_BLESS=1` as above and read them.
5. Add unit tests that assert the code and its `(line, col)`. A safe fix
   also gets a test that applies it to an input other than the explanation's
   example and checks that the code is gone.

The tests in `diagnostic/explanations.rs` fail when a code lacks its
explanation, its README row or its fixture, or when a Broken program does not
report exactly its code. They also run every Fixed program, and every Broken
program with its safe fixes applied, through the checker, so the checker
must accept any syntax those programs use.

## Repository layout

| Path | What it is |
| --- | --- |
| `crates/sisuc` | The compiler: lexer, parser, checker, codegen and linking. |
| `crates/sisuc/tests/run`, `run-fail`, `diagnostics` | End-to-end cases, each with the output it must print. |
| `crates/sisuc/tests/programs` | The programs the small-stack tests run. |
| `crates/runtime` | `sisu-runtime`, a static library with a C ABI linked into every compiled program. |
| `bench` | The benchmark programs, and under `features/` the Sisu-only ones for CodSpeed. |
| `codspeed.yml`, `codspeed-main.yml` | The CodSpeed benchmark lists: `codspeed.yml` on every run, `codspeed-main.yml` on pushes to `main` only. |
| `fuzz` | The front-end fuzz target, its own Cargo workspace, run by `scripts/fuzz.sh`. |
| `Makefile` | The entry point for the commands; `make help` lists its targets. |
| `scripts` | `install-llvm.sh`, `mutants.sh`, `coverage.sh`, `verify.sh`, `codspeed-prep.sh`, `fuzz.sh`, `diag_diff.py`, which compares the diagnostics of two `sisuc` builds for a review, and the scripts prek hooks run, such as `check-expect-messages.sh` and `slow_tests.py`. |
| `site` | The website's pages, data and build script (`make site`). |
| `docs` | Architecture, roadmap, decisions (`adr/`) and agent guides (`agents/`). |
