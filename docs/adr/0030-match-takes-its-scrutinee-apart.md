# `match` takes its scrutinee apart

Status: accepted when part 5 of the mir merges. It supersedes
[ADR 0019](0019-enums-are-values-named-through-their-enum.md)'s rule that
`match` moves nothing out of its scrutinee.

ADR 0019 had an arm copy each binding and release the scrutinee. A box that
the scrutinee held alone was freed and then allocated again by the next
construction of the same enum. `tree_rewrite` pays for that: each round
allocates and frees 8,190 boxes.

When an arm commits on an owned scrutinee of a counted enum, it takes the
scrutinee apart:

- **The commit decides which parts move.** A counted binding moves into its
  name. A boxed field that a binding reads, or that a sub-pattern looks
  inside, is tested. A counted part that nothing moves or tests is released.
  The scrutinee is empty afterwards.
- **A box held alone becomes a reuse token.** If a tested box has count 1, its
  contents are taken apart by the same rule and its block becomes a token. A
  construction of the same enum builds into the token's block instead of
  calling `sisu_alloc`; every exit that builds nothing frees it. A shared box
  gives no token: its contents are retained and its count drops by one.
- **The test runs after the guard.** A failing guard never sees a moved field.
- **Only enums.** A class has identity, which `is` sees.
- **One kind of arm keeps the copying commit.** An arm whose `|` binds a
  counted name or tests a box. A boxed `F?` field does not: its binding is
  copied before the take-apart and its box stays in the rest, so the box is
  released with it and never becomes a token.

## Considered options

- **Keep ADR 0019's commit.** One count up and down per part, plus a
  `sisu_free` and a `sisu_alloc` per rebuilt box: 8,190 of each per round of
  `tree_rewrite`. It needs no new statement, and it costs those allocations.
- **Take apart while testing, before the guard.** Reuse starts as early as
  possible. A failing guard would then see moved fields, and every failure
  edge would need to put them back.
- **A per-pattern helper that tests counts itself.** It would put control flow
  at each use site, against [ADR 0025](0025-mir-holds-every-count.md)'s rule
  that codegen decides nothing. The take-apart is instead a per-type helper
  next to `drain`, and the pattern's shape is fixed in the statement.
- **Take apart each alternative of an `|` on its own success edge.** The
  alternatives bind a name at different places, and which one matched is known
  only at run time. This is linear in the alternatives. The user deferred it
  to a follow-up on 2026-10-21, so such an arm keeps the copying commit:
  `Add(l, r) | Mul(l, r) => Expr.Add(simplify(l), simplify(r))` still frees
  two boxes and allocates two per rewrite, when the input boxes are held
  alone.
- **An `F?` token from a helper that returns `null` for an absent box.** A
  list rewrite such as `Link(s, next) => Chain.Link(s, if let n = next { ... })`
  would reuse the tail's box. The user deferred it to a follow-up on
  2026-10-21, so a boxed `F?` field is copied, never reused, and its box is
  released with the rest.

## Prior art

Checked with GitHits.

- **Lean 4** inserts `reset`/`reuse` before borrow inference and keeps reuse
  targets owned (lean4@a6f30730 `src/Lean/Compiler/LCNF/Passes.lean:144-158`,
  `InferBorrow.lean:19-48`).
- **Roc** runs its reset and reuse pass after inc/dec insertion (roc@c47a8e9c
  `crates/compiler/load_internal/src/file.rs:2900-2945`).
- **Koka** reuses after Perceus (koka@198fc3bb `src/Backend/C/FromCore.hs:91-98`).

## Consequences

- A shared box keeps today's counts in balance, but its whole contents are
  retained and its unbound counted parts released again. That is one retain and
  one release more than ADR 0019's commit per unbound counted part, and per
  tested box, of a shared box. Copying only the bindings would put a count test
  at the use site.
- An `|` arm with a counted binding keeps the copying commit, and a boxed `F?`
  field is copied and never reused, until the two follow-ups above land.
- The certifier tracks tokens like any owned value: a token must be built into
  or freed exactly once on every path (rule 1).
- A token made before a loop is not used inside or after it. The pass loses
  that reuse, never soundness.
- Part 4's inference keeps a parameter owned when its `match` would reuse a
  box. [ADR 0029](0029-parameters-are-borrowed-unless-consumed.md) borrows a
  parameter that is only read, and a borrowed scrutinee is never taken apart.
  It does so even when the `match` is not the parameter's last use: the
  scrutinee is then a copy, its box is shared, nothing is reused, and the
  caller pays a retain where it used to lend the value.
