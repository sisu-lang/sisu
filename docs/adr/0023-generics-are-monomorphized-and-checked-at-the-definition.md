# Generics are monomorphized and checked at the definition

Status: accepted, 2026-10-16. It settles how milestone 8 compiles and checks
generic code, and the syntax that writes it.

A generic function, class, enum, interface or type alias takes type
parameters in angle brackets after its name: `fn largest<T: Ord>(xs: [T])`,
`class Stack<T>`. Two rules decide how the compiler treats them.

- **Compiled by monomorphization.** Each distinct list of type arguments gets
  its own compiled copy, and only the copies a program reaches are compiled.
  An instantiation's symbol names its arguments: `sisu.largest<i64>`.
  `largest<i64>` compiles to the code of a `largest` written for `i64`.
- **Checked once, at the definition.** Inside a generic body, a value of type
  `T` supports only what `T`'s bounds give it: the bounds' methods, and the
  operators they map to. Anything else is an error at the definition, such as
  "`T` has no `<`", with the help "add a bound: `T: Ord`". A call that
  compiles never reports an error inside the generic body. A bound that a
  type argument does not meet is reported at the call.

Four more rules go with them.

- **Bounds combine with `&`.** `T: Shape & Ord`. A comma would read as a
  second type parameter.
- **Type arguments are written in angle brackets, and `<` starts a list only
  when a list follows.** After a name in an expression, `<` starts a
  type-argument list only if a well-formed list follows and its closing `>`
  is followed by `(` or `.`. Otherwise `<` is a comparison, so
  `f(a < b, c > d)` stays two comparisons. Swift and C# also look at what
  follows the closing `>`.
- **Type arguments are inferred locally, as in Swift.** From the arguments
  (`largest([3, 9, 2])`) and from the expected type
  (`let s: Stack<string> = Stack(items: [])`) only. A type argument that
  neither gives is an error asking for an annotation. Wider inference can
  come later without breaking a program.
- **An interface takes type parameters, associated types, or both.** Use an
  associated type when each type has one answer (`Source.Item`), and a type
  parameter when a type can conform several ways (`Convert<Fahrenheit>` and
  `Convert<Kelvin>`). A type conforms to an interface with the same arguments
  at most once.

## Interfaces

Milestone 8 part 4 adds interfaces to the same two rules. A call through a
bound is checked at the definition, and each instantiation is compiled to
the code of a copy written by hand.

- **Static dispatch through `mono`.** A call of a requirement on a value of
  type `T` is an interface call in the checked tree. `mono` replaces it with
  a call of the method of the conformance that the type argument has. So
  `total<Circle>` calls `Circle.area` directly, and the later stages
  see an ordinary call. Interface values and dynamic dispatch
  are a later milestone. ADR 0029 infers the modes of a conformance method
  for each instantiation: `Circle.area` can borrow `self` while
  `Square.area` takes it, because a requirement's signature carries no
  ownership.
- **A conformance owns its methods.** The methods of `extend T: I { ... }`
  belong to that conformance. Two conformances of one type may each have a
  method `name`, and the symbol of a method names its interface:
  `Celsius.Convert<Fahrenheit>.convert`. A method in the body of the type
  with the same name wins a call on the type, as an inherent method wins in
  Rust. The method of the conformance is still reached through a bound.
  When no method of the body fits, a call collects the methods of the
  conformance blocks whose target covers the receiver. More than one is
  ambiguous, and the expected type picks one when only one of them returns
  it.
- **A declaration list meets the interfaces it inherits.** `class Cube:
  Solid` with `Solid: Named` has the methods of `Named` in its body. A
  `extend Cube: Solid` block holds the methods of `Solid` only, so `Named`
  needs a conformance of its own. A conformance holds only when the type
  conforms to what its interface inherits, under the same conditions.
