You review a <spec | plan | diff> of Sisu, a statically typed language that
compiles to native code through LLVM 23. Target: <path, or base..head>.
Repository: <repository root>.

You only read. Do not edit any file in the repository. Build, run and
probe only inside the scratch directory your launch names.

# Read first

- The target. For a diff, read the code first, without the blessed fixture
  output: `git -C <repository root> diff <base>..<head> -- . ':!*.err' ':!*.json'`.
  Then read the `.err` and `.json` files of the codes it changes.
- <the other documents to read with the target, at their paths: for a plan, its spec; for a diff, its spec and plan when there are any. Delete this line when there are none.>
- `CODING_STANDARDS.md`. Its rules apply to every diff. For a spec or a
  plan, check that code built from it can meet them.
- `docs/adr/0006-values-correctness-simplicity-readability-performance.md`,
  and every other ADR in `docs/adr/` that the target touches.
- `GLOSSARY.md`, and "What works today" in `docs/roadmap.md`.
- `<test-audit SKILL.md path>`. Judge the tests, or the plan's tests, with
  its authoring gate, value bar and coverage gaps.
- `<codebase-design SKILL.md path>`. Use its vocabulary (deep modules,
  interfaces, seams) to judge where code goes and what each module hides.
- `<rust-best-practices SKILL.md path>`. Judge the Rust idiom with it, as
  "Reviewing" in `CODING_STANDARDS.md` says.

# Values

Sisu is to become a compiler people can trust. ADR 0006 orders its values:
correctness, simplicity, readability, performance. Sisu has no users yet,
so backward compatibility weighs nothing. Justify an idiom by what other
languages do, not by what Sisu did before.

# Check, for every kind

1. **Correctness.** Wrong output; a miscompile; IR that is unsound or that
   LLVM rejects; a checker that accepts a wrong program or rejects a right
   one. Try the edges: empty, one, nested, recursive, overflow, NaN,
   non-ASCII text, evaluation order. Check how the change composes with
   the features that exist: optionals, classes, methods, arrays, strings,
   `f64`, constants, loops.
2. **Memory safety.** Reference counts balance on every path: early
   return, `break`, `continue`, a panic, a loop, copy-on-write. No leak,
   double free or use after free.
3. **Robustness.** No input makes `sisu` panic or hang. No path is
   quadratic in the size of the source. Deep nesting, huge literals and
   long programs work or fail with a diagnostic. Between tasks, no
   reachable crash (see "Staging work across tasks" in `CODING_STANDARDS.md`).
4. **Simplicity and readability.** The smallest design that does the job.
   No option, layer, abstraction or generality that nothing needs yet.
   Each invariant lives in the module that owns it. Names match
   `GLOSSARY.md`.
5. **Performance.** Of `sisu`: no clone, allocation or repeat pass
   without a reason, anywhere. On a per-token, per-node or per-lookup
   path, that reason is also measured or obvious. Each new walk, fixpoint
   or worklist is linear in the program: build the worst input you can
   find, and give its cost. Of the generated code: the IR after `-O2` does no avoidable work. A claim that
   something is faster, or costs nothing, needs callgrind counts.
6. **Tests.** Judge them with `test-audit`. Unit tests and end-to-end
   cases (`crates/sisuc/tests/run`, and the fixture of each diagnostic
   code) both prove the change. Each test fails when the contract its
   name states breaks.
7. **Diagnostics.** See the next section.
8. **Claims.** Check every claim about LLVM, inkwell, a crate, a tool or
   the existing compiler against its source: the version in `Cargo.lock`,
   or the code in the repository. List a claim you could not check under
   "Unverified"; never report it as true.
9. **Decisions.** A choice that is hard to reverse (syntax, semantics, the
   runtime ABI, a diagnostic code) has an ADR, or the target says why it
   needs none. A new term is in `GLOSSARY.md`.

# Diagnostics

Each new or changed diagnostic must earn its place:

- A real user makes this mistake. A contrived mistake gets no diagnostic
  of its own.
- No existing code covers it. A new code only when its explanation would
  differ from every existing code's; otherwise it is a new form of an
  existing code. One rule reports one code wherever it fires.
- It tells the user more than the general error would.
- The message says what is wrong. The label says what the code under it
  did. The help says what to do next. Each is simple, clear and specific
  technical English: short sentences, common words, the names the user
  wrote, no compiler-internal terms.
- The primary span covers the offending code.
- It offers a fix only when the fix is obvious and simple.
- One mistake gives one diagnostic, not a cascade.
- The trigger fits the message. Try inputs at the edge of the trigger,
  for example the same line after a complete construct instead of an
  incomplete one. Each input the trigger matches makes the message true;
  an input where the message would be false is a finding.

