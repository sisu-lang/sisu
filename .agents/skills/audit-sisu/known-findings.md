# Known findings

Behavior a run must not report as new: intended, documented, or ruled
won't-fix. Step 5 of the skill adds a row each time the user rules on a
finding. A finding that is ticketed is a GitHub issue, not a row here.

| Behavior | Ruling | Source |
| --- | --- | --- |
| A reference cycle leaks | Intended until `weak` lands | ADR 0005, ADR 0008 |
| `--rc-checks` misses a use after free whose freed memory still holds a count of 1 or more | Tool limit; Valgrind and ASan cover it | `docs/development.md` |
| A program that panics (exit 101) leaves its objects allocated | Intended: the process ends | `crates/sisuc/tests/run-fail` cases run without Valgrind |
| If the allocator call remains and cannot provide the block, the program aborts with exit 134 and `memory allocation of N bytes failed`. At `-O2`, LLVM may eliminate an allocation and its free, as rustc's allocator lets it, including when it can compute the values the program reads without the heap block (an array's length, an element of a fill), and then nothing aborts | Intended (ir-1, user ruling 2026-10-20) | `largest_array_should_reach_the_allocator_and_abort` in `crates/sisuc/tests/programs.rs`; `sisu_alloc` in `crates/sisuc/src/codegen/abi.rs` |
