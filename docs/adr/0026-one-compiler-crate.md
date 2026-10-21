# The compiler stays one crate

Status: accepted, 2026-10-21.

`sisuc` is one crate. The architecture review proposed splitting it in two
at the `mir` seam: an LLVM-free front crate (`source`, `lexer`, `parser`,
`ast`, `symbol`, `types`, `graph`, `check`, `tir`, `mir`, `diagnostic`,
`report`, `front_end`) and a back end (`codegen`, `link`, `fuzz`, the
driver). The aim was shorter edit-and-test cycles: a front-end edit would
not recompile codegen or relink unit tests against LLVM.

We chose `mir` over `tir` as the seam because `codegen` reads `tir` in ten
`use` lines and `mir` in two, a `tir` seam would make `check`'s tests
import `mir::tests` backwards across it, and `mir/` is where the work
lands through M8 and M9.

We did not build the split. Today's numbers already show that an edit
rebuilds the test binaries in about 2 s, so the rule below cannot be met.

## Measurements

Base commit 2337f25, one `sisuc` crate. Machine: 32 cores, rustc 1.99.0,
shared with other sessions (load 5 to 9). Each row is the median of five
timed runs after an untimed warm-up. An edit inserts one line at the top
of the file, so every span in it shifts. After each run the file is
restored and rebuilt untimed.

| Id | Measurement | Median (s) |
| --- | --- | --- |
| M1 | clean test build (`cargo nextest run --workspace --locked --no-run`) | 10.90 |
| M2 | `check/expr.rs` edit, test build | 1.97 |
| M3 | `lexer/mod.rs` edit, test build | 2.11 |
| M4 | `codegen/mod.rs` edit, test build | 1.98 |
| M5 | `check/expr.rs` edit, `cargo check --workspace --tests` | 0.69 |
| M6 | `check/expr.rs` edit, fuzz build | 1.18 |
| M7 | `mir/lower.rs` edit, test build | 1.94 |

`--no-run` does not run nextest's setup script, so the runtime's build
(`libsisu_runtime.a`) is outside these times. That is right: a split does
not touch the runtime.

The decision rule, set before measuring: **split** when both hold.

1. A split's M2, M3 or M7 is at least 30 % and at least 5 s below today's.
2. A split's M4 is no more than 10 % above today's.

Otherwise one crate. The user rules on the table.

## Decision

One crate. Condition 1 cannot hold: M2, M3 and M7 are about 2 s, so even
a free rebuild saves under 2 s, short of 5 s. Incremental compilation
already reuses the unchanged parts of the crate. After a split, `sisuc`
would still rebuild incrementally and relink on every front-end edit.

## Consequences

- An edit to the front end or `mir` still recompiles the crate, codegen
  included, and relinks the unit tests against LLVM. That costs about 2 s.
- No `pub` seam, no `test-support` feature and no second crate to keep
  lint-clean.
- Propose the split again only when a measurement beats the rule above,
  for example when edit rebuilds (M2, M3, M7) grow well past 5 s. A
  clean build (M1) alone does not count: edits are the daily cost.
