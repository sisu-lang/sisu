# Classes compare by identity

Status: accepted, 2026-10-16. It reverses milestone 2's rule that `==`
compares classes structurally, now that milestone 8 makes `==` a method of the
`Eq` interface.

A class has no `==` of its own. The compiler writes no `Eq` for a class, so
`a == b` on two objects is an error unless the class conforms to `Eq` by
hand. `a is b` compares identity: whether both sides are the same object.
A class prints its name, with its type arguments, and never its fields; a
class that wants other text writes `to_string` by hand.

An array or an enum that holds a class has `==` only when the class conforms
to `Eq`, because the generated `Eq` for an array or an enum asks each element
or payload for its own.

We chose this because a deep comparison hidden behind `==` never ends on a
cycle, and `weak` fields (ADR 0008) let programs build cycles on purpose. A
class is an object with an identity, so asking whether two objects are the
same is the question `is` already answers. A class that wants a value
comparison says so by conforming to `Eq`, and decides for itself which fields
count and whether to follow a reference.

## Considered options

- **Structural `==` (milestone 2)**: two objects are equal when each pair of
  fields is equal. It returns at once when both sides are the same object, but
  two distinct object graphs that contain cycles never finish comparing. The
  cost also hides: `a == b` reads as one cheap test and may walk a whole
  graph.
- **Identity `==` (Java, C#, Kotlin, Python)**: `==` on a plain class compares
  identity by default. C#, Kotlin and Python let a class define its own `==`
  (`operator ==`, `equals`, `__eq__`), while Java's `==` always compares
  references. Nothing can hang, but in Sisu `==` and `is` would then mean the
  same, and two ways to write one test break "one obvious way".
- **No `==` unless the class conforms to `Eq` by hand (Swift)**: Swift gives a
  class no `==` until it conforms to `Equatable`, and spells identity `===`.
  Sisu follows Swift: a class has `==` only when it conforms to `Eq` by hand,
  and identity is spelled `is`. Every use of `==` on a class then reads as the
  comparison its author wrote, and `is` stays the one way to ask about
  identity.

## Consequences

- `tests/run/equal_self_cycle.sisu` and the diagnostics that assumed
  structural `==` change in stage 5 of milestone 8.
- An array or enum holding a class has `==` only when the class conforms to
  `Eq`. `contains` on an array of classes needs the same.
- A program that compared classes with `==` before milestone 8 must conform to
  `Eq` or compare with `is`.
- Neither `print` nor a generated `==` follows a class reference, so the
  compiler writes no code that walks a cycle. An `eq` that a program writes
  follows the references it names, and it must stop on a cycle by itself.
