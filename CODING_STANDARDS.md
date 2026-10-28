# Coding standards

Reviewers apply these rules to every diff. Clippy, rustfmt and prek already enforce the mechanical rules (`[workspace.lints]` in `Cargo.toml`). This file holds the judgement calls that no check can make. Report a broken rule as a finding, with file:line. On a branch-wide diff, read the code first, without the blessed fixture output (`git diff <base> -- . ':!*.err' ':!*.json'`), then the `.err` and `.json` files of the codes it changes.

## Reviewing

- **Approve against the done condition.** Approve when every line of the task's done condition holds in the diff and the code, the fix reaches the cause the task names and not only its repro, and the diff adds no wrong output, crash, leak, unsound IR or newly rejected valid program. Check a cheap claim yourself with a repro or a test. When the task states no done condition, write the bar you applied above your verdict. Everything else is a nit, and a nit never blocks.
- **Block regressions, not old gaps.** Compare with the code before the diff. A defect the diff introduces blocks. A defect the old code had too is a nit: the diff names it in a comment or doc note, and fixing it is a separate task.
- **Diagnostic wording blocks when it misleads.** Messages, labels and help text are clear and specific, and follow "Writing" below. Block for a wrong code, a span on the wrong token, a cascade, a crash, a false warning or wrong behaviour, for help or a fix that is wrong or does not check, and for wording that is wrong, misleading, vague, full of compiler jargon, or different from the spec's text.
- **A diagnostic improvement is a nit for an issue.** Clear wording that could be better, a correct span that could be tighter, more help, a weak unsafe fix, docs page wording, or a missing specialized code where a general error is correct: the diff does not fix it. Write it down and go on; it becomes a GitHub issue after the merge (`.agents/skills/review-sisu/SKILL.md`, step 4).
- **A nit names what it changes.** Give the input whose output, diagnostic or cost changes, or mark the nit "comment only". Drop a nit whose change no test could observe.
- **A comment's facts were observed.** A comment or doc that states run-time behavior, such as a printed value, an order, or what the OS or a library does, matches a real run or the cited source. Check each such claim in the diff. A wrong one blocks.
- **Rust follows the `rust-best-practices` skill.** Apply the skill (`AGENTS.md`, Code rules) to Rust in the diff. Where the skill and these rules or the lints do not agree, these rules apply. A departure from the skill is a nit, unless it also causes a defect that the rules above block.
- **Export a commit to build it.** `git -C <repo> archive <sha> | tar -x -C <scratch dir>` gives a clean copy to build, run or mutate; link `.llvm` into it from the repository. Do not use `git worktree add`: it leaves an entry in the shared `.git` that a sandbox cannot prune.

## Tests

