# Special cases are attributes, not compiler name lists

Status: accepted, 2026-10-27. Written down on 2026-11-01.

When a check must treat some library functions or types in a different way,
the declaration carries an attribute. The compiler holds no list of library
names. The standard library and user code use the same attribute.

The first case is the warning for an unused value (W0004). Some methods
return a value that a caller often does not need, such as a method that
removes an element and returns it. The collections design marks them
`@discardable`, after Swift's `@discardableResult`:

```sisu
@discardable
fn remove(mut self, key: K) -> V? { ... }
```

## Why

- **A list inside the compiler is a hidden special case.** A reader of the
  library cannot see why one method warns and a similar one does not. An
  attribute is on the declaration.
- **User code gets the same power.** A program's own method that returns an
  ignorable value takes the same attribute. A compiler list serves only the
  names its authors knew.
- **The list and the library drift.** A list must change with each library
  change, in a different part of the repository.

## Considered options

- **A fixed list of library names in the checker**: rejected for the reasons
  above.
- **A naming convention**, such as a prefix for ignorable results: rejected. A
  name then carries a rule that the reader must know, and a rename changes
  behaviour.
- **An attribute on the declaration (chosen).**

## Consequences

- A spec that needs a check to treat library code in a different way proposes
  an attribute first. A list of names needs a reason why an attribute cannot
  work.
- Attributes need one syntax before the first one lands. The roadmap keeps
  "Attribute syntax" under "Open decisions".
- A name that the language reference defines, such as `main` or a built-in
  interface, is part of the language. This ADR covers library code.
