# A slice shares its block with the value it was taken from

Status: accepted, 2026-10-31. It answers the open question "Substring views"
in the roadmap and the last Consequence of
[ADR 0014](0014-strings-are-utf-8-values.md).

A range index, such as `xs[a..b]` or `s[a..b]`, gives a *slice*. A slice is an
ordinary `[T]` or `string`. It shares its elements with the value it was taken
from, and taking it copies nothing. There is no `Slice<T>` type: "slice" names
a value, not a type. Seven rules follow.

- **A value has three words (layout A).** An array value and a string value
  are `{block, data, len}`. `block` points at the block, for its count.
  `data` points at the first element of the value, inside the block. `len` is
  the length of the value, and `xs.len()` reads it with no load from memory.
  A value that covers its whole block has `data` at the first element and the
  same `len` as the block.
- **The header's `len` counts the elements that are set in the block.**
  Freeing a block releases each of them. Slices that still point into it do not change this.
  Dropping a value releases its block and walks no elements, so an element is
  released once.
- **Writes never reach the parent.** A range index is a new value, not a place
  ([ADR 0007](0007-arrays-are-values.md),
  [ADR 0011](0011-let-mut-marks-what-can-change.md)). So `xs[1..3] = ys`,
  `xs[1..].push(4)` and `f(mut xs[1..3])` are errors. A write to a slice whose
  block has count 1 happens in place. With a larger count it copies first, and
  the copy holds only the elements of the slice.
- **`push` has three paths.** (1) The count is 1, the value ends at the last
  set element of the block, and the block has spare capacity: write in place.
  (2) The count is 1, the value covers the whole block, and the block is
  full: grow the block. (3) Otherwise copy the value into a new block, with
  extra capacity when the value does not cover its block, and push there. A
  push never writes over an element that another value can see. A queue loop
  such as `q = q[1..]; q.push(x)` stays linear.
- **The compiler trims where a slice is placed inside another value.** A *trim*
  is a copy of one small slice into a block of its own. It stops a slice from
  keeping a large parent block alive. The trim points are:
  - a class constructor argument and a store to a class field;
  - an enum payload, and a struct field when structs land (milestone 9);
  - an element of an array literal, the value of `[v; n]`, `push` and
    `xs[i] = v`;
  - the conversion to `any I`;
  - the copy-out of a `mut` argument into a class field or an array element,
    when `mut` arguments land;
  - a closure capture, when closures land (milestone 9).

  A trim point looks through an optional. A local, an argument, a return
  value, `Some(x)` and the range index itself are not trim points.
- **The step rule.** At a trim point, a value that covers its whole block is
  stored as it is. So is a value with more than a quarter of the capacity of
  the block in bytes. So is any value in a block of less than 256 bytes.
  Otherwise the value is copied into a block of exactly `len` elements. The
  benchmarks of the work set the final numbers.
- **A `{` after `..` in the head of a `for`, `if`, `while` or `match` starts
  the body.** So `for i in 0.. { ... }` is a range with no end, as in Rust. The
  rule does not hold inside `(...)`, `[...]` or a call's arguments, and does
  not hold outside a head.

## Why

- **A copy costs the time that slicing should save.** A recursive function that
  splits its input copies at every level. So a binary search that takes log n
  steps copies n elements. The self-host lexer builds the text of a token one
  `char` at a time for the same reason.
- **One sequence type.** A program learns one type, and its indexes always
  start at 0. Swift's `ArraySlice` keeps the indexes of the parent, so `s[0]`
  can crash. Go's `append` can overwrite the elements of a neighbor. Sisu has
  neither problem.
- **A user does not remember to ask for a copy.** In Sisu a copy is never
  visible, because arrays and strings are values. So the compiler makes the
  copy where a slice can outlive its parent. Java shared substrings until
  Java 7u6 and then made them copy. Go added `strings.Clone`. Swift made
  `Substring` and `ArraySlice` separate types.
- **A trim never changes the elements of a value.** It changes what a program
  prints in one case: a trim frees the elements outside the slice earlier, so a
  `weak` reference ([ADR 0008](0008-weak-references.md)) to one of them gives
  `None` earlier.

## Considered options

- **Layout B, `{block, start: u32, len: u32}`.** It has two words. But every
  access adds `start`, and an array cannot hold more than about 4 billion
  elements. It is the fallback if layout A fails the speed gate.
- **Layout C, a view block.** Each slice is a heap block `{count, parent,
  start, len}`, and a value stays one word. But every element access first
  checks which kind of block it holds, and every slice allocates. That works
  against short-lived slices, the main use.
- **A `Slice<T>` type.** Rejected on 2026-10-17: it adds a second type, and
  indexes that depend on the parent.
- **A `clone()` method, or a sharing marker such as `xs^[a..b]`.** Rejected.
  The compiler trims, so a program rarely needs a copy. If real programs hit
  the case that trimming misses, a `clone()` method can come later.
- **A trim at a range index.** Rejected. A rule "trim at `xs[a..b]` when `xs`
  is at its last use" copies at every second level of a recursive search. This
  happens when nothing outside holds the array. A rule that depends on where
  the slice goes lets a `let` change the memory and time of a program. It also
  needs a new mir analysis.
- **A safe range on arrays, as a second method.** `get(i)` and `get(a..b)` on
  one type would be an overload
  ([ADR 0039](0039-sisu-has-no-function-overloading.md)). A name such as
  `slice` reads as an iterator method. A bad range panics, as `xs[i]` does.
  Milestone 9 gives arrays one `get` whose argument is an index or a range,
  through an interface. Clamping a bad range, as Python does, turns an
  off-by-one error into a wrong answer with no report.

## Consequences

- Every stored `[T]` and `string` grows from 8 to 24 bytes (class fields,
  `[[T]]`, enum payloads), and an argument takes three registers. The first
  task changes the layout alone and measures it. A rise of more than 5% in the
  instruction count of any benchmark program stops the work.
- `[T]?` and `string?` keep a null `block` for `None`. A pointer has no second
  niche, so `string??` stays a flag and a value.
- A slice of a dropped large array that a long-lived local keeps is never
  trimmed. Catching it needs the lifetime of each value, which needs a borrow
  checker or a garbage collector.
- A trim point decides by size alone. It also copies a small slice of a parent
  that stays alive, which costs one allocation and the bytes of the slice.
- On a string, the bounds are byte positions. A bound inside a character
  panics, and `s.get(r)` gives `None` for it.
- A bad range panics with exit code 101, as a bad index does.
- The borrowed-parameter pass ([ADR 0029](0029-parameters-are-borrowed-unless-consumed.md))
  sees a range index of a parameter as a use that the result keeps. Hoisting
  `make_unique` out of a loop ([ADR 0021](0021-a-loop-makes-its-written-arrays-unique-before-it-starts.md))
  counts a range index as a copy of its base.