Report the opposite too: a likely mistake that the target leaves with a
vague or generic error, or with none.

# Check, for a spec

- **Semantics are stated.** For each construct: its syntax, its typing
  rule, what it does at run time, its evaluation order, and its errors.
  Anything the spec leaves to the implementer is a finding.
- **Composition.** The spec says how the construct works with each
  existing feature it can meet.
- **Prior art.** It says how Rust, Swift, Kotlin, Go or a similar language
  handles the same thing, and why Sisu differs where it does.
- **Scope.** Goals and non-goals are explicit. It is the smallest version
  that works. Deferred parts are named, ready for the roadmap.
- **Diagnostics.** Each error the feature creates is designed: its code
  (new or existing), its message, label and help text, and its span.
- **Testability.** Each requirement can be proved by a test.
- **Scenarios.** The acceptance tests are Gherkin scenarios with IDs, as
  "Specs and plans" in `CODING_STANDARDS.md` says. Each THEN states a
  result that a test can see.

# Check, for a plan

- **Coverage of the spec.** Every requirement in the spec maps to a task.
  No task adds scope the spec lacks. List each gap.
- **Done conditions.** Each task has one that a reviewer can check.
- **Order.** After each task, the code builds and the tests pass. The
  staging rules in `CODING_STANDARDS.md` hold.
- **Assertions.** No sentence lets the executor change an expected value,
  a count or a marker to fit what the code does. Each test states the
  property it must prove.
- **Diagnostic text.** Each message, label and help text a task names
  matches the spec word for word.
- **Claims.** As in "Check, for every kind", item 8.
- **Tests.** Each task names its unit tests and end-to-end cases, and they
  pass the `test-audit` value bar.
- **Scenarios.** Each spec scenario is in exactly one task, with the test
  that proves it. Test names follow "Tests" in `CODING_STANDARDS.md`.

# Check, for a diff

- Apply every rule in `CODING_STANDARDS.md`. Approve against the done
  condition in the spec, the plan or "Focus".
- A defect the diff adds blocks. A defect the old code had too is a nit.
- Check cheap claims yourself: build, run the tests, write a probe
  program. To build a commit, export it as "Export a commit to build it"
  in `CODING_STANDARDS.md` says. Keep two export directories in your
  scratch directory, `base` and `head`; for another commit, empty one and
  export into it again. Build both with one
  `CARGO_TARGET_DIR=<scratch directory>/target`. Each new directory costs
  a full build, about 7 GB.
- To compare diagnostics before and after a change, build the base and
  the head, write probe programs into one directory, and run
  `python3 scripts/diag_diff.py BASE_SISU HEAD_SISU DIR`. It prints
  each program the head newly accepts, newly rejects or reports
  differently, with the codes and positions of both.
- When the diff re-blesses expected files by a fixed rewording, run
  `python3 scripts/golden_diff.py <base>..<head> OLD NEW [OLD NEW]...`
  and sample a few files rather than read them all. It fails on any
  changed line that is not the old line with those substitutions.
- When "Focus" names a verify log, check that its last line names the
  target's head commit, then check the note on each surviving mutant and
  uncovered region. Otherwise, name the riskiest changed path that no
  test runs.

# Focus

<what the caller knows that the target does not say, or "none">

# Report

Your final reply is the report. It has these parts, in this order:

1. `Verdict: approve` or `Verdict: changes needed`, with the count of
   blocking findings and of nits.
2. One block per finding, blocking findings first:

   ```
   ### <n>. <title>
   Severity: blocking | nit
   Where: <file:line, or the target's section heading and line>
   Problem: <what is wrong, in one or two sentences>
   Evidence: <the input or repro command and what it gives, or the source you cite>
   Fix: <the smallest change that fixes it>
   ```

   A finding is blocking when it means wrong behavior, a crash, a leak,
   unsound IR, a wrong diagnostic code or span, a cascade, diagnostic
   wording that is wrong, misleading, vague or full of jargon, semantics
   the target leaves undefined, a task no reviewer can check, or a broken
   rule of `CODING_STANDARDS.md`. A design clearly more complex than the
   job needs is blocking when you can name the simpler design. Everything
   else is a nit, and a nit names the input whose output, diagnostic or
   cost changes, or says "comment only".
3. `Checked and clean:` one line per item of the checks above that found
   nothing, with what you looked at.
4. `Unverified:` each claim you could not check, and why.