- **An associated type is written through a type parameter.** `S.Item` for
  `S: Source`, and `S.Item.Key` for an associated type of an associated
  type. Inside the interface it is `Item`, and inside a conformance `Item`
  is the type it chose. A choice never names an associated type of its own
  conformance, and an `extend` block never names one in its target. A choice
  can still name an associated type of one of its parameters, as `T.Item.Item`,
  so the replacement of the associated types of a known type can lead back
  to the associated type it started with. The replacement keeps a list of the
  types it replaces and stops at such a loop. The checker reports the choice that
  closes the loop once, and the call that needs the type fails. `mono` does
  the same replacement for each instantiation. A loop that no signature
  writes can reach `mono`, which stops with an internal error. A call that
  compiles finds the conformance of each base.
- **An instantiation cycle through an interface is a cycle of weights.** The
  rule for `fn f<T>(x: T) { f([x]) }` becomes a graph where each edge has a
  weight, the depth at which an argument nests a type parameter. A call
  through a bound reaches every conformance of its interface, so each
  interface has one hub node, and a path through the hub stands for an edge
  from the parameter of the caller to a parameter of each conformance. An
  edge out of the hub weighs minus the depth of the parameter in the
  conformance's target. An associated type adds the growth of its choices, and
  a choice that names `T.A.B` adds the growth of `A` and of `B`. A cycle of
  positive weight among the choices has no end, even when no choice starts it.
  A cycle with a total weight above zero is an error naming the call that
  grows.

  The rule is conservative by design. Take `Start<T>.run`, which calls
  `dispatch(Done(value: [self.value]))`, with `Done<T>.run` calling nothing.
  The hub cannot see that the receiver of `dispatch` is then a `Done`, so
  the compiler reports four copies that never run. No rule is exact:
  conformances on distinct targets, dispatched through bounds, can run a
  counter machine, so whether the copies end is undecidable. Go has no edge
  that shrinks, and Rust stops at a recursion limit in `mono`. Sisu cannot
  do that, because a call that compiles never fails in `mono`. A finer
  graph, with one hub for each outer type of the receiver, would accept
  `Start` and `Done`. It waits until a program needs it.

The section leaves two features open: specialization, which is a more
specific conformance that wins over a general one, and type tests on `T`.
The roadmap lists both under "Open decisions". Nothing here rules them out.

## Considered options

- **Brackets, `[T]` (Go, Scala)**: no `<` rule needed. But array types and
  indexing already use brackets in Sisu, so `Stack[i64]` would read as an
  index and `[Stack[i64]]` as nested brackets.
- **Angle brackets with a rule for `<` (Swift, C#, Rust, Kotlin, Java)**:
  readers already know the shape. A comparison and a type-argument list look
  alike, and the parser must decide with the `<` rule. Rust avoids the
  question by requiring `::<` in an expression; Sisu follows Swift and C#
  instead, since explicit arguments are rare and inference covers most calls.
- **Checking each instantiation (C++ templates)**: a generic body needs no
  bounds, but a call that compiles can fail deep inside the body, with an
  error about code the caller did not write. Sisu checks the definition
  against its bounds, as Rust and Swift do, so an error points at the call
  or at the definition, never into an instantiation.
- **Erasure (Java, Kotlin)**: one compiled copy serves every type, so code
  is smaller. But on the JVM, erasure boxes primitive values (an `int`
  becomes an `Integer`) and calls through the object, which monomorphization
  avoids.
- **Associated types only (Rust's `Iterator`, Swift's protocols)**: one
  answer per conforming type, as for `Source.Item`. It cannot say that
  `Celsius` converts to two types. **Type parameters only (Java's
  `Comparable<T>`)**: say that, but `Source<T>` then lets one type be a
  `Source` of two item types, so `for x in it` could not know the type of
  `x`. Sisu allows both.

## Consequences

- A program's compiled size and compile time grow with the number of
  instantiations it reaches. Separate compilation (milestone 11) must decide
  where an instantiation lives.
- A recursion that would instantiate infinitely many copies, such as
  `fn f<T>(x: T) { f([x]) }`, is an error naming the cycle, found by the
  checker before codegen.
- A generic definition's bounds are part of its signature. Adding a bound
  later breaks the callers that do not meet it.
- `where` clauses are not part of this milestone. A bound goes in the angle
  brackets, and an associated type may carry one (`type Item: Display`), so
  adding `where` later breaks no program.
