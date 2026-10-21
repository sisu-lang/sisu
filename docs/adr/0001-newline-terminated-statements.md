# Statements end at newlines, not semicolons

Amended by [ADR 0028](0028-a-semicolon-stands-in-for-a-line-break.md): a `;`
stands in for a line break between two things on one line.

No Sisu line needs a `;`. The lexer inserts a `Newline` token at a line break
after a token that can end a statement (Go's rule), unless the innermost open
bracket is `(` (Python's rule, stopped at `{` so a block inside parentheses
still separates statements). We chose this over Rust-style semicolons so
programs read without punctuation on every line; the cost sits in the lexer,
where it is easy to test. See the milestone 1 spec, "Statements end at
newlines".

## Considered options

- **Rust-style `;`**: a simpler lexer, but a block's value then depends on
  whether its last expression has a `;` (`x` versus `x;`), and every line
  carries punctuation.

## Consequences

- Layout rules: an operator ends a line rather than starting the next one,
  `{` stays on the line of its `fn`, `if` or `while`, and `} else` stays on
  one line.
- `-` is both unary and binary, so a line starting with `-` and a space is an
  error; otherwise `a` followed by a line `- b` would compile as two
  statements.
- Class bodies, `enum` bodies and `match` arms are one-item-per-line
  constructs: a field, a variant or an arm ends at its newline, or at a `;`
  before the next one on the same line (ADR 0028), and a comma between two of
  them is an error.
- Every later piece of syntax must be checked against the insertion rule.
  Already decided: from milestone 2, a line starting with `.` continues the
  previous one, so method chains can put each call on its own line (as in
  Swift and Kotlin).
- From milestone 8, a `>` can end a statement too, as the close of type
  arguments (`b: Box<i64>`). The parser drops the line break after a `>`
  that compares, so `a >` still continues onto the next line, and after the
  type parameters of a function's header, so `fn f<T>` goes on to its
  parameters. A call's type arguments get no such exception: `f<i64>` then
  `(x)` on the next line ends at the line break, as `f` then `(x)` does.
