# Structs replace classes, and the variable decides what can change

Status: accepted, 2026-11-06. Not built. Until the removal merges, the ADRs
that this one changes describe the compiler.

Sisu removes classes. A struct is the one kind of named data with fields, and
one rule decides what can change: the variable.

```sisu
let t = Team(leader: Person(name: "Ilse"), size: 1)
t.leader.name = "Aino"      // error: `t` is a `let`

let mut u = t
u.leader.name = "Aino"      // changes `u`; `t` stays as it was
```

With classes, the write through `t` compiled, because a `let` protected only
the binding. A reader of `let t` could not know whether `t` changes. Under
this decision, a `let` value never changes, at any depth.

## Decision

1. **One kind of named data.** `struct` replaces the reserved word `record`,
   and `enum` stays. `class`, `weak`, `is` and `AnyObject` go. A value has no
   identity. A generated `==` compares fields, and it exists only when each
   field has `==`, as for a struct in the milestone 9 design. A function field
   and an `any` have none.
2. **The variable decides.** Nothing in a `let` can change, and all of a
   `let mut` can, within what privacy allows. A field has no `mut`. Binding,
   passing or storing a struct that is not move-only behaves as a copy. A
   count or a shared block can stand behind that copy, but no program can see
   it.

   The rule covers the value. State outside the value is not in the rule: the
   file system, the position of a descriptor and the queue of a channel. An
   operation that changes the position of a descriptor takes `mut self`, so a
   read needs a `let mut` or a `mut` parameter. Channel and task handles are
   the one named exception: they stay shared, with their own synchronized
   state (ADR 0010), and a `let` handle can send.

   `pub mut` goes. A module that holds a value in a `let mut` can write its
   `pub` fields. A field that other modules can only read is private, with a
   `pub` method that returns it. A type with a private field can be built only
   in its own module (ADR 0035).
3. **Links are values.** A list or a tree is an enum with a box, or a plain
   nested value. A graph holds ids into an array. A library type `Arena<T>` with
   checked ids can come when a program needs it. Its private fields need the
   modules of milestone 11.
4. **No shared cell that can change, other than those handles.** A closure
   captures a copy. A callback gets its state as a `mut` parameter or returns
   it. The `[mut name]` capture of milestone 9 goes, because it is a hidden
   class. A `Shared<T>` that can never change, for large read-only data across
   tasks, stays a later item.
5. **`mut self` works through `any` and through a bound.** An interface can
   require `mut self`, and the call `list[i].on(mut m, x)` changes the element
   in place. An observer with its own state must not copy that state on each
   event.
6. **A struct can be move-only.** A file or a socket has one owner and no
   copy. A value that holds a move-only value is move-only too. A plain
   parameter or a plain `self` reads the value and cannot store it. A `move`
   hands the value over, and a use after the move is an error. A `mut`
   argument of a move-only type is taken in place, because it has no copy. The
   drop block goes on a move-only struct. The check runs in the checker, with
   line numbers, and does not depend on an optimization.

   A move-only value crosses into a task by a move. The move-only parts move.
   Each other counted part is copied, because another value can still hold that
   block (ADR 0010).

   The spike writes `struct File: NonCopy`, and `move x` on a parameter. The
   spec chooses the final forms, under [ADR
   0041](0041-special-cases-are-attributes-not-compiler-name-lists.md).

## What this changes

- [ADR 0008](0008-weak-references.md) and [ADR
  0022](0022-classes-compare-by-identity.md) go.
- [ADR 0005](0005-reference-counting.md): counts stay, as an implementation of
  copies. A cycle of counted values can no longer be made, so nothing leaks
  from one.
- [ADR 0011](0011-let-mut-marks-what-can-change.md): the class rules and the
  question "which to use" go.
