# Developing Sisu

This guide covers building `sisu`, compiling a Sisu program with it, and
running the same checks as CI. Sisu builds and runs on x86-64 and ARM64 Linux and
on ARM64 macOS. CI runs the tests on ARM64 Linux and ARM64 macOS on each push
to `main`. On macOS the tests that need Valgrind, `/dev/full`, GNU ld or gcc
are skipped, and `make coverage`, `make mutants`, `make verify`, `make fuzz`,
`make bench` and `make callgrind` do not work.

## Prerequisites

- The Rust toolchain pinned in `rust-toolchain.toml`. rustup installs it on
  first use.
- GNU Make, which runs the commands below.
- A C compiler on `PATH` as `cc`. `sisu` uses it to link programs. On macOS,
  the Xcode Command Line Tools provide it and Make.
- On Ubuntu: `libzstd-dev`, `libxml2-dev` and `zlib1g-dev`, which LLVM links
  against. On macOS: Homebrew's `zstd`; LLVM looks for it in
  `/opt/homebrew/lib`.
- [prek](https://prek.j178.dev) for the Git hooks, with
  [typos](https://github.com/crate-ci/typos) and `shellcheck`, which two of
  the hooks run.
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
$ make setup   # LLVM 23 into .llvm/23, the pre-commit hook, cargo-mutants, cargo-llvm-cov, cargo-fuzz
```

## Build and test

```console
$ make help    # every target
$ make build   # sisu and the two runtime archives
$ make test    # unit tests and end-to-end cases
$ make lint    # fmt, clippy (also with the fuzz feature), expect messages, typos, file hygiene, shell and workflow lint, as in CI
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
| `run/` | exit 0 with empty stderr unless there is a `<name>.err`, in a plain and an `-O2` build, both also under Valgrind on Linux, and in a `--rc-checks` build | `<name>.out`: stdout; optional `<name>.err`: stderr |
| `run-fail/` | panic with exit code 101, the same in a plain and an `-O2` build | `<name>.out`: stdout; `<name>.err`: stderr |
| `diagnostics/` | report diagnostics under `sisu check`, exiting 1 if one is an error and 0 if all are warnings; a case named `<CODE>.sisu` or `<CODE>-<case>.sisu` reports that code | `<name>.err`: the diagnostics; `<name>.json`: the same under `--format json` |

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

The runs go in order: plain and `-O2` without Valgrind, then, on Linux, both
under Valgrind, then `--rc-checks`, and the first failure ends the case. On
Linux the Valgrind runs build with `--allocator system`, because Valgrind sees
no block that mimalloc hands out; the plain runs there are system builds too.
A double free usually aborts in the plain run on Linux, so Valgrind never
reports it there; on macOS the plain runs use mimalloc, which does not check.
To show what Valgrind says about a broken build, build the program with
`--allocator system` and run its binary under `valgrind --error-exitcode=1`
yourself. A program built without the flag fails under Valgrind on mimalloc's
startup read and shows none of its Sisu allocations
([ADR 0031](adr/0031-compiled-programs-allocate-with-mimalloc.md)).

mimalloc reads the `MIMALLOC_*` variables of the environment. An option such as
`MIMALLOC_VERBOSE=1` can write to stderr and fail the tests that check a
default build's stderr, so unset every `MIMALLOC_*` variable before `make test`.

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
files (advisory)" names the rest of those files, and any region on an
`unreachable!` or `panic!` line, whose message states an invariant no test
should reach. Test code is left out:
`crates/*/tests/`, and unit-test modules in their own file, `src/**/tests.rs`
or under `src/**/tests/`. It never fails on low
coverage. Agents run it before the mutants. Commit new files, or
`git add -N` them, so the diff sees them.

`make verify [BASE=<base>]` runs coverage, then mutants, and always ends with
one line: `verify: PASS`, or `verify: FAIL` and why, ending
`(HEAD <sha>)`, or `(HEAD <sha> with uncommitted changes)` when the working tree
has uncommitted Rust changes. PASS needs no uncovered
region touching the changes and no missed mutant; a mutant that hangs the
tests until cargo-mutants' timeout counts as caught. A mutant proven
equivalent goes into `exclude_re` in `.cargo/mutants.toml`, with the proof in
a comment, so later runs skip it. A region you explain
rather than cover still fails it, so read the reasons, not only the word. A
diff that changes no `.rs` file passes at once, with nothing run.

One `make verify` runs at a time in a worktree; a second one prints
`verify: FAIL: another make verify is running in this worktree` and stops.
`LOG=<file>` writes the whole output to that file, opened only once the run
holds the lock. Use it instead of a shell redirect, which would empty a running
verify's log before the second run could refuse.

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
in `codspeed.yml`: the `fib`, `sieve`, `binary_trees`, `quicksort`, `nbody`,
`mandelbrot` and `fannkuch_redux` programs at reduced inputs, the `strings`,
`enum_match`, `shared_arrays`, `compare` and `tree_rewrite` programs from
`bench/features/`, and the compiler on `bench/hello` (an `-O2` build), on
`bench/binary_trees` (`check` and an `-O2` build) and on a generated
200-section input that uses most of the language. The `-O2` build of that
input comes in two sizes:
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

## Fuzz generated programs

The second fuzz target, `fuzz/fuzz_targets/program.rs`, turns each input's
bytes into a Sisu program that is well typed by construction, the standard
input it reads, and a rewritten copy that must print the same
([ADR 0033](adr/0033-every-language-feature-has-a-generator-rule.md)). The
generator lives in `crates/sisuc/src/generate/`, and the runner that judges
the program in `crates/sisuc/src/fuzz_program.rs`. Both compile only in tests
and under the `fuzz` feature, never into the released `sisu`. Each program
must pass five properties, cheapest first:

1. It checks clean. Warnings are allowed.
2. Its mir builds and certifies, and LLVM verifies its module, without and
   with `--rc-checks`.
3. Its O0, `-O2` and `--rc-checks` builds each run and exit 0 or 101, never by
   a signal or past 10 s, and agree on stdout, stderr and exit code. The
   `--rc-checks` build reports no count fault, and no leak when it exits 0:
   a panic exits at once, before the leak check.
4. The rewritten copy runs as the program does. Panic locations are left out
   of the comparison.
5. The program prints the integer answers the generator worked out itself, or
   panics with the overflow or division by zero it expects.

Name the target with `--target`, before the other arguments:

```console
$ make fuzz TARGET=program [SECS=<seconds>]
$ scripts/fuzz.sh --target program --replay <input>
$ scripts/fuzz.sh --target program --tmin <input>
$ scripts/fuzz.sh --target program --cmin
$ scripts/fuzz.sh --target program --coverage
```

The script first builds the runtime archive in release mode and points
`SISU_RUNTIME` at it, since the builds link it. The target's corpus is
`fuzz/corpus/program`, its failing inputs go to `fuzz/artifacts/program/`,
and it starts from the corpus alone, with no seeds and no dictionary. Its
limits are `-max_len=4096 -timeout=300 -len_control=0`: libFuzzer tries
full-length inputs at once, since a short input runs out of bytes and gets a
small program. After a run, the script replays up to 3 new `crash-` inputs
for up to 340 s each.

Expect about 1 program a second per core when the machine is busy. Each input
is built six times, the program's three builds and its copy's three, and
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
`fuzz/artifacts/program/<input name>.dump/`. The replays after a fuzz run
write the same directories.

`--coverage` works for either target. It runs `cargo fuzz coverage` over the
target's corpus, and for `front_end` its seeds too, then prints the line
coverage of each compiler stage (lexer, parser, check, mir, codegen) and how
many inputs reach codegen. `make fuzz-coverage [TARGET=program]` runs it, for
`front_end` by default. A program input that the checker rejects is a crash,
never a corpus entry, so every program input reaches codegen.

### The nightly program job

The `Fuzz programs` workflow (`.github/workflows/fuzz-program.yml`) runs
every night at 03:53 UTC for 30 minutes. It works as
[the front-end job](#the-nightly-job) does, with its own corpus cache,
`fuzz-program-corpus-`, and its own concurrency group per ref, so neither job
waits for the other. On a failure it uploads `fuzz/artifacts/` as
`fuzz-program-failure`, with the `.dump` directory of each replayed crash, and
the minimized inputs as `fuzz-program-minimized`. Nothing files an issue or
blocks a merge: watch the runs and act on a red one.

Program inputs are slow, so watch the corpus as well. The job summary gives
the corpus size before and after cmin and whether cmin finished within its
600 s. A cmin that runs out leaves the corpus whole, and it grows from night
to night. Fork mode first merges the restored corpus, and the summary's
executions count only the fuzzing after that merge: few executions, or
`unknown`, mean the run spent its time replaying its corpus.

A first job, `scripts/fuzz_program_plan.py`, decides how long to fuzz and
whether to measure coverage, and sets the job's time limit from that. A run
by hand with `workflow_dispatch` takes the seconds to fuzz, 60 to 14,400, and
whether to run `--coverage`. GitHub offers that only for a workflow already on
`main`.

To fuzz a branch, open a pull request and add one label `fuzz-program`
(2 hours), `fuzz-program <n>m` or `fuzz-program <n>h`, such as
`fuzz-program 45m`. The run starts when the label is added and again on each
push, which cancels the run in progress. Removing the label stops later
pushes from starting runs; cancel a run in progress from the Actions tab. Any
`<n>m` or `<n>h` from 1 minute to 4 hours works, which leaves the job inside
GitHub's 6-hour limit, and each label must first exist in the repository. A
pull request with two such labels fails the plan, and adding any other label
starts nothing. A labeled run always ends with `--coverage` and puts its
stage lines in the job summary. It restores the newest corpus of its own
earlier runs or of `main`, and the corpus it saves stays with the pull
request. GitHub runs no workflow for a pull request with a merge conflict, so
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
   then fix the root cause. A runtime fix also runs `make miri`.
4. For a generator bug, add a test in `crates/sisuc/src/generate/coverage.rs`
   that fails on it, then fix the generator. Never steer the generator around
   a compiler bug.

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
commit, the host and the LLVM version, which a bug report needs because Sisu
has no releases. A wrong command line exits with 2, so it differs from a
program that does not compile (1). `sisu help` and `sisu` alone print the usage.

`sisu` links every program with `libsisu_runtime_mimalloc.a`, or with
`libsisu_runtime_system.a` for `--allocator system`, which it looks for next to
its own executable. `cargo build --workspace` puts both archives and `sisu`
there; `cargo build -p sisuc` does not build the runtime.

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

## Look inside the compiler

These flags run the pipeline up to one stage and stop there.
[Architecture](architecture.md) describes the stages.

| Command | Prints |
| --- | --- |
| `sisu build --emit tokens file.sisu` | The lexer's tokens |
| `sisu build --emit ast file.sisu` | The parser's syntax tree |
| `sisu build --emit tir file.sisu` | The checker's typed tree, `tir` |
| `sisu build --emit mir file.sisu` | The mid-level IR, `mir`, after parameter inference, the reuse pass, the ownership pass, the hoist, elaboration and pair removal: every copy, move and release is a statement, a borrowed parameter is marked `borrow`, a `match` arm that takes its scrutinee apart prints `take_apart` and `free`, and a construction that builds into a freed box prints `into [..]` |
| `sisu build --emit ir file.sisu` | The LLVM IR, before and after `mem2reg` |
| `sisu build -O2 --emit ir file.sisu` | The LLVM IR, before and after LLVM's `default<O2>` pipeline |
| `sisu build -O3 --emit ir file.sisu` | The same, with the `default<O3>` pipeline |
| `sisu build --emit ir-raw file.sisu` | The LLVM IR before any pass, as one module that LLVM's `opt` reads |
| `sisu build --rc-checks --emit ir-raw file.sisu` | The same, with the count checks of `--rc-checks` built in |
| `sisu check file.sisu` | Only diagnostics; exits with code 1 if any is an error |
| `sisu explain E0001` | The explanation of a diagnostic code, in any case; unknown codes exit with code 1 |

`--format json` writes diagnostics, tool errors and a closing summary to
stderr as JSON Lines. `build`, `run` and `check` take it; `explain` does not.
[Diagnostic codes](diagnostics/README.md#json-output) describes the objects.

`-O2` or `-O3` works with `--emit ir` and when building an executable:
`sisu build -O2 file.sisu`.

`sisu build --linker lld file.sisu` links with `ld.lld` instead of `cc`'s
default linker; `bfd`, `gold` and `mold` work the same way. `sisu` passes the
name to `cc -fuse-ld=`. When the link fails and `ld.<name>` (`ld64.<name>` on
macOS) is not on `PATH`, the error names it, where gcc names only `ld`. `--linker` works only
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
| `bench` | The benchmark programs, and under `features/` the Sisu-only ones for CodSpeed. |
| `codspeed.yml`, `codspeed-main.yml` | The CodSpeed benchmark lists: `codspeed.yml` on every run, `codspeed-main.yml` on pushes to `main` only. |
| `fuzz` | The fuzz targets, `front_end` and `program`, in their own Cargo workspace, run by `scripts/fuzz.sh`. |
| `Makefile` | The entry point for the commands; `make help` lists its targets. |
| `scripts` | `install-llvm.sh`, `mutants.sh`, `coverage.sh`, `verify.sh`, `codspeed-prep.sh`, `fuzz.sh`, `fuzz_program_plan.py`, which plans each run of the program fuzz workflow, `diag_diff.py`, which compares the diagnostics of two `sisu` builds for a review, `golden_diff.py`, which checks that a re-bless only reworded expected files, and the scripts prek hooks run, such as `check-expect-messages.sh` and `slow_tests.py`. |
| `site` | The website's pages, data and build script (`make site`). |
| `docs` | Architecture, roadmap, decisions (`adr/`) and agent guides (`agents/`). |
