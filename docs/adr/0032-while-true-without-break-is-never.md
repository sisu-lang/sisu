# `while true` with no `break` is `never`

Status: accepted, 2026-10-22; amended by [ADR 0034](0034-loop-and-loop-labels.md).

A `while` whose condition is the literal `true` has type `never` unless a
`break` targets it. Otherwise it has type `unit`, as before.

- Sisu has no labelled `break`, so a `break` targets the innermost loop around
  it. A `break` inside a nested `while`, `while let` or `for` ends that loop
  and does not count.
  ADR 0034 adds loop labels; a labeled `break` counts for the loop it names.
- `return`, `panic`, `todo` and `continue` do not count: none of them reaches
  the statement after the loop.
- Every `break` the checker meets counts, even one that can never run:
  `while true { return 1; break }` stays `unit`, and its `break` gets W0001.
- The rule applies to the literal only. `while 1 == 1` and a `const` that is
  `true` stay `unit`. `while (true)` is the literal, since the AST has no
  parenthesis node.
- `while let` and `for` do not change.

The existing block rules do the rest: a `never` statement makes its block
`never`, so E0031 no longer fires, and the statement after the loop gets
W0001. No diagnostic code is added or changed.

## Why

A function `-> T` that leaves only from inside `while true`, by `return`, had
to end with a dead tail expression to satisfy E0031. Code written after such a
loop got no W0001, though it can never run. Typing the loop `never` fixes both
without new syntax.

Go and Rust set the precedent. In Go, a `for` with no condition and no `break`
that refers to it is a terminating statement. In Rust, `loop` has type `!`
unless a `break` leaves it, and a `break` counts wherever it appears.

## Considered options

- **Folding other always-true conditions** (`1 == 1`, a `true` constant):
  rejected. It makes the type depend on constant evaluation, and the literal is
  the one spelling people write on purpose.
- **A `loop` or `forever` keyword**: rejected. [ADR 0004](0004-python-syntax-rust-semantics-swift-memory-model.md)
  asks for one obvious way, and `while true` already exists.
- **`while` with a value and `break v`**: rejected. It is a larger feature than
  this problem needs.
- **Labelled `break` and `continue`**: out of scope. This ADR does not decide
  them.
