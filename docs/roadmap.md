# Roadmap

Sisu grows one milestone at a time. Each milestone ends with a program that
proves it works. Its design is settled in a working spec before work starts.
The specs are not published. The decisions they settled are listed below,
and the lasting ones are recorded as ADRs in [adr](adr).

## Milestones 1 to 7

These are adopted. Together they give Sisu a working imperative core, and
enums with `match`.

| # | Milestone | Done when | Status |
| --- | --- | --- | --- |
| 1 | Number crunching: `i64`, `bool`, functions, `if`, `while`, recursion | Fibonacci and a prime counter print correct answers | Done |
| 2 | Classes, methods, heap, reference counting, optionals with `?.` and `??`, `break` and `continue` | A linked list and a binary tree run clean under Valgrind | Done |
| 3 | Arrays and `u8` | A prime sieve prints correct answers | Done |
| 4 | Strings | A program reverses and compares strings | Done |
| 5 | Standard streams and arguments | A word-count tool reads standard input | Done |
| 6 | Floats and conversions: `f64`, conversions that can fail return an optional, an unused-value warning, format specs | A Mandelbrot set prints as text, and a projectile table prints with format specs and a conversion to `i64?`, all clean under Valgrind | Done |
| 7 | Enums with payloads, `match` with destructuring, alternatives, guards, literal and range patterns and an exhaustiveness check, `if let` and `while let` with enum patterns, and `..=` in `for` | An evaluator over a recursive `enum Expr` of `Num(i64)`, `Add` and `Mul` prints correct results, and a state machine runs with `match`, `if let` and `while let`, all clean under Valgrind; a non-exhaustive `match` is a compile error | Done |

### What works today

`sisuc` compiles:

- `i64` and `bool`, with `let` for immutable bindings, `let mut` for
  variables, and inferred types
- functions and recursion
- `if` as an expression, `while`, `break`, `continue` and `return`
- comparison chains such as `0 <= i < n`, and short-circuit `&&` and `||`
- integer arithmetic that panics on overflow and division by zero instead of
  wrapping
- classes with fields and methods, whose objects are freed by reference
  counting when the last reference goes away; a field is bare, set once by
  the constructor, or `mut`
- `==` that compares objects field by field, and `is` that tests identity
- optionals such as `i64?` and `Node?`, with `None`, `?.`, `??`, `if let` and
  `while let`
- enums, declared with `enum` and a list of variants, each with no payload,
  labeled fields (`Rect(w: i64, h: i64)`) or unlabeled ones (`Num(i64)`);
  `Expr.Num(2)` builds a value, an enum has methods and compares with `==`,
  and a recursive enum such as `Add(Expr, Expr)` works with no annotation,
  its recursive fields stored behind counted boxes; enum values never change
- `match` on an enum, `i64`, `u8`, `bool`, `char` or `string`, as an
  expression or a statement, with one arm per line; a pattern is a variant,
  bare or through its enum (`Num(n)`, `Expr.Num(n)`), whose sub-patterns
  nest, or a name that binds, or `_`; a pattern is also a literal (`0`, `'a'`,
  `"x"`, `true`) on `i64`, `u8`, `char`, `bool` and `string`,
  or a range (`'A'..='Z'`, `0..10`) on `i64`, `u8` and `char`, while at a
  class, array or optional position it is only a name or `_`; patterns may
  be alternatives
  (`Add(Num(0), x) | Add(x, Num(0))`), and an arm may add a guard,
  `n if n > 0 =>`; a `match` that misses a value does not compile and names
  one it misses, such as `Mul(_, _)`, and an arm that no value reaches warns
  (`W0006`); `if let` and `while let` take the same patterns on an enum
- `f64`, written `2.5`, `1e-3` or `3.0e8`, with IEEE 754 arithmetic and
  comparisons (`nan` is not equal to itself, and `-0.0` equals `0.0`);
  `i64(x)`, an `i64?` that is `None` unless `x` is a whole number in range,
  and `f64(n)`; `[f64]` with `sum`, `==` and `contains`; `f64` in constants
  and interpolation; methods such as `sqrt`, `pow`, `sin`, `is_nan` and
  `hypot` on `f64`, `min`, `max` and `clamp` on `f64` and `i64`, and the
  constants `f64.pi`, `f64.tau`, `f64.e`, `f64.infinity`, `f64.nan`,
  `f64.max`, `f64.min_positive` and `f64.epsilon`; `print` and
  interpolation show an `f64` as the shortest text that reads back the same
  (`1.0`, `1e16`, `inf`, `nan`)
- format specs in interpolation, `{x:>8.2}`: an alignment (`<`, `>`, `^`), a
  width in code points, and a precision of decimals on an `f64`; a spec is
  `[align][width][.precision]`, and the interpolation still allocates once
- `u8`, a byte whose arithmetic panics on overflow like `i64`'s, and the
  conversions `i64(...)` and `u8(n)`, which is a `u8?` that is `None` when
  `n` does not fit, with no panic
- arrays such as `[i64]` and `[[bool]]`, built with `[1, 2, 3]` or `[0; n]`,
  read and written by index, compared with `==`, and walked with `for x in xs`
  or `for i in 0..n` (`..=` includes the end); `len`, `sum` and `contains` are methods on them
- `char`, one Unicode scalar value such as `'ä'`, compared by code point and
  converted with `i64(c)` and `char(n)`, a `char?` that is `None` for a
  surrogate or a value above `0x10FFFF`, with no panic
- `string`: UTF-8 literals with escapes, `+` and `+=`, comparison by bytes,
  `len()` in bytes, and interpolation such as `"n = {n}"`; `+=` on a string
  that nothing else holds appends in place, whether it is a local, a `mut`
  field or an array element
- text as arrays: `s.chars()` gives a `[char]`, `s.bytes()` a `[u8]`, and
  `string(cs)` builds a `string` from a `[char]`; each call builds a new value
- `eprint(x)`, which prints like `print` but to standard error; `read_line()`,
  a `string?` that is the next line of standard input without its line break,
  or `None` at the end of the input; and `s.words()`, a `[string]` of the runs
  between white space
- `panic("why")`, which stops the program with a message, and the keyword
  `todo`, which does the same and warns at compile time; either may stand on
  the right of `??`, as in `name ?? panic("no name")`
- copy-on-write arrays: `let b = a` shares until one of them is written, or
  until a loop that writes one of them starts (ADR 0021), and a counted
  local's last read moves it instead of copying
- `let _ = e` and `for _ in ...`, which drop a value without a name;
  shadowing in the same scope; `0x`, `0b` and `0o` integer literals; block
  expressions such as `let a = { ... }`; and top-level `const`
- a warning, `W0004`, for an expression statement whose value nothing uses,
  such as a bare `u8(n)` or `twice(21)`; `let _ = e` drops the value on
  purpose
- error messages in the style of `rustc`, pointing at the code, each with a
  stable code that `sisuc --explain` describes, a JSON form, and fixes where
  the checker can offer one

### Diagnostics foundation

Before the rest of milestone 4, every diagnostic gets a stable code, one
registry for its wording, a tested explanation and a JSON form for tools
([ADR 0013](adr/0013-diagnostics-have-stable-codes.md)). Milestones keep
their numbers. Milestone 4's strings are built on top of it, and every later
milestone adds its codes the same way.

### Decided in milestone 1

These came out of the review of the milestone 1 spec and are now part of it:

- `let` immutable by default, `var` mutable (ADR 0002), with local type
  inference. Milestone 4 renamed `var` to `let mut` (ADR 0011).
- `if` is an expression.
- No semicolons; statements end at newlines (ADR 0001).
- One built-in `print(e)` for `i64` and `bool` instead of `print_int`.
- Compound assignment `+= -= *= /= %=`, desugared by the checker (moved from
  the parser in milestone 2).
- Digit separators: `100_000`.
- Diagnostics in the style of rustc: the code underlined, secondary labels
  such as "declared with `let` here", and a `help:` line.
- `break` and `continue` scheduled for milestone 2.
- From milestone 2, a line starting with `.` continues the previous line, so
  method chains can be written one call per line (ADR 0001).

### Decided in milestone 2

These came out of the milestone 2 design and are now part of its spec:

- Optionals for every type, `i64?` and `bool?` included, with `?.`, `??` and
  `None` as the empty value. `None` becomes the `Option<T>` case in
  milestone 8.
- `if let` and `while let` for optionals, brought forward from milestone 7.
- `==` compares classes structurally; `a is b` compares identity. Reversed in
  milestone 8: classes compare by identity
  ([ADR 0022](adr/0022-classes-compare-by-identity.md)).
- The checker reports every error in one run, using a poison type.
- The checker builds a typed, desugared tree (`tir`) and codegen reads only
  that.
- Reference counting (ADR 0005).

### Decided in milestone 3

- Sisu has no comprehensions. Milestone 9's iterator methods are lazy:
  `.filter(...).map(...).sum()` builds no intermediate array. A
  `filter_some`-style method on `[T?]`, which keeps the present elements,
  comes with them.

### Decided in milestone 4

- A `string` is always valid UTF-8, a value shared until written, with `len()`
  in bytes and no `s[i]` ([ADR 0014](adr/0014-strings-are-utf-8-values.md)).
  Text is taken apart with `chars()` and `bytes()`, and `string(cs)` builds a
  string back from a `[char]`. A `char` is one code point, so grapheme
  clusters such as `e` plus a combining accent reverse wrongly. That is
  accepted.
- `<`, `<=`, `>` and `>=` compare strings by bytes, which for valid UTF-8 is
  code-point order: `"Zoe" < "anna"` and `"z" < "ä"`. There is no Unicode
  normalisation and no case folding.
- `+=` on a string appends in place when nothing else holds it, on a local, a
  field or an array element. `s = s + t` and `names[i] = names[i] + t` append
  alike, and a prepend (`"x" + s`) always copies. There is no builder type:
  `+=` on a string held once is the builder.
- Interpolation, `"n = {n}"`, is the only way to build text from numbers and
  `bool`; a `char` also goes through `string([c])`. `string + char` and
  `string + i64` are errors that point to interpolation.
