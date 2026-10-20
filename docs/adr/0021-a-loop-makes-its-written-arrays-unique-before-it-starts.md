# A loop makes its written arrays unique before it starts

Status: accepted, 2026-10-16. On 2026-10-18,
[ADR 0025](0025-mir-holds-every-count.md) moved the hoist from codegen to
`mir` (`mir/hoist.rs`) and widened it: inside the loop, only a copy, a move
or a whole assignment of the local keeps the check, and a read in place does
not, where codegen's hoist allowed only an index base and `len`.

When a loop writes elements of a local array, and nothing in the loop
shares that local, codegen makes the array unique once, before the first
iteration. The writes inside the loop then skip the count check for that
array. Nothing in the loop shares the local when the loop reads it only
with `len`, or by an index built from variables, literals, fields, `len`
and operators, and never binds it, passes it, stores it, returns it,
replaces it, declares it, or reads it for the last time. An index that
calls a function or holds a block, such as `a[if c { a[0] = 9; 0 } else
{ 1 }]`, keeps a second reference to the array while it runs, so the
write inside it must still check the count.

We chose this for performance, which ADR 0006 ranks last, because it
changes no value. The count check made the array's pointer change inside
the loop as far as LLVM could tell, so LLVM kept the check and the
length load in every iteration. On quicksort, moving the check out of
the partition loop took the benchmark from 4.82 to 4.04 billion
instructions, and its fastest run from 690 to 640 ms, against C's 610 in
the same session
([Benchmarks](../benchmarks.md#quicksort-on-an-array-value)). Swift's
optimizer copies its copy-on-write check to the start of a loop when the
array does not escape the loop, so that the checks inside it fold away.

## Considered options

- **Leave it to LLVM**: LLVM cannot move the check. It may store a new
  block into the local, and in a loop like quicksort's it runs only on
  some iterations.
- **Hoist only when the loop surely writes**: no extra copy, but the
  writes in real loops sit under conditions, as quicksort's swap does, so
  the rule would rarely apply.
- **Prove uniqueness across the whole program**, as Roc's Morphic analysis
  does: it would remove the check outside loops too, at the cost of a
  whole-program pass (roadmap).

## Consequences

- A loop that could write a shared array copies it before it starts, even
  when it then runs zero times or never reaches the write. No value
  changes: the cost is one copy, which the first write would usually have
  made anyway.
- The copy can come before an out-of-bounds panic in the loop, and a
  program that never reached a write can now run out of memory making
  that copy.
- Only the array the local holds is made unique early. In `g[i][j] = v`,
  the row `g[i]` is still checked at each write, and a field root such as
  `obj.xs[i] = v` is checked at each write.
- A loop that reads the array in any other way, such as passing it to a
  function, keeps the check at each write.
