# An assignment through `?.` writes when the object is there

Status: accepted, 2026-11-01. It retires [E0022](../diagnostics/E0022.md)
(milestone 8).

An assignment or a compound assignment (`+=`, `-=`, `*=`, `/=`, `%=`) can go
through `?.`, as a `push` can. The target `c?.value = 2` writes the field
`value` of the object that `c` holds. The rule has two parts:

- **When every `?.` step holds a value, the program writes.** The object is
  calculated first, then the right side, then the write.
- **When a `?.` step is `None`, the program does nothing.** The right side does
  not run, and nothing is written. The statement has no value in both cases.

The target is still a place. A field without `mut` is an error
([E0001](../diagnostics/E0001.md)), as it is for `c.fixed = 1`. A `?.` step
before an index, such as `c?.rows[0].n = 3`, is an error
([E0062](../diagnostics/E0062.md)): the index would need an array that may be
`None`.

## Why

A `push` through `?.` already has this rule (`xs?.push(x)`,
`bag?.items?.push(x)`). Without it for an assignment, the program must unwrap
the object with `if let` to write a field, and does not need to unwrap it to
`push` onto the same path. One path then has two rules.

## Considered options

- **Keep E0022 (the program unwraps with `if let`)**: rejected. The rule makes
  `c?.items?.push(x)` valid and `c?.value = x` an error, with the same path
  and the same meaning.
- **Allow it (chosen).** The right side stays in the branch where the object
  exists, so no side effect runs for a missing object.

## Prior art

Swift has the same rule for optional chaining. In
`john.residence?.address = someAddress`, Swift does not evaluate the right side
when `residence` is `nil`. See "Accessing Properties Through Optional Chaining"
in
[The Swift Programming Language](https://docs.swift.org/swift-book/documentation/the-swift-programming-language/optionalchaining/).

## Consequences

- E0022 retires. Its page stays, with a status line that links this ADR.
- E0062 gives its `?.` fix in the target of an assignment and of a `push`, as
  it does in a read, unless an index follows the step in the chain.
- The program fuzzer writes through `?.` ([ADR 0033](0033-every-language-feature-has-a-generator-rule.md)).
