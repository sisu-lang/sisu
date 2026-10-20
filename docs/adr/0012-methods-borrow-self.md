# Methods borrow `self`

Status: superseded by ADR 0029 before it was built, 2026-10-20.

A method receives `self` borrowed: the caller keeps its reference for the
whole call, and the method neither adds 1 to the count of `self` nor
subtracts 1 when it returns. Every other counted parameter stays an owned
reference (ADR 0005), released by the function that receives it. We chose
this because `self` is the parameter that methods almost never store, and
the binary_trees benchmark spends 11% of its time counting it: each call of
`self.left?.check()` writes the child's count twice and calls
`sisu_rc.release.Node`, while no count ever reaches 0 there
([Benchmarks](../benchmarks.md), "Where binary_trees spends its time"). Swift passes
`self` the same way, as a guaranteed parameter.

The caller must keep `self` alive for the call. It decides from where the
receiver comes:

- **A local, or a bare field reached from a borrowed object**: no count
  change. The method cannot reach the caller's locals, and a bare field
  never changes, so the object that holds it holds the receiver too
  (ADR 0011).
- **A `mut` field, a weak field or an array element**: the call could
  overwrite that place and drop the receiver. The caller adds 1 before the
  call and subtracts 1 after it.
- **Any other expression, such as `make(d).check()`**: the caller owns the
  result already and releases it after the call.

## Considered options

- **Keep `self` owned**: one rule for every parameter, but the caller adds 1
  for the call and the method subtracts 1 again on every call, including
  recursive ones such as a tree walk.
- **Borrow every counted parameter (Swift's default)**: saves the same pair
  for every argument, but a constructor or a setter stores its arguments, so
  it would add 1 to each one it keeps, where an owned argument is stored
  as it is. Decide this when a benchmark shows argument counting matters.
- **Remove pairs that cancel in an optimization pass (the roadmap's `mir`)**:
  more general, but it needs a new IR, and it must prove the same facts this
  rule gives the caller by construction.

## Consequences

- A method that stores `self`, for example in another object's field, adds 1
  to its count first, as a caller does with a borrowed local today.
- A `mut self` method on a record (ADR 0011) borrows its receiver the same
  way.
- When closures arrive (milestone 9), a closure that captures a `let mut`
  local can change it during a call, so a receiver held in such a local must
  be counted like a `mut` field.
