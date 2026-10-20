# Area: test blind spots

Find compiler behavior that no test would notice breaking. A blind spot is
a `gap` finding whose suggested test passes today and fails with the
mutant or change it names.

## Probes

- Coverage: run the toolbox's Coverage command with base `$(git rev-list --max-parents=0 HEAD)`, so every file counts as changed. Each uncovered region in a file of the area is a lead, and it is cheaper than a mutant run.
- Mutants: run the toolbox's "Mutants" commands on the files where a silent mistake costs most, in this order: `mir/ownership.rs`, `codegen/rc.rs`, `codegen/array.rs`, `check/place.rs`. Each surviving mutant is a finding unless it is equivalent; say why it is. Each file takes long, so record in `probes.md` which files you finished and which remain.
- Feature matrix: list every feature in `docs/roadmap.md` "What works today" and every helper in `docs/architecture.md`. For each, find a `tests/run` case that exercises it on an object, on an array, and inside an optional. Each missing cell is a lead; write the probe, and it becomes a finding only when a mutant or a wrong output shows the gap matters.
- `o2-calls` markers: a `tests/run` case whose output depends on a value crossing a call, with no marker, may pass with the call inlined away. Check each case's `-O2` IR for the calls it relies on.
- `run-fail` cases run without Valgrind or `--rc-checks`: find the behavior before the panic that nothing checks.
