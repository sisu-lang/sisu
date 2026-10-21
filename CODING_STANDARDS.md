# Coding standards

Reviewers apply these rules to every diff. Clippy, rustfmt and prek already enforce the mechanical rules (`[workspace.lints]` in `Cargo.toml`). This file holds the judgement calls that no check can make. Report a broken rule as a finding, with file:line. On a branch-wide diff, read the code first, without the blessed fixture output (`git diff <base> -- . ':!*.err' ':!*.json'`), then the `.err` and `.json` files of the codes it changes.

## Reviewing

- **Approve against the done condition.** Approve when every line of the task's done condition holds in the diff and the code, the fix reaches the cause the task names and not only its repro, and the diff adds no wrong output, crash, leak, unsound IR or newly rejected valid program. Check a cheap claim yourself with a repro or a test. When the task states no done condition, write the bar you applied above your verdict. Everything else is a nit, and a nit never blocks.
- **Block regressions, not old gaps.** Compare with the code before the diff. A defect the diff introduces blocks. A defect the old code had too is a nit: the diff names it in a comment or doc note, and fixing it is a separate task.
- **Diagnostic wording blocks when it misleads.** Messages, labels and help text are simple, clear and specific technical English. Block for a wrong code, a wrong position, a cascade, a crash or wrong behaviour, and for wording that is wrong, misleading, vague, full of compiler jargon, or different from the spec's text. A weak unsafe fix, or a rewording where both versions are clear, is a nit.
- **A nit names what it changes.** Give the input whose output, diagnostic or cost changes, or mark the nit "comment only". Drop a nit whose change no test could observe.
- **A comment's facts were observed.** A comment or doc that states run-time behavior, such as a printed value, an order, or what the OS or a library does, matches a real run or the cited source. Check each such claim in the diff. A wrong one blocks.
- **Export a commit to build it.** `git -C <repo> archive <sha> | tar -x -C <scratch dir>` gives a clean copy to build, run or mutate; link `.llvm` into it from the repository. Do not use `git worktree add`: it leaves an entry in the shared `.git` that a sandbox cannot prune.

## Tests

- **A test proves its name.** It must fail when the contract it names breaks. For example, a test that byte offsets are not character offsets needs non-ASCII input. A test that a line break inside `( )` inserts no `Newline` needs the break after a token that could end a statement.
- **Every diagnostic code has a fixture.** It is `crates/sisuc/tests/diagnostics/<CODE>.sisu`, and its `.err` and `.json` pin its text and `line:col`. Unit tests assert codes and positions, never wording; the `test-wording` prek hook rejects a test that reads a message, a label or help text. Every layout rule in "Statements end at newlines" has a case outside `( )` and a case inside.
- **IR tests check the contract, not LLVM's naming.** Match opcodes, predicates, intrinsic names and block-name prefixes. Value names such as `%0` or `%cmp` change when unrelated code changes. Prove runtime behaviour end to end with a case in `crates/sisuc/tests/run`.
- **Plan-pinned tests keep their expected values.** When behaviour grows, add a new case.
- **Every surviving mutant is a finding.** Run `make mutants BASE=<base>` (see `AGENTS.md`). Each survivor needs a test that kills it, or a note in the report on why it is equivalent.
- **Every uncovered region your diff touches is a finding.** Run `make coverage BASE=<base>` (see `AGENTS.md`). An uncovered region that touches changed code with a behavior change needs a test that runs it, or a note in the report on why none is needed. Gaps the diff did not make, and edits with no behavioral effect such as formatting or renames, are exempt. Coverage is a prompt, not a target: a test that runs a line without checking its effect does not count.
- **A call boundary carries an `o2-calls` marker.** A `tests/run` case that relies on a value crossing a real call declares it with `// o2-calls: <caller> <callee> <count>`; the harness checks the `-O2` IR (see `docs/development.md`). The judgement left to review: does the case rely on a call it does not mark?
- **A test of an ownership or optimisation path proves the path runs.** Last-use analysis moves a value at its last use, so a copy test whose original is never read again tests a move, and passes with copying broken. Keep the original live past the change and check it is unchanged, or show that a mutation of the path fails the test.
- **An evaluation-order test fails on any swap.** Give each operand its own visible effect, such as a distinct `print` or a distinct panic, so that running any two in the other order changes the output. `cargo mutants` does not reorder operands, so no survivor flags a test that misses this.
- **Programs that exit 0 are `tests/run` cases.** Each one runs plain and at `-O2`, both under Valgrind, and with `--rc-checks`. Bless its `.out` with `SISU_BLESS=1`, then check the file by hand against the program, and comment each printed line with its value.
- **Complexity is pinned by structure, not by timeout.** A performance fix gets a test of the structural property that makes it fast, and its gain is measured with callgrind (see Performance). Do not add a test that fails by hanging until a nextest `terminate-after` override kills it, and never assert on wall-clock time. The timeout tests already in `.config/nextest.toml` stay until each is replaced.
- **A shortcut that only skips work is not pinned.** When a surviving mutant's only effect is extra work that stays linear, such as a reparse a guard skips, accept it with a note in the report that names the work. Test-only counters and stack-depth tricks to observe it cost more than the work they guard.
- **A test fits its time budget.** On a GitHub-hosted runner a unit test takes under 1 s and an end-to-end test, any test in `crates/*/tests`, under 4 s; a developer machine takes about half that. `scripts/slow_tests.py` reads nextest's JUnit report and warns about each test over budget, in CI and in the pre-commit hook. It never fails a run, because wall-clock time varies with the machine. A test you add or change that goes over budget is a finding: shrink its input, or split a loop over cases into one test per case so nextest runs them in parallel.

