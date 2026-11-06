# Tasks share no objects

Status: proposed, 2026-10-10. It is accepted when concurrency gets a milestone
spec (roadmap, "Concurrency").

Changed in part by [ADR 0046](0046-structs-replace-classes.md). A value that
cannot be copied crosses by a move: its move-only parts move, and each other
counted part is copied. A copy no longer needs to keep cycles, and it still
keeps a record of each shared block.

Concurrent code runs in tasks that share no heap objects. A value that crosses
into another task, as a `spawn` capture, a channel `send` or a `join` result,
is copied. Only channel and task handles are shared. We chose this so
reference counts stay non-atomic (ADR 0005) and Sisu code cannot have a data
race: no object is ever reachable from two tasks, so no two threads ever
touch the same count or `mut` field. Erlang and Elixir processes work the
same way: each process has its own heap, and a message is copied into the
receiver's heap.

## Considered options

- **Shared memory with atomic counts and a check of what may cross (Swift 6
  `Sendable`)**: no copies, but Swift makes every count atomic, which slows
  every program, single-threaded ones included, and the checker needs a
  sharing rule for every type.
- **Two kinds of counted pointer (Rust `Rc` and `Arc`, with `Send` and
  `Sync`)**: counts are atomic only where a value is shared, but the type
  system must track which values may cross, and programmers choose a pointer
  kind for every shared value.
- **Moving or sharing by reference capability (Pony `iso` and `val`)**: no
  copies and no shared mutable state, but every reference carries a
  capability in its type, which is the hardest of these to learn.
- **Shared memory with no check (Go)**: the cheapest to build, but a data race
  corrupts a count, and the crash appears rarely and far from its cause.

## Consequences

- A copy costs time and memory in proportion to the value. Programs should
  send results, not whole data structures.
- One crossing copies a value with its shape: two references to one object
  inside the value still point to one object in the copy, and a cycle stays a
  cycle. Two crossings of the same object give two independent copies, so `is`
  is false between them.
- Strings are copied too, because a string carries a non-atomic count like
  any object.
- A value that cannot be copied, such as a file handle, cannot cross. The
  checker rejects it.
- Channel and task handles keep their own synchronized state and atomic count
  in the runtime, outside the counting of ADR 0005.
- Large read-only data cannot be shared without a copy. A later `Shared<T>`
  must make the whole object graph safe to share, not only its root.
- A count of 1 does not prove that a value can be moved instead of copied: an
  object deeper inside it can still have another owner.
- A task cannot die alone on a panic until there is a way to leave its frames
  and free its objects. Until then a panic ends the program.
