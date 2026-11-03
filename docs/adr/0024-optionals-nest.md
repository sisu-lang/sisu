# Optionals nest

Status: accepted, 2026-10-16. It reverses milestone 2's rule that an optional
of an optional is the same optional, now that `T?` is shorthand for the
prelude's `Option<T>` (milestone 8).

`i64??` is a type, and it is `Option<Option<i64>>`. `T?` and `Option<T>` are
the same type. Generic code nests optionals too: `first<T>` on a `[i64?]`
returns an `i64??`, so "the array was empty" (`None`) and "the first element
was `None`" (`Some(None)`) stay apart.

- **A plain value converts through one layer.** `let x: i64? = 5` works as
  before. The middle state of an `i64??` is written `Some(None)`.
- **`??` and `if let` peel one layer.** On an `i64??`, `x ?? 5` gives an
  `i64?`: the `5` converts through one layer, so an empty `x` gives `5`. So
  does `x ?? None`. Getting an `i64` takes two steps: `(x ?? None) ?? 0`.
  `if let y = x` binds `y: i64?`.
- **`?.` flattens one layer, as it does today.** `a?.b` where `b: i64?` gives
  `i64?`, not `i64??`. A chain such as `a?.b?.c` stays one optional.
- **Printing tells every state apart.** A `T?` holding `x` prints `x`'s
  text, except when `x` is itself an optional holding `None` at its innermost
  layer; then it prints `Some(` + `x`'s text + `)`. `None` prints `None` at
  every depth. So an `i64??` prints `None`, `Some(None)` or `5`, and an
  `i64???` prints `None`, `Some(None)`, `Some(Some(None))` or `5`.
- **Niches keep the layout small.** An `Option`-shaped enum, with one
  payload-free variant and one variant with a single field, stores its empty
  variant in a bit pattern its payload can never hold. A program cannot see
  it, and a niche never reaches the runtime: printing decodes it first.
  - A class uses a null pointer. A string or an array uses a null
    `block`, the first of its three words.
  - `char?` uses `0x110000`, so it takes 4 bytes, not 8.
  - `bool?` uses `2`.
  - An enum uses a spare tag value.
  - A nested optional uses the next spare value: `char??` uses `0x110001`.
  - `i64?`, `u8?` and `f64?` have no spare value and keep a separate flag.

  One rule covers every `Option`-shaped enum, including the program's own.

## Considered options

- **An optional of an optional is the same optional (milestone 2, Kotlin)**:
  `T??` collapses to `T?`. It is simple, but generic code loses information:
  a `Map<K, V?>` lookup that returns `V??` could not tell a missing key from a
  key whose value is `None`. And `Option<T>` would stop behaving like an
  ordinary generic enum, since `Option<Option<T>>` would not be a different
  type from `Option<T>`.
- **Nested optionals, with `??` and `if let` peeling one layer (Swift, and
  Rust's `Option<Option<T>>`)**: each layer is a state a program can observe,
  and generic code treats `T?` like any other enum. A program must unwrap each
  layer by hand.
- **A `?.` that does not flatten**: `a?.b` with `b: i64?` would give `i64??`.
  It keeps the layers exact, but a chain of field reads would end in a pile of
  layers for no benefit. `?.` keeps flattening, as it does today.

## Consequences

- A program from milestones 2 to 7 never wrote `T??`, since the checker
  rejected it, so none changes meaning. The roadmap's note that milestone 8
  must keep `T?` of an optional as the same optional is dropped.
- An optional class reference stays a null pointer, in an array too, so the
  layout of an optional reference does not change. `char?`, `bool?` and an
  optional enum shrink to their niche: `char?` takes 4 bytes instead of 8.
- `i64?`, `u8?` and `f64?` still carry a flag, so they take more bytes than
  their payload.
