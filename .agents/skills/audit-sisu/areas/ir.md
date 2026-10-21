# Area: IR quality

The module codegen hands LLVM must be sound (valid, free of undefined
behavior) before it is fast. Unsound IR may look fine at `-O0` and break at
`-O2`, or under the next LLVM.

## Soundness

For every case in `crates/sisuc/tests/run` and `run-fail`, and for your own
probes, run the commands in the toolbox, "IR":

- `build --emit ir-raw` exits 1 for a program `check` accepts: the verifier rejected codegen's module. That is `crash`.
- Lint flags an operation: a lead. It becomes `unsound` when you show a Sisu program reaches that block. A hit only in a block behind a constant branch (`br i1 false`) is `slow` at most: codegen emits dead code. Lint after `simplifycfg` removes most of those.

Every flag is a promise LLVM may exploit: `nsw`, `nuw`, `exact`,
`inbounds`, `nonnull`, `noalias`, `dereferenceable`, `noundef`,
`readonly`, `willreturn`, `nounwind` and `noreturn`. Check them two ways:

- **Differential.** Strip `nuw`, `nsw`, `exact` and `inbounds` from the raw IR with `sed`, then build both versions with `$L/opt -passes='default<O2>'` and `$L/clang` (linked with the runtime archive) and run them. Different output or exit is `wrong`: a flag was false.
- **Edges.** Find where codegen adds each flag and feed it the value at its edge. For example, `codegen/array.rs` multiplies an array's length by the element size with `nuw`: give `[x; n]` an `n` near `i64::MAX / size`.

Also read the raw IR for:

- loads and stores whose type or alignment differs from what was allocated, and array headers read at offsets other than the ones `TargetData` gives;
- `undef` or `poison` reaching a branch, a call argument or a store;
- runtime declarations against the Rust signatures in `crates/runtime/src/lib.rs` (`runtime` covers the ABI in depth).

The `--rc-checks` code cannot be emitted as IR (toolbox, "IR"); `rc` runs
it. Report that as a `gap` only once.

## Cost

Compare the `-O2` output of a probe with what it should cost:

- retain and release pairs on the same pointer that survive `-O2` with nothing in between that could free it;
- overflow and bounds checks inside a loop that `-O2` could not hoist or remove (the remarks command in the toolbox shows why);
- a loop that does not vectorize when the same loop in C does. The C must keep Sisu's semantics: the same overflow checks (`__builtin_add_overflow`), bounds checks, and panic order. Compile it with `$L/clang -O2 -S -emit-llvm`;
- missing attributes that would let LLVM do more: `sisu_alloc` as an allocator (`allockind`, a `noalias` return), helpers as `nounwind`, cold panic paths not marked `cold`;
- a helper called where an inline sequence would do, or one emitted per type that could be shared.

Start from `fib.sisu`, a prime sieve over a `[bool]`, nested loops summing
a `[[i64]]`, building and walking a linked list, and the `-O2` build of
each case in `tests/run`.

A `slow` finding gives the instruction count (callgrind, toolbox "IR")
before and after the suggested fix. Measure the fix by editing the raw IR
by hand and building it with `opt` and `clang`.
