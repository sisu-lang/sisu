---
name: audit-sisu
description: Exploratory audit of sisuc and the runtime. Probes IR quality, reference counting, crashes, miscompiles, runtime unsafe code, limits, diagnostics, test blind spots and compile time; reports reproduced findings with a suggested fix and test each. `/audit-sisu` runs every area, `/audit-sisu <area>` one.
disable-model-invocation: true
---

# Audit Sisu

An exploratory audit, run by hand now and then. The tests prove what someone
thought to check; this run hunts for what nobody did. It writes probe
programs, builds and runs them, and reports each problem it can reproduce,
with a suggested fix and a suggested test. Until the user picks findings at
the end, it writes nothing in the repository; then it writes only tickets
and rows of `known-findings.md`.

Run it only when the user asks. In Claude Code that is `/audit-sisu [area...]`;
in Codex, `$audit-sisu [area...]`.

## Areas

| Area | Hunts for | Guide |
| --- | --- | --- |
| `ir` | IR that LLVM rejects, undefined behavior, avoidable cost after `-O2` | [areas/ir.md](areas/ir.md) |
| `rc` | Leaks, double frees, use after free, broken copy-on-write, count imbalance on any path | [areas/rc.md](areas/rc.md) |
| `crash` | `sisuc` panicking, hanging or failing on a checked program; a compiled program dying by a signal | [areas/crash.md](areas/crash.md) |
| `bugs` | Wrong output, `-O0` and `-O2` disagreeing, the checker accepting or rejecting the wrong program | [areas/bugs.md](areas/bugs.md) |
| `runtime` | Unsound `unsafe` in `crates/runtime`, codegen calls that break its ABI | [areas/runtime.md](areas/runtime.md) |
| `limits` | Deep recursion, huge values, long programs, overflow edges | [areas/limits.md](areas/limits.md) |
| `diagnostics` | Bad programs that get no diagnostic, a wrong span, or a confusing message | [areas/diagnostics.md](areas/diagnostics.md) |
| `coverage` | Compiler paths no test reaches, surviving mutants | [areas/coverage.md](areas/coverage.md) |
| `compile-time` | `sisuc` doing avoidable work: repeated steps, allocations on hot paths, IR LLVM only deletes, failing late | [areas/compile-time.md](areas/compile-time.md) |

With no argument, run all nine; otherwise run the named ones.

## Steps

### 1. Set up

From the repository root:

1. `cargo build --workspace`. Every probe uses this checkout's `target/debug/sisuc`.
2. `mktemp -d "${TMPDIR:-/tmp}/sisu-audit-XXXXXXXX"` makes the run directory, `RUN`. Shell variables do not survive between tool calls, so from here on write `RUN`, the repository root and the tool paths as literal paths.
3. Write `RUN/known-tickets.md`: the title and path of every ticket under `.scratch/*/issues/` whose `Status:` is not `wontfix` or `resolved`, or "none" when `.scratch/` does not exist. `.scratch/` is untracked, so read it in the main checkout, the first path `git worktree list` prints.

Done when `sisuc` builds and `RUN/known-tickets.md` exists.

### 2. Explore

Dispatch one general-purpose subagent per area, one that can run shell
commands and write files, with the brief below. Start as many at once as
the host allows and queue the rest. A host without subagents runs the
areas one after another itself, with the same brief.

```text
You audit one area of the Sisu compiler: <area>. Repository: <repo root>.
Run directory: <RUN>/<area>/ (create it). Read these first:
- .agents/skills/audit-sisu/areas/<area>.md: what to hunt and the probes to run
- .agents/skills/audit-sisu/toolbox.md: commands, exit codes, timeouts, recipes
- .agents/skills/audit-sisu/finding.md: what counts as a finding and how to write it
- .agents/skills/audit-sisu/known-findings.md and <RUN>/known-tickets.md: what is already known
- docs/roadmap.md "What works today": the language you may use. Copy syntax from
  crates/sisuc/tests/run/*.sisu; a probe that does not compile tests nothing.
Never edit the repository; write only under your run directory. If the
Write tool refuses a file there, write it with a shell heredoc. In every
Repro block keep `D=<a fresh directory>` literal; the main session fills it. Keep
<RUN>/<area>/probes.md as you go: one row per bullet of the area guide and
per probe of your own, with the probe files, the command, how many programs
ran, which oracles ran, and the result ("clean", a finding id, "skipped:
<reason>"). Write findings to <RUN>/<area>/findings.md, with leads at the
end. Reply with one line: findings, leads, and probes run.
Done when every guide bullet has a row in probes.md, you ran at least five
probes of your own aimed at the area's blind spots, and every finding
reproduces from its own Repro block in a fresh directory.
```

### 3. Reproduce

1. Read each area's `probes.md`. Send an area back to its subagent when a guide bullet has no row, or is skipped for a reason the toolbox answers.
2. Run each finding's `Repro` block with `D` set to a fresh `RUN/repro/<id>/`. Keep the finding only when the result matches what it claims.
3. Drop a finding that `known-findings.md` or `RUN/known-tickets.md` already covers.
4. Merge findings from different areas that share a root cause; keep the higher severity.

Note each dropped finding and why.

### 4. Cross-check

An independent reviewer reads every kept finding. Write the prompt to
`RUN/cross-check.md`: list the findings files and ask, per finding, whether
it is a real defect or behavior that `README.md`, an implemented ADR or
`docs/development.md` calls intended; whether the severity matches
`finding.md`; whether the fix reaches the root cause; and whether the test
would fail as `finding.md` requires.

- When `sol-review` is on `PATH`, run `sol-review high RUN/cross-check.md RUN/cross-check-report.md <repo root>`. Run it outside the sandbox: codex needs the network and writes its state under `~/.codex`.
- Otherwise, dispatch one read-only subagent with the same prompt.

Wait for its report. Drop a finding it shows is intended or not a defect,
citing its reason. Change a severity when it cites `finding.md`. Keep every
other disagreement as a `Cross-check:` line in the finding. When no reviewer
ran or its run failed, say so at the top of the report.

### 5. Report

Write `RUN/report.md`: one section per kept finding, ordered by severity as
[finding.md](finding.md) ranks it; then the leads from every area, one line
each; then the dropped findings with the reason. Tell the user the path,
the count per severity, and the top findings in one line each.

Then ask which findings to file. For each one the user picks, write a
ticket at `.scratch/audit-<date>/issues/<NN>-<slug>.md` per
`docs/agents/issue-tracker.md`, with `Status: needs-triage` and the
finding's text as the body. For each one the user rules intended or won't
fix, add a row to [known-findings.md](known-findings.md).