## Staging work across tasks

- **No reachable crash lands in a stage.** A feature that a later task implements is rejected by the checker until then, never left to a codegen panic. An existing code reports the feature until it lands.
- **A stage's later arms panic by name.** Inside one stage, when a task adds a syntax or `tir` node that a later task of the same stage handles, the later code's match arm is `panic!("<node> lands in Task N")`, and Task N removes it. The stage's last task leaves none behind. A checker arm that an explanation program reaches is the exception: the explanation tests in `diagnostic/explanations.rs` check Fixed programs and safe fixes, so a panic there fails the suite. That arm returns a stopgap type instead, and Task N replaces it.
- **A restriction on a type sits where the type is made.** Put it in the constructor every path goes through (as `optional_of` does), not where an annotation is read.

## Code

- **An `expect` message states the invariant.** Write what should hold and why, with "should", naming the thing involved: `the checker should reject \`break\` outside a loop`. The panic then reads as a broken assumption. "builder is positioned" describes success, and the error that follows it reads as noise. A prek hook checks that non-test messages say "should"; the reviewer checks that they name the invariant and why it holds.

- **A CI loop over found input has a total time bound.** A script or workflow step that loops over input whose count it does not control, such as artifacts, corpus files or test cases, caps the loop's total time. The cap fits the job's `timeout-minutes` with every later step, such as a failure upload, still ahead. A job killed at its limit skips those steps and loses their output.

- **Sizes come from `TargetData`.** Sizes and alignments in generated code are measured with LLVM's `TargetData`, never written as numbers. The 16-byte array header is two `i64`s measured the same way.

- **Prefer a crate to hand-written code.** Where a well-maintained crate that `deny.toml` allows does the job, use it, and check it with GitHits first.

## Performance

- **Borrow or move before you clone.** Code that only reads a value takes a reference, and code that hands it on moves it. Anywhere in `sisuc`, a `clone()`, `to_string()`, `to_vec()` or new allocation needs a reason: the receiver keeps the value and the source still needs it. On a per-token, per-node or per-lookup path, the reason is also measured or obvious. Names travel as `Symbol`s and become a `String` only where they leave for `tir` or a diagnostic.
- **The clean path does no repeat work.** A step that lexes, parses or checks the whole source again runs only when its result is used, as `front_end::sound_fixes` does.
- **A cost claim is measured.** A diff that claims a speedup, or that a change costs nothing, gives callgrind instruction counts before and after (see `AGENTS.md`). CodSpeed's simulation mode runs under callgrind too. Without a measured gain, the simpler code wins ([ADR 0006](docs/adr/0006-values-correctness-simplicity-readability-performance.md)).
- **An optimization is pinned by a structural test.** CodSpeed reports but never fails a pull request, so an optimization that a benchmark relies on also gets a structural test at the interface of the module that owns it. When the contract shows in the generated code, that is a test in `crates/sisuc/src/codegen/tests.rs` that asserts the shape of the `-O2` IR, as `partition_makes_its_array_unique_once` does.
- **Hash maps are `FxHashMap` and `FxHashSet`.** Keys are compiler data, not attacker input, so the std SipHash defence buys nothing.

## Diagnostics

- **One rule, one code.** A rule reports the same code wherever it fires: lexer or parser, inside or outside brackets.
- **Wording lives in the registry.** No call site writes diagnostic text.
- **A recovery or fix change is probed for cascades.** Fixing only the first error must leave every other diagnostic in place ([ADR 0013](docs/adr/0013-diagnostics-have-stable-codes.md)). Probe skipped text inside, before and after an item; a broken item header; a redeclared name; and a failed condition before a branch that returns. `each_safe_fix_removes_exactly_its_diagnostic` checks the fixtures; the reviewer probes the inputs no fixture covers.
- **The primary span covers the offending code.** For "end of line" and "end of file", it is an empty span at that point. `Diagnostic::render` asserts in debug builds that every span sits on character boundaries and does not end with a line break.
- **A span comes from its token or node.** Carry the keyword's or operand's own span into the AST rather than computing one as `span.start + n` from a wider span: a parenthesized expression's span starts at `(`, and a comment can put multi-byte characters before the token. Stepping one byte past a known ASCII byte, such as a `"`, is fine.

## Files the compiler writes

- **Leave the input and existing files intact.** An output path that names the input is an error. Intermediate files go to fresh, exclusively created, owner-only paths, and are removed on every exit path.
