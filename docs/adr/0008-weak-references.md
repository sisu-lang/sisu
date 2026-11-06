# Weak references break cycles

Status: superseded by [ADR 0046](0046-structs-replace-classes.md) before it
was built, 2026-11-06.

Reference counting leaks a cycle (ADR 0005). A field declared
`weak mut parent: Node?` breaks one: it does not keep its object alive, and
reading it gives `None` once the object has been dropped. `weak` is allowed
only on a `mut` field (ADR 0011) whose type is an optional whose payload type
conforms to `AnyObject`: `C?` for a class `C`, `T?` for a type parameter
`T: AnyObject`, and `(any I)?` for a class-only interface `I` (milestone 8).
A weak field reads like any optional, with `?.`, `??` and `if let`, so it adds
no syntax for reading and no way to crash. We chose this over a cycle
collector so that objects are still freed at a known point with no runtime
beyond an allocator, as ADR 0005 requires. Swift's `weak` works the same way.

The strong and weak counts share the one 8-byte header word that every object
already has: the strong count in the low 32 bits, the weak count in the high
32 bits. Objects do not grow. When the strong count reaches 0 the object is
dropped and its fields are released; its memory is freed once the weak count
is 0 too.

## Considered options

- **A cycle collector (CPython)**: frees cycles with no keyword, but needs a
  runtime that traces every class's fields, pauses the program, and frees an
  object in a cycle at an unknown time.
- **`unowned` (Swift)**: a weak reference that is not optional and panics
  when read after its object is gone. It saves `?.` at each read, at the cost
  of a crash that only shows at run time.
- **A second header word, or a side table created on first use (Swift)**: no
  limit on either count, but 8 more bytes per object, or a branch and an
  extra allocation on the paths that change a count.
- **Indices into an array**: works with no language change, and stays a good
  idiom for graphs, but an index is not checked against the array it points
  into.

## Consequences

- Each count has a limit of 2^32 - 1. Passing it panics, as integer overflow
  does (ADR 0003), so every count increment checks the limit. Reaching it
  needs at least 32 GiB of references to one object.
- An object with live weak references keeps its memory, but not its fields,
  until the last weak reference goes.
- The compiler sees the whole program, so a class that no `weak` field
  points at can keep the drop code it has today.
- Arrays are values and cannot be weakly referenced, so their header is
  unchanged.
- A weak `any` value is two words: the weak object pointer and the method
  table.
- A lint warns about the cycle shapes it can see: two classes whose fields
  point at each other, and a class with both a `[C]` and a `C?` field of its
  own type. A doubly linked list looks like a binary tree to a lint, so a
  `--leaks` build mode reports the cycles that actually happen.
- A class that stores a closure capturing that same object is a cycle a
  field cannot break. Milestone 9 decides how a closure captures weakly, as
  Swift's `[weak self]` does.
