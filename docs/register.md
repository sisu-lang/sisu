# Architecture register

This file holds two kinds of facts that no other committed file holds yet:

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
  decision, of any kind. Its row goes away when the work that builds it
  merges, and the text moves into an ADR, the reference or the glossary.
- **A debt row** goes away when the milestone that pays it fixes it. A new row
  needs measured evidence: the Rust item or file, and a count where there is
  one.

## Rulings not built yet

| Ruling | Decided | Committed text that says otherwise | Built by |
| --- | --- | --- | --- |
| The value type is `struct`, not `record`. `class` stays. | 2026-10-28 | `docs/roadmap.md`; ADR 0011 | Milestone 9 part 1, Task 0: a new ADR, the lexer, E0083 |
| Tests use the `@test` attribute, not `test` blocks. | 2026-10-26 | `docs/roadmap.md` (`test` blocks after milestone 6); ADR 0027 | The attributes work |
| The fixed-size inline array is `FixedArray<T, N>`. There is no `Slice<T>` type: `xs[a..b]` is a `[T]` that shares its elements. | 2026-10-17 | none | Milestone 9 part 1, Task 0 (roadmap) |
| `mut` parameters (`fn set(mut cells: [i64])`, called as `set(mut b.cells)`) replace the write rule. `exchange` becomes a prelude function. | 2026-10-30 | `docs/roadmap.md` (`x.exchange(with: v)` as a method) | `mut` parameters step 1, after milestone 8 part 8 and before milestone 9 |
| A closure can share a `let mut` local with `[mut name]` in its capture list. | 2026-10-29 | none | Milestone 9 stage 5, after the `mut` parameters design |
| A bare variant `V` means `E.V` when the expected type is the enum `E`. A name that is also a local is an error. | 2026-10-31 | ADR 0019 (bare `Num(2)`, the reserved `.Variant` form) | After slices: a new ADR |
| A deep drop counts its depth, and past a limit it puts the rest in a queue that the outermost drop empties (#191). | 2026-11-01 | ADR 0005 and ADR 0019, Consequences | The deep-drop plan |
| `sisu check` builds the MIR. Copies show as the loop-copy warning only, with no run-time copy report. | 2026-10-26 | none | The loop-copy warning |

## Debt

| Problem | Where | Pay before |
| --- | --- | --- |
| The five milestone 9 plans were written before the MIR. They put closure and weak-field counts in codegen, and do not name ADR 0029. | `docs/superpowers/plans/2026-10-16-milestone-9-part-*.md` (not committed) | Milestone 9 part 1 |
| Five decision points end in a wildcard arm, so a new variant gets the old answer with no build error. | `check::generic::rank_walk`, `check::generic::meets`; `conformance::matches`, `Unifier::resolve`; `mir::Rvalue::may_call_written` | Milestone 9 |
| `Body` holds four lifetimes in 48 fields: the generic scope, the body, one-shot hints and span caches. A closure needs a new body inside the outer generic scope. | `check::Body` | Closure checking |
| The clobber rule is in three copies. | `mir::params::clobbers`; `mir::pairs::kind`; `mir::certify::Clobbers::new` | Milestone 9 indirect calls |
| Codegen helpers apply the ADR 0029 calling convention by hand, a second time after the MIR. | `codegen::show` (`show_written`, `call_written_eq`) | Milestone 9 thunks |
| The walk over the parts that a helper visits is written twice. Mono checks the conformances that apply; `calls_written` takes a wider set on purpose. | `Mono::walk`; `conformance::calls_written` | Milestone 9 part 2 |
| The lexer's layout tracker restates parser positions, and `selfhost/lexer.sisu` copies it. | `lexer` (`Statements`, `continues_params`, `check_line_start`); `selfhost/lexer.sisu` | Milestone 9 function-type syntax |
| Number behaviour comes from `ty != Type::U8`, and about 20 hand-written lists name the number types. | `codegen::arith`; `check` | Number types, task 1 |
| The fuzzer copies two checker tables by hand, and passes its context in the `synth`, `typed` and `bare` flags. | `generate::coverage` (`CONVERSIONS`, `PURE_PRELUDE`) | Number types |
| The checker reads its name tables directly in 82 places. | `check` | Milestone 11, Task 0 |
| A function's identity is its display name (`"main"`). | `mir::params`, `mir` (`f.name == "main"`) | Milestone 11 or the C FFI |

Paths name Rust items in `crates/sisuc/src/`, not lines, so that a row stays
true when the file changes.
