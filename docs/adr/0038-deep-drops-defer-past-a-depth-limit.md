# Deep drops defer past a depth limit

Status: accepted, 2026-11-01. It changes [ADR 0005](0005-reference-counting.md)
and [ADR 0019](0019-enums-are-values-named-through-their-enum.md) (#191).

A drop adds a bounded amount of stack, however deep the value is. Before this
ADR, a drop recursed once for each object when a chain went through an array
or an enum. A chain of 1 000 000 objects overflowed the 8 MiB stack, and the
program stopped with signal 11 and printed nothing. These shapes crashed:
`class A { _next: [A] }`, `[A?]`, `[[A]]`, an enum field such as
`enum Link { End; To(A) }`, a generic enum `Box<A>`, an enum with boxes that
holds `A?`, two classes joined through an array, and `enum Tree { Leaf;
Node([Tree]) }`.

The rule has four parts:

- **Counting is the default.** A counted field, element or payload is a
  *recursing part* unless its type, with each `?` stripped, is bounded. The
  bounded types are: a `string`; a class in the holder's class cycle (ADR
  0005); an array whose element type is not counted, or is bounded as the
  element of an array (a class element is never bounded, because an array is
  in no class cycle); and an enum with a clear reach flag. A drop function
  with a recursing part *counts*. The drop function of a class in a cycle of
  two or more classes frees the other objects of the cycle in its drain, with
  their fields. So it counts when a class of its cycle has a recursing part,
  and all classes of the cycle share one drop weight. A class with a drop
  block counts too, when drop blocks land.
- **One reach flag for each enum component** (ADR 0019). The flag is clear
  when each payload field of each member, `?` stripped, is not counted, is a
  `string`, is an enum of the same component, or is an enum of another
  component whose flag is clear. Any other payload sets it: a class, an
  `any` type, an array of any element type, and each kind of reference that
  a later milestone adds.
- **Drop weights.** A counting drop function adds its *drop weight* to a
  global *drop depth*. The weight is 1, plus the largest number of helper
  frames (`release`, `walk`, `drain`, the drop of a bounded array, the
  string drop) that one of its parts can open before the next counting drop
  function starts. So the depth counts frames, not objects.
- **A depth limit with a queue.** At or above the *depth limit*, a counting
  drop function puts its block and itself at the back of the runtime's
  deferred queue, sets a flag in the generated code, and returns. The block
  keeps its fields, and its count stays 0. Otherwise it adds its weight,
  drops the block, and subtracts the weight. The drop that brings the depth
  back to 0 empties the queue when the flag is set, then clears the flag.

The depth limit is 256 frames. It is set so that the depth limit plus the
largest drop weight of the test programs, times the largest frame, is at most
64 KiB: a quarter of the 256 KiB stack of the small-stack tests.

The queue is first in, first out, so the deferred blocks of one holder drop
in their declared order. Only one loop empties it at a time: a drop that
returns to depth 0 while the loop runs does not start a second loop, so
emptying the queue uses a constant stack. The queue allocates memory only
when a drop reaches the limit, and keeps its capacity after it is empty. A
failed allocation stops the program, as a failed `sisu_alloc` does.

When weak references land ([ADR 0008](0008-weak-references.md)), the queue
adds 1 to the weak count of a block that it takes, and the drop function
subtracts it before it frees the block. A weak release then cannot free a
block that waits on the queue. A weak read of it gives `None`, because its
strong count is 0.

## Order guarantees

1. **A holder's drop block before the release of its fields.** When drop
   blocks land, an object's drop block finishes before the compiler releases
   the object's fields. A value that the drop block releases itself loses
   its reference at that statement.
2. **Locals one after the other.** When a scope ends, the whole drop of one
   local, deferred drops included, is complete before the next local ends.
   The exception is a scope that runs inside a drop, such as the scope of a
   drop block. There, a deferred drop of a local waits until the outermost
   drop empties the queue.
3. **Deterministic.** A program drops its values in the same order on each
   run.
4. **Declared order up to the depth limit.** Inside one value, fields drop in
   declared order, and elements in index order. Past the depth limit, a
   block can drop after a block that it does not hold and that does not hold
   it, such as the next element of an array.

## Why

#189 removed the recursion for a chain of classes that hold each other in
direct fields, and ADR 0019 removed it for recursive enums. Each fix covers
one kind of reference. A depth limit covers each kind of reference that
frees its block through a drop function, including `any` values, closure
captures and each kind that a later milestone adds, with no new rule.

## Considered options

- **One drop graph** over classes, arrays, enums and `any` tables, with a
  pending list for each strongly connected component, as #189 does:
  rejected. Each kind of reference needs a rule, such as a table slot for
  `any` and closures. A missed rule crashes again.
- **A depth limit with a deferred queue (chosen).** It needs no type graph.
  Its cost is a few instructions in each counting drop function.
- **A stack that grows on the heap**, as Rust's `stacker` crate does:
  rejected. It keeps a strict depth-first order, but its memory grows with
  the depth of the value.
- **Leave it**, with the limit that ADR 0005 and ADR 0019 state: rejected. A
  valid program crashes with no message.

## Prior art

CPython's "trashcan" bounds the depth of a container's deallocation in the
same way: past a limit, it puts the object on a list, and the outermost
deallocation empties the list. Up to 3.13 it counts the depth. Since 3.14 it
checks the margin of the C stack.

## Consequences

- A drop through an array, an enum or a class outside the cycle adds a
  bounded amount of stack. `binary_trees`' `Node`, whose fields are all links
  of its class cycle, does not count, and its drop does not change.
- A counting drop function costs a load, a compare, two stores and a test
  at depth 0.
- Past the depth limit, a deferred block can drop after a block that it does
  not hold (guarantee 4).
- The #189 class drains and the ADR 0019 enum drains stay. A later change
  can measure whether the queue makes them unnecessary.
- A drop that starts when the stack is almost full can still overflow it, as
  any call can (ADR 0027).