- [ADR 0043](0043-mut-parameters-copy-in-and-out.md): "What reaches a place" and
  `MutLostWrite` go. Only the `get`, `set`, `hash` or `eq` of a map entry or a
  user `Indexable` place can still reach a place. All conditions of a take go
  except three: the run-time test for two `mut` arguments that overlap stays,
  the condition on a resource type reverses for a move-only value, and a call
  with a `mut` place through a map entry or a user `Indexable` type copies each
  of its `mut` arguments. Otherwise, a `mut` argument through struct fields and
  array elements is taken. None of these is built. The table below depends on
  this take: `add(mut s.xs, i)` on a struct costs 2.08 M at 20 000, and the same
  call on a class field, copied in, costs 1 612 M.
- [ADR 0010](0010-tasks-share-no-objects.md): "a value that cannot be copied
  cannot cross" becomes "it crosses by a move". A copy no longer needs to keep
  cycles. It still keeps a record of each shared block, so that block copies
  once.
- [ADR 0035](0035-a-file-is-a-module-and-its-names-are-private-unless-pub.md):
  `pub mut` goes, as decision 2 says. The modules design already gives this
  rule for a struct.
- [ADR 0029](0029-parameters-are-borrowed-unless-consumed.md): the rule for a
  `weak` field goes, and a move-only value is never consumed by inference. Its
  rules that copy a `mut` field and an array element before a call change: see
  "Still open".
- The drop-blocks design rejects a mark on a parameter and a hand-over at the
  last read. Its rewrite must follow this or say why not.

The class rules in ADRs 0007, 0015, 0017, 0019, 0020, 0023, 0024, 0030, 0036,
0037, 0038, 0042 and 0044 go with this. The first spec lists the diagnostic
codes that go.

## Why

- **Correctness and simplicity come first** ([ADR
  0006](0006-values-correctness-simplicity-readability-performance.md)). A
  write through a shared object is an effect that the reader cannot see at the
  line that causes it. ADR 0006 asks for "no hidden aliasing". This trades a
  hidden write through an alias for a lost write to a copy, which a reader can
  see at the line.
- **Less to build after this point.** `weak`, weak captures, the cycle warning
  and the reach analysis are all planned and not built. A copy into a task
  loses only its case for a cycle. It still keeps a record of each block with a
  count above 1. With no record, a value made by `t = T.Node(t, t)` 60 times,
  which is 120 blocks, would copy as 2^60 nodes.
- **The decision is cheapest now.** Milestone 9 stage 1 builds `weak`. After
  that, this decision also deletes that work and its tests.

## What was measured

A spike compiler ran the programs below. It adds structs beside classes, and
each struct is still a counted heap object. Each program printed the same bytes
in both versions, and Valgrind found no memory error in the struct versions.
The table gives Callgrind instructions with
`--allocator system --codegen-units 1`. The last column gives the growth at 4x
the input, struct and class.

| Program | Structs | Classes | Growth |
| --- | --- | --- | --- |
| Lexer | 274.8 M | 259.1 M | Linear, in the first spike |
| Tree with parent links, as ids | 19.3 M | 15.0 M | Not measured |
| Small server | 313.6 M | 296.1 M | Linear, in the first spike |
| Task board with undo | 276 M | 606 M | Quadratic in both: 15.6x, 16.2x |
| Task board with no history, without `find`, `related`, `del`, `watch` | 435 M | 398 M | Linear: 3.9x, 4.0x |
| Parser and resolver, the whole dump | 46.4 M | 34.9 M | Linear: 4.1x, 4.0x |
| The same, parser part only (the dump less lexing) | 19.3 M | 9.7 M | By subtraction: 4.3x, 4.0x |
| The same, a snapshot and an undo for each function | 496 M | 484 M | Quadratic in both: 14.7x, 15.2x |
| Binary search tree, 20 000 keys: enum against class | 88.4 M | 27.4 M | About linear: 4.4x, 4.3x |
| The same keys: array of nodes with indexes against class | 28.2 M | 27.4 M | About linear: 4.3x, 4.3x |
| Observer with its own state, 20 000 events, copied back by hand | 1 620 M | 3.32 M | 15.9x, 3.3x |
| The same observer, `mut self` through `any` (structs: two observers; classes: one) | 3.67 M | 3.32 M | Linear: 3.4x, 3.3x |