- **A test proves its name.** It must fail when the contract it names breaks. For example, a test that byte offsets are not character offsets needs non-ASCII input. A test that a line break inside `( )` inserts no `Newline` needs the break after a token that could end a statement.
- **A test name says what should hold.** Write `<subject>_should_<result>`, and add `_when_<condition>` when the result depends on a condition. Use common words and no abbreviations. Do not repeat the module path. Keep the name to about 12 words; a longer name usually checks two things. Example: `emit_tokens_should_print_each_token_with_its_position`.
- **An end-to-end test has GIVEN, WHEN and THEN lines.** A test in `crates/*/tests` has a doc comment with a GIVEN line, a WHEN line and a THEN line, in that order. Use AND for a second line of the same kind. Each line is one sentence. THEN states the result that the assertions check. When the test proves a spec scenario, copy the scenario's lines.
- **A unit test gets a comment only when its input needs one.** The comment says why the input finds the defect, for example "non-ASCII text, so byte offsets and char offsets differ". It does not repeat the name.
- **The `test-names` hook checks new tests.** It rejects a `#[test]` without `_should_` in its name, and a test in `crates/*/tests` without GIVEN, WHEN and THEN. The tests in `scripts/test_names_baseline.txt` are older than the rule. When you change one of them, make it obey the rules above and remove its line from the file. Never add a line. When you move a file, change the path in its baseline lines. The hook also rejects a commit that changes the body of such a test, and a baseline line that names no test. `tests/run` cases and diagnostic fixtures are files, not functions, and keep their names.
- **Every diagnostic code has a fixture.** It is `crates/sisuc/tests/diagnostics/<CODE>.sisu`, and its `.err` and `.json` pin its text and `line:col`. Unit tests assert codes and positions, never wording; the `test-wording` prek hook rejects a test that reads a message, a label or help text. Every layout rule in "Statements end at newlines" has a case outside `( )` and a case inside.
- **IR tests check the contract, not LLVM's naming.** Match opcodes, predicates, intrinsic names and block-name prefixes. Value names such as `%0` or `%cmp` change when unrelated code changes. Find a value by its data flow, not by its position among the allocas or instructions. Prove runtime behaviour end to end with a case in `crates/sisuc/tests/run`.
- **Test a property where it lives.** A property of a directory gets one test for each directory. A harness entry for each file is for a contract of each file.
- **Size an input to its property.** Make the input about 10x the smallest size that shows the property, and state the margin in a comment. "A complexity guard is a step counter first" sizes a timeout test, not this rule.
- **A deleted test names its keeper.** When you delete or fold a test, name its keeper in the commit or the report. The keeper is the test that still proves the contract of the deleted test. Also name the mutation that makes the keeper fail. A test with no contract, such as an assertion-free probe, needs a note that says why instead.
- **A table tests one rule.** Each row of a table test checks the same rule, and each failure message names its row. Make one row fail to see the name.
- **Plan-pinned tests keep their expected values.** When behaviour grows, add a new case.
- **Every surviving mutant is a finding.** When you run `make mutation-test BASE=<base>` (only when asked, see `AGENTS.md`), each survivor needs a test that kills it, or a note in the report on why it is equivalent.
- **Every uncovered region your diff touches is a finding.** Run `make coverage BASE=<base>` (see `AGENTS.md`). An uncovered region that touches changed code with a behavior change needs a test that runs it, or a note in the report on why none is needed. Gaps the diff did not make, and edits with no behavioral effect such as formatting or renames, are exempt. Coverage is a prompt, not a target: a test that runs a line without checking its effect does not count. A note that no test can reach a region names the harness it tried: a fake tool on `PATH`, `TMPDIR` or file modes, `/dev/full`, a closed pipe. A plan that says a path is untestable is a claim to check, not an exemption.
- **A call boundary carries an `o2-calls` marker.** A `tests/run` case that relies on a value crossing a real call declares it with `// o2-calls: <caller> <callee> <count>`; the harness checks the `-O2` IR (see `docs/development.md`). The judgement left to review: does the case rely on a call it does not mark?
- **A test of an ownership or optimisation path proves the path runs.** Last-use analysis moves a value at its last use, so a copy test whose original is never read again tests a move, and passes with copying broken. Keep the original live past the change and check it is unchanged, or show that a mutation of the path fails the test.
- **An evaluation-order test fails on any swap.** Give each operand its own visible effect, such as a distinct `print` or a distinct panic, so that running any two in the other order changes the output. `cargo mutants` does not reorder operands, so no survivor flags a test that misses this.
- **A test of a side effect fails with the effect's call deleted.** For a kill, a wait, a write or a cleanup, delete the one call that does it and run the test; it must fail. A test that also passes when the effect happens some other way, such as a child that exits on its own before it is killed, proves nothing about the call. `cargo mutants` replaces whole function bodies and operators but never deletes a single call, so no survivor flags a test that misses this.
- **Programs that exit 0 are `tests/run` cases.** Each one runs plain and at `-O2`, both under Valgrind, and with `--rc-checks`. Bless its `.out` with `SISU_BLESS=1`, then check the file by hand against the program, and comment each printed line with its value. Examples in the language reference (`docs/reference/`) are the exception: they check the documentation, not the compiler, so each gets one plain build and one run.
- **A complexity guard is a step counter first.** A performance fix gets a test of its structural property: a `#[cfg(test)]` step counter, checked by `assert_near_linear` (see `crates/sisuc/src/mir/tests/mod.rs`). An example is `FIELD_READS` in `check/mod.rs`, which `class_cycle_search_reads_linearly` checks. Measure the gain of the fix with callgrind (see Performance). A mutation that adds work and no tick proves a counter test. A mutation that plants or moves a tick proves nothing. Remove a tick that no counter test needs.
  - A timeout test is allowed only for a named regression that no counter sees, proved by such a mutation. It meets all of these conditions:
    - The current code, measured alone, is within its budget (see "A test fits its time budget").
    - The regression, measured alone, takes at least 2x the kill.
    - It has a default override and a `ci` override in `.config/nextest.toml`.
    - The `ci` kill is double the default kill.
    - Its doc comment gives both times, with the load average.
    - It calls the narrowest entry point that still runs the regression's code. A test that calls `exhaust::check` on a `match` skips the front end. It cannot find a front-end regression.
  - A guard that you add or change meets these conditions. A guard changes when its body, input size, period or `terminate-after` changes. A rename of a filter leaf is not a change. An older guard in `.config/nextest.toml` stays until you change it.
  - Never assert wall-clock time. A test of the language reference may carry a `terminate-after` guard, so that a mistake in a page, or in the checker of its examples, fails instead of hanging the run. It is not a time check.