- `let mut` replaces `var` (ADR 0011); `let _` and `for _` drop a value
  without a name; same-scope shadowing is allowed; `0x`, `0b` and `0o`
  literals; block expressions; top-level `const`; and a list of reserved
  keywords for later milestones.
- Left for later: text searching such as `split`, `trim` and `find` (milestone
  5 brings `words()` instead), format specifiers (milestone 6), printing
  classes, arrays and optionals (milestone 8), and building a `string` from
  `[u8]`, which needs UTF-8 validation and a failure path (milestone 5's input
  or milestone 12).

### Decided in milestone 5

- `fn main(args: [string])` is the one way to get the program's arguments:
  the program name is not in `args`, and each argument is repaired to valid
  UTF-8 as `read_line` repairs a line. `main` takes no `argc`, `argv` or
  return value; a chosen exit code comes from a function that never returns.
- `read_line()` removes `\n` and `\r\n` from the end of a line and keeps a
  lone `\r`. A last line with no line ending still comes back, and an empty
  line is `""`, not `None`. Each ill-formed UTF-8 sequence becomes one
  `U+FFFD`, as Rust's `String::from_utf8_lossy` does, so a `string` stays
  valid UTF-8. End of input is sticky.
- `words()` splits on Unicode `White_Space`, which includes the no-break
  space `U+00A0`, and drops empty pieces. Matching `wc -w` byte for byte
  waits for a locale story. `s.words().len()` counts the words without
  building the `[string]`, and the word functions trust that a string is
  valid UTF-8 in a release build
  ([ADR 0016](adr/0016-word-functions-trust-the-utf-8-invariant.md); added
  after milestone 5, for the `wc` benchmark).

### Decided in milestone 6

- Floats follow IEEE 754: `f64` arithmetic, methods and comparisons never
  panic, `nan` is not equal to itself, and `-0.0` equals `0.0`
  ([ADR 0017](adr/0017-floats-follow-ieee-754.md)).
- A conversion that can fail returns an optional, and none panics: `i64(x)`
  on an `f64` is `None` unless `x` is a whole number in range (`i64(2.7)` is
  `None`), `u8(n)` and `char(n)` are `u8?` and `char?`, and `f64(n)` rounds to
  the nearest `f64`
  ([ADR 0018](adr/0018-a-conversion-that-can-fail-returns-an-optional.md)).
- Methods and constants live on `f64` (`x.sqrt()`, `f64.pi`), not in a `math`
  module. `f64` prints as the shortest text that reads back the same, with
  `.0` on whole numbers. Exponent form starts at `1e16` and below `1e-4`:
  `1e15` prints `1000000000000000.0`, `1e16` prints `1e16`, `0.0001` prints
  `0.0001`, and `1e-5` prints `1e-5`. `libm`'s
  results are not correctly rounded, so tests print them through a precision.
- A bare expression statement whose value nothing uses warns (`W0004`);
  `let _ = e` drops a value on purpose.
- A format spec is `[align][width][.precision]`. Width counts code points and
  never cuts; a precision rounds half to even from the exact binary value and
  never uses exponent form.
- The proof programs are `mandelbrot.sisu` and `projectile.sisu` in
  `crates/sisuc/tests/run/`.

### Decided in milestone 7

- An enum is a value: a tag plus the payload of its variant, stored inline.
  `Expr.Num(2)` builds one, a pattern uses the bare name (`Num(n)`), and a
  payload field whose enum type leads back to the declaring enum is stored
  behind a counted box the compiler adds, so a recursive enum needs no
  annotation. A box that reaches count zero is freed from a pending list,
  not by recursion, so deep values free in constant stack. Enum values never
  change in place
  ([ADR 0019](adr/0019-enums-are-values-named-through-their-enum.md)).
- A variant labels all its payload fields or none, the labels are written in
  the declaration, a call gives every label in declared order, and a pattern
  binds by position
  ([ADR 0020](adr/0020-labels-belong-to-the-declaration.md)).
- `match` owns its scrutinee and moves nothing out of it: arms are tested in
  order, straight into basic blocks, with backtracking for alternatives and
  guards, so there is no `mir`. A part the scrutinee held alone is counted up
  and down once; reusing a box waits for a benchmark.
- `match` is checked for exhaustiveness and reachability in one pass. A
  guarded arm may not match, so it never covers a value. `i64`, `u8` and
  `char` are finite domains, so ranges can cover them. A missing value is an
  error that names one; an arm no value reaches warns (`W0006`).
- A range pattern has Rust's two forms: `a..b` leaves out its end, as `for`
  does, and `a..=b` includes it. `for` gains `..=` too.
- Alternatives bind the same names with the same types, and a guard runs once
  per alternative tried, so its side effects repeat.
- `Option`, `Result`, `Some`, `None`, `Ok` and `Err` are reserved, so
  milestone 8 can make them bare prelude names without breaking a program.
- `if let x = opt` on a `T?` is an implicit `Some`: only a name or `_` may
  stand there until milestone 8 settles the `Some` and `None` pattern rule.
- Order (`<`) on an enum is an error, as on a class.
- The proof programs are `expr.sisu` and `state.sisu` in
  `crates/sisuc/tests/run/`.

### What panics

After milestone 7, these are all the ways a Sisu program panics, each with
how to avoid it. Each milestone keeps the table current.

| Operation | Panics when | Without the panic |
| --- | --- | --- |
| `+ - *`, unary `-` on `i64` or `u8` | the result overflows | check the range first; checked arithmetic is a gap |
| `/` and `%` on `i64` or `u8` | the divisor is zero, or `i64` minimum divided by `-1` | check the divisor first |
| `xs[i]`, `xs[i] = v` | `i` is outside the array | check `0 <= i < xs.len()`; an optional `get(i)` is a gap |
| `[v; n]` | `n` is negative, or the array would be too large to allocate (`[0; 9223372036854775807]`) | check `n` first, and keep it to a size the program can use |
| `xs.sum()` on `[i64]` | the sum overflows | gap |
| `panic(msg)`, `todo` | always | they are the deliberate panics |
| `print`, `eprint`, `read_line` | the write or read fails | none until `Result` (milestone 12) |
| Conversions | never | they return an optional |
| `f64` arithmetic, methods, comparisons | never | IEEE 754 ([ADR 0017](adr/0017-floats-follow-ieee-754.md)) |
| `x.clamp`, `x.min`, `x.max` | never | |

Out-of-memory aborts and stack overflow are not panics.

## Beyond milestone 6

Status: proposal, 2026-10-09, revised 2026-10-11 after a review of the
milestone order by gpt-6.1-sol. Nothing from here on is decided until it lands
in a milestone spec or an ADR.

The rest of this document collects what comes after milestone 6, and the
ergonomic features that should be folded into milestones 2 to 6 while they
are still cheap.

## Guiding goal: ergonomic and readable

Sisu should be pleasant to write and easy to read. The roadmap is ordered by
what makes everyday code nicer, not by compiler-theory depth. Features that
only help the compiler (debug info, an optimizing IR, concurrency) wait until
the language itself reads well.

When values conflict, ADR 0006 orders them: correctness, then simplicity,
then readability, then performance.

Principles, recorded with the language mix in ADR 0004 so every later syntax
choice is checked against them:

- Code reads top to bottom, without punctuation noise.
- One obvious way to do a thing.
- Errors speak in plain words and point at the code.
- Memory is managed for you (reference counting, ADR 0005).

Sisu takes its syntax's readability from Python, its correctness from Rust,
and its memory model and everyday ergonomics from Swift (ADR 0004).

### Target sample

Every later milestone should move real code toward this:

```
import os

enum Shape {
    Circle(radius: f64)
    Rect(w: f64, h: f64)
}

fn area(s: Shape) -> f64 {
    match s {
        Circle(r)  => 3.14159 * r * r
        Rect(w, h) => w * h
    }
}

fn main() {
    let shapes = [Shape.Circle(radius: 1.0), Shape.Rect(w: 2.0, h: 3.0)]
    let mut total = 0.0

    for s in shapes {
        total += area(s)
    }

    print("Total area: {total}")

    let name = os.env("USER") ?? "stranger"
    print("Hello, {name}!")
}
```

The sample builds a variant through its enum, with every label, and matches
it by the bare name with positional bindings, as milestone 7 decided
([ADR 0019](adr/0019-enums-are-values-named-through-their-enum.md),
[ADR 0020](adr/0020-labels-belong-to-the-declaration.md)).

## Fold into milestones 2 to 6

Each item is cheap when its milestone lands and expensive to retrofit:

| Milestone | Add | Why |
| --- | --- | --- |
| 4 | Reserve the keywords that later milestones bring: `import` and `pub` (planned for milestone 2, it did not land), `enum`, `match`, `record`, `type`, `weak`, `const`, `test` and `todo`. Landed in milestone 4, stage 1 (`E0083`); it also reserves `echo`, `interface`, `extern` and `as`, and `const` became a keyword with top-level constants | A program that names a variable `match` would break when that keyword lands |
| 4 | `let mut` replaces `var`; a class field is bare, set once by the constructor, or `mut` (ADR 0011). `var` stays reserved, so using it is error `E0013`, "there is no `var` in Sisu", with the help "Sisu declares a variable with `let mut`" (on a field, "a field that can change is marked `mut`"). | Every program and test changes, so it is cheapest before strings and enums add more code |
| 4 | Top-level `const NAME = expr`, where `expr` uses only literals, operators and other constants; the checker folds it to a value. Planned for milestone 3, it did not land. | Named limits, such as a sieve size, without global variables |
| 4 | String interpolation `"x = {x}"`. `print(x)` and `"{x}"` go through one internal operation per type that turns a value into text. Milestone 8's `Display` interface replaces that operation, so neither is rewritten. | The biggest single win for real programs |
| 4 | `_` as a pattern that binds nothing: `for _ in 0..n`, `let _ = f()`. Milestone 7's `match` uses it as the catch-all `_ =>`. Landed in milestone 4, stage 1: `_` binds nothing, so a second `let _` in one scope is fine and `_` cannot be read. | A loop or a call whose value is not needed takes no made-up name |
| 4 | Integer literals in hexadecimal, binary and octal: `0xFF`, `0b1010`, `0o17`, with digit separators | Bytes and bit masks, which `u8` brings in milestone 3, read as what they are |
| 4 | A block as an expression: `let x = {`, then statements on their own lines, then `}`. Its value is the last expression, and its names end at the `}`. Parentheses still group operators. | Scopes a temporary without a helper function, and reference counting releases it at the `}`, not at the end of the function. `if` blocks already return values, so the checker mostly supports it |
| 5 | `panic("msg")` and `todo`, as expressions that never return. `todo` also warns at compile time. Both report the file and line, and exit with code 101 like other panics. Landed in milestone 5, stage 1. | `read_line() ?? panic("no input")` handles a missing value in one line until `Result` arrives in milestone 12. `todo` marks unwritten code that still compiles |
| 6 | Float literals in scientific notation: `3.0e-4`. Landed in milestone 6. | Physics and graphics constants read as written |

