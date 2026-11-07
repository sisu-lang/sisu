# Deep show and equality grow the stack

Status: accepted, 2026-11-07. It adds to
[ADR 0038](0038-deep-drops-defer-past-a-depth-limit.md), which still sets the
rule for drops.

`print`, `to_string()`, an interpolation, `dbg`, `==`, `!=` and `contains` use
a helper for each type. A chain through an array or an `any` box calls the
helpers once for each link. Before this ADR, a chain of 100 000 links
overflowed the 8 MiB stack, and the program stopped with signal 11 and printed
nothing. A chain of 20 000 links through an array did this under `print`.

An enum component already uses a work list on the heap, so its helpers do not
recurse. Only two kinds of helper can recurse with no bound:

- the show and `==` helpers of an array, and
- the show glue of an `any` box.

## Decision

A helper of these kinds runs through the runtime's `sisu_stack_grow` in one
case: the type of its element, or of the boxed value, is a *recursing part*
(ADR 0038). That function calls `stacker::maybe_grow`. It runs the helper on the
current stack when at least 100 KiB is left, and otherwise on a new segment of
1 MiB.

Other helpers do not change. A leaf array such as `[i64]` or `[string]` gets no
check, and a program with no recursive type has no call to
`sisu_stack_grow`. The compiler uses the same crate and the same sizes for its
own walks over a type.

The generated helper keeps its name and its parameters. It stores its
parameters in a struct on the stack and gives a thunk and the struct to
`sisu_stack_grow`. The thunk loads the parameters and calls `<helper>.body`.

## Why

Output order must follow the structure of the value, and `==` must stop at the
first difference. A drop has no such rule past its depth limit, so ADR 0038 can
defer a block to a queue. A show or `==` helper cannot defer its work and
return. It must finish a child before it writes the next text or returns a
result.

## Considered options

- **Grow the stack (chosen).** It needs one runtime function and one wrapper in
  codegen, and each helper keeps its form. It covers each path through an array
  or a box, and each kind that a recursing part gets later.
- **A work list on the heap for each helper**, as enum components do: rejected.
  A helper must then save its position in each array and box, and each helper
  gets a new form. It uses memory on the heap for the same depth too.
- **A limit with an error message**: rejected. A valid program would stop on a
  value that fits in memory.
- **Leave it**: rejected. A valid program crashes with no message.

## Consequences

- Memory grows with the depth of the value, not with its size. The deep test
  programs use about 1 MiB for each few thousand links. The memory goes back to
  the system when the helper returns.
- A helper on the path pays for a stack check at each array or box. It pays no
  cost for each element of a leaf array.
- `stacker` and `psm` are now in each compiled program, because the runtime is
  one object in its archive. On Linux x86-64, a stripped `hello` grows by 12 KiB
  and runs about 5 000 more instructions at startup.
- `stacker` needs a known stack limit, or it grows on each call. It reads the
  limit once for each thread.
- A value with very deep text still needs memory for that text. For example,
  `"{a:#}"` indents each line, so its output grows with the square of the depth.
