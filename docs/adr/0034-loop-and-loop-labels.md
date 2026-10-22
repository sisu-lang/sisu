# `loop`, `break` with a value, and loop labels

Status: accepted, 2026-10-22.

`loop { body }` is an expression that runs `body` until a `break` targets it,
or a `return`, `panic` or `todo` leaves it. A `break v` gives the loop that
value. A name before a loop, `outer: for ...`, is a **loop label**, and
`break outer`, `continue outer` and `break outer v` reach that loop instead of
the innermost one.

- `loop` is a keyword. Its type is `never` with no `break` that targets it,
  `unit` with only bare `break`s, and `T` when every `break` carries a `T`.
  A bare `break` beside a `break` with a value whose type is not `never` is an
  error. With only `never` values beside bare `break`s the loop is `unit`, and
  when every value is `never` the loop is `never`.
- `break v` is allowed only when the loop it targets is a `loop`. In a
  `while`, `while let` or `for` it is an error whose help says to use `loop`.
  `continue` never takes a value.
- A loop label is a plain name followed by `:`, before `loop`, `while`,
  `while let` or `for`. It may stand on its own line. There is no punctuation
  in it (`'outer`, `@outer`).
- **Label or value.** After `break` or `continue`, a name is the loop label
  when a loop around the jump has that label, and whatever follows it is the
  value. Otherwise the name begins the value. The parser decides this from the
  loop labels it has open, so `break outer -1` is the label `outer` and the
  value `-1` when `outer` is a loop label, and the value `outer - 1` when it
  is not.
- A loop label may not share its name with anything in scope at a jump that
  uses it, and a loop may not reuse the label of a loop around it. A loop
  label that no jump names is a warning with a fix that deletes it.
- A jump never leaves its function, and a jump that would leave a loop header
  stays E0082.
- `while true` warns (W0009) and points at `loop`, with a fix that rewrites it.
  [ADR 0032](0032-while-true-without-break-is-never.md)'s rule stays: a
  `while true` with no `break` is `never`, which is why the fix is certain.
- A labeled `break` lowers to the target loop's releases and one jump, as a
  plain `break` lowers to its own loop's. A loop label costs nothing at run
  time.

This ADR reverses one point of ADR 0032: that ADR rejected a `loop` keyword
because a `return v` covers the same need. A search that gives a value, and a
way out of two loops at once, are worth a keyword.

## Why

Two things were awkward to write. A search that gives a value took a `let mut`
and a flag, or a function so that `return x` could end it. Leaving two loops
took a flag and a second `break`. Both are common, and the flag version hides
what the code means.

Plain names keep labels readable without new punctuation. The label-or-value
rule keeps `break outer -1` from needing type checking to read: the parser
knows which names are open loop labels, so one line has one reading.

[ADR 0006](0006-values-correctness-simplicity-readability-performance.md)
asks that a design name its costs:

- `break x` reads by context. Whether `x` is a loop label or a value depends
  on the loops around it. The clash rule makes this safe: a loop label cannot
  share a name with a value or type in scope at a jump that uses it, so a
  reader never meets `break search(rest)` that silently means the loop label
  `search`.
- An endless loop has two spellings, `while true` and `loop`, until the
  `while true` warning moves code to `loop`.

Prior art:

- Swift and Go have plain-name labels but no `break` value.
- Rust has `break` values but writes labels as `'label`.
- Zig has both, but writes `break :label value`.

None has both with a plain name after `break`, which is why Sisu needs the
label-or-value rule.

## Considered

- **The checker resolves labels.** The parser always reads `break <expr>`, and
  the checker turns a bare name into a loop label. `break outer -1` then
  parses as `outer - 1` before anyone knows `outer` is a loop label, so the
  checker has to rewrite it. Rejected: that is the ambiguity this design rules
  out.
- **Desugar loop labels into flags.** A flag per labeled loop and a chain of
  plain `break`s, with no change to the typed tree or mir. Rejected: it costs
  run time, gives worse diagnostics, and cannot carry a value.
- **`repeat` as the keyword.** Rejected: it reads as a loop that tests its
  condition at the end, and `repeat` stays free for one.
- **`forever` as the keyword.** Rejected: it is false, since the loop ends at
  a `break`.
- **A bare `while { }`.** Rejected: it reads as a missing condition, and a
  typo compiles.
- **Rust's `'outer`, Kotlin's `@outer`, Zig's `:outer`.** Rejected: `'outer`
  looks like a char literal such as `'a'`, and the others are punctuation the
  user did not want.
- **Labels on plain blocks** (Rust's `'a: { ... break 'a v }`). Out of scope:
  a second way to leave early.
- **`while` or `for` that give a value.** Out of scope: only `loop` has a value
  on every path that leaves it.
