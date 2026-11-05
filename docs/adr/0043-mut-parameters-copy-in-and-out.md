# A `mut` parameter changes the caller's place, copied in and out

Status: accepted, 2026-10-30. It supersedes the rule "No way to change the
caller's array" of [ADR 0007](0007-arrays-are-values.md) and the sentence
"there is no `inout`" of [ADR 0011](0011-let-mut-marks-what-can-change.md). It
retires the error E0019, "a parameter cannot be `mut`".

A function can change an array, a string or a struct that its caller holds. The
parameter says `mut`, and the call site says `mut` again:

```sisu
fn set(mut cells: [i64], i: i64) { cells[i] += 1 }

set(mut b.cells, i)
```

Before this decision, the same call was `b.cells = set(b.cells, i)`. Each call
copied the whole array. 20 000 calls on a field of 1 000 000 elements took 17.9
billion instructions. With a `mut` parameter and the take (step 2), they take 9.4
million instructions.

## Decision

A `mut` parameter is written `mut name: T`. Its argument is written
`mut place`. The meaning of a call with `mut` arguments is copy-in/copy-out.
The call runs in this order:

1. **Evaluate the arguments** from left to right. For a `mut` argument, evaluate
   its root when the root is not a binding, and each index along its place, once.
   Keep the values. Read no element and no object along the place yet.
2. **Copy in** each `mut` argument, in argument order. Follow its place from
   its root, read each object along it, keep each object alive until the
   store-back, and read the value of the place. An index that is out of range
   along the place panics here, as a read `xs[i]` does. So `f(mut xs, g(mut xs))`
   runs `g` first, and `f` sees the change of `g`.
3. **Call the function.** Code that reads the place during the call sees the
   value that the place had at the copy-in. A write to the place by other code
   during the call stays until the store-back replaces it.
4. **Store back** each `mut` value into its place, in argument order. When two
   places are one, the last one wins. Each store-back is the assignment
   `place = value` on the objects that the copy-in kept. Where that assignment
   would panic because an array element no longer exists, the compiler drops the
   value instead. This is a lost write.

The store-back runs on every return path of the function, early returns
included. A `mut` argument is any place except a path through `?.`: if
`x = v` compiles and `x` has no `?.`, `f(mut x)` compiles. A path through `?.`
is a place with two types, one to read and one to write, so no parameter type
fits it. It is the type error `MutArgumentType`. A `mut` argument must have
exactly the type of its parameter, with no conversion.
Two `mut` arguments of one call cannot name the same place, nor one place that
holds the other, when the compiler can see it (`MutPlaceTwice`).

A `mut self` receiver (milestone 9) is the first `mut` slot of its call, and it
has the same meaning.

A class object stays shared. The copy-in keeps the object alive and reads its
field. So the program never panics because two accesses overlap, and a lost
write is defined behavior:

```sisu
fn grow(mut cells: [i64], b: Board) { b.cells = [9]; cells[0] = 1 }
let b = Board(cells: [0, 0])
grow(mut b.cells, b)
print(b.cells)        // [1, 0]: the write of [9] is lost
```

## Delivery in two steps

- **Step 1: the meaning.** The syntax, the checker, every call path, the
  diagnostics and the lowering give copy-in/copy-out everywhere. A `let mut`
  local is moved in, with no copy, by the last-use rule of ADR 0007. A field
  or an element is copied in: the first write of the function copies once, as
  `x = f(x)` does today. No program is slower than before.
- **Step 2: the take.** The compiler moves a class field or an array element out
  of its place for the call, instead of sharing it. It does this only when
  nothing else reaches the place during the call. The first write of the
  function then does not copy. A take changes nothing that a program can
  observe. A hidden, test-only switch builds a program with no take, and the
  test suite and the fuzzer check that both builds print the same. Step 2 also
  adds the warning `MutLostWrite`, for the lost writes that the compiler can
  see.

The take applies only when all of these hold:

- the value is an array or a string;
- the place is a field of a class object or an element of an array, with no
  struct field along its path;
- no code of the call reaches the place;
- the value can hold no resource type;
- the call reads no `weak` field or capture whose type the value can hold: a
  take can release the old value earlier than a copy does;
- two `mut` arguments that can overlap pass a run-time test that they do not.

A map entry and a place of a user `Indexable` type are copied, never taken:
their store-back runs `set`.

### What reaches a place

The analysis keeps one summary for each function: the keys that it reads and
writes, closed over everything it calls. A key is a field of a type. A read and
a write both block a take. The summary adds:

- a call that the compiler cannot see: a call through `any I`, a call through a
  function value, an `extern fn`, and a `mut` parameter of a function type;
- the code that the copy-in and the store-back of the other `mut` arguments of
  the call run: an `Indexable` `get` or `set`, and the `hash` or `eq` of a map;
- a closure or a named function in the arguments, when the callee calls it;
- generated code (`print`, interpolation, `==`) and the written conformance
  methods that it calls;
- a drop block that a release in the call can run (drop-blocks design);
- a `[mut name]` cell of a closure (milestone 9), as a key of its own.

A new feature that runs code during a call adds itself to this list, with a
test that has one row for it.

## Why

- **One concept, not five.** The review of 2026-10-29 found five separate
  mechanisms for one missing concept. The concept is "a call changes a value
  that its caller owns". The mechanisms are `[mut]` captures, the built-in
  `exchange`, the collections write rule, the last-use handover, and `move x`.
  Each needed edits to the others. None of them made
  `self.cells = step(self.cells)` fast.
- **One meaning for every place**, class fields included, with no run-time
  check and no panic.
- **The call site shows the change.** A reader sees which arguments change, and
  where the store-back happens.
- **Sisu has no users yet.** None of the other mechanisms is in the compiler, so
  the change costs spec edits and no program.

## Considered options

- **A. Keep the old plans** (`x = f(x)`, `[mut]` captures, a reserved built-in
  `exchange`, the exclusivity error of milestone 9, the collections write
  rule). It costs nothing now. Each later feature (closures, collections, `any`
  boxes, tasks) adds another rule, and a class field stays slow.
- **B. `mut` parameters with copy-in/copy-out (chosen).** It costs a second
  kind of parameter, a marker at the call site, and an analysis for the take.
- **B with a compile error for a lost write.** Rejected. An error is right only
  if it rejects no valid program. The compiler cannot prove that for calls
  through closures, interfaces and shared objects. So a lost write is defined
  behavior, with a warning where the compiler can see it (step 2).
- **B with Swift's run-time exclusivity check.** Rejected. It panics when two
  accesses overlap, and a panic in Sisu is the last resort
  ([ADR 0040](0040-a-failure-the-caller-can-handle-is-a-value-not-a-panic.md)).
- **`inout` as the keyword.** Rejected. Mojo replaced it with `mut`.
- **`&` as the marker.** Rejected. A reader from C, C++ or Rust reads `&x` as an
  address or a borrow, and `&` is bitwise and in Sisu.
- **No marker at the call site (Mojo).** Rejected. A reader cannot see which
  arguments change. Adding the marker later breaks every call. Removing it later
  breaks none.
- **A compile error for every call whose callee can reach the place.** Rejected.
  It rejects valid programs through closures, interfaces and shared objects.
- **Both steps in one plan.** Rejected. The take is the risky part, and it needs
  its own review and its own differential check.

## Correction to ADR 0007

ADR 0007 rejected `inout` partly because "Swift works the same way, without
`inout`". That is wrong. Swift has `inout`, and its `+=` takes the left side as
`inout`. ADR 0007 and ADR 0011 stay as written, each with a line that points
here.

## Consequences

- ADR 0007 and ADR 0011 are superseded in part. A function now changes the
  caller's place with a `mut` parameter. A method that changes its receiver
  still takes `mut self`.
- ADR 0029 changes. A borrowed argument is retained before the copy-in when
  its value comes from a place that a `mut` argument names, holds or is held
  by. The `mut` argument is one of the same call, or of a call nested in a
  later argument.
- `exchange` becomes a prelude function, `exchange(mut place, with)`
  ([ADR 0015](0015-the-prelude-holds-only-what-almost-every-program-uses.md)).
  The reserved method `x.exchange(with: v)` of the collections design goes.
- E0019 is retired, as ADR 0024 retired E0014. Its file gains the line
  `Status: retired in ADR 0043`, and `sisu explain` serves it from `RETIRED`.
  New codes E0166 to E0171 name the new errors (`MutMarkerMissing`,
  `MutMarkerNotMut`, `MutNotArgument`, `MutArgumentNotPlace`,
  `MutArgumentType`, `MutPlaceTwice`).
- A `mut` parameter lives for one call. A closure cannot share it with
  `[mut name]` (milestone 9), and a function cannot return it as a place.
- A `mut` parameter cannot have a default value, and an `extern fn` cannot have
  a `mut` parameter: copy-in/copy-out is a Sisu calling convention, and the C
  ABI has none.
- A loop that changes each element in place (`for mut row in grid`) needs
  iterators. It gets its own design after milestone 9.
