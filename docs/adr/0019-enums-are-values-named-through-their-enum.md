# Enums are values named through their enum

Status: accepted, 2026-10-15. Milestone 7 builds it. It builds on
[ADR 0005](0005-reference-counting.md)'s reference counts, and it is the
enum half of [ADR 0011](0011-let-mut-marks-what-can-change.md)'s
self-containment rule. On 2026-10-18,
[ADR 0025](0025-mir-holds-every-count.md) superseded its statement that
`match` stays in `tir` with no `mir`: `match` now lowers to `mir`, which keeps
the same scheme of arms in order and backtracking choice points. When part 5
of the mir merges, [ADR 0030](0030-match-takes-its-scrutinee-apart.md) will
supersede its rule that `match` moves nothing out of its scrutinee.

An enum is a value: a tag plus the payload of the variant it holds, stored
inline, with no heap block of its own. Every value of an enum is one of its
variants, and a `match` must cover all of them.

- **A variant is built through its enum, with `.`.** `Expr.Num(2)` builds a
  value and `State.Idle` builds a payload-free one. `.` is Sisu's one access
  operator, as for fields, methods and milestone 11's module paths. There is
  no `::`. A variant lives inside its enum, so `class Num` and `Expr.Num`
  coexist. A bare `Num(2)` means a top-level `Num`; when there is none and an
  enum has the variant, the error says "write `Expr.Num`".
- **A pattern uses the bare name.** `Num(n) => n`. The type of the value
  being matched decides which enum's variants are searched, so a pattern is
  never ambiguous. A qualified pattern (`Expr.Num(n)`) is accepted too.
- **No local, parameter or binding takes the name of an enum.** So
  `Shape.Circle` always means the enum's variant, never a method on a local.
  An error now can be relaxed later; a shadowing rule cannot be taken back.
- **Recursion needs no annotation.** The checker builds a "holds inline"
  graph over enums: an enum holds the enums its payloads name directly, and
  an optional of an enum adds an edge too, while arrays, classes and
  optionals of classes are already pointers and add none. Every payload
  field whose enum type lies in the declaring enum's strongly connected
  component is stored behind a box, a counted heap block. The choice is
  deterministic and breaks every cycle: direct (`Expr` in `Expr`), mutual
  (`A` holds `B`, `B` holds `A`) and optional (`next: Expr?`). It may box
  more than the minimum: `enum A { X(B) }` with `enum B { Y(A), Z }` boxes
  both fields. Inside a component every link is therefore a box.
- **A box that reaches count zero is freed from a pending list, not by
  recursion.** It is linked into a list through its own count word, which is
  dead by then. One loop takes boxes off the list, releases their payloads'
  counted parts and frees them; a child box that reaches zero joins the list
  instead of being freed by a nested call. The list lives on the stack of the
  release that drains it and needs no allocation. The stack stays constant
  for right-deep, left-deep, mixed and mutual shapes. A class or array in a
  payload is released as today, so a chain that alternates through classes
  or arrays keeps ADR 0005's limit of one stack frame per object.
- **The box goes on an enum's field when a cycle runs through a record or
  a generic type.** A cycle made of records alone stays ADR 0011's error. In
  milestone 8, a cycle through `Wrap<E>` boxes the declaring enum's field,
  never a field inside `Wrap<T>`.
- **Enum values never change in place in milestone 7.** A `let mut` variable
  can be given a new value. There is no field access on an enum, and a
  pattern binding is an immutable local. Mutable state goes in a class
  payload, which binds the reference. Because values never change, a box is
  shared freely: copying an enum retains its boxes and never deep-copies. A
  later milestone may relax this; no milestone 7 program writes in place, so
  none would break.
- **`match` owns its scrutinee and moves nothing out of it.** The scrutinee
  is evaluated once into a temporary that the `match` owns. Patterns test it
  in place. When an arm commits, each binding is retained, the temporary is
  released, and then the body runs. A scrutinee in its last use therefore
  hands its parts to the bindings with their counts back where they were.
  There is no field-level move-out, and no move out of a box with count one.
  That costs one count up and down for a part the scrutinee held alone, and
  it is the price of keeping a failed guard or alternative free of any
  release. Box reuse in `match` depends on it and waits for a benchmark
  (roadmap, "Reusing a box in `match`").
- **`match` stays in `tir`; there is no `mir`.** Codegen compiles it straight
  to basic blocks: arms in order, each pattern tested in place, nested
  alternatives through backtracking choice points. Code size stays linear in
  the pattern without a decision tree, and no later milestone before closures
  needs a `mir`.
- **A later `.Variant` short form is for expressions only.** A line that
  starts with `.` continues the previous one (ADR 0001), so inside an arm
  list a `.count` line after `Running(c) => c` is valid milestone 7 code. A
  pattern never starts with `.`, and no arm line ever does, so adding the
  short form breaks no program.
- **`if let` on `T?` is an implicit `Some`.** `if let x = opt` binds the
  payload, as today; in milestone 7 only a binding or `_` may stand there.
  Milestone 8 settles the rule, at any position of optional type: `Some(p)`
  and `None` match the optional itself; a variant, literal or range pattern
  matches the value inside; an alternative applies the rule to each
  alternative; and a bare binding or `_` covers the whole optional, as Swift's
  `switch` does. Only the top of an `if let` or `while let` peels one layer,
  so `if let x = opt` binds the payload. The user ruled on 2026-10-16,
  amending the rule of 2026-10-15, which left `None` uncovered in
  `match o { Some(n) => n\n _ => 0 }`. No enum can have a variant named
  `Some` or `None`
  ([ADR 0015](0015-the-prelude-holds-only-what-almost-every-program-uses.md)),
  so the rule never has to choose.

## Considered options

- **Bare names everywhere (Kotlin after an import, Haskell, Rust after
  `use Enum::*`)**: `Num(2)` reads best. But it clashes with a class `Num`,
  and in a pattern a misspelled variant silently becomes a catch-all
  binding. Sisu keeps the bare name only where the matched type decides it.
- **Swift's `.num` short form**: short, but it needs a known expected type at
  every use, and a line starting with `.` already continues the line above
  (ADR 0001). It stays open for expressions only.
- **Rust's `Expr::Num`**: a second access operator beside `.` for a case that
  `.` already reads well.
- **Rust's explicit `Box` and Swift's `indirect`**: the program says where the
  pointer goes. But a recursive enum that must be annotated to compile is
  something this milestone sets out to avoid, and the checker can see every
  cycle itself.

## Consequences

- A program that has a local named like an enum must rename it.
- `Option` and `Result` are enums, so milestone 8 and milestone 12 build on
  this layout. The `None` of an optional class reference stays a null
  pointer.
- Copying an enum retains each counted part and releasing it releases them,
  through per-enum helpers that switch on the tag. `retain`, `release` and
  `equal` stay the only entry points.
- `print` of an enum value waits for milestone 8's `Display`-like interface,
  as `print` of a class does.
