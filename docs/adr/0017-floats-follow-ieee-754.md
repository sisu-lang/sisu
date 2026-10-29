# Floats follow IEEE 754

Status: accepted, 2026-10-15. Integers panic ([ADR 0003](0003-arithmetic-panics.md));
`f64` does not. It replaces the roadmap's line that a silent NaN is a wrong
answer.

`f64` arithmetic, methods and comparisons never panic. Division by zero gives
infinity or NaN, `(-1.0).sqrt()` gives NaN, and an operation that overflows gives
infinity, as IEEE 754 says. A program that wants a stop checks the value
itself.

Two rules follow from IEEE 754 and are worth stating.

- **`min`, `max` and `clamp` order `-0.0` below `0.0`.** They follow IEEE
  754-2019 `minimumNumber` and `maximumNumber`, so `(-0.0).max(0.0)` is `0.0`
  and `(0.0).min(-0.0)` is `-0.0`. `==` still says `-0.0 == 0.0`.
- **`==` on a composite holding an `f64` has no same-object shortcut.** It
  compares every element. An array or enum that holds NaN is therefore not
  equal to itself, even when both sides are the same value. A class has no
  generated `==` ([ADR 0022](0022-classes-compare-by-identity.md)), so a class
  holding NaN is equal to itself under `is`. A written `eq` that compares an
  `f64` field follows IEEE 754.

## Considered options

- **Panic on NaN and on division by zero (Python's `ZeroDivisionError`)**:
  a wrong answer would stop the program. But a check after every operation
  costs time and blocks vectorisation, and numeric code everywhere expects
  IEEE results.
- **IEEE 754 (Rust, Swift, C)**: the same results on every machine, no check
  per operation, and what a numeric programmer already knows.

## Consequences

- `f64.nan != f64.nan`. An array or enum holding NaN is unequal to itself,
  and `contains(f64.nan)` is false.
- `f64` conforms to `Eq` and `Ord` with IEEE semantics, so generic and
  concrete code agree (milestone 8): `nan.eq(nan)` is false and NaN is neither
  less nor greater than anything. `f64` gets no `Hash`.
- A NaN can travel a long way before it shows up as a wrong answer. Printing
  it as `nan` is what makes it visible.
