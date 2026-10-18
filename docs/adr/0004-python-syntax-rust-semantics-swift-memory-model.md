# Python syntax, Rust semantics, Swift memory model

Sisu takes the best parts of three language families and settles conflicts
between them with one rule: the surface syntax follows Python's readability,
the semantics follow Rust's correctness, and memory management and everyday
ergonomics follow Swift. Where Python is the only outlier on a
spelling the other two share, the familiar spelling wins: logical operators
are `&& || !`, not `and or not`.

| From | Take | Leave behind |
| --- | --- | --- |
| Python | Little punctuation, comparison chains, `for x in xs`, string interpolation, a full standard library | Dynamic typing, indentation as syntax |
| Rust | `if` and blocks as expressions, `let mut` (ADR 0011), enums with an exhaustive `match`, `Result` and `?`, overflow checks, diagnostics, trait-like interfaces | Lifetimes, the borrow checker, `;` deciding a block's value |
| Swift | `let`, newline-terminated statements, `T?` with `?.` and `??`, reference counting, named and default arguments | Nothing yet |

Principles that every later syntax choice is checked against:

- Code reads top to bottom, without punctuation noise.
- One obvious way to do a thing.
- Errors speak in plain words and point at the code.
- Memory is managed for you.

## Considered options

- **Follow one language (Swift)**: fewer decisions, but it would
  give up Rust's exhaustive `match` and `Result`, and Python's comparison
  chains and readable loops.
- **`and or not` for logical operators**: reads closest to English, but Rust and
  Swift both write `&& || !`, so most readers expect those.

## Consequences

- Earlier decisions follow the rule: no `;` (ADR 0001), immutable by default
  (ADR 0002, spelled `let mut` since ADR 0011), panics instead of wrapping
  (ADR 0003).
- A new feature names which family it comes from and why; a conflict is
  settled by the rule above, not case by case.
