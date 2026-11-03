# Arrays are values, shared until written

Superseded in part by [ADR 0043](0043-mut-parameters-copy-in-and-out.md): a
`mut` parameter changes the caller's place, so the rule "No way to change the
caller's array" no longer holds. The decisions below stay as written.

Binding an array, passing it or storing it behaves as a copy: no other holder
can change your array. The copy is made lazily. An array is a
reference-counted heap block, and a write to an element first checks the
count: at 1 it writes in place, above 1 it copies the block, then writes. A
read, a binding, an argument and a loop over an array never copy.

Two rules keep copies predictable:

- **Last use hands over.** When a local is read for the last time before it
  is reassigned or goes out of scope, its reference moves instead of being
  shared. So when nothing else holds the array, `xs = fill(xs)` and
  `let mut ys = xs` on a parameter's last use make no copy. If something else
  still holds it, the write copies, as value semantics requires. The rule is a
  language guarantee, not an optimizer pass, and it never causes an error.
- **No way to change the caller's array.** There is no `inout` or `&`
  parameter. A function that changes an array returns the new one, and the
  caller writes `xs = f(xs)`, so every change to `xs` reads `xs = ...`, or
  calls a method that changes its receiver, such as `xs.push(x)`, on a
  `let mut xs` (ADR 0011). Shared mutable data is a class: a class is for
  something with identity.

We chose this for correctness (no aliasing surprises) and simplicity (one rule
for every array, nothing new at the call site), which ADR 0006 ranks above
performance. Swift works the same way, without `inout`.

## Considered options

- **Arrays as references (Python, Kotlin, Java)**: the simplest to build, on
  top of ADR 0005 unchanged, but any function you call can change your array,
  and `let xs` freezes only the name.
- **Moves and an explicit `clone()` (Rust)**: no hidden copy at all, but the
  checker must prove each written array has one owner. That needs moves,
  "use of a moved value" errors, owning parameters and rules for arrays held
  in class fields: a light borrow checker, which the roadmap rules out.
- **`inout` parameters (Swift)**: a function changes the caller's array in
  place, at the cost of a marker at the call site and a second kind of
  parameter. ADR 0011 later allows only the receiver to change, with
  `mut self`.

## Consequences

- `let xs` stops every slot of the array, and of arrays nested in it, from
  being replaced; `let mut xs` allows element writes. Class objects an array holds
  stay shared and mutable: value semantics does not deep-copy them.
- Every element write costs a load, a compare and a branch for the count,
  except in a loop that checks it once before it starts (ADR 0021).
- A copy happens only where two holders exist and one of them writes, which
  is where value semantics needs one; a loop that could write makes that
  copy before it starts (ADR 0021).
- Records (roadmap milestone 10) are expected to follow the same rule.
