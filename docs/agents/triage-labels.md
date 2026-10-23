# Labels

Each GitHub issue has one kind, one area and one triage label. A `bug`, `diagnostic` or `perf` issue also has one severity label.

## Kind

| Label        | Use for                                                     |
| ------------ | ----------------------------------------------------------- |
| `bug`        | Wrong output, unsound code, a crash or a panic              |
| `diagnostic` | An error or warning that is wrong, misleading or missing    |
| `perf`       | Compile time, run time or memory that is too high           |
| `cleanup`    | Code that works but is harder to read or change than needed |
| `test`       | Tests, fuzzers and other checks                             |

## Area

`lexer`, `parser`, `checker`, `mir`, `codegen`, `runtime`, `cli`, `fuzz`.

## Severity

| Label             | Meaning                                                                                        |
| ----------------- | ---------------------------------------------------------------------------------------------- |
| `severity:high`   | Wrong program output, unsound code, or a crash or panic on a valid program                     |
| `severity:medium` | A misleading or missing diagnostic, a false error, or a large slowdown on a realistic program  |
| `severity:low`    | Cosmetic wording, a small defect in a fix, or a slowdown only on unusual or hand-built input   |

## Triage

The skills speak in terms of five canonical triage roles. The GitHub labels use the same strings.

| Label in mattpocock/skills | Label in our tracker | Meaning                                  |
| -------------------------- | -------------------- | ---------------------------------------- |
| `needs-triage`             | `needs-triage`       | Maintainer needs to evaluate this issue  |
| `needs-info`               | `needs-info`         | Waiting on reporter for more information |
| `ready-for-agent`          | `ready-for-agent`    | Fully specified, ready for an AFK agent  |
| `ready-for-human`          | `ready-for-human`    | Requires human implementation            |
| `wontfix`                  | `wontfix`            | Will not be actioned                     |

When a skill mentions a role (e.g. "apply the AFK-ready triage label"), use the corresponding label. A closed issue is resolved; a `wontfix` issue is closed as not planned.

A fallback issue in `.scratch/` records its labels on `Labels:` and `Status:` lines near the top.
