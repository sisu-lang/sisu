# Architecture register

This file holds two kinds of facts that the docs do not show, or show only in
part:

- **Rulings not built yet.** A design decision that is made, but that the code
  and the docs do not show. Where a committed doc says the opposite, the
  ruling wins.
- **Debt.** A known structural problem, and the milestone that must fix it.

Rules do not go here. A rule lives in an ADR (`docs/adr/`), in
`CODING_STANDARDS.md` or in `docs/architecture.md`. Bugs do not go here: they
are GitHub issues.

## How to use it

- **Reviewers** read both tables. A diff that goes against a ruling, or that
  makes a debt row larger, is a finding. A diff that grows a debt row names
  the row.
- **A ruling** goes into this file in the next pull request after the
  decision, whatever that pull request changes. Its row goes away when the
  work that builds it merges, and the text moves into an ADR, the reference or
  the glossary.
- **A debt row** goes away when the milestone that pays it fixes it. A new row
  needs measured evidence: the Rust item or file, and a count where there is
  one.
- **The `structure` area of `audit-sisu`** counts each debt row again, and
  finds a row whose count grew.

## Rulings not built yet

| Ruling | Decided | Committed text that says otherwise | Built by |
| --- | --- | --- | --- |
| The value type is `struct`, not `record`. `class` stays. | 2026-10-28 | `docs/roadmap.md`; ADR 0011 | Milestone 9 part 1, Task 0: a new ADR, the lexer, E0083 |
| Tests use the `@test` attribute, not `test` blocks. | 2026-10-26 | `docs/roadmap.md` ("What can start now", "Recovering from a panic"); ADR 0027; the lexer reserves `test` (E0083) | The attributes work |
| The fixed-size inline array is `FixedArray<T, N>`. There is no `Slice<T>` type: `xs[a..b]` is a `[T]` that shares its elements. | 2026-10-17 | none; the roadmap's "fixed-size inline array" bullet points here | Milestone 9 part 1, Task 0 (roadmap) |
| A `mut` parameter lets a function change the caller's array or struct: `fn set(mut cells: [i64])`, called as `set(mut b.cells)`. `exchange` becomes a prelude function. | 2026-10-30 | ADR 0007 and ADR 0011 (no parameter changes the caller's value); E0019; `docs/roadmap.md` ("`inout` parameters stay out", `x.exchange(with: v)` as a method) | `mut` parameters step 1, after milestone 8 part 8 and before milestone 9 |
| A bare variant `V` means `E.V` when the expected type is the enum `E`. A name that is also a local is an error. | 2026-10-31 | ADR 0019 (bare `Num(2)`, the reserved `.Variant` form); `docs/roadmap.md` ("Names and namespaces" keeps Swift's `.circle` open, with a note) | After slices: a new ADR |
| `sisu check` builds the mir. Copies show as the loop-copy warning only, with no run-time copy report. | 2026-10-26 | `docs/architecture.md` (check stops after the checker; the mir is built on the build path only); `GLOSSARY.md` (Front end); ADR 0023 and E0030 (a chain that mono stops passes `sisu check`) | The loop-copy warning |

## Debt

| Problem | Where | Pay before |
| --- | --- | --- |
| The five milestone 9 plans were written before the mir. They put closure and weak-field counts in codegen, and do not name ADR 0029. | `docs/superpowers/plans/2026-10-16-milestone-9-part-*.md` (the owner's working plans, not committed) | Milestone 9 part 1 |
| `Body` holds, in 48 fields, four kinds of state that the checker keeps for different lengths of time: the generic scope, the body, one-shot hints and span caches. A closure needs a new body inside the outer generic scope. | `check::Body` | Closure checking |
| The clobber rule is in three copies. | `mir::params::clobbers`; `mir::pairs::kind`; `mir::certify::Clobbers::new` | Milestone 9 indirect calls |
| Codegen helpers apply the ADR 0029 calling convention by hand, a second time after the mir. | `codegen::show::show_written`; `codegen::rc::call_written_eq` | Milestone 9 thunks |
| The walk over the parts that a helper visits is written twice. Mono checks the conformances that apply; `calls_written` takes a wider set on purpose. | `Mono::walk`; `conformance::calls_written` | Milestone 9 part 2 |
| The lexer's layout tracker restates parser positions, and `selfhost/lexer.sisu` copies it. | `lexer` (`Statements`, `continues_params`, `check_line_start`); `selfhost/lexer.sisu` | Milestone 9 function-type syntax |
| Number behaviour comes from `ty != Type::U8`, and about 20 hand-written lists name the number types. | `codegen::arith`; `check` | Number types, task 1 |
| The fuzzer copies the checker's conversion rules by hand, and keeps its own list of prelude functions that have no effect and cannot panic. It passes its context in the `synth`, `typed` and `bare` flags. | `generate::coverage` (`CONVERSIONS`, `PURE_PRELUDE`); `generate::Gen` | Number types |
| The checker uses its five name maps directly in about 80 places, and no lookup function hides them. | `check` (`function_ids`, `class_ids`, `enum_ids`, `interface_ids`, `constants`) | Milestone 11, Task 0 |
| A function's identity is its display name (`"main"`). | `mir::params`, `mir` (`f.name == "main"`) | Milestone 11 or the C FFI |

Paths name Rust items in `crates/sisuc/src/`, not lines, so that a row stays
true when the file changes.
