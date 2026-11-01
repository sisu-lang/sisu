# Sisu has no function overloading

Status: accepted, 2026-10-16. Written down on 2026-11-01.

One name in one scope names one function. Two functions or two methods of one
type cannot share a name and differ only in their parameters.

A method that takes a closure gets its own name, as Kotlin's `sumOf` does:

```sisu
let total = xs.sum()
let paid = orders.filter(o => o.paid).sum_of(o => o.price)
```

## Why

- **Other features cover its uses.** Named and default arguments
  ([ADR 0004](0004-python-syntax-rust-semantics-swift-memory-model.md)) cover
  optional parameters. Generics and interfaces cover one operation on many
  types.
- **Overload resolution fights closure inference.** To choose an overload, the
  checker must know the argument types. A closure's parameter types come from
  the overload that the checker chooses. Swift has this loop, and reports
  "the compiler is unable to type-check this expression in reasonable time".
- **Diagnostics stay specific.** A call to one function has one signature to
  compare. A call to an overloaded name gets a list of candidates and a guess
  about which one the programmer meant.
- **One obvious way.** Two functions with one name and different behaviour
  break the rule in [ADR 0006](0006-values-correctness-simplicity-readability-performance.md).

## Considered options

- **Overloading by parameter types, as Swift, C++, Java and C# do**: rejected
  for the reasons above.
- **Overloading by the number of parameters only**: rejected. Default
  arguments already give a shorter call, with one declaration to read.
- **No overloading (chosen)**, as in Rust, Go and Python. A closure variant
  has a name that says what it does.

## Consequences

- A second function with the same name stays an error
  ([E0034](../diagnostics/E0034.md)), whatever its parameters are.
- The standard library gives a closure variant its own name, such as
  `sum_of` beside `sum`.
- Operators are a separate question. The roadmap keeps unrestricted operator
  overloading under "Deliberately avoided", and the generated conformances of
  milestone 10 come from a fixed list.
