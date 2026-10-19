# A `;` stands in for a line break

Status: accepted, 2026-10-19. It amends
[ADR 0001](0001-newline-terminated-statements.md).

A `;` is a separator where the two things around it, on one line, would
otherwise need a line break between them: two statements in a block, two
fields or methods in a class body, two variants, or two `match` arms. It
separates them exactly as a line break does: `let x = 2; print(x * 21)`,
`if x { a; b }`, `class P { x: i64; y: i64 }`, `enum E { A; B }`,
`match n { 0 => a; _ => b }`.

Every other `;` is redundant: at the end of a line, before `}`, at the end of
the file, at the start of a line, right after `{`, every extra `;` in `;;`,
and between two top-level items, which need no separator. A redundant `;`
compiles, with warning W0007 and the safe fix "remove the `;`". Inside `( )`
and `[ ]`, where line breaks separate nothing, a `;` is still a syntax error.
`[value; length]` is unchanged.

## Why

An inline program for `sisu run -e` needs two things on one line:
`sisu run -e 'fn main() { let x = 2; print(x * 21) }'`. The change is to the
language, not to `-e`, so a program that works inline works pasted into a file.

ADR 0001 gave two reasons against Rust-style `;`. Neither applies. No line
needs a `;`, so no line carries punctuation. A block's value no longer depends
on `x` versus `x;`: `{ x; }` still gives `x`, and in a function that returns
`unit`, W0004 "unused value" fires as well, with its `let _ =` fix. A redundant
`;` is reported, so two spellings of the same code do not live side by side
unremarked.

## Considered options

- **Silence on a redundant `;`**: rejected. Swift, Kotlin, Python and Go
  accept a trailing `;` in the compiler and flag it in their tools (SwiftLint
  `trailing_semicolon`, Kotlin's "redundant semicolon", pylint
  `unnecessary-semicolon`, `gofmt`). Sisu has no separate linter; its compiler
  warnings play that role. Silence would also hide a Rust habit.
- **A `-e` dialect**: rejected, an anti-goal. Any rule that makes `-e` text
  parse differently from a file is two languages.
- **`,` separators**: a non-goal. ADR 0001 keeps a `,` between fields,
  variants or arms an error.

## Consequences

- E0012 retires, the first retired code. No `;` is an error of its own any
  more. Its explanation page stays, with a retirement status line.
- E0015 keeps its code, since its explanation is still about the comma, but
  its message changes: members are separated by a line break or `;`, not `,`.
  Its fix is "replace the `,` with `;`" when the `,` is mid-line. A `,` that
  ends its line or comes before `}` or the end of the file is removed instead,
  because a `;` there would itself be W0007.
- E0011's "expected end of line" becomes "expected end of line or `;`"
  wherever a separator could go. It has no fix, since the person often left out
  an operator and a `;` would be a guess.
- Where removing a `;` could join two lines into a different statement, it
  is kept for the parser to report, not called redundant. In `x\n; .f()` and
  `let x = ;` the `;` stays and the parser reports E0011. In `x;\n.f()` the
  `;` separates, and the parser reports the `.`. This keeps the safe-fix rule
  true by construction: removing a W0007 `;` leaves a line break or nothing
  where nothing was needed.
- [ADR 0001](0001-newline-terminated-statements.md)'s consequence on
  one-item-per-line constructs is amended: a field, a variant or an arm ends
  at its newline, or at a `;` before the next one on the same line.