- **A shortcut that only skips work is not pinned.** When a surviving mutant's only effect is extra work that stays linear, such as a reparse a guard skips, accept it with a note in the report that names the work. Test-only counters and stack-depth tricks to observe it cost more than the work they guard. A complexity fix is different: pin it as "A complexity guard is a step counter first" says.
- **A language feature has a generator rule.** A new AST, TIR or type variant, operator, prelude function or built-in method is done only when the program fuzzer generates it; the coverage test in `crates/sisuc/src/generate/` fails until it does ([ADR 0033](docs/adr/0033-every-language-feature-has-a-generator-rule.md)).
- **A test fits its time budget.** On a GitHub-hosted runner a unit test takes under 1 s and an end-to-end test, any test in `crates/*/tests`, under 4 s. A time on another machine is only a hint. `scripts/slow_tests.py` reads nextest's JUnit report and warns about each test over budget, in CI and in the pre-commit hook. It never fails, because wall-clock time varies with the machine and its load. A test you add or change that goes over budget in CI is a nit: shrink its input, or split a loop over cases into one test per case so nextest runs them in parallel.

## Specs and plans

Reviewers apply these rules to specs and plans (`review-sisu`).

- **A spec states its acceptance tests as scenarios.** Write each one in Gherkin form: `Scenario:`, then GIVEN, WHEN and THEN lines. Use AND for a second line of the same kind. Each line is one sentence. Standard Gherkin writes `Given`; Sisu writes the keywords in capitals so that they are easy to see and a test comment can copy them.
- **THEN states a result that a test can see.** For example: output, an exit code, a diagnostic code at `line:col`, or the shape of the `-O2` IR. "It works" and "it is handled" are not results.
- **Similar cases share one outline.** When cases differ only in their input and result, write one `Scenario Outline:` with an `Examples:` table. Each row has a Proves column that says what the row adds.
- **Each scenario has an ID.** Use `S1`, `S2` and so on. A change to the spec never uses an ID again.
- **A plan maps each scenario to a test.** Each task lists its scenario IDs and the test that proves each one, as `file::test_name`. Each scenario is in exactly one task. A task is done when its scenarios pass.
- **A test keeps its scenario's words.** The GIVEN, WHEN and THEN lines of an end-to-end test copy its scenario. Do not write the ID in the test: specs stay out of the repository.

```gherkin
Scenario: S4. Code after `continue` is unreachable
  GIVEN a `while` loop whose body has `print(i)` on line 6, column 9, after `continue`
  WHEN `sisu check` runs on it
  THEN it reports W0001 at 6:9

Scenario Outline: S5. Only a statement that always leaves makes the next line unreachable
  GIVEN a `main` whose body is <first> on line 2 and then `print(1)` on line 3, column 5
  WHEN `sisu check` runs on it
  THEN it reports <result>

  Examples:
    | first                | result         | Proves                                |
    | `return`             | W0001 at 3:5   | `return` always leaves                |
    | `if true { return }` | no diagnostic  | a branch can continue to the next line |
```

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

