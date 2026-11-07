# A large value stays in memory

Status: accepted, 2026-11-07. It changes how codegen passes a value that
[ADR 0029](0029-parameters-are-borrowed-unless-consumed.md) passes. It does not
change who owns the value.

An enum is one LLVM aggregate, `{ tag, [n x iA] }`. Before this ADR, codegen
copied it as one value at each `let`, parameter, return and element. LLVM splits
such a value into one value for each element. A variant of 10 000 `i64` fields,
or an enum of 683 KiB, made `sisu build -O2` take gigabytes or crash. Instruction
selection crashes at 65 536 elements.

The rule has three parts:

- **A value of an LLVM struct type over 256 bytes stays in memory.** In codegen,
  the value is a pointer to a stack slot. `LARGEST_LLVM_VALUE` holds the limit,
  and `Codegen::by_memory` holds the rule. Copies are `memcpy` calls.
- **A parameter over the limit is a pointer to the caller's memory.** The callee
  only reads through it. A caller copies an array element or a field to a slot
  first, so no write can reach the callee during the call.
- **A result over the limit goes to a pointer that the caller gives.** It is the
  last parameter, and the function returns nothing. A table entry of an `any`
  value (`DynCall`) follows the same convention.

The ownership rules of ADR 0029 do not change. Counts move as they did before.

## Why 256 bytes

Measured with an enum of `i64` fields at `-O2`. "Loop" reads and writes array
elements through functions that inline. "Recursion" passes the enum through a
recursion that does not inline. Each column gives the time in memory, divided by
the time as an LLVM value.

| Bytes | Loop | Recursion |
|-------|------|-----------|
| 16 | 1.00 | 0.98 |
| 24 | 1.00 | 1.01 |
| 32 | 1.21 | 3.60 |
| 64 | 1.11 | 3.13 |
| 136 | 0.69 | 1.62 |
| 200 | 0.52 | 1.20 |
| 264 | 0.58 | 1.02 |
| 392 | 0.70 | 0.85 |
| 1032 | 0.66 | 0.80 |
| 4104 | 0.70 | 0.58 |

Near 256 bytes, memory is no slower in either shape, within the noise of the
runs. A larger value gains nothing at
run time as an LLVM value, and its build time grows with its size. In a program of
300 functions that pass and store the enum, an LLVM value takes this long, at
`-O2`, compared with memory: 2.5 times at 256 bytes, 11 times at 1 KiB, and 25
times at 2 KiB.

The limit cannot go under 24 bytes today, because codegen reads the words of a
string or an array from an LLVM value.

Structs can share this rule when they land
([ADR 0046](0046-structs-replace-classes.md)), because the rule reads the LLVM
type and not the Sisu type.

## Considered options

- **A limit of a few words.** The lowest limit that works, 24 bytes, makes an enum
  of 32 to 136 bytes 1.6 to 3.9 times slower in a recursion.
- **64 bytes, one cache line.** The loop shape crosses over near it. The recursion
  shape does not.
- **Boxing a large payload**: rejected. Each value would allocate, and a program
  could not hold a large value on the stack.

## Consequences

- Each program whose values are all 256 bytes or less gets the same LLVM IR as
  before. All programs of `bench/` do.
- A value stays on the stack, as it did before. A value near the size of the stack
  still overflows it at run time. The program then stops with `SIGSEGV` (ADR 0027).
  An enum of 2.7 MiB overflows the 8 MiB stack at `-O0`, where each temporary has
  its own slot. An enum of 10.9 MiB overflows it at `-O2`.
- A stack frame can be large. Valgrind reports false errors for a frame over 2
  MiB. So a run case holds an enum of 171 KiB. A larger one runs only in
  `tests/programs.rs`.
