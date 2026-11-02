# `let ... else`, and `return`, `break` and `continue` are expressions

Status: accepted, 2026-11-02.

A program often gets the value out of an optional or an enum, and leaves
when there is none. Before this decision, it nested the rest of the function
inside an `if let`, or wrote a `match` with two arms. Sisu now has two
smaller forms for this:

```sisu
fn area(s: Shape) -> f64 {
    let Circle(r) = s else { return 0.0 }
    3.14 * r * r
}

fn total(xs: [i64?]) -> i64 {
    let mut sum = 0
    for o in xs {
        sum = sum + (o ?? continue)
    }
    sum
}
```

## Decisions

1. **Let-else.** `let pattern = value else { block }` takes the same
   patterns as `if let`, with the same rules. On an optional, a plain name
   means `Some(name)`. The names that the pattern binds are in scope from the
   next statement to the end of the enclosing block, and not in `block`. The
   block must not finish: it has the type `never` (ADR 0032), or the error
   E0172 applies. `let mut` is allowed only before a single name, and that
   name owns its value, as a `let mut` local does: a change never writes into
   the value that it came from. A `let` with a pattern that is not a single
   name and no `else` is the error E0174.
2. **`return`, `break` and `continue` are expressions** of type `never`,
   with their values and loop labels, wherever an expression can stand. Each
   one takes everything to its right that an expression can take:
   `a ?? return b ?? c` is `a ?? return (b ?? c)`. The rules for a `never`
   value do not change: `f(return)` is E0043, and `while (o ?? break) > 0`
   is E0082.
3. **The `}` rule (Rust's rule).** The value of a let-else cannot end in
   `}`. The parser reads the value first, and an `if` reads its own `else`,
   so `let x = if c { 1 } else { return 0 }` keeps its meaning. A value that
   ends in `}` before `else` is the error E0173, with a safe fix that puts the
   value in `( )`.

Two forms do the same job: `let v = o else { return 0 }` and
`let v = o ?? return 0`. Both stay, with no lint. `??` suits an optional, a
plain name and a one-line exit; let-else suits a pattern (a variant,
alternatives) or an `else` block of more than one statement.

## Why

- **One line unwraps and leaves, and keeps the name**: `let b = b ?? return 0`.
- **No new drop rules.** A name that let-else binds lives and is dropped as
  a `let` name is.
- **A long chain stays flat.** Each let-else is one statement, so a chain of
  them does not nest the rest of the block.

## Considered options

- **Flow narrowing** (after `b != None`, `b` has the type of its value):
  rejected. It costs a loop pre-scan, a table of paths, and "narrowed here,
  changed here" errors. Kotlin narrows, and its most common complaint is
  "smart cast is impossible": a test that looks the same as one that narrows
  does not narrow. Rust and Swift do not narrow. A copy of a class reference
  is the same object, so a new binding already writes through. The one case
  that only narrowing covers is a write through an optional struct, and the
  conditional `mut` call `fill(mut g?.cells)` covers it. Narrowing comes back
  only as its own design, and only if real programs show many `?? return`
  lines that it would remove.
- **The checker rewrites let-else into a `match`** whose arm binds names
  that later statements use: rejected. The mir drops an arm's bindings at the
  end of the arm, before their later uses, so a fix changes the drop rules.
- **The checker nests the rest of the block inside an `if let`**: rejected.
  1000 let-else lines make a tree 1000 levels deep, and the spans of W0001
  and the unused-name warnings move.
- **A new `tir` statement that the mir lowers with the pattern code of
  `if let` and `match` (chosen).**
- **A `guard` keyword (Swift)** for the `}` rule: rejected. Rust's rule
  needs no new keyword, and the form reads like `if let`.

## Consequences

- Milestone 10 reuses E0174 for tuple and struct patterns, and widens
  let-else to a tuple or a struct with a pattern that can fail. Until then,
  `let One(n) = v` on an enum with one variant is E0174.
- `mut` inside a pattern stays an open decision, for `if let`, `match` and
  let-else together.
- Let-else does not reuse the box of a counted enum, as `match` does.
