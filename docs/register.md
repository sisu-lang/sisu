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
- **A field count** in a row ("in N fields", with the struct as the Where
  cell, such as `check::Body`) is a test:
  `register_field_counts_should_match_their_structs` in
  `crates/sisuc/tests/architecture.rs` fails when the struct has another
  number of fields.

## Rulings not built yet

| Ruling | Decided | Committed text that says otherwise | Built by |
| --- | --- | --- | --- |
| The value type is `struct`, not `record`, and `class` goes (ADR 0046). The variable decides what can change. `pub mut` goes. | 2026-11-06 (`struct`: 2026-10-28) | `docs/roadmap.md`; `docs/reference/`; `docs/architecture.md`; `docs/diagnostics/`; `README.md`; `site/`; `GLOSSARY.md` (Class, Record, Object, Reference, Identity, Weak reference, Class-only interface, Weak capture, Take, Reach, Lost write); the ADRs that ADR 0046 names. Not committed: each spec that names a class | The class removal: the specs, then structs beside classes with the lexer and E0083, then the migration, then the deletion (ADR 0046) |
| Tests use the `@test` attribute, not `test` blocks. | 2026-10-26 | `docs/roadmap.md` ("What can start now", "Recovering from a panic"); ADR 0027; the lexer reserves `test` (E0083) | The attributes work |
| The fixed-size inline array is `FixedArray<T, N>`. | 2026-10-17 | none; the roadmap's "fixed-size inline array" bullet points here | Milestone 9 part 1, Task 0 (roadmap) |
| A bare variant `V` means `E.V` when the expected type is the enum `E`. A name that is also a local is an error. | 2026-10-31 | ADR 0019 (bare `Num(2)`, the reserved `.Variant` form); `docs/roadmap.md` ("Names and namespaces" keeps Swift's `.circle` open, with a note) | After slices: a new ADR |
| `sisu check` builds the mir. Copies show as the loop-copy warning only, with no run-time copy report. | 2026-10-26 | `docs/architecture.md` (check stops after the checker; the mir is built on the build path only); `GLOSSARY.md` (Front end); ADR 0023 and E0030 (a chain that mono stops passes `sisu check`) | The loop-copy warning |
| When projects arrive, `sisu.toml` gets a setting for the codegen unit count, and a setting for LTO once LTO exists. No environment variable sets the count. | 2026-11-02 | none | The `sisu.toml` work |
| `--sanitize address` works on macOS through compiler-rt: `sisu` runs the pass and links `libclang_rt.asan` itself, and does not leave the link to `cc`. Today the flag is a usage error on macOS, because the link of Apple's `cc` stops at `Undefined symbols: ___asan_version_mismatch_check_v8`. | 2026-11-09 | none; `docs/development.md` says the flag works only on Linux, which stays true until this is built | A follow-up to `--sanitize address` |

## Debt

| Problem | Where | Pay before |
| --- | --- | --- |
| The five milestone 9 plans were written before the mir. They put closure and weak-field counts in codegen, and do not name ADR 0029. | `docs/superpowers/plans/2026-10-16-milestone-9-part-*.md` (the owner's working plans, not committed) | Milestone 9 part 1 |
| `Body` holds, in 55 fields, four kinds of state that the checker keeps for different lengths of time: the generic scope, the body (such as `uninferred`, until the uses settle the help), one-shot hints (such as `break_hint` and `context_hint`) and span caches. A closure needs a new body inside the outer generic scope. | `check::Body` | Closure checking |
| The clobber rule is in three copies. | `mir::params::clobbers`; `mir::pairs::kind`; `mir::certify::Clobbers::new` | Milestone 9 indirect calls |
| Codegen helpers apply the ADR 0029 calling convention a second time after the mir. `abi::{param_types, out_param_types, function_type, split, join}` own how a value crosses a call. `Codegen::declare`, `entry_glue` and `dyn_call` each put the parameters in order again; the thunks that milestone 9 adds are the part that remains. | `codegen::show::show_written`; `codegen::rc::call_written_eq`; `Codegen::declare`; `codegen::any::{entry_glue, dyn_call}` | Milestone 9 thunks |
| The walk over the parts that a helper visits is written twice. Mono checks the conformances that apply; `calls_written` takes a wider set on purpose. | `Mono::walk`; `conformance::calls_written` | Milestone 9 part 2 |
| The lexer's layout tracker restates parser positions, and `selfhost/lexer.sisu` copies it. | `lexer` (`Statements`, `continues_params`, `check_line_start`); `selfhost/lexer.sisu` | Milestone 9 function-type syntax |
| The call row (no argument settles a type parameter in a call with a failed part, so the result is typed only when the receiver or a written type argument binds each parameter that it names) is stated again at eight sites besides `failed_call`, whose doc states it. Each one repeats the row in its own words. | `check::enums` (`generic_variant_args`); `check::call` (`construct_class`); `check::optional` (`some_of`, `failed_some`); `check::prelude` (`dbg`); `check::generic` (`written_args`, `failed_build`, `infer_call`) | No plan owns it yet: part 6 built the rule and no later part changes the call row |
| `resolve` returns `Option<Type>`, so three sites that it just reported must take a `declaration` promise. `resolve -> Result<Type, Reported>` removes the three promises. | `check::member` (`written_list`); `check::pattern` (`variant_pattern`); `check::generic` (`type_argument`) | No plan owns it yet: no later part changes `resolve` |
| Number behaviour comes from `ty != Type::U8`, and 38 hand-written lists name the number types. The `structure` area of `audit-sisu` counts them. | `codegen::arith`; `check` | Number types, task 1 |
| The fuzzer copies the checker's conversion rules by hand, and keeps its own list of prelude functions that have no effect and cannot panic. It passes its context in 11 `bool` fields of `Gen`, such as `synth` and `typed`, and `last_literal`, which `literal` sets for `declared`. | `generate::coverage` (`CONVERSIONS`, `PURE_PRELUDE`); `generate::Gen` | Number types |
| A function's identity is its display name (`"main"`). | `mir::params`, `mir` (`f.name == "main"`) | Milestone 11 or the C FFI |
| The checker's type walks, except `holds`, keep their seen sets for one field, scan or declaration only. `holds` keeps its cache for the whole check. For the other walks, `m` fields that each name one alias of `k` distinct types cost `m * k` steps. Take `k` aliases in a binary tree and `k` enums that each hold its root. They take 0.06 s at `k` = 1,000 and 1.0 s at `k` = 4,000 (`sisu check`, release build). One enum takes 6 ms at `k` = 4,000. Also, `Buildable` scans a nested field again for each level, so one field `And<E0, And<E1, .. End>>` of depth `n` costs `n * n` steps. The parser stops `n` at 256 (E0071), where one such field takes under 0.02 s. A field that cannot be built is scanned again for each key under it that is built later. Each scan reads the whole field, so one field with `t` distinct types costs `t * t` steps. The growth is the same on `main`, but here each scan also fills two hash tables, so a scan costs about twice as much. In a release build, `sisu check` takes 58 ms on `main` and 103 ms here for a balanced field with 1,024 leaves. For one field of 2,000 types, it takes 134 ms on `main` and 208 ms here (best of 3). | `check::Checker::holds_table`; `check::enums::Buildable` (`Walk`); `check::cycle::named`; `check::enums::TYPE_WALK_STEPS` | Milestone 11 |

Paths name Rust items in `crates/sisuc/src/`, not lines, so that a row stays
true when the file changes.
