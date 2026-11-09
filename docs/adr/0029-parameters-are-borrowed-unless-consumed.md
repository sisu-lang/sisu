# Parameters are borrowed unless the function consumes them

Status: accepted, 2026-10-21 (mir part 4, #112). It replaces ADR 0012, which
was proposed and never built.

Changed in part by [ADR 0046](0046-structs-replace-classes.md), when classes
go. The rule for a `weak` field goes, and a move-only value is never consumed
by inference. The rules that copy a `mut` field and an array element before a
call change: see "Still open" in ADR 0046.

Changed by [ADR 0043](0043-mut-parameters-copy-in-and-out.md). The caller
retains a borrowed argument before the copy-in of a `mut` argument, a local or
not. This holds when the value of the argument comes from a place that the `mut`
argument names, holds or is held by. The `mut` argument is one of the same call,
or of a call nested in a later argument. So `f(xs, mut xs)` gives `f` the old
`xs` in its first parameter, whatever `f` writes to its second.

Codegen passes a value over 256 bytes through a pointer, and returns it through
a pointer that the caller gives
([ADR 0048](0048-a-large-value-stays-in-memory.md)). Who owns the value does not
change.

A counted parameter is borrowed when the function only reads it: the caller
holds its reference for the whole call, and neither side counts it. A
parameter is owned when the function consumes it, which is any of these uses:

- storing it in a field or an aggregate;
- returning it;
- passing it to an owned parameter, or capturing it;
- assigning it to a named local, which covers ADR 0007's handover to a
  `let mut`;
- selecting it into the result of an `if`, `match` or `??`: a destination
  that is assigned in more than one statement is a join, and the assignment
  consumes the parameter. A temporary that is assigned once and only reads the
  parameter, such as the operand of `print(s)`, consumes nothing when it is
  assigned and used in one block; a temporary used in another block, such as
  an argument that a `break` in a later argument makes the call wait for,
  consumes it;
- consuming a binding or a projection borrowed out of it, such as returning
  the binding of an `if let` on it (the backward rule).

One more rule runs from the caller to the callee, the tail rule. Within one
recursive group, an owned argument to a tail call makes the callee's
parameter owned. Otherwise the caller would release the argument after the
call and lose the tail call. The argument is owned when it is a fresh value,
a local the caller owns, or a parameter the caller owns. A borrowed binding
rooted in a parameter makes the callee's parameter owned only if that
parameter turns owned, and then the binding becomes an owned copy.

The parameters of `main` stay owned, because the C entry point passes an
array and releases nothing after the call.

The compiler infers this on mir, across calls, in one search over the whole
program. The result is what solving Sisu's own constraints one strongly
connected component at a time, callees first, would give, as Lean 4 and Roc
solve theirs. The tail rule is Sisu's extension: Lean's `preserveTailCall`
(lean4@a6f30730 `InferBorrow.lean:408-415`) applies to self-calls only, and
Sisu applies it across the recursive group.

A parameter that is owned takes an owned reference, copied or moved as
today. For a borrowed parameter, the caller decides what to pass from where
the argument comes:

- **A local, or a field without `mut` of a borrowed place**: passed as it is.
  If the call is the local's last use, the caller moves it into a temporary
  and releases that after the call.
  The callee cannot reach the caller's locals, and a field without `mut`
  never changes.
- **A `mut` field, a weak field or an array element**: copied before the call
  and released after it, as ADR 0012 said. The call could overwrite that
  place and free the value.
- **Any other argument**: the caller owns it already and releases it after the
  call.

A `match` or `if let` on a borrowed place borrows its scrutinee for the arm,
and its bindings stay borrowed.

## Considered options

- **Borrow only `self` (ADR 0012).** It leaves `eval`'s parameter and every
  read-only argument counted.
- **Borrow every parameter (Swift's default).** A constructor or setter adds 1
  to each argument it keeps, where an owned argument is stored as it is.
- **Only remove pairs (part 3).** A pair that is cancelled inside one function
  cannot reach across a call.

## Consequences

- A release moves from the callee to the caller.
- ADR 0021's hoist stays sound. A copy of a borrowed parameter that outlives
  the call is a consuming use, so it makes the parameter owned. Every copy a
  callee makes of a borrowed parameter is released before the callee returns,
  and the block's count is what it was when the call began.
- The certifier starts a borrowed parameter borrowed. Its owner is the caller.
- Interpolation still makes a parameter owned. Narrowing that waits for a
  benchmark that shows the cost. `contains` and `sum` are calls of prelude
  functions, which loop over `self` with `for`, so their `self` is borrowed.
  The `x` of `contains` is inferred for each instance: it is borrowed when the
  `==` of the element type is generated, which reads both operands in place,
  and owned when the written `eq` stores its argument.
- In Sisu, the tail rule can make a caller's call a tail call, but it never
  bounds the stack of a recursion until releases move to a local's last use.
- A function called through a closure keeps owned parameters (milestone 9).
- A call through the table of an `any` value (`DynCall` in mir) has a fixed convention in place of
  an inferred one, since no single callee stands behind it. The receiver is borrowed: the caller
  keeps the `any` value, and a receiver that it reads from a `mut` field or from an element is
  copied first, as for any borrowed argument there. Every other argument is owned. The entry of
  the table adapts to the concrete method's own flags: it copies the receiver when the method
  owns `self`, passes on each argument that the method owns, and releases each argument that the
  method only borrows, after the call, because the caller of a borrowing method holds its
  arguments for the whole call.

## Amendment, 2026-11-09: interpolation reads its parts

An interpolation no longer consumes its parts (#625). It writes each part into a new string and
keeps none of them, so lowering reads the parts as `print` reads its operand. A counted part is a
borrow site, or a temporary that is released after the statement. A part that is a whole local is
read in place. The exception is a later part of the same interpolation that mentions that local
other than as a whole read. The certifier reads the parts as operands. So a parameter that a
function only interpolates is now borrowed. The consequence "Interpolation still makes a parameter
owned" no longer holds. The benchmark: one `string` local interpolated 800 times took 2,525M
instructions to build at `-O2`, and 345M with this change.
