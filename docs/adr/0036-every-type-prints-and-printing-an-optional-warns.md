# Every type prints, and printing an optional warns

Status: accepted, 2026-10-28. It builds on
[ADR 0024](0024-optionals-nest.md), whose rule for the text of an optional
still holds. It retires [E0060](../diagnostics/E0060.md) (milestone 8).

Every type but `unit` and `never` prints with `print` and `eprint`, and
interpolates in `"{x}"`. A value prints the way the code that built it reads:
an array prints as `[1, None]`, an enum variant as `Rect(w: 2, h: 3)`, and an
object as its class name. A type gets this text from a generated `Display`.

An optional is a special case, because the text of a missing value is the word
`None`:

- **Printing an optional at the top level compiles with
  [W0015](../diagnostics/W0015.md).** `print(x)`, `eprint(x)` and `"{x}"`
  warn when the type of `x` itself is an optional. The warning points at `x`.
  Its help gives three fixes: unwrap with `if let`, give a default with `??`,
  or call `.to_string()` to print `None` on purpose.
- **An optional inside another value does not warn.** An array of optionals,
  the payload of an enum and the optional inside an optional print as their
  code reads. A call of `.to_string()` does not warn. A generic function that
  prints a `T` does not warn, also when `T` is an optional where it is called.
- **A precision on an optional is an error.** `"{f:.2}"` with `f: f64?` gives
  [E0111](../diagnostics/E0111.md) and no W0015. For an `f64?`, the help says
  to give a default with `??` first. A width or an alignment on an optional
  only warns.

## Why

The bug to catch is `Hello None`. A program that writes a missing name as text
has a mistake that no error shows. A warning tells the author to choose what
the program writes. It does not reject a program that is right, such as a debug
print of a value that can be `None`.

## Considered options

- **Reject an optional, as milestones 2 to 7 did (E0059, E0060 and E0098)**:
  rejected. The rule gave a different error for each place: a plain optional,
  an optional array and an interpolated part. It also made one type refuse the
  text that every other type prints.
- **Print an optional with no diagnostic (Kotlin)**: rejected. Kotlin gives a
  null the text "null" with no diagnostic. The doc of
  [`Any?.toString()`](https://kotlinlang.org/api/core/kotlin-stdlib/kotlin/to-string.html)
  says that a null receiver gives `the string "null"`. A program that prints a
  missing value by accident then writes that word and shows no error.
- **Warn for an optional at any depth**: rejected. `[1, None]` is a value that
  the program may want to see, and the author wrote its optionals on purpose.
- **Warn, and also warn for a precision on an optional**: rejected. A precision
  on an `f64?` is already an error, so a second report for the same part adds
  nothing and its help leads to the error again.
- **Apply a precision to the value inside an `f64?`**: rejected. A precision has
  no meaning for `None`, and the author must choose the text for a missing
  value.

## Consequences

- E0060 retires. Its page stays, with a status line that links this ADR.
- E0059 keeps its count, no-value and function-name forms. E0098 keeps its
  `unit` form. Neither has an optional form.
- `x.to_string()` of an optional prints `None`, or the text of the value inside,
  by the `Some` rule of ADR 0024.
- The program fuzzer prints optionals at the top level, so the generated
  programs reach W0015.
