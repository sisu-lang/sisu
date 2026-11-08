# Developing Sisu

This guide covers building `sisu`, compiling a Sisu program with it, and
running the same checks as CI. Sisu builds and runs on x86-64 and ARM64 Linux and
on ARM64 macOS. CI runs the tests on ARM64 Linux and ARM64 macOS on each push
to `main`. On macOS the tests that need Valgrind, `/dev/full`, GNU ld or gcc
are skipped, and `make coverage`, `make mutation-test`, `make fuzz`,
`make bench` and `make callgrind` do not work.

## Prerequisites

- The Rust toolchain pinned in `rust-toolchain.toml`. rustup installs it on
  first use.
- GNU Make, which runs the commands below.
- A C compiler on `PATH` as `cc`. `sisu` uses it to link programs. On macOS,
  the Xcode Command Line Tools provide it and Make.
- On Ubuntu: `libzstd-dev`, `libxml2-dev` and `zlib1g-dev`, which LLVM links
  against, and on ARM64 also `libffi-dev`. On macOS: Homebrew's `zstd`; LLVM
  looks for it in `/opt/homebrew/lib`.
- [prek](https://prek.j178.dev) for the Git hooks, with
  [typos](https://github.com/crate-ci/typos),
  [cargo-shear](https://github.com/Boshen/cargo-shear) and `shellcheck`,
  which three of the hooks run.
- [cargo-nextest](https://nexte.st). Run the tests with it, not
  `cargo test`: its setup script builds the two runtime archives that
  the end-to-end tests link against.
- [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) and
  [cargo-mutants](https://mutants.rs) for the commands below.
- [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov) for
  `scripts/coverage.sh`.
- On Linux, Valgrind. The end-to-end tests run each program under it to
  catch leaks and memory errors. macOS has no Valgrind, so there the tests
  run each program natively and do not check for leaks.
- For `make miri`, a nightly toolchain with Miri:
  `rustup toolchain install nightly --component miri --component rust-src`.
- [uv](https://docs.astral.sh/uv/), for `make site`, which builds the website.
- Python 3.12 or later, which the scripts and the Python hooks need. prek installs Ruff and
  Pyrefly at the pinned versions on its first run, so nobody installs them by hand.

LLVM 23 lives inside the repository. `scripts/install-llvm.sh` downloads the
official release for the machine, x86-64 or ARM64 Linux or ARM64 macOS (about
2 GB), and keeps only what the build needs in `.llvm/23` (about 570 MB). The
macOS release's static libraries hold LLVM bitcode, which Apple's linker
cannot read, so on macOS the script also compiles them to native code with
the release's `llc`; that adds about 5 minutes. `.cargo/config.toml` points the build at it, and any system
LLVM is left alone. [Upgrading LLVM](llvm.md) covers moving to a new major.
`make audit-setup` adds `opt` and `clang`, with the ASan runtime, for the
audit-sisu skill's probes; `.llvm/23` then takes about 1.2 GB.

## Set up a clone

Run this once per clone:

```console
$ make setup   # LLVM 23 into .llvm/23, the pre-commit and commit-msg hooks, cargo-shear, cargo-mutants, cargo-llvm-cov, cargo-fuzz
```

## Build and test

```console
$ make help    # every target
$ make build   # sisu and the two runtime archives
$ make test    # unit tests and end-to-end cases
$ make lint    # fmt, clippy (also with the fuzz feature), expect messages, unused dependencies, Ruff and Pyrefly on the Python files, typos, file hygiene, shell and workflow lint, as in CI
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
rest. Neither CI nor the hook fails on a slow test: wall-clock time depends on
the machine and its load.

The pre-commit hook also runs `crates/sisuc/tests/architecture.rs` when a commit
changes a file that the test reads. CI's lint job skips that hook, because its
test job runs the test.

After the tests pass, `make test` checks each `test(...)` leaf of a filter in
`.config/nextest.toml`, and CI does the same. A leaf that matches no test, such
as one left after a rename, fails the target, because its override then does
nothing. Write a leaf as `test(=name)` or as a substring of the name: the check
does not read a regex, `~` or `#` leaf, so it fails those too.

`make test-health` prints the counts and times of your last `make test`: the
test count, the time of each test binary, the ten slowest tests and the number
of tests over budget. CI shows it in the job summary. A milestone pull request
quotes it before and after.

Each target is one line in the `Makefile`, a script in `scripts/` or a cargo
command. Run a script directly for the flags no target passes, such as
`scripts/fuzz.sh --replay`.

The end-to-end cases are files. The `cases` test binary runs one test per
`.sisu` file in three directories under `crates/sisuc/tests`:

| Directory | The program must | Expected output |
| --- | --- | --- |
| `run/` | exit 0 with empty stderr unless there is a `<name>.err`, in a plain and an `-O2` build, both also under Valgrind on Linux, in a `--rc-checks` build, and on Linux in an `-O2` build on mimalloc | `<name>.out`: stdout; optional `<name>.err`: stderr |
| `run-fail/` | panic with exit code 101, the same in a plain and an `-O2` build | `<name>.out`: stdout; `<name>.err`: stderr |
| `diagnostics/` | report diagnostics under `sisu check` (`sisu build --emit mir` for a code that only `mono` reports, such as E0163, because `check` does not run `mono`), exiting 1 if one is an error and 0 if all are warnings; a case named `<CODE>.sisu` or `<CODE>-<case>.sisu` reports that code | `<name>.err`: the diagnostics; `<name>.json`: the same under `--format json` |

The in-process tests in `crates/sisuc/src/diagnostic/explanations.rs` also read these fixtures.
They also run each program again with multi-byte comments put in (except the slow E0162 and E0163 programs),
and check that no span splits a character.
A new diagnostic with a fix needs a program in `diagnostics/` that shows the fix.
Use the shape that is most likely to go wrong, such as a typed `let` or a `(` before the token.

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

The same binary checks every program that the website shows, under
`site/programs/`. A program in `site/programs/mir/` must print its
`<name>.mir` under `sisu build --emit mir`. So a change to the mir that `sisu`
prints can fail it: bless the case, and read the diff.

A `run/` or `run-fail/` case that reads standard input has a `<name>.in` beside
it, which every run of the case gets on its standard input: plain, `-O2`, both
Valgrind runs, `--rc-checks` and, on Linux, `-O2` on mimalloc. Without the
file, standard input is empty. Write the file with `printf` or a script, byte
for byte: the formatting hooks leave the `.in`, `.out` and `.err` files of
these directories alone, so a `\r\n`, a lone `\r` or a last line without a
newline stays as written. Input that is large or generated, such as a line
longer than the runtime's input buffer, goes in a `programs.rs` test instead.

A `run/` or `run-fail/` case that takes arguments gives them in a marker line,
`// args: <arg> <arg> ...`, at the start of a line. Every run of the case gets
them, as it gets the `.in` file. The harness splits the line on ASCII spaces
and drops empty pieces, with no quoting, so `// args:` alone gives none, as
no marker does. A case has one such line at most; a second fails the case. An
argument that holds a space, or that is not valid UTF-8, goes in a
`programs.rs` test instead, which can pass any bytes.

A `run/` case that writes to standard error on purpose, with `eprint`, has a
`<name>.err` beside it. The plain, `-O2`, `--rc-checks` and, on Linux, `-O2` on
mimalloc runs must all write exactly that to stderr. Without the file, stderr
must stay empty. The Valgrind runs' stderr holds Valgrind's own log, so they
check only stdout and the exit code. To add the file to a case, `touch` it
like any expected file.

The runs go in order: plain and `-O2` without Valgrind, then, on Linux, both
under Valgrind, then `--rc-checks`, then, on Linux, `-O2` on mimalloc, and the
first failure ends the case. On Linux the Valgrind runs build with
`--allocator system`, because Valgrind sees no block that mimalloc hands out;
the plain runs there are system builds too. So that `-O2` code also runs on
mimalloc, the allocator that users get, Linux makes a further `-O2` build with
no `--allocator` flag and checks that it links mimalloc. On macOS the `-O2`
build is on mimalloc already.
A double free usually aborts in the plain run on Linux, so Valgrind never
reports it there; on macOS the plain runs use mimalloc, which does not check.
To show what Valgrind says about a broken build, build the program with
`--allocator system` and run its binary under `valgrind --error-exitcode=1`
yourself. A program built without the flag fails under Valgrind on mimalloc's
startup read and shows none of its Sisu allocations
([ADR 0031](adr/0031-compiled-programs-allocate-with-mimalloc.md)).
On Linux, `sisu` links with `-Wl,-S`, which drops the debug information of
the runtime archive. So Valgrind shows a runtime frame by its function name,
with no file and line, and `addr2line` gives `??:?` for it.

mimalloc reads the `MIMALLOC_*` variables of the environment, in `sisu` itself
and in a default build. An option such as `MIMALLOC_VERBOSE=1` can write to
stderr and fail the tests that check the stderr of either, so unset every
`MIMALLOC_*` variable before `make test`.

A `run/` case whose output depends on a value crossing a real call says so
with a marker line, `// o2-calls: <caller> <callee> <count>`. The test then
checks that the `-O2` IR of `<caller>` holds exactly `<count>` calls to
`<callee>`, because the inliner can fold a call away and keep the output
the same. A count of 0 is rejected: it holds when every call is gone. Every function except the C `main` is internal, so `-O2` would
inline a caller with one call site and the caller would vanish from the IR.
The check therefore runs `sisu build -O2 --emit ir` once for each distinct
`<caller>`, with the environment variable `SISU_KEEP_FUNCTIONS` set to that
caller alone. That marks it `noinline` in the module and changes nothing
else; `sisu` reads it only for `--emit ir`, and the usage text does not
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

A call through an `any` table has no symbol to name, so `o2-calls` cannot
count it. A second marker, `// o2-indirect-calls: <function> <count>`, counts
the calls through a pointer in the `-O2` body of `<function>`, the release
of an owned `any` too. The check runs `sisu build -O2 --emit ir` once, and
only for a case that has the marker. It sets no `SISU_KEEP_FUNCTIONS`, so the
body is the real one, and it has no rule on parameters. `<function>` must
still be a function of its own after `-O2`. The check fails with "inlined or
specialized" if `-O2` inlined it, or made a clone of it, and the body is then
not its own. Make `<function>` recurse, and not to a depth that its call
sites fix. A count of 0 is rejected, as for `o2-calls`. See
`mut_any_o2.sisu`.

`crates/sisuc/tests/programs` holds the few programs that the small-stack
tests in `programs.rs` run.

Mutation testing changes the code that a branch touched, one mutant at a
time, and reports each mutant the tests still pass. The script runs two
passes. Pass 1 tests each mutant with the unit tests only (`cargo nextest run
--lib`: about 6 s, against 47 s for the whole suite). Of the 657 mutants of
a 4,000-line diff, pass 1 did not catch 80 (54 missed, 26 timeouts); pass 2
tested those 80 with the whole suite in more than one hour. When pass 1 misses a
mutant or times out, pass 2 tests the mutants that pass 1 did not catch in two
groups. The file in the name of each mutant chooses the group. A mutant in a
file of the `LIB_ONLY` list of `scripts/mutants.sh` (the fuzz modules and the
program generator) is tested with the unit tests again, because no integration
test reaches that code. In one run, 49 mutants of `generate/` took about 2.5
hours with the whole suite and 11 minutes with `--lib`, with the same results.
Every other mutant is tested with the whole suite, so a mutant that only an
end-to-end test kills counts as caught.

Each group selects its mutants by name, with one `--re` for each. The script
drops your own `--re`, `-F`, `--regex`, `--examine-re`, `--examine-regex` and
`--iterate` from pass 2. cargo-mutants 27.1.0 ignores `--re` for the "delete
field" mutants, so each group also runs all of those in the diff. A group
counts, and judges, only the names it was given. It fails the run when it
tests fewer of them than it was given.

A mutant of a `LIB_ONLY` file that only an integration test kills shows as
missed, never as caught. So a wrong entry costs a false survivor and hides
none. A test in `scripts/test_mutants.py` checks the list against
`crates/sisuc/tests/architecture.rs` and against the tests, binaries and
`lib.rs`. The log prints one line before each group, and lists the caught
mutants (`--caught`). The summary adds the counts of all the passes.
Arguments after `--` choose
the tests yourself; the script then runs one pass with them. Each mutant still
costs one build, so a large diff can take an hour or more. A diff of more
than 300 mutants fails at once, before pass 1, with
`mutation-test: FAIL: N mutants, more than 300`. Then test a part with
`scripts/mutants.sh <base> --file <path>`, or set `MUTANTS_MAX=<n>` to test
all of it.
Run it only when the user or your instructions ask for it; it is not a
default step. In a plan, the controller runs it once per stage, on the
stage's final head after the task reviews. An executor never runs it, so it
never blocks a task.
The script mutates the
diff from where the branch left `main`. On a branch stacked on another stage,
pass that stage's tip as the base, so earlier stages stay out. Arguments after
the base go to `cargo mutants`:

```console
$ make mutation-test                        # changes since main
$ make mutation-test BASE=feat/m3-writes    # changes since the stage below
$ scripts/mutants.sh HEAD~1 --list          # list the mutants, test none
```

Commit first: the script refuses uncommitted Rust changes with
`mutation-test: FAIL: uncommitted Rust changes`, because a review takes the
log of a commit only.

The script runs `cargo mutants` under `nice -n 19`, 2 mutants at a time.
`-j` or `CARGO_MUTANTS_JOBS` sets another number. `.cargo/mutants.toml` sets
`timeout_multiplier = 2.0` for pass 1, whose unit tests are fast. Pass 2 passes
`--timeout 600`. `MUTANTS_PASS2_TIMEOUT` sets another number of seconds. A
`--timeout`, `-t` or `--timeout-multiplier` of yours replaces it.

The reason: cargo-mutants sets its wait from a baseline run. With 2 mutants at
a time, the whole suite took 99 s to 131 s for a mutant. The wait was about
2 x a 41 s baseline. So mutants that every test passes showed as timeouts and
counted as caught. After 600 s, a timeout in pass 2 is a hang.

The last line is the summary: `mutation-test: PASS` or `mutation-test: FAIL`,
then `N caught, N unviable, N missed, N timeouts`, then `(HEAD <sha>)`. PASS needs no missed mutant; a mutant that hangs the tests until
the timeout counts as caught. A mutant proven equivalent goes into
`exclude_re` in `.cargo/mutants.toml`, with the proof in a comment, so later
runs skip it. A diff that changes no `.rs` file passes at once, with nothing
run. A run that writes no new `mutants.out` (for example, `--in-diff` finds no
mutant) passes with `no mutant tested`; cargo-mutants leaves the old
`mutants.out` alone, and the script does not read it.
An argument that tests no mutant (`--list`, `--list-files`, `--check`) ends
with `mutation-test: NOT RUN` and the exit status of cargo-mutants. The summary
reads `./mutants.out`, so the script passes `--output .`, which beats the
variable `CARGO_MUTANTS_OUTPUT` and the `output` key of a config file. It
refuses `-o`, `--output`, `-d`, `--dir` and `--manifest-path`, which move the
output or the tree.

One `make mutation-test` runs at a time in a worktree; a second one prints
`mutation-test: FAIL: another make mutation-test is running in this worktree`
and stops. `LOG=<file>` writes the whole output to that file, opened only once
the run holds the lock. Use it instead of a shell redirect, which would empty a
running log before the second run could refuse.

`make test`, `make coverage`, `make mutation-test` and `make miri` also hold
one lock for all the worktrees of the clone. The lock is a file in the clone's
`.git` (`scripts/heavy-run.sh`). Two such runs at the same time link hundreds
of programs at once, and then the disk stops for every other process. So a
second run prints `heavy-run: waiting for this run to end: <command> in
<worktree>` and waits for the first. A second `make mutation-test` in the same
worktree still fails at once. When the lock cannot be taken (no `flock`, no Git
directory, a read-only `.git`), the run warns and goes on with no lock.
`SISU_HEAVY_LOCK=skip` runs it with no lock on purpose, for example while a
mutation test of hours holds the lock and the machine has cores to spare. Each
mutant of a mutation run tests 8 tests at a time. `NEXTEST_TEST_THREADS` sets
another number.

## Coverage and performance

`make coverage [BASE=<base>]` runs the tests under `cargo-llvm-cov` and prints
a `Coverage:` line, then two lists. "Uncovered regions touching changed code"
names regions that a changed line touches. "Other uncovered regions in changed
files (advisory)" names the rest of those files, and any region on an
`unreachable!` or `panic!` line, whose message states an invariant no test
should reach. It also names a region on an `assert!`, `assert_eq!` or
`assert_ne!` line that ran (or a `debug_` one): such a region is the message,
which runs only when the assert fails. Test code is left out:
`crates/*/tests/`, and unit-test modules in their own file, `src/**/tests.rs`
or under `src/**/tests/`. It never fails on low
coverage. Commit new files, or
`git add -N` them, so the diff sees them. Before the report, the output has a
line `coverage: measured (HEAD <sha>)`, with `with uncommitted changes` when the
tracked tree differs from HEAD, or when HEAD changes during the run. The review
script (`diff_brief.py --coverage`) refuses coverage of another commit or of
uncommitted changes.

Programs that the tests compile link an uninstrumented runtime, whose two
archives are built in `target/llvm-cov-runtime` and copied next to the
instrumented `sisu`. Runtime
coverage therefore comes from the runtime's own unit tests. One of them,
`print_panics_when_standard_output_cannot_be_duplicated`, is excluded under
coverage: it exhausts file descriptors, and the profiler's error breaks its
exact stderr check.

The CI job "Coverage (informational)" runs the script. It writes the report to
the job summary and keeps one pull request comment up to date (same-repository
pull requests only). On `main` it reports totals only. It never turns a check
red.

CodSpeed measures Sisu against itself in simulation mode. The benchmarks are
in `codspeed.yml`:

- the `fib`, `sieve`, `binary_trees`, `quicksort`, `nbody`, `mandelbrot` and
  `fannkuch_redux` programs, at reduced inputs;
- the `strings`, `enum_match`, `shared_arrays`, `slice_keep`, `slice_loop`, `compare`,
  `tree_rewrite`, `interp`, `interfaces`, `any_shapes` and `show` programs from
  `bench/features/`;
- the lexer in Sisu: `selfhost_lexer` runs the `-O2` build of
  `selfhost/lexer.sisu` on `bench/features/selfhost_lexer/input.sisu`, and
  `build_selfhost_lexer` is an `-O2` build of `selfhost/lexer.sisu`;
- the compiler on `bench/hello` (an `-O2` build), on `bench/binary_trees`
  (`check` and an `-O2` build) and on a generated 200-section input that uses
  most of the language.

The `-O2` build of that input comes in two sizes:
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
`target/release/sisu check target/codspeed/large/main.sisu` under callgrind,
as `make callgrind` does.

Set these once in the CodSpeed UI: "Informational Status Checks on Failure"
on, PR comment mode "On Change", threshold 10%. Do not make the check
required.

CodSpeed comments on a pull request only when a change passes the threshold.
A pull request that changes `codegen`, the runtime or ownership is merged
only after someone has read its CodSpeed report, including changes below the
threshold. Any regression that is kept is explained in the description.

### The nightly compile-speed job

The `Compile speed` workflow (`.github/workflows/compile-speed.yml`) runs
every night at 03:41 UTC, and by hand with `workflow_dispatch`. It builds
`sisu` in release mode and installs `valgrind` and Go. Then it runs two
scripts:

1. `scripts/compile_speed.py --check-only` builds the smallest size of the
   compile-speed program in each language and compares the output of the
   four programs. It times nothing.
2. `scripts/scaling.py` runs the scaling sweep
   (`docs/benchmarks.md`, "The scaling sweep"). A failed check in step 1 does
   not skip it.

The job fails when a check fails, when a stage is superlinear for a shape
and `KNOWN` in `scripts/scaling.py` has no row for it, and when a row looks
fixed. A known case is a line on stdout and a warning annotation on the run.
It does not fail the job. The job never fails on a measured time.

The table of the sweep is in the summary of the run. The artifact `scaling`
holds `table.md`, `counts.json` and the program of each shape at each size.
A sweep that stops early still writes the shapes that it measured, then a line
`scaling: <message>`. To run the sweep on
your machine, use `make scaling`. In the job, each script has a total time
bound. The job gives `--time-bound 300` to step 1 and `--time-bound 900` to
the sweep. A script that runs over exits with code 1. The bound is a guard
against a hang. A local `make scaling` has a bound of 1800 seconds, the
default of `scripts/scaling.py`.

The scheduled run starts only from the default branch. After a change to the
workflow, start it once by hand from the Actions tab. GitHub disables a
`schedule` trigger as the fuzz job below says.

## Fuzz the front end

The fuzz target, `fuzz/fuzz_targets/front_end.rs`, feeds arbitrary bytes to
`sisu check`. It renders the report as text and as JSON, and asserts that
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
  which inputs count. The summary gives, under each replayed crash, the
  panic location and the first line of the panic message. So you do not have
  to search `fuzz.log`, which holds binary bytes (`grep -a`). The message is
  cut to 300 bytes and shown in a code span.
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

When the fuzz job of a scheduled run fails, or GitHub stops it at its time
limit (the job then ends `cancelled`), the `report` job runs
`scripts/fuzz_issue.sh`. It opens a GitHub issue with the title
`Nightly fuzz failure: front_end` and the labels `needs-triage`, `bug` and
`fuzz`, or, when an open issue with that label and title exists, adds a
comment to it. The text is the summary and the run's URL. The script adds no
severity label: the maintainer who triages the issue adds it, and writes the
text again in the usual form. A run that you start by hand opens no issue.
The `report` job is the only job with `issues: write`.

GitHub disables a `schedule` trigger in a public repository after 60 days
without activity in it. That stop sends no failure email: the only sign is a
missing nightly run. Re-enable the workflow on the Actions tab.

GitHub starts a scheduled run late. From 2026-10-24 to 2026-11-06, the scheduled
runs of this repository started 4.5 to 8 hours after their cron times. Treat a
nightly run as missing only when it has not started by 12:00 UTC.

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

## Fuzz generated programs

The second fuzz target, `fuzz/fuzz_targets/program.rs`, turns each input's
bytes into a Sisu program that is well typed by construction, the standard
input it reads, and a rewritten copy that must print the same
([ADR 0033](adr/0033-every-language-feature-has-a-generator-rule.md)). The
generator lives in `crates/sisuc/src/generate/`, and the runner that judges
the program in `crates/sisuc/src/fuzz_program.rs`. Both compile only in tests
and under the `fuzz` feature, never into the released `sisu`. Each program
must pass six properties:

1. It checks clean. Warnings are allowed.
2. Its mir builds and certifies, and LLVM verifies its module, without and
   with `--rc-checks`.
3. Its O0, `-O2` and `--rc-checks` builds each run and exit 0 or 101, never by
   a signal or past 10 s, and agree on stdout, stderr and exit code. The
   `--rc-checks` build reports no count fault, and no leak when it exits 0:
   a panic exits at once, before the leak check.
4. The rewritten copy runs as the program does. Panic locations and the
   `<file>:<line>:<col>: ` that starts each `dbg` line are left out of the
   comparison; the text and the value of a `dbg` line are compared. So no
   rewrite changes the argument of a `dbg`.
5. The program prints the integer answers the generator worked out itself, or
   panics with the overflow or division by zero it expects. Each method that
   the generator writes for an interface with an `i64` or `string` result (or
   a `T` result, when the argument of the conformance is one of them) returns
   a value that no other method returns. Each call of such a method in
   `shapes`, or in a generic function that it calls, prints a line
   `@d<n> <value>`. An interface can also hold a requirement with a `mut`
   parameter of `i64`. Each method that meets it writes a value of its own to
   that parameter, and each such call in `shapes`, or in a generic function that
   it calls, passes a place and prints it after the call as `@d<n> <value>`.
   The calls through `any` and through generic functions that take a `mut`
   parameter print their lines in the same way. These lines are, in order, a
   prefix of the values that a correct dispatch reaches; a `main` that returns
   before the calls prints none.
6. A broken bound gives E0154 and nothing else. For each bound site, an
   argument of a call of a bounded generic function or class whose parameter
   is the bare type parameter that no earlier argument names, a copy of the
   program has that argument replaced by a value that does not conform to the
   bound, and `class NoBound {}` at its end. The checker, alone, with no
   build, reports at least one error. Each error is E0154, and each starts at
   the replaced argument. Any other error is a cascade, as in issue #283.
   Property 6 runs after property 1, before the builds.

Name the target with `--target`, before the other arguments:

```console
$ make fuzz TARGET=program [SECS=<seconds>]
$ scripts/fuzz.sh --target program --replay <input>
$ scripts/fuzz.sh --target program --tmin <input>
$ scripts/fuzz.sh --target program --cmin
$ scripts/fuzz.sh --target program --coverage
$ make fuzz-gaps                     # scripts/fuzz.sh --target program --gaps
```

To check one seed of the series that the unit tests generate from, with
properties 1 to 3, name it in `SISU_FUZZ_SEED`. A failure of property 1 shows
only the errors, and W0010 or W0011, not the warnings that the property
allows:

```console
$ SISU_FUZZ_SEED=72 cargo nextest run -p sisuc --lib seed_from_env --run-ignored only
```

The script first builds the runtime archive in release mode and points
`SISU_RUNTIME` at it, since the builds link it. The target's corpus is
`fuzz/corpus/program`, its failing inputs go to `fuzz/artifacts/program/`,
and it has no dictionary. It starts from the corpus and from 128 seeds, the
inputs `seed(0)` to `seed(127)` of the unit tests. `scripts/fuzz_seeds.py`
writes them as full-size inputs of 4096 bytes. At about one execution a
second, the fuzzer cannot grow an input to that size by itself. Its limits are
`-max_len=4096 -timeout=300 -len_control=0`. With them, libFuzzer tries
full-length inputs at once, since a short input runs out of bytes and gets a
small program. After a run, the script replays up to 3 new `crash-` inputs
for up to 340 s each.

Expect about 1 program a second per core when the machine is busy. Each input
is built eight times, the program's four builds and its copy's four, and
most of that time is `cc` linking. Fork mode runs one job per core, so the
rate scales with the cores: a runner with few cores fuzzes far fewer programs
than a 32-core machine. Before `-len_control=0`, when the programs were
smaller, the rate was about 2 a second per core when busy and 5 on an idle
core, where one input took about 0.2 s on one thread, nine tenths of it
linking.

A replay prints the failure to stderr: the failed property as
`property <n>: <what disagreed>`, or `compiler panic`, then the program, the
rewritten copy and the standard input, each under a `==> <name>` line, then
each build's exit status, stdout and stderr. It also writes `program.sisu`,
`rewritten.sisu`, `stdin.txt` and `failure.txt` to
`fuzz/artifacts/program/<input name>.dump/`. A failed property 6 also writes
`broken.sisu`, the broken copy that failed, which shows the checker's
diagnostics when you run `sisu check` on it. The replays after a fuzz run
write the same directories.

`--coverage` works for either target. It runs `cargo fuzz coverage` over the
target's corpus and seeds, then prints the line coverage of each compiler
stage (lexer, parser, check, mir, codegen) and how many inputs reach codegen.
`make fuzz-coverage [TARGET=program]` runs it, for `front_end` by default. A
program input that the checker rejects is a crash, never a corpus entry, so
every program input, seeds included, reaches codegen.

### The generator gaps

`--gaps` works for the program target only. It finds the functions that a valid
program reaches and the generator's fixed seeds do not: the generator gaps
(`GLOSSARY.md`). It runs `cargo fuzz coverage` twice. The first run is
`program` over the 128 seeds alone, with no corpus. So the result depends on
the commit and not on what a night kept. The second run is `front_end` over the
`.sisu` files of `crates/sisuc/tests/run` and `crates/sisuc/tests/run-fail`.
`tests/diagnostics` holds invalid programs and stays out. Then
`scripts/fuzz_gaps.py` compares the two exports with `fuzz/known_gaps.txt`.
`make fuzz-gaps` runs it. A run takes about 4 minutes when the builds are warm.
The check reads function names with `c++filt -s rust` from GNU binutils. A
`c++filt` that is missing, or that cannot read Rust names, gives exit 2.

The check names a function by its file below `crates/sisuc/src/`, its first line
and its name. The name has no crate hashes and no generic arguments, so the
instances of a generic function merge. A closure counts for its enclosing
function. The check leaves out `generate/`, `fuzz.rs`, `fuzz_program.rs`,
`report.rs`, `diagnostic/json.rs` and `diagnostic/render.rs`. The comparison
prints a section `## Generator gaps`, and `--gaps` appends it to
`fuzz/artifacts/summary.md`. `--gaps` exits 0 when no gap is new, 1 when a gap
is new, and 2 when it cannot trust a measurement. A directory with no program
stops the script with exit 2. A failed `find`, build, `cargo fuzz coverage` or
`llvm-cov` stops it with the status of that tool. In both cases it stops before
the comparison and writes no section. On CI the step then writes a section that
says the check did not finish. A broken measurement is never a pass.

When a gap is new, do one of two things. Add a generator rule or a fixed form
that makes the seeds reach the function
([ADR 0033](adr/0033-every-language-feature-has-a-generator-rule.md)). Or add
its key to `fuzz/known_gaps.txt` under a reason. The list has sections. A
section is a line `## <reason>`, and then one key on each line: the file, a
TAB, and the name, as the check prints it. A key has no line number, so an edit
above a function does not change it. A key before the first section, or
a section with an empty reason, gives exit 2. A key that matches no gap gives a
warning, and you remove it. A key that matches two definitions gives exit 2:
remove it and add a generator rule.

After a change to the generator or to `tests/run`, run `make fuzz-gaps`, and
remove each key that it warns about. On a pull request, CI runs the check only
with a `fuzz-program` label. `review-sisu` runs it on the final diff of a branch.
A nightly tells you otherwise.

The check has three limits:

- It works for each function. A branch that is missed in a function that the
  seeds reach stays hidden.
- Only what `tests/run` and `tests/run-fail` reach can be a gap. A valid
  construct with no case in those directories is invisible.
- The reference compiles at O0 through whole-module IR. Code that only the O2
  and multi-unit builds reach is outside the check, and every run of the
  program target still reaches it.

### Add a rule for a new language feature

A language feature is an AST, `tir` or type variant, an operator, a prelude
function or a built-in method. It needs a drawn rule: the bytes of the input
choose it, so it combines with the other rules and can find bugs.
`every_variant_should_be_generated` fails `make test` until a seed's program
holds the feature. That test reads `plain`, which has no form of `gap_forms`,
so a fixed form cannot pass it. Do these steps in this order:

1. Add a case that holds the feature to `crates/sisuc/tests/run`.
2. Write the rule in `crates/sisuc/src/generate/`. Use `expr.rs` for an
   expression of a wanted type, `stmt.rs` for a statement and `item.rs` for an
   item. Use `generic.rs` for a generic form and `mut_call.rs` for a `mut` call.
3. Keep choice 0 the smallest valid choice, because every choice is 0 when the
   bytes run out.
4. A draw moves what each later part of a seed writes. When a pinned test must
   stay on its seed, draw from another stream, as `mut_pick` does. Or gate on
   the whole input, as `leaves` does. Do not loosen a test to fit a moved seed.
5. Teach the walkers of `generate/coverage.rs` the feature. `universe()` reads
   each variant of the enums in `ast.rs`, `tir.rs` and `types.rs`. A new variant
   needs an arm in the walker of its enum, such as `ast_expr`, `ast_stmt`,
   `ast_pattern`, `tir_expr`, `tir_block`, `tir_pattern` or `ty`. Those matches
   have no `_` arm, so the build asks for the arm. A built-in method that lowers to an
   existing variant can have a row in `RECEIVERS`. With a row, each receiver
   must be generated. With no row, the bare variant must still appear, but no
   receiver is demanded. `universe()` also reads `CONVERSIONS` and `TEXT`. A
   method in `prelude.sisu` or in `METHODS` of `check/number.rs` needs no row,
   because `universe()` reads it.
6. When the feature adds a capability that no variant shows, add a key to
   `REACH` (`generate/coverage.rs`) and mark it in `Walk`. A variant that no
   valid program holds goes into `EXCLUDED`, with its reason.
7. Run `make test` and `make fuzz-gaps`. When a gap stays, because no drawn rule
   reaches the function within the seeds, use the steps in the next section.

### Add a fixed form for a generator gap

A fixed form makes the seeds reach a function. It always writes the same text,
so it finds fewer bugs than a drawn rule. Use it only for a gap. Do these steps
in this order:

1. Add a case that holds the construct to `crates/sisuc/tests/run`. The
   reference then reaches the function, so the check can see a gap.
2. Write the construct in `Gen::gap_forms` (`crates/sisuc/src/generate/item.rs`).
   An item goes into `GENERIC_GAP_ITEMS`, `ANY_GAP_ITEMS` or
   `LOWERING_GAP_ITEMS`, or into the text at the top of `gap_forms` when it fits
   none of them. A statement goes into the list of lines in `gap_forms`. Add it
   to the comment above `gap_forms`.
   Use fixed text and draw no byte from the seed, because a draw moves what each
   later part of a seed writes. The coverage test leaves this text out, so the
   form cannot hide an older key.
3. Call `reached` at the decision in `crates/sisuc/src/check/`, and put
   `#[cfg(test)]` on the call, because `reached` exists only in tests. Add the
   point to `GAP_POINTS` (`generate/coverage.rs`). A search of the program's text
   only guesses at that decision, and a near form passes it. A module that cannot
   call `check` keeps a counter, as `RESOLVE_ALL_CALLS` does. `Walk::program`
   resets the counter and turns its count into a point. A point in `mir`, `lang`
   or codegen is a counter too. Use a needle in the IR only when no text of the
   program can write it. Add the point to `LOWERING_POINTS`, and make `lowered`
   mark it.
4. Add a row to `gap_point_keys_should_mark_what_the_checker_did`, or to
   `lowering_points_should_mark_what_the_compiler_did` for a point of lowering:
   a whole program that reaches the point. Add a near program that does not
   reach it.
5. Run `generated_programs_should_reach_each_gap_rule` and
   `generated_programs_should_reach_each_lowering_rule`. They read only the
   seeds that the nightly reads. Remove the form for a moment: the test must fail.
6. When the form prints a line, update the two pins in
   `crates/sisuc/src/fuzz_program.rs`: the count of lines in
   `a_budget_program_should_finish_within_the_limit`, and the stdout in
   `a_failure_should_print_the_program`. Update the counts in their comments too.
7. Remove the key from `fuzz/known_gaps.txt`. `make fuzz-gaps` must exit 0.

A mark must prove that the function ran. A counter does. A needle in the IR
must not match text of the program, such as a string constant. It must not
match a name that the program chooses, such as a user function or a type.
`lowered` matches a line of one kind: a `define` line, a block label at the
start of a line, or an indented `call` line. A near program that spells the
needle must not set the mark. A near program also tests that the form takes the path you
expect. For example, `"a" < "b" < "c"` goes to `check::string_chain` and never
reaches `lang::compare`, so that chain needs a generic function. And
`sisu_words_count` is no mark for `len_words`, because `words` calls it too.

When the form finds a compiler bug, fix the bug in the same work, with its own
case in `tests/run`.

### The nightly program job

The `Fuzz programs` workflow (`.github/workflows/fuzz-program.yml`) runs
every night at 03:53 UTC for 60 minutes. It then measures `--coverage` (30
minutes at most) and `--gaps` (15 minutes at most). It works as
[the front-end job](#the-nightly-job) does, with its own corpus cache,
`fuzz-program-corpus-`, and its own concurrency group per ref, so neither job
waits for the other. On a failure it uploads `fuzz/artifacts/` as
`fuzz-program-failure`, with the `.dump` directory of each replayed crash, and
the minimized inputs as `fuzz-program-minimized`. When a scheduled run fails
or is stopped at its time limit, its `report` job opens an issue titled `Nightly fuzz failure: program`, or
comments on the open one, as for the front end. A failed `plan` job opens one
too. Nothing blocks a merge.

Program inputs are slow. Fork mode first runs each restored corpus input and
each of the 128 seeds once, one at a time. `-max_total_time` does not stop
that pass. The pass also counts toward the fuzz time. With 500 corpus inputs,
it took 16 and 18 minutes in the runs of 2026-11-06 and 2026-11-07. Those runs
fuzzed for 30 minutes, which left about 12 minutes for new inputs, so the job
now fuzzes for 60. New finds are mutants of the full-size seeds, so the pass
can get longer as nights pass. So after the restore, the "Sample the corpus" step keeps 500 inputs
at random and removes the others. Each run then starts with at most 500
corpus inputs and the seeds.
The run saves the corpus inputs and its new finds, and the next run samples
them again. A removed input is lost; the fuzzer can find its coverage again. The
summary's executions count only the fuzzing after the first pass. The summary
also gives the time of that pass. It warns when the pass took over half of the
run, because such a run tried few new inputs. It also says when no job finished,
because the number of new inputs is then not known. The job
summary also gives the corpus size before and after cmin and whether cmin
finished within its 2400 s. A cmin that does not finish leaves the corpus as it
was and puts a warning on the run, because the run stays green.

A first job, `scripts/fuzz_program_plan.py`, decides how long to fuzz and
whether to measure coverage, and sets the time limits of the fuzz step and of
the job from that. A fuzz step that passes its limit fails, and the later steps
still run, so its log and the corpus are kept. The run then fails, and a
scheduled run opens or comments on the `Nightly fuzz failure: program` issue.
The `Generator gaps` step runs only after a fuzz step that passed. On the
nightly, a new gap fails the step and so the run, and a check that did not
finish does too. The step appends its section to the job summary and uploads
`fuzz/artifacts/summary.md` as `fuzz-program-failure`, so the issue holds the
gap list or the reason. In any other run the step cannot fail the job. A run
by hand with `workflow_dispatch` takes the seconds to fuzz, 60 to 14,400, and
whether to run `--coverage` and `--gaps`. GitHub offers that only for a
workflow already on `main`.

A run by hand measures no coverage unless you ask for it. To run what the
nightly runs:

```console
$ gh api --method POST repos/sisu-lang/sisu/actions/workflows/fuzz-program.yml/dispatches \
    -f ref=main -f 'inputs[seconds]=3600' -f 'inputs[coverage]=true'
```

Then read the `Generator gaps` section of the job summary. With a new gap, the
run and the step still show success.

To fuzz a branch, open a pull request and add one label `fuzz-program`
(2 hours), `fuzz-program <n>m` or `fuzz-program <n>h`, such as
`fuzz-program 45m`. The run starts when the label is added and again on each
push, which cancels the run in progress. Removing the label stops later
pushes from starting runs; cancel a run in progress from the Actions tab. Any
`<n>m` or `<n>h` from 1 minute to 220 minutes works, which leaves the job inside
GitHub's 6-hour limit. Each label must first exist in the repository. A
pull request with two such labels fails the plan, and adding any other label
starts nothing. A labeled run always ends with `--coverage`, and with `--gaps` when the fuzz
step passed. It puts its stage lines in the job summary, and its gap section
when `--gaps` ran. It restores the
newest corpus of its own earlier runs or of `main`, and the corpus it saves
stays with the pull request. GitHub runs no workflow for a pull request with a merge conflict, so
resolve it first.

### Fix a program fuzz failure

1. Minimize the input with `--tmin`, then replay the newest
   `minimized-from-` file with `--replay` for its dump. Fewer bytes mean fewer
   generator choices and a smaller program.
2. Decide which side is wrong. A program that does not check, or a rewrite or
   known answer that is wrong by the language's rules, is a generator bug.
   Anything else is a compiler or runtime bug.
3. For a compiler or runtime bug, `program.sisu` becomes a `run/` or
   `run-fail/` case, and `stdin.txt`, when it is not empty, its `<name>.in`.
   Check its `.out` by hand, as `CODING_STANDARDS.md` asks, see the case fail,
   then fix the root cause. A runtime fix also runs `make miri`. For property 6,
   the case is `broken.sisu`, cut down to the call: it becomes a case of
   `crates/sisuc/tests/diagnostics/`, and the checker's output must be the one
   E0154 at the replaced argument.
4. For a generator bug, add a test in `crates/sisuc/src/generate/coverage.rs`
   that fails on it, then fix the generator. Never steer the generator around
   a compiler bug.

## The nightly build

The `Nightly build` workflow (`.github/workflows/nightly-build.yml`) builds
`sisu` for x86-64 Linux, ARM64 Linux and ARM64 macOS from the newest `main`
commit whose `ci.yml` push run passed. It publishes the builds as a GitHub
release `nightly-<YYYY-MM-DD>-<short SHA>`, marked latest. The release holds
the three tarballs, `install.sh`, `welcome.sisu` and `SHA256SUMS`. The install
line runs `install.sh` from `releases/latest/download/`. The term for a
published build is "nightly build" (`GLOSSARY.md`).

The workflow runs at 02:17 UTC, by hand with `workflow_dispatch`, and on a
pull request that changes its files (see "Dry run"). GitHub can delay the 02:17
run by hours (see [the nightly job](#the-nightly-job)). The logic that needs the
GitHub API is in `scripts/nightly_build.py`. It has four commands: `pick`,
`publish`, `prune` and `report`. The jobs run in this order:

| Job | What it does |
| --- | --- |
| `pick` | Chooses the commit. It takes the newest `main` commit that has a successful `ci.yml` push run. That commit must be newer than the commit of the newest `nightly-*` release. Its `.github/workflows/` tree must equal the one at the head of `main`. It writes the answer to the job summary. |
| `build` | Builds, checks and packs one tarball for each of the three targets. |
| `checksums` | Writes `SHA256SUMS` with three lines. It uploads that file, `install.sh` and `welcome.sisu`. |
| `clean-machine` | Runs `install.sh` and the site's programs on eight clean systems. |
| `installer-tests` | Runs the installer tests that need a broken or an unsupported machine. |
| `publish` | Creates a draft release, uploads the six files and publishes the draft. |
| `live-check` | Runs the install line against the real URLs. |
| `prune` | Deletes the nightly builds beyond the 14 newest. |
| `report` | Opens or updates the issue `Nightly build failure`. |

A run that finds no new green commit ends with success, and its job summary
says `no new green commit`. A run that fails before the last step of `publish`
publishes nothing, and the old nightly build stays latest. The next run picks
the newest green commit again. A failed `live-check` or `prune` comes after the
publish, so the new nightly build stays latest. The next `publish` deletes a
draft that a failed or killed `publish` leaves.

GitHub disables a `schedule` trigger after 60 days without activity in the
repository (see [the nightly job](#the-nightly-job)).

### Dry run

A pull request that changes a file on the `paths` list of the workflow runs
it. A run by hand on a branch other than `main` does the same.

Both are dry runs. A dry run builds the head commit of the branch or the pull
request and runs `pick`, `build`, `checksums`, `clean-machine` and
`installer-tests`. It skips `publish`, `live-check`, `prune` and `report`, so
it publishes nothing and opens no issue. `pick` also asks the question that
`main` would ask, and the job summary shows `main would: <answer>`. The
tarballs stay as artifacts for 7 days.

Start a dry run by hand with the Actions tab, or with
`gh api --method POST repos/sisu-lang/sisu/actions/workflows/nightly-build.yml/dispatches -f ref=<branch>`.
A run by hand on `main` is not a dry run: it can publish. A new push to a pull
request cancels the run in progress.

A dry run of a pull request behind `main` can fail with no fault in the pull
request (see "Read a failure").

### Read a failure

Find the first red job. A job that GitHub skipped is not a failure: a run with
no new commit opens no issue.

- `pick` fails when a `gh` call fails. Its other answers end with success.
- These `build` steps have rules that are not obvious:
  - `nightly_check.py version`: `sisu --version --verbose` must print a
    `commit:` line with the first 7 characters of the picked commit, and print
    nothing to stderr. A commit `unknown` means that the build had no `git`, or
    that `git` did not trust the checkout.
  - `nightly_check.py linux`: the highest strong `GLIBC_` reference is at most
    2.35, the highest `GLIBCXX_` is at most 3.4.30, and `NEEDED` holds only
    `libm`, `libz`, `libzstd`, `libstdc++`, `libgcc_s`, `libc` and the loader.
    A new library needs a decision. It does not pass by itself.
  - `nightly_check.py macos`: `otool -L` lists only `/usr/lib/` and `/System/`
    paths. `codesign -v` passes. The `minos` of one member of
    `libLLVMSupport.a` and of `libzstd.a` is at most 15.0. A rise of the macOS
    floor of 15 is the maintainer's decision.
  - `cargo about --fail`: for example, a crate has a license that
    `.config/about.toml` does not accept. The same step fails when the
    toolchain has no `share/doc/rust/COPYRIGHT-library.html`. The third-party
    file also holds the notices of LLVM's embedded code
    (`.config/notices/LLVM-EMBEDDED.txt`) and those of Rust's standard
    library. On macOS, the step that appends the `zstd` license fails when
    Homebrew has no `LICENSE` for `zstd`.
  - `nightly_check.py pack`: the tarball must hold exactly the directory
    `sisu/` with `sisu`, both runtime archives, `LICENSE` and
    `THIRD-PARTY-LICENSES.txt`. A `._` file is an error.
- `checksums` fails when `SHA256SUMS` does not have three lines.
- `clean-machine` fails in `install.sh`, in the commit check, or in
  `scripts/nightly_smoke.sh`. The commit check runs `sisu --version --verbose`
  from `~/.sisu/bin`. It prints no text when the commit differs. The smoke
  script runs each program of `site/programs/run` that has an `.out` file,
  with both allocators. Each run must exit with 0, print nothing to stderr and
  print the `.out` file. The failure line names the program and the allocator.
- `installer-tests` fails in `scripts/test_install.py` or in
  `scripts/nightly_installer_real.py`.
- `publish` ends with success and `published=false` when its own check of the
  commit fails. The job summary then says
  ``the commit is not newer than the newest nightly build``, or
  ``the newest green commit has other workflow files than `main` ``. The job
  fails when an artifact is missing: the error names the files. The job also
  fails when two artifacts hold one file name. That check comes before any
  write to the releases. The error names the file and both paths.
- `live-check` fails when an install line fails, or when the output of
  `welcome.sisu` differs from `site/programs/run/welcome.out`. It almost always
  means a URL or upload problem. It does not roll back. The nightly build stays
  published.
- `prune` fails when a `gh release delete` call fails. It names the tag on
  stderr and goes on with the other tags. A kill in `prune` is safe, because
  the next `prune` continues.
- `report` needs the label `nightly-build`.
- `pick` and `publish` fail when the newest `nightly-*` release has a tag that
  does not end with a seven-character short SHA. The error names the tag. Delete
  that release, or give it a correct tag.

`publish` and `prune` have no time cap of their own. The `timeout-minutes` of
each job is the limit.

Three failures have no bad build behind them:

- A by-hand re-run of a failed `live-check` after a newer nightly build is
  published compares the new asset with the old `welcome.out`.
- A pull request behind `main` runs the merged workflow of `main` on the head
  of the pull request. Update the branch to end it. A run by hand on a branch
  runs the file of that branch.
- A "re-run all jobs" can take the artifacts of an earlier attempt, because
  artifact ids are not in creation order. `clean-machine` then stops on the
  commit or the checksum. An exit 1 with no text from its commit check means
  that the tarball comes from another commit. A new run clears it: a run by
  hand, the next scheduled run on `main`, or a new push to a pull request.

#### The issue

When a job of a scheduled run fails or is cancelled, `report` opens the issue
`Nightly build failure` with the labels `needs-triage`, `bug` and
`nightly-build`. When an open issue with that title and label exists, it adds
a comment. The text names the run's URL and the failed jobs. A run by hand and
a dry run open no issue. A successful run does not close the issue: the
maintainer closes it. A red `main` opens no issue: `pick` skips a red commit.

#### After a failed prune

`prune` runs `gh release delete --cleanup-tag`. That call deletes the release
first and the tag second. When the tag delete fails, no later `prune` lists the
tag, because `prune` lists releases. After a failed `prune`, look for a tag
with no release. Compare the output of `git ls-remote --tags origin 'nightly-*'`
with the output of
`gh api --paginate 'repos/sisu-lang/sisu/releases?per_page=100' --jq '.[].tag_name'`.
Delete such a tag with
`gh api --method DELETE repos/sisu-lang/sisu/git/refs/tags/<tag>`.

### A bad nightly build

A commit can pass CI and every check and still have a serious bug. There is no
skip list. Marking the previous nightly build as latest moves new installs
back. `pick` does not publish the bad commit again, because it compares with
the newest `nightly-*` release. But any newer green commit that still has the
bug gets published. So:

1. Merge a revert or a fix to `main`.
2. When CI passes, run the workflow by hand on `main`.
3. If the fix takes longer and installs must not get the bad build, do these
   steps in this order:
   1. Disable the workflow:
      `gh api --method PUT repos/sisu-lang/sisu/actions/workflows/nightly-build.yml/disable`.
      A disable does not stop a run in progress.
   2. On the Actions tab, cancel each queued run and each run in progress on
      `main`. Wait until they stop.
   3. Mark the newest nightly build without the bug as latest:
      `gh release edit <tag> --repo sisu-lang/sisu --latest`.
   4. When the fix merges, enable the workflow:
      `gh api --method PUT repos/sisu-lang/sisu/actions/workflows/nightly-build.yml/enable`.
   5. When CI passes, run the workflow by hand on `main`.

### The gap between the two downloads

`install.sh` gets `SHA256SUMS` and the tarball in two requests. If a new
nightly build is published between them, the two files come from different
builds. This gap is a few seconds long and opens once a day. Sisu accepts it:

- The checksum finds the mismatch. The install stops before anything changes,
  and a second run of the install line works.
- An older `install.sh` that gets a newer tarball with another layout also
  stops safely. It checks that the three files are there before it moves them.

To close the gap, `publish` would write the tag into `install.sh`. Then the
published file would differ from the file that `clean-machine` ran. `fetch` in
`install.sh` holds a `shortcut:` comment for the gap. If users report this
mismatch, write the tag into the installer.

### Two installs at the same time

`install.sh` takes the lock `~/.sisu/install.lock` before it changes anything.
The lock is a symbolic link, and its target is the PID of the run. A second run
that starts while the first run is alive stops at once. It prints that another
install is running, and it changes nothing. The lock of a run that was killed
does not stop the next run. The next run sees that the PID is not alive. It
removes the lock and goes on. Without the lock, two runs would share
`~/.sisu/tmp` and could delete the compiler.

### Pruning

`prune` keeps the 14 newest nightly builds. It counts every published release
whose tag starts with `nightly-`, also one with a malformed tag, and orders
them by `published_at`. It deletes the others with their tags. It reads only
`nightly-*` tags, so it cannot touch another release. It runs only after
`live-check` succeeds, so a run that publishes nothing prunes nothing.

### The tests

- `scripts/test_install.py`, `scripts/test_nightly_build.py`,
  `scripts/test_nightly_check.py` and `scripts/test_nightly_workflow.py` run in
  the `python-tests` hook. The tests of `nightly_build.py` use a fake `gh` and
  stored API replies. The tests of `install.sh` use fake system commands.
  `SISU_TEST_SH` names another shell for them, such as `bash --posix`. The
  workflow test reads `nightly-build.yml` as text.
- `scripts/nightly_installer_real.py` needs a real machine: Docker on Linux,
  macOS on a Mac. Its name does not match `test_*.py`, so no hook runs it. The
  `installer-tests` job runs it with `SISU_ARTIFACTS=<dir>`. The directory holds
  the tarball of the host, `install.sh` and `SHA256SUMS`.
- The end-to-end tests of `crates/sisuc/tests/cases.rs` do not run against a
  release `sisu`. They find the binary with `CARGO_BIN_EXE_sisu` at compile
  time. The eight `clean-machine` rows run the site's programs with
  `scripts/nightly_smoke.sh`.

## The growing proof program

`selfhost/lexer.sisu` is Sisu's lexer, written in Sisu. It is the first
version of the growing proof program (see [the roadmap](roadmap.md#the-growing-proof-program)).
It reads standard input and prints one token per line, in the format of
`sisu build --emit tokens`. It follows the clean path only: on input that has
a lexer error, its output is not defined, and it can panic.

The program has two passes. The first pass reads each line with `read_line`
and walks its `chars()` once. It keeps every token of the input in an array.
The second pass judges each run of `;` as `separate` and `judge` in
`crates/sisuc/src/lexer/mod.rs` do, and prints the tokens.

Run it by hand on a file:

```console
$ target/debug/sisu run selfhost/lexer.sisu < crates/sisuc/tests/run/fib.sisu
```

The check is a set of tests:

- `cases.rs` has one `selfhost_lexer` case for each `.sisu` file under
  `crates/sisuc/tests/`, for each file under `selfhost/`, and for the two
  compiler sources `prelude.sisu` and `lang.sisu`. `sisu build --emit tokens`
  runs once on the file. If it exits 0, the plain, the `-O2` and the
  `--rc-checks` build of the lexer must print the same tokens, byte for byte.
  If it exits 1 with a diagnostic code on standard error, the file has a lexer
  error and is excluded. Any other result fails the case with the exit status
  of `sisu`.
- `selfhost_corpus_should_compare_a_file_from_each_directory` in `programs.rs`
  names files that must be compared, with the lexer run on each, and a file
  that must be excluded. An excluded case passes, so this test makes sure that
  the check cannot pass with nothing compared.
- `all_tokens.sisu` in `crates/sisuc/tests/tokens/` holds every kind of token,
  every reserved word and each rule of the lexer. A unit test in the lexer fails
  when a kind has no token in it.
- The four `selfhost_valgrind_` tests run the plain and the `-O2` build of the
  lexer under Valgrind on `all_tokens.sisu` and on `lexer.sisu`.

`support::selfhost` builds the lexer once for a test run and keeps the builds
in `target/tmp/selfhost`.

A change to the rules of the lexer changes `selfhost/lexer.sisu` in the same
pull request, because the check fails when the two lexers differ. For
example, when a top-level `type` makes a `;` run behave as `const` does, the
change goes into `separate` and into the program together.

CodSpeed has two benchmarks of the lexer. `selfhost_lexer` runs the `-O2`
build once on `bench/features/selfhost_lexer/input.sisu`.
`scripts/codspeed-prep.sh` fails when its output differs from the tokens that
`sisu build --emit tokens` prints for that file. `build_selfhost_lexer` is an
`-O2` build of the lexer. The input is fixed: do not change it, so that the
numbers compare between commits.

## Compile and run a program

`sisu build` takes a source file and writes the executable, named after the file
without its extension, in the current directory; `-o` names it:

```console
$ cargo build --workspace
$ target/debug/sisu build crates/sisuc/tests/run/fib.sisu
$ ./fib
832040
```

`sisu run` builds into a private temporary directory, runs the program and
removes the directory. The program has `sisu`'s stdin, stdout and stderr, gets
everything after the file as its arguments (a `--` there is not passed on), and
its exit code is `sisu`'s; a panic gives 101. `sisu`'s own flags go before the
file. A program that does not compile does not run, and `sisu` exits with 1:

```console
$ target/debug/sisu run crates/sisuc/tests/run/fib.sisu
832040
$ target/debug/sisu run -O2 wc.sisu words.txt
```

`-e` gives an inline program in place of the file, to `build`, `run` and
`check`. The text is a whole program, and diagnostics and panics name the
source `<inline>`. `build -e` needs `-o`, because there is no file name to
take the output name from. After `run -e code`, every argument except a
first `--` is the program's:

```console
$ target/debug/sisu run -e 'fn main() { let x = 2; print(x * 21) }'
42
$ target/debug/sisu check -e 'fn main() { print(1 }'
$ target/debug/sisu build -e 'fn main() { print(1) }' -o one
```

`sisu --version` prints `sisu <version>`. With `--verbose` (`-vV`), it adds the
commit, the host and the LLVM version. A bug report needs them because each
nightly build prints the same version. A wrong command line exits with 2, so it
differs from a program that does not compile (1). When stderr cannot take the
report, for example on a full disk, `sisu` exits with 1. `build` still writes
its output, and `run` does not start the program. A reader that stops early, as
in `sisu check a.sisu 2>&1 | head -1`, is not an error. `sisu help` and `sisu`
alone print the usage.

`sisu` links every program with `libsisu_runtime_mimalloc.a`, or with
`libsisu_runtime_system.a` for `--allocator system`, which it looks for next to
its own executable. `cargo build --workspace` puts both archives and `sisu`
there, and so does an install from a nightly build. `cargo build -p sisuc`
does not build the runtime.

When a program breaks a rule, `sisu` prints a diagnostic to stderr and exits
with code 1. Take this `bad.sisu`:

```
fn main() {
    let x: bool = 1
}
```

```console
$ target/debug/sisu build bad.sisu
error[E0032]: expected `bool`, found `i64`
 --> bad.sisu:2:19
  |
2 |     let x: bool = 1
  |                   ^

sisu: 1 error
sisu: for more on a code, run `sisu explain <code>`: E0032
```

When a compiled program fails at runtime, for example by dividing by zero,
it prints a panic to stderr and exits with code 101:

```console
$ target/debug/sisu build crates/sisuc/tests/run-fail/div_zero.sisu
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

The language reference is in `docs/reference/`, one Markdown page for each
construct. `site/data/reference.toml` gives the order of the pages and a
description for each. The build fails when a page is not in that file or the
file names a page that does not exist. The test binary `reference`
(`crates/sisuc/tests/reference.rs`) makes one test for each example on a page.
What the test does depends on the kind of example:

- An error example: the test runs `sisu check` and compares the diagnostics
  with the markers on the page. It does not build the example.
- A pending example: the test runs `sisu check` and makes sure that the
  example does not compile yet.
- A `no-run` example: the test builds the example and does not run it.
- All other examples: the test builds and runs the example and compares its
  output with the page.

Each warning from an example that builds must have a warning marker on its
line.

`make test` runs these tests.

## Look inside the compiler

These flags run the pipeline up to one stage and stop there.
[Architecture](architecture.md) describes the stages.

| Command | Prints |
| --- | --- |
| `sisu build --emit tokens file.sisu` | The lexer's tokens |
| `sisu build --emit ast file.sisu` | The parser's syntax tree |
| `sisu build --emit tir file.sisu` | The checker's typed tree, `tir`, after `mono` has copied each generic function, class and enum once per list of type arguments, such as `first<i64>`; it also shows each prelude function that the program reaches, such as `[i64].sum`. Its first line, `// mono: fast path` or `// mono: instantiated`, says which path `mono` took |
| `sisu build --emit mir file.sisu` | The mid-level IR, `mir`, after parameter inference, the reuse pass, the ownership pass, the hoist, elaboration and pair removal: every copy, move and release is a statement, a borrowed parameter is marked `borrow`, a slice that reads its base in place prints no `move`, a `match` arm that takes its scrutinee apart prints `take_apart` and `free`, and a construction that builds into a freed box prints `into [..]`; it also shows each prelude function that the program reaches |
| `sisu build --emit ir file.sisu` | The LLVM IR, before and after `mem2reg` |
| `sisu build -O2 --emit ir file.sisu` | The LLVM IR, before and after LLVM's `default<O2>` pipeline |
| `sisu build -O3 --emit ir file.sisu` | The same, with the `default<O3>` pipeline |
| `sisu build --emit ir-raw file.sisu` | The LLVM IR before any pass, as one module that LLVM's `opt` reads |
| `sisu build --rc-checks --emit ir-raw file.sisu` | The same, with the count checks of `--rc-checks` built in |
| `sisu build --timings file.sisu` | The time of each stage, one `timings:` line each, on stderr after the diagnostics. It also works with `--emit <output>`, which times only the stages that make it, and on `sisu check` |
| `sisu build --codegen-units 1 file.sisu` | One LLVM module, optimized as a whole, as before codegen units; the default splits a large program into at most 8 units ([ADR 0045](adr/0045-codegen-units-split-by-default.md)). With `--emit ir` or `--emit ir-raw` and more than one unit, `sisu` prints each unit under a `; codegen unit` header; one unit prints with no header |
| `sisu check file.sisu` | Only diagnostics; exits with code 1 if any is an error |
| `sisu explain E0001` | The explanation of a diagnostic code, in any case; unknown codes exit with code 1 |

`--timings` does not stop the pipeline early: it only adds the `timings:` lines.
It ends with an `other` line and a `total` line. `total` is the wall time from
the start of the command to the end of its work. `other` is the rest of
`total`: reading the file, printing the diagnostics and the emitted output, and
the time between stages. In a split build, the `codegen`, `optimize` and
`object` lines give the slowest unit, a `codegen units:` line gives the count,
and the stage times can add up to more than `total`, because the units run on
several threads. This is `sisu check --timings selfhost/lexer.sisu`, on a release build:

```text
timings: lex           0.4 ms   15.2%  946 lines  2.2M lines/s  4779 tokens  11.1M tokens/s
timings: parse         0.8 ms   28.0%  946 lines  1.2M lines/s
timings: prelude       0.1 ms    2.3%
timings: check         1.4 ms   48.6%  946 lines  687.3k lines/s
timings: other         0.2 ms    5.9%
timings: total         2.8 ms  100.0%
```

- The line and token counts are those of your file only. The prelude is not
  counted.
- Tokens are the lexer's tokens without `Newline` and `Eof`. This token count
  is not the token count of the compile-speed comparison.
- The `prelude` line is the lex and parse of the prelude. The `check` line
  includes the check of the prelude.
- A rate is `-` when its time or its count is zero.
- When the lex stops, as it does on `print(1]`, the lex line has no
  throughput.
- `--timings` does not work with `--format json`, and `sisu run` does not take
  it.

`--format json` writes diagnostics, tool errors and a closing summary to
stderr as JSON Lines. `build`, `run` and `check` take it; `explain` does not.
[Diagnostic codes](diagnostics/README.md#json-output) describes the objects.

`-O2` or `-O3` works with `--emit ir` and when building an executable:
`sisu build -O2 file.sisu`.

`sisu build --linker lld file.sisu` links with `ld.lld` instead of `cc`'s
default linker; `bfd`, `gold` and `mold` work the same way. `sisu` passes the
name to `cc -fuse-ld=`. On Linux, `sisu` first runs
`cc -fuse-ld=<name> -Wl,--version`, so a linker that `cc` cannot find stops the
build before the compile. When that check or the link fails and `ld.<name>`
(`ld64.<name>` on macOS) is not on `PATH`, the error names it, where gcc names only `ld`. `--linker` works only
when building an executable.
`scripts/install-llvm.sh` puts `ld.lld` in `.llvm/23/bin`; the tests put that
directory on `PATH`, or in gcc's `COMPILER_PATH`, for the `--linker lld` tests.
On macOS, Apple's clang has no `bfd`, and for `lld` it runs `ld64.lld`, which
`.llvm/23` does not have, so the tests that link with `--linker` run only on
Linux.

`sisu build --allocator system file.sisu` links the runtime on the System
allocator, where a program otherwise allocates with mimalloc. Valgrind and ASan
track `malloc` and see no block that mimalloc hands out, so a program you run
under either by hand needs this build. `--allocator` works only when building
an executable, and `sisu run` takes it too.

`sisu build --rc-checks file.sisu` builds an executable that checks its
reference counts. Before each retain or release, it checks that the count is
at least 1. Before each read of a string, class or array local, it checks that
the local still holds a reference, which a compiler bug could have moved out.
At a normal exit, it checks that no object is still allocated.
A failed check prints a line that starts with `sisu: rc check:` and exits
with code 102. `--rc-checks` works only when building an executable or with
`--emit ir` and `--emit ir-raw`, which then print the module with the checks
built in. A use after free often goes unseen: the
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
| `crates/runtime` | `sisu-runtime`, the runtime's code: a Rust library with a C ABI. |
| `crates/runtime-mimalloc`, `crates/runtime-system` | `sisu-runtime-mimalloc` and `sisu-runtime-system`, the static archives that link it, on mimalloc and on the System allocator. Every compiled program links one. |
| `bench` | The benchmark programs, under `features/` the Sisu-only ones for CodSpeed, and under `compile/` the compile-speed units and the saved counts that the tests of `scripts/scaling.py` read. |
| `selfhost` | The lexer in Sisu, `lexer.sisu`, which the tests check against `sisu build --emit tokens` (see [The growing proof program](#the-growing-proof-program)). |
| `codspeed.yml`, `codspeed-main.yml` | The CodSpeed benchmark lists: `codspeed.yml` on every run, `codspeed-main.yml` on pushes to `main` only. |
| `.coderabbit.yaml`, `.greptile/` | The settings of the CodeRabbit and Greptile pull request reviews. Their findings are advice; CI is the merge gate. A `@coderabbitai review` or `@greptileai` comment starts a review again. |
| `fuzz` | The fuzz targets, `front_end` and `program`, in their own Cargo workspace, run by `scripts/fuzz.sh`. |
| `ruff.toml`, `pyrefly.toml` | The rules for each tracked Python file: Ruff for lint and format, Pyrefly for types. The Python hooks of `prek.toml` run them (see `CODING_STANDARDS.md`, Python). |
| `Makefile` | The entry point for the commands; `make help` lists its targets. |
| `scripts` | `install-llvm.sh`, `mutants.sh`, `coverage.sh`, `codspeed-prep.sh`, `fuzz.sh`, `fuzz_program_plan.py`, which plans each run of the program fuzz workflow, `fuzz_seeds.py`, which writes the program fuzzer's 128 seeds, `fuzz_gaps.py`, which finds the generator gaps, `text_limits.py`, which checks the sentence and comment limits on changed text, `watch_pr.py`, which waits for the next review with text, comment from someone other than you, or check result of a pull request, `diag_diff.py`, which compares the diagnostics of two `sisu` builds for a review, `golden_diff.py`, which checks that a re-bless only reworded expected files, `scaling.py`, which finds the shapes of program for which a compiler stage is superlinear, `merge_pr.py`, which squash-merges a pull request once its checks pass (`--cleanup` removes the worktree of one merged in the UI), and the scripts prek hooks run, such as `check-expect-messages.sh` and `slow_tests.py`. |
| `scripts/nightly_build.py` | The steps of the nightly build workflow that use the GitHub API: `pick`, `publish`, `prune` and `report`. |
| `scripts/nightly_check.py` | The checks of the nightly build: the binary's commit, its Linux and macOS libraries, and the tarball's names. |
| `scripts/nightly_smoke.sh` | Runs each site program that has an `.out` file with both allocators, on a clean machine of the nightly build. |
| `scripts/nightly_installer_real.py` | The installer tests that need a real machine, which `SISU_ARTIFACTS` points at. |
| `site` | The website's pages, data and build script (`make site`). |
| `install.sh` | The installer, which the nightly build publishes as a release asset (see [The nightly build](#the-nightly-build)). |
| `.config/about.toml` | The settings of `cargo-about`, which writes the third-party license file of a nightly build's tarball. `about.hbs` and `notices/` sit beside it. |
| `docs` | Architecture, roadmap, decisions (`adr/`) and agent guides (`agents/`). |