- **Borrow or move before you clone.** Code that only reads a value takes a reference, and code that hands it on moves it. Anywhere in `crates/sisuc`, a `clone()`, `to_string()`, `to_vec()` or new allocation needs a reason: the receiver keeps the value and the source still needs it. On a per-token, per-node or per-lookup path, the reason is also measured or obvious. Names travel as `Symbol`s and become a `String` only where they leave for `tir` or a diagnostic.
- **The clean path does no repeat work.** A step that lexes, parses or checks the whole source again runs only when its result is used, as `front_end::sound_fixes` does.
- **A pass is linear in the program.** A walk, a fixpoint or a worklist does work in proportion to the declarations and nodes that the source writes. Its cost is bounded by those items, not by the types or paths it derives from them, such as each instance of a generic type. For each new walk, the reviewer builds the worst input they can find and states its cost, from callgrind. The guard for that cost follows "A complexity guard is a step counter first" in Tests.
- **A cost claim is measured.** A diff that claims a speedup, or that a change costs nothing, gives callgrind instruction counts before and after (see `AGENTS.md`). CodSpeed's simulation mode runs under callgrind too. Without a measured gain, the simpler code wins ([ADR 0006](docs/adr/0006-values-correctness-simplicity-readability-performance.md)).
- **An optimization is pinned by a structural test.** CodSpeed reports but never fails a pull request, so an optimization that a benchmark relies on also gets a structural test at the interface of the module that owns it. When the contract shows in the generated code, that is a test in `crates/sisuc/src/codegen/tests.rs` that asserts the shape of the `-O2` IR, as `partition_makes_its_array_unique_once` does.
- **Hash maps are `FxHashMap` and `FxHashSet`.** Keys are compiler data, not attacker input, so the std SipHash defence buys nothing.

## Diagnostics

- **One rule, one code.** A rule reports the same code wherever it fires: lexer or parser, inside or outside brackets.
- **Wording lives in the registry.** No call site writes diagnostic text.
- **A recovery or fix change is probed for cascades.** Fixing only the first error must leave every other diagnostic in place ([ADR 0013](docs/adr/0013-diagnostics-have-stable-codes.md)). Probe skipped text inside, before and after an item; a broken item header; a redeclared name; and a failed condition before a branch that returns. `each_safe_fix_removes_exactly_its_diagnostic` checks the fixtures; the reviewer probes the inputs no fixture covers.
- **The primary span covers the offending code.** For "end of line" and "end of file", it is an empty span at that point. `Diagnostic::render` asserts in debug builds that every span sits on character boundaries and does not end with a line break.
- **A span comes from its token or node.** Carry the keyword's or operand's own span into the AST rather than computing one as `span.start + n` from a wider span: a parenthesized expression's span starts at `(`, and a comment can put multi-byte characters before the token. Stepping one byte past a known ASCII byte, such as a `"`, is fine.

## Writing

- **Text follows ASD-STE100 Simplified Technical English.** This applies to diagnostic messages, labels and help text, comments and doc comments, docs, commit messages, and pull request and issue text. An instruction has at most 20 words, and a description at most 25 words. Write one idea in each sentence. Use the active voice and the present tense. Use common words, and give each word one meaning: "use", not "leverage" or "utilise". Do not use idioms. Code names, types, error codes and flags stay as they are.
- **Only new and changed text must comply.** A diff does not reword old text that it does not touch.
- **A violation is a nit.** It blocks only when the text also misleads or is vague, as "Diagnostic wording blocks when it misleads" says.

## Files the compiler writes

- **Leave the input and existing files intact.** An output path that names the input is an error. Intermediate files go to fresh, exclusively created, owner-only paths, and are removed on every exit path.
