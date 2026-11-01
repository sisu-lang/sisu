# A failure the caller can handle is a value, not a panic

Status: accepted, 2026-10-29. Written down on 2026-11-01. It extends
[ADR 0018](0018-a-conversion-that-can-fail-returns-an-optional.md) from
conversions to every function and method.

A panic stops the whole program (exit code 101). When a caller can check for a
failure or recover from it, the result type shows the failure. Use one of
these forms, in this order:

1. **A total behaviour**, when one result is the obvious answer for every
   input. In milestone 9's design, `take(n)` with `n <= 0` gives no
   elements. A later `zip` stops at the end of the shorter input.
2. **An optional `T?`**, when the failure has one reason that the caller
   already knows. `u8(n)` gives `None` when `n` does not fit (ADR 0018). In
   the collections design, `m.get(k)` gives `None` for a missing key.
3. **A `Result<T, E>`**, when the failure has more than one reason and the
   caller needs to know which.

## Where a panic stays

A panic is correct only where every value-based answer is worse:

- **The other answer is silently wrong.** Integer overflow panics
  ([ADR 0003](0003-arithmetic-panics.md)), because a wrapped number is a wrong
  result that no type shows.
- **The other answer is not memory-safe.** In the drop-blocks design, a drop
  block that keeps its object alive panics.
- **The short form has a checked partner.** An array index `xs[i]` and a map
  index `m[k]` panic when the element is not there, as in Python and Rust. A
  caller that cannot be sure checks first, or uses an optional form such as
  `m.get(k)` (collections design).
- **An explicit `panic("...")`** in the program.

## Why

Sisu has optionals and the prelude enum `Result`, and it has no exceptions. An
API that panics on input that the caller could check makes the program stop
for a case that the type system could show. ADR 0018 made this change for
conversions: `u8(x)` and `char(n)` panicked, and now return an optional.

## Considered options

- **A panicking variant beside each checked one**, such as Python 3.10's
  `zip(strict=True)` or Swift's trapping `Int(x)`: rejected. The short form
  is the one people write, and it stops the program on input that nobody
  checked.
- **Rust's pairs of `unwrap`-style and fallible APIs**: rejected for new APIs.
  The caller can write `?? panic("...")` in plain sight when a stop is what
  the program needs.
- **Values first, panics only where the list above allows (chosen).**

## Consequences

- A spec or plan that adds a function that can panic names the reason from
  "Where a panic stays". A review blocks a new panic without one.
- A new index operator comes with a way to check first: a length, a
  `contains`, or an optional `get`.
- Two panics in the roadmap's "What panics" table are not in the list above:
  `[v; n]` with a negative or too large `n`, and an I/O failure. Each gets a
  decision when its area changes.
- Specs written before 2026-10-29 were not searched for panics. Each one gets
  a check when its plan runs.
