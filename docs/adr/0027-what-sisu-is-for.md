# What Sisu is for

Status: proposed, 2026-10-19. This ADR has two parts, accepted separately.
"What Sisu is for" records the owner's answers and is accepted on its own.
"Proposed 1.0 scope" is a proposal; each part of it is accepted only when the
owner accepts it, not because the questions under "Still to decide" have
answers. Together they are the definition of 1.0 that the roadmap's "Gaps
before production" asks for first.

## What Sisu is for

**Who.** Curious developers and hobbyists, and possibly later teams that build
real products with it.

**What programs.** Programs where users want fast and reliable software, built
as a fast, small, robust binary:

- Command-line tools and scripts that are easy to write and fast to run.
- Server-side software.

**Why Sisu.** Native code that reads like Python, strict enough that
agent-written code is safe to accept, and fast and easy to verify. Agents now
write much of the code, so the slow step is no longer writing it but checking
it.

What Sisu checks today:

- Before the program runs, the checker rejects programs it can prove wrong. It
  reports independent errors together where it can recover, without a cascade
  of follow-on errors, each with a stable code, a JSON form and fixes where it
  can (ADR 0013).
- While it runs, integer overflow, division by zero and an index outside an
  array stop the program and name the line (ADR 0003, roadmap "What panics").
  A conversion that can fail returns an optional the program must handle
  (ADR 0018), and there is no null.

What it cannot check: `f64` follows IEEE 754, so an overflow gives infinity
and an invalid operation gives NaN without stopping (ADR 0017). No checker
can tell whether a program does what its author meant, and a weak test passes
a wrong algorithm. Sisu's checks give evidence that code is right, not proof,
so "safe to accept" means the evidence is cheap to get and hard to fake.

**How far.** From a learning project to a stable 1.0. Until 1.0, any change
may break a program that compiled before it. From 1.0, Sisu promises which
programs keep compiling.

## Proposed 1.0 scope

### Acceptance programs

1.0 is done when three programs build, run and fail as their specs say, as
each milestone is done when its test passes:

1. **A file-processing tool**: reads a directory of text files, aggregates
   them and prints a report. A missing or unreadable file is reported with
   its path and a non-zero exit code, not a panic.
2. **A script that drives other programs**: started with `sisu run`, it runs
   a subprocess, parses its output and checks its exit status.
3. **A small service**: a TCP service behind a proxy that serves a bounded
   number of connections at once and stops cleanly when asked.

Each program's spec states how it is built and run, how it fails, where it
is deployed (the target and what that machine needs), and its budgets for
start time, binary size and peak memory. The benchmarks already measure
binary size and the start of `hello` ([Benchmarks](../benchmarks.md)); the
budgets make those numbers a release gate.

### Required for 1.0

- A language reference and a stability policy.
- An install that needs no clone of the repository.
- The `sisu` binary with `run`, `build`, `check`, `test` and `fmt`.
- `test` blocks (roadmap, scheduled after milestone 6) whose results repeat
  when the test controls its inputs, seeds and clock.
- A standard library large enough for the three acceptance programs.

### Not in 1.0

Windows, WebAssembly, browser front ends, GUI frameworks, games, a package
registry, services with very many connections at once, and a compiler
written in Sisu.

### Non-goals

These hold beyond 1.0:

- **Threads that share mutable heap objects.** Tasks share no objects; only
  channel and task handles are shared (ADR 0010, proposed).
- **Freestanding targets.** Every program has reference counting and a
  runtime, so kernels, drivers and programs without an operating system are
  out of scope.
- **Python compatibility.** Sisu borrows Python's look, not its dynamic
  typing, its semantics or its libraries.
- **Zero-cost abstractions.** Sisu pays for reference counting and run-time
  checks to stay correct (ADR 0003, ADR 0005), and optimizations remove only
  what they can prove unnecessary (ADR 0006).

## Still to decide

- **How the service survives a panic.** Today, and in the roadmap's
  "Concurrency" proposal, a panic ends the whole program, and a task that
  dies alone is listed under "Later". The options are in "Recovering from a
  panic": a supervisor such as systemd restarts the process, or a panic ends
  only its task, and a loop in the service restarts the task and logs the
  panic. In the second, whatever the task opened outside its memory, such as
  a socket, must be closed by the runtime or by user drop code. The service's
  spec must also say how an accepted connection
  reaches the task that serves it, since a handle that cannot be copied
  cannot cross tasks.
- **The service's scale.** How many connections at once a thread per task
  must carry before more I/O and scheduling design is needed.
- **Platforms and deployment.** The target triples and the minimum machine
  for each, and whether a Linux build is fully static.
- **Native libraries.** TLS and SQLite need bindings, through runtime
  wrappers (ADR 0009) or a C FFI, and whether 1.0 needs either.
- **Budgets.** The numbers for start time, binary size and peak memory.
- **The stability promise.** What keeps compiling, how a feature is
  deprecated, and whether Sisu has editions as Rust does.
- **`Map` order, the clock and random numbers.** Whether milestone 11's
  `Map` iterates in a defined order, and how a test fixes the clock and the
  seed. Each needs its own rationale.
- **Decoding input.** Every line and argument is repaired to valid UTF-8
  (roadmap, "Decided in milestone 5"), so a file tool can change bytes
  without saying so. Milestone 11 brings `read_bytes()`; still open is
  whether reading text can fail on invalid UTF-8 instead of repairing it,
  and which of the two a file read does by default.
- **Text width.** A format width counts code points (roadmap, "Decided in
  milestone 6"), so a table holding CJK characters, emoji or combining
  accents is misaligned in a terminal, and `chars()` splits a grapheme
  cluster (ADR 0014). The file tool's spec says whether 1.0 needs display
  width or grapheme clusters, and its tests include such text.
- **Deferred until their priority is chosen**: property tests in `sisu test`,
  stack traces, a panic on stack overflow in place of `SIGSEGV`, and a
  language server.

## Considered options

- **No stated domain**: every gap in "Gaps before production" is equally
  urgent, so the roadmap cannot order them. This was the state before this
  ADR.
- **A systems language**, competing with C, Rust and Zig: it needs control
  over memory that reference counting gives up (ADR 0005), and it ranks
  performance higher than ADR 0006 does.
- **Scripts only, run by an interpreter**: it gives up the small native
  binary that both kinds of program want.
- **Servers first, with many connections at once**: that needs more I/O and
  scheduling design, whether `async`, green threads or an event loop, and a
  larger runtime. It waits.
- **A list of required features as the definition of 1.0**: every gap
  becomes a release gate, and a one-person project never reaches it.
  Acceptance programs make the list only as long as they need.

## Consequences

- The roadmap's "Target domain" gap points here, and the acceptance programs
  order the rest of "Gaps before production".
- A feature proposal also says what it does for checking: whether a mistake
  it allows is a compile error, a loud failure at run time, or silent.
- If the service is accepted, recovering from a panic and user drop code are
  decided with it in mind, before milestone 12's file handles, as the
  roadmap already requires.