### Portability rules

`sisuc` already compiles for the machine it runs on: codegen asks LLVM for the
host triple and takes the data layout from it. Two rules keep it that way, so
that new targets stay cheap:

- From milestone 2, take sizes and alignments from the target data layout.
  Never write a pointer size as `8`.
- Runtime functions take and return only integers, `bool`, floats and
  pointers, never structs by value. How a struct is passed differs between
  x86-64, ARM64 and Windows. Clang applies those rules for C, but LLVM does
  not, so `sisuc` would have to implement them for each target.
  `sisu_panic(msg, len)` already follows this rule. The runtime never builds
  or reads a Sisu object either; text crosses as a pointer and a length
  (ADR 0009).

## Early tooling

These help you learn from the compiler and read Sisu code. Each one is small,
so it does not wait for the later goals:

- One `sisu` binary with subcommands, as with the `go` and `zig` commands.
  One binary means the compiler and the build tool can never be different
  versions, unlike Cargo and `rustc`. Planned for milestone 2, it did not
  land and is not scheduled yet. It renames `sisuc` to `sisu` and adds two
  subcommands:
  - `sisu build` does what `sisuc` does today, with the same flags.
  - `sisu run file.sisu` compiles to a temporary executable and runs it.

  Later subcommands are `sisu check` (type checking only), `sisu fmt`,
  `sisu test` and `sisu lsp`. No manifest at first: a single file, or a
  directory whose entry point is `main.sisu`, builds without one. A `sisu.toml`
  arrives only when a project needs dependencies.
- A `-O2` flag, added in milestone 2. It runs LLVM's `default<O2>` pass
  pipeline in place of `mem2reg`, both for `--emit ir` and for the
  executable. Without the flag, only `mem2reg` runs, as in milestone 1.
  Comparing the IR with and without the flag shows what the optimizer
  removes.

  `-O3` takes the same place and runs `default<O3>`. It adds non-trivial
  loop unswitching, argument promotion, call-site splitting and a higher
  inlining threshold to O2; vectorization already runs at O2. The extra
  passes cost compile time and code size and often gain nothing, so the
  tests and benchmarks stay at `-O2`.
- `--emit ir-raw`, added in milestone 2. It prints one module before any pass
  runs, so the output is a valid `.ll` file for LLVM's `opt` tool. (`--emit ir`
  prints two modules, before and after the pass pipeline.) With `opt` you can watch
  the optimizer one pass at a time, with no further compiler code:
  - `opt -S -passes='mem2reg,simplifycfg,instcombine' x.ll` runs the named
    passes in order.
  - `--print-changed` prints the IR after each pass that changed it.
  - `-passes=dot-cfg` writes each function's control-flow graph as a `.dot`
    file for Graphviz.
  - `--opt-bisect-limit=N` runs only the first N passes, to find the pass
    that breaks a program.

  `opt` must be version 22, because an older `opt` may not parse the IR.
  `scripts/install-llvm.sh` does not keep `bin/opt`. Decide whether it should,
  checking the GitHub Actions cache limit first.
- A `--time` flag that prints how long each phase took: lexing, parsing,
  checking, codegen, LLVM's passes and linking. CI compiles a large generated
  `.sisu` file with it, so a change that slows the compiler shows up in the
  change that made it.
- A TextMate grammar for syntax highlighting in VS Code, from milestone 3,
  once classes and arrays fix most of the syntax. It is one JSON file and
  needs no compiler changes.
- ARM64 Linux and macOS on ARM64 as hosts. LLVM publishes `Linux-ARM64` and
  `macOS-ARM64` builds, so `scripts/install-llvm.sh` picks the download by
  `uname -s` and `uname -m`, and CI gets a job for each. The compiler should
  need no changes, but this is untested. On macOS, `cc` is Apple's clang, so
  tests that assume GNU tools or ELF files break. LLVM publishes no build for
  Intel Macs. Check which GitHub Actions runners the repository can use first.
- A `--leaks` build mode that finds reference cycles (ADR 0008). The runtime
  records each live allocation with the place it was made. When `main`
  returns, every object still alive has leaked. Codegen emits, only in this
  mode, a function per class that names the class and lists the objects its
  fields point at, and the runtime calls it to print each cycle:
  `Node` at `tree.sisu:14:16`, `.parent` back to the first `Node`. The runtime
  never reads an object's layout itself (ADR 0009), and a normal build pays
  nothing. Valgrind reports the lost bytes but not the class or the field.

## Fuzzing

The compiler is fuzzed in two stages:

1. Front-end crash fuzzing. Done. A libFuzzer target feeds arbitrary bytes to
   `sisuc --check` and fails on a panic, a stack overflow, a timeout or an
   out-of-memory, or on a malformed span or fix in the report. An input that
   checks clean is also compiled, and fails when LLVM rejects the module. CI runs it
   nightly; [development.md](development.md#fuzz-the-front-end) covers it.
2. Wrong-code oracles. Open. Generators of well-typed programs, run
   differentially (a build without `-O2` against one with it, for example),
   would catch a program that compiles but computes the wrong answer, which
   crash fuzzing cannot see.

## Proposed milestones 8 to 12

| # | Milestone | Done when |
| --- | --- | --- |
| 8 | Generics (monomorphization) and interfaces, with `print` generic through a `Display`-like interface; `Option` and `Result` join the prelude, with the bare names `Some`, `None`, `Ok` and `Err` in expressions and patterns, and on a `T?` a pattern whose outermost name is `Some` or `None` matches the optional itself while any other pattern matches the value inside it | `print(x)` works for any printable type; `T?` is shorthand for a library `Option<T>` |
| 9 | Closures and lazy iterator methods (`map`, `filter`, `count`, `sum`), plus a `filter_some`-style method on `[T?]` | Closures first: a function that takes an `(i64) -> bool` runs with a closure that captures a local. Then `words.filter(w => w.len() > 3).count()` runs |
| 10 | Records, named arguments, default parameters, and `p as name` to bind a whole sub-pattern | A `Point` record prints and compares with no hand-written methods |
| 11 | Modules, `pub` visibility, and a standard library written in Sisu (`Map`), including `os.env(name)`, `os.exit(code)`, and an `io` module with `read_lines()`, read-everything and `read_bytes()` | Modules first: a program of two files, each with a class of the same name, builds and runs. Then a word-frequency tool reads like a Python script |
| 12 | Error handling: `Result<T, E>` and a `?` operator | File I/O code has no nested error checks |

Order matters in these places:

- Milestone 7 comes before generics, because `Option` and `Result` are
  enums. Optionals still change twice: milestone 2 builds `T?` into the
  compiler, and milestone 8 turns it into shorthand for the library
  `Option<T>` (as in Swift). The syntax stays, so no program breaks. One
  thing must also stay: an optional class reference is a pointer that is
  null for `None` (see "Open decisions"). Optionals nest from milestone 8
  ([ADR 0024](adr/0024-optionals-nest.md)).
- Milestone 7 has both `..` and `..=` in `for` and in range patterns, as
  Rust does. `..` already leaves out its end in `for`, so Swift's `...` or
  Kotlin's inclusive `..` would change the meaning of existing loops.
- Milestone 8 comes before the standard library, which needs generic
  collections.
- Milestone 12 needs only enums and generics, so it can start as soon as
  milestone 8 is done, before closures and records. The standard library
  (milestone 11) then reports errors with `Result` from the start.
- Milestone 5 covers standard input, output and error, and the program's
  arguments. Their signatures never change: the
  end of input is `None`, not an error, and a failed read or write panics
  (ADR 0009). Opening, reading and writing files waits for `Result` in
  milestone 12, so no I/O signature changes later.

### Benchmarks keep up

A milestone that adds a feature with a run-time cost adds a program under
`bench/features/`, or extends one, and adds its row to "What CodSpeed
covers" in [Benchmarks](benchmarks.md#what-codspeed-covers).
Otherwise its spec says why the feature needs none.

Each milestone also adds its new syntax to the generator of the large input
in `scripts/codspeed-prep.sh`, so that `check_large` and the builds see it.
A change to the generator shows as a step in `check_large`,
`build_large_20` and `build_large`, and the pull request says so.

The bench step of a pull request stays under 3 minutes, read from the job
log (see [Coverage and performance](development.md#coverage-and-performance)).
A benchmark that would push it over shrinks its input, or moves to
`codspeed-main.yml`, which runs only on pushes to `main`.

The first candidates:

- 8: a generic container used at two types, and a call through an interface.
- 9: an iterator chain next to the same loop written by hand, with both
  printing the same result, so the cost of the chain shows.
- 10: comparing and printing records.
- 11: word frequency with `Map`; `strings` grows into it.
- 12: `?` on a path that runs often.

### The growing proof program

Status: agreed 2026-10-14; it starts in milestone 8, not 7 (the user's
decision, 2026-10-15). Each milestone's proof program is a few dozen
lines, too small to show what goes wrong at size: error handling that
nests, awkward module boundaries, missing collection methods, slow builds.
So from milestone 8 on, one program grows with the language: Sisu's own
lexer, written in Sisu, and later its parser. It turns the self-hosting
stretch goal into steps.

- Milestone 8 starts it, with tokens as an enum and the lexer over the
  `chars()` of its input, read from standard input (milestone 5). It waits
  for generics and interfaces, which make the token type and its printing
  read well.
- From then on, a milestone is done only when the program uses its feature
  where the feature fits: generics and interfaces (8), closures and iterator
  methods (9), records (10), modules (11) and `Result` for lexer errors (12).
  A feature that reads badly in the program is changed while its milestone
  is still open.
- The check is built in: for each `.sisu` file in the compiler's tests that
  lexes without errors, the program prints the same tokens as
  `sisuc --emit tokens`. The parser, once it exists, is checked against
  `--emit ast` the same way.
- Milestone 8's spec settles where the program lives and how the tests run
  it.

Once the lexer exists, CodSpeed gets two benchmarks from it: one runs the
lexer on a fixed corpus of `.sisu` files, whose output prep checks against
`sisuc --emit tokens`, and one builds the lexer at `-O2`. The focused
benchmarks stay, because the program changes in every milestone, and its
numbers compare only between commits that leave it unchanged.

## Dependencies and parallel work

A milestone's number names it; its prerequisites, not its number, fix when
it can start. Work whose prerequisites are done can be built in parallel,
each on its own branch. Several milestones split into parts with different
prerequisites, so a part can start long before the rest.

Parallel branches that touch the same compiler stage conflict when they
merge, even when their designs are independent. Milestone 4's hexadecimal
literals and milestone 6's floats both change the lexer's number scanner,
for example. Land the smaller one first.

### Prerequisites

"Decision" means an entry in "Open decisions" that the milestone spec must
settle before work starts.

| Work | Needs | Unblocks |
| --- | --- | --- |
| Milestone 4: strings | Done | 5, `panic("msg")`, `Display` in 8, the proof programs of 6 and 9 |
| Milestone 5: standard streams and arguments | 4 | The word-count tool |
| Milestone 6: floats and conversions | None | Float formatting in `"{x}"`; its Mandelbrot proof printed text, so it landed after 4 |
| Milestone 7: enums and `match` | Done | 8, 12 |
| Milestone 8: generics and interfaces | 4, 7 | 12, iterator methods (9), generated conformances on request (10), the standard library (11) |
| Milestone 12: `Result` and `?` | 7, 8 | File I/O, the standard library (11), concurrency |
| `weak` fields (ADR 0008) | None | Closures that capture their owner |
| Milestone 9: closures and function types | `weak` fields. Decisions: closure syntax, weak captures, argument labels | Iterator methods (9), concurrency |
| Milestone 9: lazy iterator methods | 8, closures | The standard library (11) |
| Milestone 10: named arguments, default parameters | Decision: argument labels | Records (10) |
| Milestone 10: records, tuples, newtypes | 4 for `print`. Decisions: values with fields, argument labels | Generated conformances on request, tuple patterns in closure parameters (9) |
| Milestone 10: generated conformances on request | 7, 8, records. Decision: equality and hashing | `Map` keys of user types (11) |
| Milestone 11: modules and `pub` | Keywords reserved, a build of more than one file. Decision: names and namespaces | The standard library (11), opaque types (11) |
| Milestone 11: standard library in Sisu | 8, 12, iterator methods (9), modules | Concurrency |
| Concurrency | 8, 9, 11, 12 | |

### What can start now

With milestones 1 to 7 done, each of these tracks can run beside the others
once the decisions it needs are settled:

- **Decisions**: every entry in "Open decisions" is design work with no
  code. Settle the shared ones first, because several tracks build on each:
  what is left of argument labels (records and named arguments), names and
  namespaces for modules, closure syntax for closures, and values with fields
  for records.
- **Closures**: `weak` fields, then closures and function types from
  milestone 9. A function type names concrete types, such as
  `(i64) -> bool`, so closures need no generics.
- **Calls**: named arguments and default parameters from milestone 10.
- **Modules**: modules and `pub` from milestone 11, after the keywords are
  reserved and `sisuc` can build more than one file. A program of one file
  still builds with no `import`.
- **Small additions** from the table above, each a branch of its own:
  `let mut` replacing `var` (land it first: it touches every test program),
  reserved keywords, `const`, `_`, hexadecimal literals, block expressions
  and `todo`. All but `todo` landed in milestone 4, stage 1; `todo` landed
  with `panic("msg")` in milestone 5, stage 1, so `test` blocks can panic.
  Scheduled right after milestone 6: tests in the source file,
  `test "add handles negatives" { assert(add(-1, 1) == 0) }`, with a built-in
  `assert`. A normal build drops `test` blocks; `sisuc --test`, later
  `sisu test`, builds a `main` that runs each one and reports which panicked.
  A test is in the same module, so it sees private names. A panic ends the
  whole process (`sisu_panic`), so the spec decides how the other tests still
  run: one child process per test, or stopping at the first failure. Tests
  sit next to the code, as in Rust and Zig, with no test module or attribute,
  and the compiler's own `tests/run` cases can use `assert`.
- **Tooling**: everything under "Early tooling", and from "Later and stretch
  goals" parser error recovery, the lints whose features exist, linking with
  `lld`, and debug info.

Shared machinery also orders work that is otherwise independent. `weak`
fields split a class object's count word in two (ADR 0008), while arrays and
milestone 4's strings keep one count, so the shared `@sisu_rc.retain` splits
by kind. Land `weak` fields and strings one after the other, not as two
rewrites of `codegen/rc.rs` at once.

Once 7 is done, milestone 8 starts. Once 8 is done, milestone 12 and
the Sisu prelude can start, iterator methods once closures are done too, and
generated conformances on request once records are done too. The standard
library comes after them, and concurrency last.

### Smaller additions to milestones 8 to 11

Each is small once its milestone's main feature exists:

- Milestone 8: type aliases, `type Index = Map<string, [i64]>`. An alias is
  another name for the same type, so it adds no type safety; for a distinct
  type, use a newtype (milestone 10). Also `dbg(x)`, which prints `x` with its
  file and line on standard error and returns `x`, so it can wrap any
  expression while debugging. It needs the `Display`-like interface.
- Milestone 8: `Eq` and `Ord` interfaces in the prelude; milestone 11:
  `Hash`, with `Map` and `Set`. `Map` keys need `Hash`, and `sort` needs
  `Ord`. `Hash` has the shape
  `interface Hash: Eq { fn hash(self, h: mut Hasher) }`, which is Swift's
  `hash(into:)`: a type feeds its fields to the runtime's `Hasher`. Classes
  and `f64` get no `Hash` (see "Open decisions").
- Milestone 8: a prelude written in Sisu, compiled with every program before
  modules exist. Array `sum` and `contains` (milestone 3) and the string
  methods (milestone 4) are built into the checker and codegen today. Once
  generics exist, each one that Sisu code can express moves into the
  prelude. What depends on a value's layout, such as an array's length,
  element access and allocation, stays built in. Milestone 11 turns the
  prelude into the standard library's first module.
- Milestone 8: two data-oriented changes, ordered around generics (agreed
  2026-10-14, revised 2026-10-16). Interned types now come first in
  milestone 8, before generics, so that generic types are built on the
  handle from the start; niches arrive with `Option<T>` in stage 3.
  `Option<T>` stores `None` as a value `T` can never hold, so `char?`
  becomes an `i32` with `0x110000` for `None` (8 bytes to 4) and `bool?` an
  `i8` with 2. One rule covers every `Option`-shaped enum; `char?` gets no
  special case before it, since milestone 7's enums take no niche. `i64?` and
  `u8?` have no spare value and keep `{ i1, T }`. A `None` sentinel never
  reaches the runtime: printing decodes it first. Interned types (done in
  milestone 8 stage 1): `tir::Type` is a 4-byte handle into one table, as
  `Symbol` is for names, so copying, comparing and hashing a type cost an
  integer operation. Monomorphization multiplies the types the checker
  builds, so `Type` became a handle before generics, and generic types are
  designed on it once.
- Milestone 9: `for` over any iterator. Milestone 3's `for` lowers to a loop
  over an array or a range. An iterator `for` lowers to a loop that calls
  `next()` until it returns `None`, before the last-use pass runs, so a
  `break` or `return` inside it still releases what the loop holds. A range
  then becomes a value that can be passed and stored, so the glossary's
  "range" entry changes with it.
- Milestone 9: iterator `sum` over `i64` items needs a
  `where Item == i64`-style constraint or a per-type built-in, since
  milestone 8 has no `where` clauses.
- Milestone 9: function types, written `(i64, i64) -> i64` as in Swift and
  Kotlin. A named function and a closure have the same type, so a function
  can take, return and store either. A closure that captures nothing is a
  function pointer: codegen passes it a null environment. There is no split
  into Rust's `Fn`, `FnMut` and `FnOnce`, because reference counting keeps
  the captured values alive. A closure captures by value, so it gets a copy
  of a record or an array (ADR 0007), and assigning to a captured `let mut` is a
  compile error. Swift and Kotlin share the variable instead, which is hidden
  shared state. A class that stores a closure capturing that same object is a
  cycle, so the closure needs a way to capture weakly, as Swift's
  `[weak self]` does (ADR 0008; see "Weak captures" in "Open decisions").
- Milestone 9: `weak` fields (ADR 0008), at the latest. A stored closure
  that captures its owner is the first cycle ordinary code makes without
  meaning to.
- Milestone 9, once tuples (milestone 10) exist: patterns in closure
  parameters, so a closure can take a tuple apart. A tuple pattern keeps its
  own parentheses, as Rust writes `|(k, v)|`: `entries.map(((k, v)) => k)` takes one tuple apart, and
  `(k, v) => k` always has two parameters. The meaning is in the syntax, so
  a reader does not need `map`'s signature to know which it is.
- Milestone 10: records declare fields in the body, one per line, as
  classes do (ADR 0011). A record's fields take no `mut`: nothing in
  `let p` changes, and any field of `let mut p` can. A method that changes
  its record takes `mut self` and is called only on a place that can change,
  so `p.move_by(1, 2)` changes `p` with no copy. A record that contains
  itself is an error whose help says to use a class.
- Milestone 10: generated conformances on request. A record or enum that
  declares `Hash` or `Ord` after its name, as in `record Point: Hash {`, gets
  the methods written by the compiler from its fields, in declaration order,
  as Swift synthesizes `Hashable` and `Comparable`. It is a compile error on
  the declaration when a field lacks the interface, not at a distant `Map`
  that uses the type as a key. `==` and `print` need no declaration: a
  record, enum or tuple has them when its fields do. Function values have
  neither until "Equality, hashing and order" decides otherwise. The list is
  fixed by the compiler: this replaces
  derive macros, not user code generation.
- Milestone 10: `mut self` requirements in interfaces arrive with records,
  and milestone 10 decides whether a class's `self` method satisfies one, as
  in Swift.
- Milestone 10: array `push` and the other methods that change an array take
  `mut self` (ADR 0011) and arrive with records. `push` grows the block in
  place when nothing else holds it, doubling its capacity when full, and
  copies first when it is shared (ADR 0007).
- Milestone 10: a copy of a record with some fields changed,
  `Point(..p, y: 0)`, as Gleam writes it and as Kotlin's `copy(y = 0)` does.
  It also builds a labeled enum variant from a bound one, which is how a
  payload changes (see "Values with fields"):
  `if let p: Page = doc { doc = Page(..p, title: "New title") }`. The pattern
  that binds the whole payload is settled with "Argument labels". Label
  shorthand, `area(width:, height:)`, passes the local variables of the
  same names.
- Milestone 10: default parameters, as in Kotlin. Only trailing parameters
  take a default, and a named argument can skip one. The default expression
  runs at each call, never once as in Python. At first a default cannot
  refer to an earlier parameter.
- Milestone 10: tuples, for returning more than one value. A tuple is a
  record with no name: a value with the same layout, and the same type
  wherever it is written. Its elements can have names, written name first
  like parameters, `(q: i64, r: i64)`, or none, `(i64, i64)`. The names are
  part of the type, so `(q: i64, r: i64)` is not `(a: i64, b: i64)`.

  ```
  fn divmod(a: i64, b: i64) -> (q: i64, r: i64) {
      (a / b, a % b)
  }

  let res = divmod(7, 2)
  print(res.q)
  let (q, r) = divmod(7, 2)
  ```

  A literal without names, such as `(a / b, a % b)`, is accepted where the
  checker already knows the tuple type. `.0` and `.1` read an element by
  position, named or not. `let` and `for` take a tuple apart by position,
  so `for (key, value) in map` works whatever the names are. A tuple has
  `==` and `print`, and `Hash` and `Ord` when every element has them; it
  declares no interfaces and has no methods. Use a record once the value
  has a meaning of its own, or once a public API passes it around.
- Milestone 10: newtypes, as in Rust. A one-field record such as
  `record UserId { value: i64 }` is a distinct type: passing an `OrderId` or a
  plain `i64` where a `UserId` is expected is a compile error. Because
  records are values (ADR 0007), the compiler guarantees it costs nothing at
  run time: no heap object of its own and no extra reference count, and it
  is passed and stored exactly as its field is, as Rust's
  `#[repr(transparent)]` promises.
  Generics are monomorphized (milestone 8), so unlike Kotlin's `value class`
  it is never boxed. It gets `==` and `print` like any record, but not its
  field's operators: `Meters * Meters` is not `Meters`, so arithmetic is
  written as methods. With milestone 11's opaque types, a newtype whose
  constructor is private holds only checked values, such as
  `Email.parse(s) -> Email?`. A newtype over a class or an array still
  counts that reference, as the field alone would.
- Milestone 11: opaque types. The type is `pub`, but its constructor and
  fields are private to its module, so other modules must build it through a
  function that checks the value.

### Modules (milestone 11)

- One file is one module, and its path names it: `src/geometry/shapes.sisu` is
  `geometry.shapes`. A parent file needs no `mod shapes` declaration, as in
  Rust. A directory is not a package either: in Go, a file uses names from
  sibling files without importing them, so it cannot be read on its own.
- `import geometry.shapes` lets code call `shapes.area(s)`.
  `import geometry.shapes.{area, Shape}` brings those names in without a
  prefix.
- Names are private to their module unless marked `pub`.
- Modules may import each other in a cycle. The checker sees the whole
  program at once, so this costs nothing.
- Codegen puts the whole program into one LLVM module. Each symbol is
  prefixed with its module path; `main` keeps its name.
- A `pub` function writes out its parameter and return types, with no
  inference across modules. A module can then be compiled from the `pub`
  signatures it imports, without their bodies.
- Separate compilation and build caching wait until `--time` shows builds
  are slow. Then codegen emits one LLVM module per Sisu module and LLVM
  compiles them in parallel, as Rust's codegen units do. Each object file is
  cached under a hash of its source and of the `pub` signatures it imports.
  Monomorphized generics complicate this: a generic function's copy is
  emitted in the module that uses it.
- A small prelude is visible in every file without an import: `print`,
  `Option` and `Result`, each once it exists.
- Each generated symbol carries its module path, the reference-count helpers
  too: today `@sisu_rc.drop.<Class>` is named by the class alone, so two
  modules with a class of the same name would clash.

## Concurrency (after milestone 12)

Status: proposal, 2026-10-10, revised after a review by gpt-6.1-sol. The
memory model is ADR 0010 (proposed). Concurrency needs generics for
`Channel<T>` (milestone 8) and closures for `spawn` (milestone 9).

The model takes isolation from Elixir and typed channels from Go:

- **Tasks share no objects (ADR 0010).** Every value that crosses into another
  task is copied: a `spawn` capture, a `send`, a `join` result. Channel and
  task handles are shared. A value that cannot be copied, such as a file
  handle, is a compile error. Reference counts stay non-atomic, and data races
  cannot happen.
- **Typed, bounded channels.** `Channel<T>(capacity: n)` with `send`, `recv`,
  `close` and `for x in ch`. A full channel makes `send` wait, so a fast
  producer cannot fill memory.
- **Actors are a library pattern, not a language feature**: a task that loops
  over a channel of enum messages with `match`. Its state lives in that one
  task and needs no lock.
- **Structured tasks.** `scope(s => { s.spawn(...) })` ends only after every
  task it started has ended, as with Kotlin's `coroutineScope`, Swift's
  `TaskGroup` and Rust's `thread::scope`. No task outlives its scope, and
  `join()` returns the task's result.
- **One OS thread per task.** The runtime starts it with Rust `std::thread`
  and builds channels from `Mutex` and `Condvar`. Following the portability
  rules, runtime functions take and return only pointers and integers.
- **A panic ends the whole program**, as `sisu_panic` does today.

```rust
fn main() {
    let lines = Channel<string>(capacity: 100)
    let total = scope(s => {
        s.spawn(() => {
            for line in read_lines("big.txt") { lines.send(line) }
            lines.close()
        })
        let mut workers: [Task<i64>] = []
        for _ in 0..4 {
            workers.push(s.spawn(() => {
                let mut words = 0
                for line in lines { words += word_count(line) }
                words
            }))
        }
        workers.map(w => w.join()).sum()
    })
    print("words: {total}")
}
```

How it uses the machine:

- **CPU-bound work** wants about one task per core (`cpu_count()`). Linux
  spreads the threads across the cores, so the work runs in parallel.
- **A blocked task** sleeps on a condition variable and uses no CPU.
- **Thousands of tasks are practical, not millions.** Rust gives each thread
  a 2 MiB stack by default, but that is address space: only the pages a task
  touches use memory. Measured on Linux with Rust `std::thread`
  (2026-10-10): starting and joining a thread takes about 30 µs, and 1,000
  waiting threads use about 11 KB of memory each. The cost of copying a value
  is not yet measured.

The copy follows ADR 0010. Codegen generates `@sisu_rc.copy.<Class>` next to
the drop glue. One visited map covers a whole crossing, so shared objects and
cycles keep their shape. Like drop, it loops along a class's own-type field,
so a long list copies in constant stack.

The spec must settle these before work starts:

- **Closing a channel.** Queued messages are still received after `close`.
  Decide what `send` after `close` and a second `close` do, and who may close:
  separate sender and receiver handles, as in Rust, prevent a receiver from
  closing by mistake.
- **Order.** Messages arrive in the order their `send` calls succeeded.
- **Leaving a scope early** through `return` or `?`. Decide whether the scope
  waits for its tasks or cancels them. Cancellation can only be cooperative,
  at a channel wait; forcing a thread to stop is unsafe.
- **`join`.** `join() -> T` now. When a task can die alone on a panic, add
  `try_join() -> Result<T, Panic>`, so no program breaks.
- **What crossing means** for a closure that captures closures, for an
  iterator, for a `weak` field whose object is not part of the copy
  (ADR 0008), and for an object in a top-level `const`.
- **Lazy iterators.** Milestone 9's `map` is lazy, so mapping `s.spawn` over
  a range and then `join` over the result would start each task and join it
  before starting the next. The example collects the handles first for that
  reason.

The work splits into three milestones: copying across tasks; tasks and
scopes; channels. The tests cover shared objects and cycles in a copy,
closures inside closures, messages freed while still queued, a `close` racing
with `send`, every way to leave a scope, and results passed back by `join`.

Later, each with its own design:

- `select` over several channels, and timeouts.
- A task that dies alone on a panic. That needs a way to leave its frames and
  free its objects, and channels and files it holds must be cleaned up. A
  memory arena per task frees memory but cleans up nothing else.
- `Shared<T>`, for deeply immutable data read by many tasks without copies.
  The whole object graph must be safe to share, not only its root.
- Many tasks on a few threads (M:N), as in Go. Task and channel semantics do
  not depend on which thread runs a task, but blocking C calls, thread-local
  state, stack limits and loops with no yield point all behave differently,
  so this is not a drop-in change.
- A deadlock report when every live task waits on a channel, `join` or
  `scope`. Nothing outside the program can wake such tasks, but the check
  must run under the runtime's lock, and a task waiting for I/O or a timer
  does not count.
- `Mutex<T>` and atomics. Defer them: a mutex that hands out references to
  class objects would leak them to other tasks.

## Later and stretch goals

These do not make code nicer to write, so they wait:

- C FFI: `extern fn` to call libc or a C library. A C pointer is an opaque
  `CPtr` that Sisu code passes back to C, with no arithmetic and no
  dereference. Basic I/O does not need it: the runtime owns that (ADR 0009).
  - A class's memory layout is unspecified: codegen orders its fields by
    alignment, largest first, so that less padding sits between them. A class
    that must match a C struct therefore asks for a C layout explicitly, in
    declared order. That choice serves the FFI and costs the padding back,
    the cost ADR 0006 asks a language-level choice to name.
- Linking with `lld` (`-fuse-ld=lld`) when it is installed, because linking
  time grows with the program. `--linker lld` (or `bfd`, `gold`, `mold`)
  chooses the linker by hand; the default is still `cc`'s.
- Parser error recovery, so a syntax error does not hide the next one.
- A fixed-size inline array: its length is part of its type, and it is stored
  inline (on the stack, or inside the object that holds it) with no header,
  no count and no heap block. Name and syntax are open (Swift's
  `InlineArray<3, i64>`, Rust's `[i64; 3]`, which would clash with today's
  `[x; n]` making a `[T]`). A built-in literal length needs no const
  generics; generic code over the length does. Build it when a benchmark shows
  the heap `[T]` costs too much.
- A `Value` bound for type parameters, the mirror of `AnyObject`: every type
  that is not a class, nor an optional of one, conforms, and nothing else can.
  Milestone 8 rejects `[x; n]` in a generic body when `x` is a `T` or `T?`,
  because `T` may be a class and every element would be the same object
  (E0067); `T: Value` would allow it. The ban only rejects programs, so
  lifting it breaks none. Open: whether an enum or array that holds a class
  conforms.
- More lint checks as compiler warnings, as in Go's `vet` and `rustc`'s
  built-in lints, with no separate tool and no configuration file. The
  checker already warns about code after `return`, `break` or `continue`, and
  about a `let mut` (`var` before milestone 4) that is never reassigned, and
  comparing a value that is not optional with `None` is an error. Still to
  come:
  - a class with no `mut` field that nothing compares with `is`, suggesting
    `record` (ADR 0011);
  - an unused variable or import;
  - a name starting with `_`, such as `_score`, is exempt from the unused
    variable lint, as in Rust and Gleam;
  - `.0` or `.1` on a tuple outside a short closure, suggesting element
    names or `let (a, b) = ...`;
  - a cycle shape, suggesting `weak` (ADR 0008): two classes whose fields
    point at each other, or a class with both a `[C]` and a `C?` field of its
    own type. From milestone 9, also a closure stored in a field of the
    object it captures.

  An attribute such as `@allow(unused)` could later silence one declaration.
- Debug info (DWARF), so `gdb` steps through Sisu source.
- A mid-level IR, `mir`, between `tir` and LLVM IR. Each step down is a
  lowering: it rewrites a feature with simpler parts, so the stages below
  never see it. Rust has the same split: its HIR is a desugared tree, close
  to `tir`, and its MIR is a control-flow graph of basic blocks. Swift's SIL and
  Zig's AIR sit at the same level. `tir` only records what the checker
  understood; it does not optimize, and codegen lowers it straight to LLVM IR
  today.

  `mir` is worth its cost only for what LLVM cannot know: the language's
  rules. General optimization (inlining, loops, constants) stays with LLVM,
  which does it better. The first of these needs introduces `mir`:
  - Reference-count elision: removing increment and decrement pairs that
    cancel out, which is why Swift has SIL. ADR 0012 (proposed) removes the
    pair around a method's `self` without it.
  - Explicit count statements. Today codegen decides where to add and
    subtract 1 while it emits LLVM IR, and `last_use.rs` must repeat
    codegen's evaluation order by hand. In `mir`, one pass writes every
    retain, release and move as a statement, and LLVM emission follows them.
    Debug builds of the compiler then check each function, as Roc's borrow
    certifier does: on every path, each owned reference is moved or released
    once, nothing is used after its release, and a borrow ends before its
    owner does. Valgrind and `--rc-checks` stay, because the checker cannot
    see the runtime or how LLVM IR is emitted.

  Milestone 7 compiles `match` without `mir`: arms are tested in order,
  straight into basic blocks, with backtracking for nested alternatives.

  It brings its own `--emit mir` printer, and desugaring moves out of
  codegen into the `tir`-to-`mir` step. The name follows `tir`; `sil` would
  clash with Swift's.
- Borrowing every counted parameter, after ADR 0012 is built and measured.
  ADR 0012 borrows only `self`, and leaves the other parameters until a
  benchmark shows that argument counting matters. Then infer it, as Lean 4
  does ("Counting Immutable Beans") and Roc does. A parameter starts
  borrowed. It becomes owned when the function stores it, returns it, passes
  it to an owned parameter, or puts it in a `let mut` local that it then
  changes. The last case keeps ADR 0007's handover: a borrowed array has
  count 2, so the first write would copy it. Recursive functions are solved
  together, one strongly connected group of the call graph at a time. The
  result is one flag per parameter on `tir`, computed as last use is, so it
  needs no `mir`. Last use must not move a borrowed parameter. Callers keep
  ADR 0012's rule: they add 1 to an argument read from a `mut` field, a weak
  field or an array element. A function called through a closure keeps owned
  parameters.
- Reusing a box in `match`. Milestone 7 boxes the enum fields that close a
  cycle. When a `match` takes apart the last reference to a box, and its arm
  builds a box of the same enum type, as
  `Add(l, r) => Expr.Add(simplify(l), simplify(r))` does, the old box could
  hold the new value in place of a `sisu_free` and a `sisu_alloc`. Koka
  (Perceus) and Roc call this reuse. Enums have no identity, so a program
  cannot tell. Classes stay out, because `is` sees identity. Reuse needs
  `match` to move fields out of a box with count 1 first, which milestone 7
  does not build: its `match` adds 1 to each binding and then releases the
  box. A box kept for reuse stays off the pending list, and an arm that does
  not reuse it frees it on every exit. An enum benchmark in `bench/`, such as
  an expression simplifier, measures the move and the reuse separately
  before either is built.
- Handing over an array held in a field or an element. The last-use rule
  moves only locals, so `self.cells = step(self.cells)` and
  `grid[i] = f(grid[i])` share the array with the call, and the first write
  inside it copies the whole array. The compiler could take the array out of
  the place for the call and store the result back, with no new syntax, so
  ADR 0007 still holds. While the place is empty, the call must not reach it
  another way, for example through a shared object: that needs an
  exclusivity check, as Swift enforces for `inout`. A `mut self` method
  called on a field, `obj.pos.move_by(1, 2)`, needs the same check
  (ADR 0011). Until then, copy the field into a local, set the field to `[]`,
  and assign it back. `inout` parameters stay out (ADR 0007).
- Proving uniqueness at compile time. Roc's Morphic analysis follows each
  array through the whole program to prove that a write holds the only
  reference, so the write needs no count check. A loop now makes its written
  arrays unique once before it starts (ADR 0021), which took the quicksort
  benchmark from 4.82 to 4.04 billion instructions and its fastest run from
  690 to 640 ms, against C's 610
  ([Benchmarks](benchmarks.md#quicksort-on-an-array-value)). One count check
  is left in each call of `quicksort`, before the partition loop. The
  analysis would remove that one and the checks of writes outside loops; no
  benchmark has measured what that gains.
- Removing bounds checks that LLVM keeps. On quicksort, removing by hand the
  bounds checks that codegen still emits took the time from 622 to 586 ms
  ([Benchmarks](benchmarks.md#quicksort-on-an-array-value)). `for j in lo..hi`
  indexes `a[j]` with no proof that `hi` is below the length. A first step is
  to find which checks LLVM keeps that a range fact about the loop would
  remove.
- Link-time optimization with the runtime. Build `sisu-runtime` as LLVM
  bitcode and link it into the program's module, so LLVM can inline runtime
  calls such as `sisu_alloc` and `sisu_free`. The reference-count helpers
  (`@sisu_rc.*`) are already generated into each program, so LLVM inlines
  them today. On the binary_trees benchmark, a direct call to `malloc` ran
  no faster than `sisu_alloc` ([Benchmarks](benchmarks.md)), so wait for a benchmark
  that shows runtime calls matter.

  Bitcode ties two LLVM versions together. LLVM reads bitcode written by
  older releases, not newer ones, so `sisuc`'s LLVM must read what the pinned
  Rust toolchain writes, and a test must prove it on each upgrade. Roc's
  builtins were Zig bitcode, so its LLVM had to follow Zig's. Generating the
  hot runtime functions as IR inside `sisuc`, as the `@sisu_rc.*` helpers
  are, avoids the coupling.
- A formatter, `sisu fmt`, after milestone 7, once `enum` and `match` have
  fixed most of the syntax. It has one style and no options, as with `gofmt`
  and `zig fmt`. It must keep comments, but the lexer drops them today
  (`crates/sisuc/src/lexer/`). As `gofmt` does, the lexer would record each
  comment's span, and the formatter would write it back next to the nearest
  syntax-tree node. It keeps single blank lines between statements, found
  from the spans. Formatting twice must change nothing, and the formatted
  file must parse to the same syntax tree.
- A language server, `sisu lsp`, reusing the lexer, parser and checker.
- Windows. There is no `cc`, so `sisuc` links with `link.exe` or `lld-link`.
  Programs are `.exe` files in the COFF object format. LLVM ships for Windows
  as an installer and a `clang+llvm` archive, so `llvm-sys` needs its own
  setup.
- Cross-compiling with `--target <triple>`. LLVM must initialize every target,
  not only the native one. The runtime must be built for the target
  (`cargo build --target`), and linking needs a cross linker and the target's
  system libraries.
- Calls to plain functions in a `const` initializer, for lookup tables. LLVM
  already folds constant arithmetic, so this adds expressiveness, not speed.
  Running the call through LLVM's JIT reuses codegen, so compile time and run
  time cannot disagree; a tree-walking interpreter would be a second copy of
  the semantics. Open points:
  - The JIT runs on the host, which can differ from the `--target`.
  - Only scalars at first, then strings. Heap objects would have to be
    serialized into globals.
  - No I/O at compile time, so builds stay reproducible.
  - A step or time limit, so a loop that never ends cannot hang the compiler.
- `@inline` and `@noinline` on functions. They set LLVM's `alwaysinline` and
  `noinline` attributes, as codegen already sets `noreturn` on `sisu_panic`.
  An attribute alone changes nothing: without `-O2`, `sisuc` must run
  `always-inline` before `mem2reg`. `default<O2>` inlines `alwaysinline`
  functions and respects `noinline`. Avoid an `inline fn` keyword, which in
  Kotlin inlines lambda arguments. `@inline` on a recursive function is an
  error. From milestone 9, calls through closures cannot be inlined. At
  `-O2`, LLVM already inlines small functions, so this waits for a need.
- More patterns after milestone 7: array patterns such as `[first, ..rest]`.
  `p as name` is in milestone 10, beside
  records.
- Doc comments, `///` before a declaration. The lexer drops comments today,
  so they wait for the formatter, which must keep comments anyway, and for
  the language server, which can show them.
- Self-hosting: the Sisu lexer and parser written in Sisu. They start as
  the growing proof program from milestone 8 (see "The growing proof
  program"); a compiler written in Sisu stays a stretch goal.

## Gaps before production

Status: proposal, 2026-10-14. Milestones 1 to 12 give Sisu a complete
language, but not one ready for production use. Nothing in this section has
a milestone or a done-when test yet. Some items appear above under "Later and
stretch goals" with no date. Each gap gets a milestone once a definition of
1.0 says it is needed.

- **Target domain.** The roadmap does not say what Sisu is for. Command-line
  tools and scripts need a fast `sisu run`, a standard library for files,
  processes, regular expressions and JSON, and builds for several platforms.
  Network services need sockets, many connections per thread, and a panic
  that ends one task instead of the program. The answer orders every gap
  below, so decide it first.
- **A language reference.** Milestone specs are not published, so the
  compiler and its tests define the language today. Users and tool authors
  need a reference that says what a program means, apart from what `sisuc`
  happens to do.
- **A stability promise.** Nothing says which programs keep compiling from
  one version to the next, how a feature is deprecated, or whether Sisu has
  editions as Rust does. Sisu has no users yet, so this costs little now and
  much later.
- **C FFI.** It is a stretch goal above. Without it, Sisu cannot call SQLite,
  a TLS library or an OS API that the runtime does not wrap. Under ADR 0009
  each OS feature is runtime code in Rust, which does not scale to a full
  standard library. WebAssembly in a browser is the same design with
  another host: there is no standard output, so a program imports JavaScript
  functions and exports its own through the `extern fn` that C uses, not a
  second mechanism.
- **Standard library scope.** Milestone 11 names `Map`, and
  milestone 12 brings files. Time, processes, directories, networking,
  regular expressions and JSON are not planned.
- **Platforms.** Sisu builds only for x86-64 Linux. ARM64 Linux and macOS
  are under "Early tooling", untested, and Windows is a stretch goal.
  WebAssembly with WASI (`wasm32-wasip1`), run by `wasmtime`, is the cheapest
  new target: LLVM 22 in `.llvm/22` has the backend, and reference counting
  needs nothing from the host. It needs cross-compiling (`--target`, a
  stretch goal above), a runtime built for the target without its Unix
  `SIGPIPE` handling, linking with `wasm-ld` in place of `cc`, and the test
  cases run under `wasmtime`. One task per OS thread (ADR 0010) does not
  carry over, because WASI threads are experimental.
- **Debug info and crash reports.** A panic prints its file and line but no
  stack trace, and `gdb` cannot step through Sisu source (DWARF is a stretch
  goal). Recursion that runs out of stack dies with `SIGSEGV`, exit code 139,
  not with a panic that names the line.
- **A language server.** It is a stretch goal above. It needs a parser that
  recovers inside a body, where today the parser reports one error per item
  (ADR 0013), and a checker that can rerun after one file changes. Both are
  expensive to add to a batch compiler later, so the front end should be
  shaped for them before modules (milestone 11) arrive.
- **Fast feedback.** Good developer experience needs an edit to reach a
  result quickly. Today every build checks the whole program and runs all of
  it through LLVM. Fast rebuilds need incremental compilation, where a change
  recompiles only what depends on it. It has three parts, each ordered by
  what `--time` shows:
  - Caching each module's object file, as "Separate compilation and build
    caching" under milestone 11 describes.
  - A checker that reruns only the items a change affects, as Rust's query
    system does. The language server above needs the same machinery, so
    design both together.
  - A development build that skips most of LLVM's work, as Roc's dev backend
    and Rust's Cranelift backend do, and a link that patches only what
    changed, as Roc's surgical linker does.
- **Packages.** "Deliberately avoided" defers a registry and lets path and
  Git dependencies come first. A manifest (`sisu.toml`) and dependency
  resolution have no design.
- **Number types.** Sisu has `i64` and `u8`, and milestone 6 added `f64`.
  The C FFI, binary file formats and hashing need `i32`, `u32`, `u64` and
  `f32`. "Integer widths and wrapping" in "Open decisions" covers only part of
  this.
- **Fuzzing the compiler.** Mutation testing measures the tests. Nothing
  feeds generated programs to the parser, checker and codegen to find
  crashes or wrong code.
- **Distributing `sisuc`.** Nothing installs Sisu outside a clone. `sisuc`
  finds `libsisu_runtime.a` only next to its own executable in a Cargo target
  directory (`link.rs`), and it links with `cc`, so users need a C toolchain.
  The release binary links LLVM statically but needs glibc, libstdc++,
  libgcc_s, libm, zlib and zstd. A compiled program needs glibc and
  libgcc_s. A first release ships the runtime beside `sisuc` and finds it
  there, and is built on an old glibc with libstdc++, zlib and zstd linked
  statically, then tested on a clean machine. A fully static `sisuc` built
  against musl needs LLVM built from source against musl. Static compiled
  programs need the runtime built for `x86_64-unknown-linux-musl` and a link
  against musl's C library.

The first step is a definition of 1.0, recorded as an ADR: the target
domain, the platforms, the stability promise, the tools it requires (a
language server, `sisu fmt`, `sisu test`, debug info), C FFI and the minimum
standard library. The milestones after 12 then follow from it.

## Deliberately avoided

- Lifetimes and a borrow checker. Reference counting keeps Sisu in the Swift
  and Kotlin ergonomic camp.
- Header files.
- Unrestricted operator overloading.
- Inheritance. A `class` is a reference type, not a hierarchy. Composition,
  enums (milestone 7) and interfaces (milestone 8) cover polymorphism, as in
  Rust and Go.
- Zig-style `comptime`, with types as compile-time values, and macros.
  Generics and interfaces (milestone 8) cover the same ground, and errors
  stay plain when type checking does not depend on running code. Macros
  hide code from error messages, the formatter and the language server. The
  other jobs macros do have other answers: generated conformances on request
  (milestone 10), `const` initializers and attributes (later goals), and, for
  serialization or bindings, a script that writes ordinary `.sisu` files
  that are committed, as Go's `go generate` does. Runtime reflection is
  avoided for the same reason, and because it costs at run time.
- Variadic parameters. An array costs two characters, `sum([1, 2, 3])`, and
  uses a type that already exists. `print` is built in, and formatting uses
  string interpolation.
- Pointers in Sisu code. A class is already a reference, and records and
  arrays are values. A raw pointer exists only as `CPtr` behind the C FFI.
- Formatter options. One style ends style debates.
- A separate linter. Lints are compiler warnings.
- `import *`. A reader must always see where a name comes from.
- Gleam's pipe `|>` and function captures `add(1, _)`. Method chains,
  continuation lines (ADR 0001) and closures (milestone 9) already do this,
  and two ways to write one call break "one obvious way".
- Gleam's `use`, which turns the rest of a block into a callback. Gleam
  needs it because it has no early return; Sisu has `return`, and `?` in
  milestone 12.
- Separate float operators such as Gleam's `+.`. The checker knows each
  operand's type, so `+` can serve both, as long as milestone 6 keeps `i64`
  and `f64` from mixing without a conversion.
- Gleam's integer rules: on Erlang its `Int` has no size limit, and division
  by zero gives `0`. Sisu's fixed-size integers panic on overflow and on a
  zero divisor.
- Mutable memory shared between tasks (ADR 0010).
- A package registry, for a long time. A registry means hosting, security
  and name squatting. If dependencies come, path and Git dependencies come
  first.

## Open decisions

- **Same-scope shadowing.** Resolved in milestone 4: allowed. `let n = n.trim()`
  in the same scope declares a new binding, and the old one stays visible to
  its own right-hand side.
- **Generic arguments and `<`.** Resolved in milestone 8
  ([ADR 0023](adr/0023-generics-are-monomorphized-and-checked-at-the-definition.md)):
  after a name in an expression, `<` starts a type-argument list only if a
  well-formed list follows and its closing `>` is followed by `(` or `.`.
  Otherwise it is a comparison, so `f(a < b, c > d)` stays two comparisons.
- **Interfaces as types.** Agreed 2026-10-14; milestone 8's spec confirms
  it. An interface is a generic bound, `T: Drawable`, dispatched statically
  after monomorphization. Bounds combine with `&`, as in `T: Drawable & Ord`.
  A list of different types that share an interface,
  such as a file and an in-memory buffer used as one `Writer`, needs an
  interface value as well:
  - The type is written `any Drawable`, so the box and the indirect call show
    where they cost, as Swift's `any P` and Rust's `dyn Trait` do. A bare
    interface name used as a type is an error whose help offers
    `T: Drawable` or `any Drawable`. `any` is a keyword only before a type,
    so `xs.any(...)` stays a method name. The relationship is called
    conformance: "`Point` does not conform to `Drawable`".
  - An `any` value is two words: a pointer to the data and a table of its
    methods, which also holds the drop code and ADR 0010's copy code. A class
    object is pointed at directly. A record, an enum or a number is boxed on
    the heap first, as Go does. `any` takes one interface. In milestone 8, an
    interface with a generic method, with `Self` outside the receiver, or
    with an associated type can be a bound but not an `any` type. The first
    two follow Rust's dyn-compatibility rule.
  - An interface may take type parameters as well as associated types. A type
    parameter fits when a type conforms several ways (`Convert<T>`); an
    associated type fits when it has one answer, as Rust and Swift use them.
    `Iterator` uses an associated type, so a type has one iterator and
    `for x in it` always knows the type of `x`:

    ```
    interface Iterator {
        type Item
        fn next(self) -> Item?
    }
    ```

    `mut self` requirements in interfaces wait for milestone 10, with records.

  - A type declares its interfaces after its name: `class Tree: Display {`,
    and `record Point: Hash {` as milestone 10 writes them.
  - An `extend` block adds to a type declared elsewhere, as Swift's
    extensions do (agreed 2026-10-15). `extend string { ... }` holds extra
    methods. `extend string: Display { ... }` holds exactly `Display`'s
    methods: one interface per block. For now only the prelude may extend
    the built-in types; letting programs use `extend` later breaks no
    program.
- **Operators on newtypes.** Without them, `Meters` arithmetic needs
  `a.plus(b)` or `a.value + b.value`, the same boilerplate Rust newtypes
  carry. An opt-in such as Haskell's `deriving` would let a newtype keep its
  field's `+` and `<`. One candidate reuses the generated conformances on request
  (milestone 10): `record Meters: Add, Ord { value: f64 }` forwards
  `+` and `<` to the field. The operators come from a fixed list and the
  compiler writes them, so this is not unrestricted operator overloading.
  Decide in milestone 10.
- **Attribute syntax.** A marker on a declaration, such as `@inline`, needs
  one general form: `@name` or `@name(args)` on the line before it, as in
  Swift and Kotlin. Decide the form before the first attribute lands, so `@`
  is not taken for something else. Later uses could include `@noinline` and
  `@deprecated("...")`. Tests use `test` blocks, not an attribute.
- **What a string is.** Resolved by
  [ADR 0014](adr/0014-strings-are-utf-8-values.md): UTF-8, always valid, a
  value shared until written, `len()` in bytes and no `s[i]`.
- **Substring views.** A `split` or `trim` that returns views into the parent's
  bytes would copy nothing, but a view keeps the parent alive. Decide after
  generics; Swift's separate `Substring` type is the leading candidate.
  Compatibility cost: if milestone 5's `split` returns `[string]`, a later
  `Substring` either changes its return type or adds a second method. Milestone
  5's `words()` returns `[string]`.
- **Small-string optimisation.** Store short strings inside the variable's
  slot, as Swift and C++ do. Programs cannot observe it. Decide when a
  benchmark asks for it.
- **Values with fields.** Agreed 2026-10-14. Resolved for enums in milestone
  7 ([ADR 0019](adr/0019-enums-are-values-named-through-their-enum.md));
  records and tuples (milestone 10) reuse the mechanism and confirm the rest:
  - Records and tuples are values with their fields inline: the newtype
    promise (milestone 10) and ADR 0007 need that. An enum is the same: a tag
    and room for its largest payload, as in Rust and Swift. A heap object
    would make a second layout, and every `i64?`, an inline `{ i1, i64 }`
    pair today, would allocate once it becomes `Option<i64>` in milestone 8.
    Milestone 7 always stores a tag; using the null pointer as the tag of an
    enum with one empty variant and one that holds a class, a string or an
    array, as `T?` does today, is not built. Milestone 8 may add that niche
    for `Option`-shaped enums, since a program cannot see layout.
  - A payload cannot change in place. A pattern such as `Circle(r)` binds a
    copy, not a reference, so `let mut e` changes only by a new value:
    `e = Circle(2.0)`. Payloads have no place to assign to, because the
    variant is known only at run time, and Sisu has no references (ADR 0007
    keeps `inout` out). The copy is shallow: a string, an array or a class in
    a payload is a pointer and a count. To change one field without
    restating the others, milestone 10's update syntax covers labeled
    variants as well as records.
  - A variant that holds its own enum, such as `Add(Expr, Expr)`, gets its
    indirection without a keyword, unlike Swift's `indirect`. The compiler
    boxes every payload field whose enum type lies in the declaring enum's
    cycle, which may be more than the minimum. The
    box is counted and never changes, since a payload cannot change, so no
    program can tell it is there. A record that contains itself stays an
    error whose help says to use a class (milestone 10): its fields can
    change, so a box would need copy-on-write.
  - The copy and release code for a value whose fields hold references.
    Build it once for every value with fields, so enums, records and tuples
    share it and arrays of them release their elements.
  - That `None` of an optional class reference stays a null pointer, in an
    array too, so milestone 8's `Option<T>` changes no layout.
- **Exclusivity.** A `mut self` method called on a field of an object,
  `obj.pos.move_by(1, 2)`, and handing an array field to a call (see "Later
  and stretch goals") both take a value out of a place for the length of a
  call. The call must not reach that place another way, such as through a
  shared object. Swift checks this at run time and panics. The other choice
  is to leave the place empty or stale during the call, which needs no check
  but lets such a read see the wrong value. Decide in milestone 10.
- **Names and namespaces.** Resolved for enums in milestone 7
  ([ADR 0019](adr/0019-enums-are-values-named-through-their-enum.md)): a
  variant is built as `Shape.Circle`, a pattern uses the bare name, and the
  type of the matched value decides which enum's variants are searched. Swift's
  `.circle` short form stays open for expressions only. Modules (milestone 11)
  still decide their part: classes and functions share one namespace, modules
  add module names to it, and every generated symbol then carries its module
  path, as "Modules" describes.
- **Argument labels.** Resolved for enum payloads in milestone 7
  ([ADR 0020](adr/0020-labels-belong-to-the-declaration.md)): a variant
  labels all its fields or none, a call gives every label in declared order,
  as a class constructor does, and arguments are evaluated left to right as
  written. Still open, before records, named arguments and closures, which
  all pass labeled values. Today any other labeled argument is an error.
  Decide:
  - whether a record is built like a class, with every label;
  - which calls may leave labels out, and whether labels may come in a
    different order;
  - whether labels and default values belong to a function type, so a call
    through a closure keeps them. Swift dropped labels from function types;
  - that arguments are still evaluated left to right as written, which the
    last-use pass already assumes, even when labels reorder them.
- **Closure syntax.** Resolved for `match` in milestone 7; the rest stays
  with milestone 9. The arm's pattern comes before its `=>` and an expression
  after it, so a closure inside an arm body, `n => xs.filter(x => x > n)`,
  parses as it reads. A guard ends at the first `=>` outside brackets, so a
  closure in a guard must be inside parentheses or an argument list,
  `Num(n) if xs.any(x => x > n) => ...`. An arm body may itself be a closure,
  and `=>` groups to the right: `F(n) => x => x + n`. Still to decide in
  milestone 9: the closure forms beyond these.
- **Weak captures.** Decide before closures. A closure stored in a field of
  the object it captures is a cycle that a `weak` field cannot break
  (ADR 0008). Decide how a closure captures one name weakly, as Swift's
  `[weak self]` does, and what the closure sees once the object is gone.
- **Recovering from a panic.** Decide with the concurrency design, and
  before milestone 12's file handles. Today a panic ends the process with
  exit code 101: right for a command-line tool, wrong for a server, where
  one request that overflows an `i64` ends every other connection. The
  candidates:
  - Never recover, and let a supervisor such as systemd restart the
    process, as with Zig's panics and Swift's `fatalError`.
  - A panic ends only its task, and `join` returns `Result<T, Panic>`, as an
    Erlang process dies alone. ADR 0010 makes this safe: a task shares no
    objects, so a dying task leaves no other task's data half changed. It
    needs a way to leave the task's frames (LLVM unwinding, or a heap per
    task freed whole), and the runtime must know which task owns each file
    handle so it can close them. Milestone 12 would have to record that
    owner, which is expensive to add to file handles later.
  - A `catch` inside a task, as Rust's `catch_unwind` and Go's `recover`
    do. Code after the catch can see class objects left half changed, and
    it is a second way to handle errors beside `Result`.

  The `test` blocks scheduled right after milestone 6 do not wait for this:
  running each test in a child process isolates its panic with no runtime
  support.
- **User drop code.** Decide before milestone 12's file handles, together with
  recovering from a panic. Today drop only frees memory; code the user writes
  to run at drop earns its place when an object holds something outside
  memory: a file, a socket, a lock. Decide its form, an interface the compiler
  calls (Rust's `Drop`) or a block in the class (Swift's `deinit`); what
  happens when the drop code stores `self` somewhere, so the object outlives
  its last reference; how it runs inside the loop that drops a class's
  own-type field (ADR 0005); and whether a panic runs it. An object in a cycle
  is never dropped, so drop code cannot be what makes a program correct.
- **Nested optionals.** Resolved in milestone 8
  ([ADR 0024](adr/0024-optionals-nest.md)): optionals nest, `??` and `if let`
  peel one layer, and `?.` flattens one.
- **Equality, hashing and order.** Resolved in milestone 8, except for
  function values:
  - Classes: [ADR 0022](adr/0022-classes-compare-by-identity.md) makes them
    compare by identity (`is`). A class has `==` only when it conforms to `Eq`
    by hand, so a comparison that could follow a cycle is never hidden behind
    `==`. Classes get no `Hash`: a class object used as a `Map` key and then
    changed would corrupt the map, and hashing by identity would disagree with
    a hand-written `Eq`.
  - Floats: [ADR 0017](adr/0017-floats-follow-ieee-754.md) settles them. `f64`
    conforms to `Eq` and `Ord` with IEEE semantics, so NaN is not equal to
    itself and `-0.0 == 0.0`, and it gets no `Hash`.
  - `Hash`'s shape is decided for milestone 11:
    `interface Hash: Eq { fn hash(self, h: mut Hasher) }`.
  - Function values: whether they have `==` and `print`, and so what a
    record or class that holds one gets.
- **Integer widths and wrapping.** `Hash` no longer needs a width or wrapping
  arithmetic in the language, since the runtime's `Hasher` holds it. `u64` and
  wrapping operators wait for the first Sisu code that needs them (ADR 0003),
  and other widths, such as `i32`, for the FFI.
