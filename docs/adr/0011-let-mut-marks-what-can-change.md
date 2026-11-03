# `let mut` marks what can change

Status: accepted, 2026-10-11. It replaces ADR 0002. The rename lands in
milestone 4; records come in milestone 10.

Superseded in part by [ADR 0043](0043-mut-parameters-copy-in-and-out.md):
"there is no `inout`" no longer holds, because a `mut` parameter changes the
caller's place. The decisions below stay as written.

One word, `mut`, marks everything that can change, wherever it is written:

- **Bindings**: `let x` cannot be reassigned, `let mut x` can. `var` is gone.
  Function parameters stay immutable. The checker warns about a `let mut`
  that never changes, so the keyword always tells the reader the truth.
- **Class fields**: a bare field is set by the constructor and never changes;
  a `mut` field can be reassigned. An object is shared (ADR 0005), so the
  field, not the binding that holds a reference, decides. A class method
  takes plain `self` and may write any `mut` field.
- **Record fields** (milestone 10): no keyword. A record is a value
  (ADR 0007), so the place that holds it decides: nothing in `let p` changes,
  any field of `let mut p` can.
- **Methods that change a record** take `mut self`. They are called only on
  a place that can change: a `let mut` binding, a `mut` field, or an element
  of an array held in one. The change reaches that place directly, with no
  copy and no marker at the call site: `p.move_by(1, 2)`. Array methods that
  change the array, such as `push`, follow the same rule.

Ordinary parameters never change the caller's value: there is no `inout`, and
ADR 0007 holds. Only the receiver of a `mut self` method can change.

Classes and records declare fields the same way, in the body, one per line,
with no commas, as statements end at a newline (ADR 0001):

```
class Player {
    name: string
    mut hp: i64

    fn hit(self, damage: i64) {
        self.hp -= damage
    }
}

record Point: Hash {
    x: i64
    y: i64

    fn move_by(mut self, dx: i64, dy: i64) {
        self.x += dx
        self.y += dy
    }
}

record UserId { value: i64 }
```

Which to use follows from one question: when a copy changes, should the
original change too? No: a record, a value such as `Point` or `Money`. Yes: a
class, a thing with identity such as `Player` or a tree node. Start with a
record.

We chose `mut` because it says what it means in every position, and one word
covers bindings, fields and receivers. `var` names the thing, not the
property, and a reader from JavaScript knows `let` as the binding that can
change. The extra letters on `let mut` also nudge code toward immutable
bindings, the point of ADR 0002.

## Considered options

- **Keep `let`/`var`, with `var self` (Swift, Kotlin)**: shorter and closest
  to ADR 0004's Swift and Kotlin, but `var` does not say "can change", and
  `var self` reads as a local copy of `self`, which is what Swift's old `var`
  parameters were.
- **`mutating fn` (Swift)**: a new keyword, written away from the `self` it
  is about.
- **`let`/`var` on each record field (Swift structs)**: two places decide
  whether one field changes. A field that must never change is protected by
  privacy instead (milestone 11).
- **Bare class fields whose mutability follows the binding (Rust)**: two
  references to one object can be `let` and `let mut`, so the binding cannot
  decide. Rust needs its borrow checker for this.
- **`inout` parameters (Swift)**: rejected in ADR 0007. Changing only the
  receiver covers `xs.push(x)` and `p.move_by(...)` without a second kind of
  parameter.
- **Fields in a header, `record Point(x: i64, y: i64)` (Kotlin, Java, C#)**:
  a one-field record fits on one line and the declaration mirrors
  `Point(x: 1, y: 2)`, but a class with many `mut` fields and their comments
  reads better in the body, and both kinds use one form.
- **`struct` for records (C, Go, Swift, C#)**: the most widely known word for
  a value that is copied, but it reads as memory layout. `record` reads as
  data that gets `==` and `print`, which milestone 10 gives it.

## Consequences

- Milestone 4 renames `var` to `let mut` and makes fields bare or `mut`:
  the lexer, the parser, the diagnostics and every test program change.
  `var` stays a reserved word, so a program that uses it gets an error with
  the help "use `let mut`".
- New errors, each with a help line: writing a record field or calling a
  `mut self` method on a place that cannot change (`let mut`); `mut self` on
  a class method (`self` is shared); writing to `self` in a record method
  without `mut self`; a comma after a field (one field per line); a record
  that contains itself (use a class); a class that names another class after
  `:` (Sisu has no inheritance: hold a field, or use an interface).
- A new lint: a class with no `mut` field that nothing compares with `is`
  has nothing a record lacks, so it suggests `record`.
- A weak field is written `weak mut parent: Node?` (ADR 0008).
- A `mut self` method called on a field of an object, `obj.pos.move_by(1, 2)`,
  must not reach `obj.pos` another way during the call. That needs an
  exclusivity rule, the same one that handing an array field to a call needs
  (roadmap, "Open decisions").
- A closure inside a `mut self` method cannot capture `self` (milestone 9).
- The error for a record that contains itself covers cycles made of records
  alone. A cycle that runs through an enum gets a box on the enum's field
  instead ([ADR 0019](0019-enums-are-values-named-through-their-enum.md)).
