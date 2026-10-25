# Memory is managed by reference counting

Every heap object carries a count of the references to it. Generated code
adds 1 when a reference is copied and subtracts 1 when one is dropped, and
the object is freed when the count reaches 0. We chose this so memory is
managed for you (ADR 0004) with no runtime beyond an allocator: every count
change is ordinary IR that `--emit ir` shows, objects are freed at a known
point, and Valgrind can prove a program frees everything (when it is built with
`--allocator system`; see
[ADR 0031](0031-compiled-programs-allocate-with-mimalloc.md)). Swift works the
same way. See the milestone 2 spec, "Reference counting".

## Considered options

- **Manual `free` (C)**: the programmer owns use-after-free and double free,
  which breaks "memory is managed for you".
- **Ownership and a borrow checker (Rust)**: no runtime cost, but it needs
  lifetimes, which the roadmap rules out, and the hardest checker to build.
- **Conservative tracing GC (Boehm)**: links an existing collector that scans
  the stack for anything that looks like a pointer. The interesting part
  would be someone else's code, and objects are freed at an unknown time.
- **Precise tracing GC (Go, Java)**: collects cycles, but finding every
  pointer on the stack needs LLVM statepoints and stack maps, or a shadow
  stack that slows every call, plus a collector with pauses.
- **One arena freed at exit**: trivial, but a long-running program grows
  forever.

## Consequences

- Cycles leak unless one reference in the cycle is weak (ADR 0008).
- Every copy of a reference costs an increment and a later decrement. A later
  optimization removes redundant pairs.
- Counts are not atomic. Concurrent tasks keep them that way by sharing no
  objects (ADR 0010, proposed).
- Drop follows a field that holds a class of the same cycle in a loop, so a
  list, or a chain that alternates between classes, frees in constant stack. A
  cycle is a set of classes that hold each other in direct fields, `X` with any
  number of `?` around it. A list's `next` is a cycle of one class. Other
  references recurse: a field of an array or an enum, and a class outside the
  cycle. `==` loops only on the last field of a class's own type, `X` or `X?`. It
  recurses on every other link until milestone 8 part 5 removes class `==`.