With the default allocator, the tree costs 14.3 M and 12.4 M. The task board
with undo is faster on structs because a snapshot of classes copies at once.
The four commands left out of the second board row are quadratic in both
versions.

The parser's dump equals `sisu build --emit ast` on 1 500 of 1 515 files. The
same 15 files fail in both versions. A move-only `File`, a stand-in with no
descriptor, ran correctly in a program and in a server loop, and 25 wrong
programs were rejected.

## What it costs

- **A write to a copy goes nowhere.** `let mut a = bank.accounts[i]` and then
  `a.balance += 100` does not change the bank. It compiles, and a warning can
  catch only a copy that nothing reads.
- **An old id can reach the wrong item.** The compiler cannot catch this. A
  checked `Arena<T>` can catch a stale id and an id from an unrelated arena at
  run time. It cannot catch an id from a copy of the same arena that has changed
  since the copy: both copies have one tag, and a read gives an unrelated item.
- **A write through `get` and `set` is quadratic.** `get` returns a value that
  the slot still holds, so the first write copies it. A node with a growing
  array in a slot cost 102.8 M at 5 000 and 1 623 M at 20 000. A method that
  takes the value out and a method that puts it back cost 2.5 M and 9.5 M, and
  a direct path costs 1.38 M and 4.83 M. Between the take and the put, the slot
  is empty: a `put` that an early return skips loses the item.
- **`mut` says less in a program with one large state.** `mut app` goes
  through many signatures: 19 of the task board's 38 functions, and 81 of the
  parser's
  163. The class version of the parser had none.
- **A snapshot held across writes copies** on each first write. A program that
  keeps one in a loop is quadratic, and it is correct.
- **Callback state is a parameter.** Rust gets stateful callbacks from its
  borrow checker, and Sisu has no borrow checker. No closure has been tested,
  because Sisu has no closures yet.
- **A tree on an enum box costs more** than a tree of classes: 3.2x in the
  table. A `match` arm cannot write a payload in place, so an insert takes
  apart and builds each level of its path. A tree that needs the speed can be
  an array of nodes with indexes, but a wrong index reads the wrong node.
- **The migration is large.** 634 tracked `.sisu` programs declare a class
  (`git grep -lE '^\s*(pub\s+)?class\s+\w' -- '*.sisu'`). Some test sharing on
  purpose and go. The compiler gets smaller only at the last step.

The total saving of work is not established. In the second review, one model
found less total work if this is decided before milestone 9, and the other
found no saving and a lower near-term risk with classes. Neither measured it.

## Order of the work

1. Rewrite every spec that names a class: milestone 9 without `weak` and
   `[mut]`, the drop-blocks design on move-only structs, and `mut self`
   through `any` with its box. The first spec commits the programs of the tests
   below, with their 1x and 4x inputs.
2. Build inline structs, the box and the take of a `mut` argument beside
   classes, with the real checks, tests and generator rules ([ADR
   0033](0033-every-language-feature-has-a-generator-rule.md)). Until step 4, a
   place whose path crosses a class object is copied in, and a class cannot hold
   a move-only value.
3. Move the tests, the benchmarks and `selfhost/` to structs. Record each
   write to a copy that this finds, because that measures the first cost
   above.
4. Delete classes.

## What would reverse this

Check these before step 4, because the deletion is the hard part to undo:

