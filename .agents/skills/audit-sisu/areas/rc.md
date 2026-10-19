# Area: reference counting

Every retain needs exactly one release on every path the program can take,
no object is touched after its count reaches 0, and an array shared by two
holders is copied before either writes it. Clean memory tools prove only the
first two, and only for the paths that ran.

## Where the counts come from

- `crates/sisuc/src/codegen/rc.rs`: the helpers (`retain`, `release.<T>`, `drop.<T>`, `eq.<T>`, `unique.<T>`). Equality helpers borrow their operands: they neither retain nor release.
- `crates/sisuc/src/mir/ownership.rs`: turns a counted local's last read into a move, which hands over its reference instead of retaining; `mir/lower.rs` writes the other counts as statements in source evaluation order. Every mismatch between that order and what `codegen/` emits is a leak, a use after free, or a missed copy. This is the first place to hunt.
- `crates/sisuc/src/codegen/mod.rs` and `codegen/array.rs`: where expressions retain, where scopes and assignments release, and copy-on-write (`unique`).

## Oracles

Every probe runs under all of these:

1. **Values.** Before the run, write the output the program must print, worked out from the value semantics in ADR 0007 and the README. Each probe prints what it reads back: after `let b = a` and `b[0] = 9`, print `a[0]` too. A mismatch is `wrong`, however clean memory is.
2. **Memory.** Valgrind on the plain and the `-O2` build, ASan on the plain build and with `asan,default<O2>`, and the layout shim (toolbox, "Memory").
3. **Counts.** `--rc-checks` plain and `-O2`; exit 102 is a finding.
4. **Allocations.** Work out how many blocks the program must allocate, and compare with Valgrind's `total heap usage: N allocs` (the runtime and libc add a few; measure an empty `main` for the baseline). More blocks than needed is `slow`; fewer is a lead for a `wrong` sharing probe.
5. **Lifetime.** A leak that frees at exit passes the four above. Free in a loop: build and drop the same structure 10 000 times, and compare Valgrind's peak with 10 iterations' (`valgrind --tool=massif`, or the allocation count with a counter in the program). Growth with the iteration count is `leak`.

Give each path its own program, with the branch chosen by a constant, so
one path's extra release cannot cancel another path's missing one.

## Probes

Build a matrix: each **value source** crossed with each **way out**, for
objects, arrays, arrays of objects, optionals of each, and nested fields.
Record each cell in `probes.md`.

Value sources: a constructor call `C(f: ...)`, a local, a parameter, a
field read, an array element, a `?.` chain, a `??` fallback, an `if` or
`if let` expression, a call's return value, an array literal or a
`[x; n]` fill.

Ways out: scope end, `return` from inside a loop, `break` and `continue`
holding a live counted local, reassignment of a `let mut` (is the old value
released?), field and element writes (is the old value released?), a
temporary that is never bound (`f(C(v: 1))`, `make().field`, `xs[0].f`), a
value passed to a parameter that the callee ignores or returns, `&&` and
`||` skipping a counted operand, a comparison chain stopping early, a panic
in the middle of an expression (exit 101 with no memory error before it).

Also probe:

- self-assignment: `x = x`, `o.f = o.f`, `xs[i] = xs[i]`, `a = b` when both name the same object;
- aliasing through a field: unlinking with `o.next = o.next?.next`, swapping two fields through a local;
- copy-on-write: a write to a shared array, to an array inside an object, to an array of arrays at depth 2 and 3, after `let b = a`, inside a `for x in xs` over the array being written, through a parameter, and after the sharing holder went out of scope (the copy should then be skipped);
- last use inside a loop body, inside one branch of an `if`, in a `while let` condition, and as two arguments of one call (`f(a, a)`);
- `==` on objects and arrays, and `is` on objects: the caller keeps both operands alive through the comparison and releases the ones it owns afterwards;
- methods: a method returning `self` or a field of `self`, and a call on a temporary (`make().m()`);
- a long list dropped in a loop, against a chain that alternates classes (see known-findings).

## Not a finding

A reference cycle leaking; a program that panics leaving objects allocated
(see [known-findings.md](../known-findings.md)). Check that a "leak" has no
cycle before you report it.
