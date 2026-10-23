# Area: compiler bugs

A miscompile prints the wrong answer and exits 0, so only an oracle
catches it. Every probe here needs one.

## What the language is

`docs/roadmap.md` "What works today" lists what is implemented. An ADR or a
roadmap entry is an oracle only for behavior this checkout implements:
`docs/roadmap.md` marks later milestones as planned, and ADR 0008's `weak`,
for example, does not exist yet. A checker that rejects planned syntax is
correct.

## Oracles

- **Differential.** Plain and `-O2` builds print the same bytes and exit with the same code. Two builds of one file give byte-identical `--emit ir-raw` text and executables.
- **Reference.** The same computation in Python or Rust, with Sisu's semantics as the toolbox's "Program generation" requires. `generated_arithmetic_should_print_what_rust_computes` in `crates/sisuc/tests/programs.rs` does this for arithmetic; extend it to control flow, arrays, objects and optionals.
- **Rewrites.** Two programs that must print the same: a `for` written as a `while`, an expression moved into a function, a `let` added for a value used once. A rewrite must keep the number, order and scope of evaluations, and every `break`, `continue` and `return` target. Swap the operands of `==` only when both are free of effects and panics.

## Probes

- Generate random well-typed programs from a small grammar (integers, `bool`, `u8`, locals, `if`, `while`, functions, arrays, one class) and run them through the differential and reference oracles. Thousands, not tens.
- Evaluation order: give every operand a visible effect (a `print`, or a distinct panic) in calls, constructor calls, array literals, indexing, compound assignment, field and element writes, `&&`, `||` and chains. Compare with the order the header of `mir/lower.rs` lists.
- Shadowing and scopes: the same name in nested blocks, in loop bodies, in `if let` and `while let` bindings, and as a parameter.
- Edges: `i64::MIN` and `i64::MAX` literals and arithmetic, `u8` 0 and 255, comparison chains with equal ends, empty loops, `break` in the first iteration, `continue` in the last.
- Optionals: `?.` on a chain where each link is `None` in turn; `??` with a right side that has effects (does it run only when needed?).
- Checker: each implemented rule (`let` immutability, `mut` fields, a bare field set once by the constructor, `is` only on objects), with a program that should pass and one that should fail.

## Not a finding

An output that differs only because a probe relies on behavior the
documents leave open. Name the document you checked.