- **The move-only check needs a general borrow checker or lifetimes** (the
  roadmap rules them out, ADR 0005). The check must accept each of these, or a
  socket server cannot be written: a read of a field beside a `mut` of another
  field, a `mut` parameter or a local that is moved and set again on each exit
  path, a loop that hands each connection to a worker, and a worker that reads
  from a connection that it got by a move. The first spec lists more test
  programs and says for each one whether the check accepts it: the spike's wrong
  programs, `for f in files`, `if let Some(f) = xs[0]`, `two(c.f, mut n)`,
  `two(mut xs[i], mut xs[j])`, `two(xs[i], mut xs[j])`, `two(c, mut c.g)`, a
  method that returns a move-only field, and the check of a generic definition.
  The real check must decide each one correctly with no lifetime or region in a
  type, no stored or returned borrow, and no summary of a callee: only its
  signature. One failure reverses this.
- **An observer through `any` costs too much:** more than 100 M instructions
  at 20 000 events, with inline structs.
- **A copy grows with the input:** more than 5x instructions at 4x the input,
  with inline structs, where the class version stays below 5x. The programs
  are the lexer, the tree with parent links scaled by an input, the small
  server, the parser part (the dump less lexing), the second board row and the
  `mut self` observer. A snapshot that a program keeps on purpose does not
  count.

One test cannot run before step 4, because tasks come after milestone 12: a
socket that moves through a channel, with a panic and a cancellation that
clean up, and a field of the socket's holder that another value shares. If it
needs a shared cell other than a channel or a task handle, the owner of the
project chooses between a marked `Shared<T>` that can change and a return to
classes.

## Still open

- A warning for a write to a copy that nothing reads, and how far it can
  reach.
- The final form of the move-only mark, and how a generic function opts in to
  a move-only type.
- The form of a hand-over: a mark on the parameter, at the call or at both,
  and whether a binding or a store moves the value. A worker must be able to
  read a socket that it got by a move, so a binding that moves or a `mut` form
  of a moved-in parameter must exist.
- A `mut` argument of a move-only type where the places can overlap at run
  time, through a map entry or a user `Indexable` type, and beside a place that
  copies. The spike rejects the overlap. Also what a holder's drop block sees
  when a call panics while its field is taken.
- When a call copies or retains an argument that it borrows. ADR 0029 copies an
  array element, and a move-only element has no copy. If that copy only goes, a
  `set` in a later argument can free the borrowed value:
  `f(s.rows[0], h(mut s[0]))`, and the same with `s.rows`. A binding has the
  same case: `for x in s.rows { h(mut s[0]) }`. The first spec answers this
  before the take is built. Until it does, the copy stays, and a move-only value
  in this position is rejected.
- How a closure or a `spawn` takes a move-only value. The milestone 9 design
  has no closure that runs only once.
- A move-only value in an `any`, in a `for` and in a slice, a field moved out
  of its struct, and a generated `==` or `print`. The spike rejects each of
  the first four.
- Whether a `?.` write and a `mut self` call through `?.` work in place on a
  value (ADR 0037). ADR 0043 says that a `?.` path is not a `mut` place.
- Whether an enum takes `mut self`. Today it cannot (E0018).
- `Arena<T>`, if it comes: a write through its methods that cannot lose the
  item, what a stale id gives under ADR 0040, and whether it is in the prelude
  (ADR 0015) or a module of the standard library.

## Considered options

- **Keep classes, and let `let` freeze fields through a reference (Rust).**
  Two references to one object can be `let` and `let mut`, so the binding
  cannot decide. Rust needs its borrow checker for this (ADR 0011).
- **Keep classes, and let a struct hold only values.** A struct is then
  frozen, but `t.leader.name = "Aino"` still compiles when `t` is a class, so
  the reader has the same question and two kinds of type.
- **A struct with a drop block holds one hidden count.** Either the fields
  live in the shared block, which is a class under another name, or two copies
  share one descriptor with different state. The count is also not atomic, so
  the value still cannot cross a task.
- **Structs plus a marked `Shared<T>` that can change.** This keeps graphs and
  caches easy, but it gives up "a `let` value never changes". It stays the
  fallback above.
