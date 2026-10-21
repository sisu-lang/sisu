<p align="center">
  <img src="docs/images/sisu-logo.png" alt="Sisu logo: a letter S folded from a ribbon that shades from blue to orange, with the word sisu on its lower curve" width="160">
</p>

# Sisu

[![CI](https://github.com/sisu-lang/sisu/actions/workflows/ci.yml/badge.svg)](https://github.com/sisu-lang/sisu/actions/workflows/ci.yml)

Sisu is a small, statically typed programming language that compiles to native
x86-64 and ARM64 Linux code. Sisu is Finnish for grit.

Sisu takes its readable syntax from Python, its strict checks from Rust, and
reference counting and optionals from Swift. Programs read top to
bottom, the compiler catches mistakes before a program runs, and memory is
freed without a garbage collector.

Sisu is for command-line tools, scripts and server-side software that should
be fast, small and reliable, and for code that agents write and people must
verify. [ADR 0027](docs/adr/0027-what-sisu-is-for.md) (proposed) records what
Sisu is for and proposes what 1.0 means.

Types are checked at compile time and inferred for local variables. There is
no null: a value that may be missing has an optional type such as `Tree?`, and
the checker makes the program handle `None`.

Sisu is a passion project at an early stage. Any change may break programs
that compiled before it. See [Status](#status).

## What Sisu values

The goal is that people are productive in Sisu. Four values serve that goal, in
order of priority:

1. **Correctness.** A program never gives a silent wrong answer. Failures are
   loud and name the source line. Nothing changes where the reader cannot see
   it, and no value converts implicitly.
2. **Simplicity.** Few concepts, rules without special cases, one obvious way
   to do a thing, and every effect visible where it happens.
3. **Readability.** Little punctuation, code that reads top to bottom, and
   errors in plain words.
4. **Performance.** Sisu compiles directly to native code, and reference
   counting keeps memory management predictable. Optimizations never change
   what a program means.

Simplicity ranks above readability because agents now write and read much of
the code. Their mistakes come from special cases and effects that happen
elsewhere, and a strict checker with plain-word errors lets them fix their own
code.

Sisu leaves out lifetimes and a borrow checker, inheritance and macros; the
[roadmap](docs/roadmap.md#deliberately-avoided) says why.

## A taste of Sisu

Classes hold fields and methods. A `Tree?` is either a tree or `None`; `?.`
calls through it and `??` gives a fallback. Reference counting frees an object
when its last reference goes away.

<!-- GitHub has no Sisu grammar; Rust's is the closest, so the examples use it. -->

```rust
class Tree {
    value: i64
    left: Tree?
    right: Tree?

    fn sum(self) -> i64 {
        self.value + (self.left?.sum() ?? 0) + (self.right?.sum() ?? 0)
    }
}

fn leaf(value: i64) -> Tree {
    Tree(value: value, left: None, right: None)
}

fn main() {
    let tree = Tree(value: 1, left: leaf(2), right: leaf(3))
    print(tree.sum())               // 6
    print(tree.left?.value ?? 0)    // 2
}
```

Arrays are values. Assigning one shares it until one side is written, so
writing a copy never changes the original; class objects inside an array stay
shared, as objects always are. `mut` marks everything that can change, and
comparisons chain as in mathematics.

```rust
fn main() {
    let primes = [2, 3, 5, 7]
    let mut copy = primes    // shared until one of them is written
    copy[0] = 11
    print(primes.sum())      // 17
    print(copy.sum())        // 26
    for p in primes {
        if 2 < p <= 5 { print(p) }    // 3, then 5
    }
}
```

Arithmetic never wraps silently. Overflow and division by zero stop the program
and name the line:

```rust
fn main() {
    let big = 9_223_372_036_854_775_807
    print(1)
    print(big + 1)
}
```

```console
$ sisu build overflow.sisu && ./overflow
1
sisu: panic at overflow.sisu:4:11: integer overflow
```

## Errors that help

The checker reports independent errors in one run, without a cascade of
follow-on errors. Each diagnostic has a stable code, points at the code, and
offers a fix when it can:

```rust
class Counter {
    count: i64
}

fn main() {
    let total = 0
    total = 5
    let c = Countre(count: total)
    print(c.count)
}
```

```console
$ sisu check counter.sisu
error[E0001]: cannot assign to `total`
 --> counter.sisu:7:5
  |
6 |     let total = 0
  |         ----- declared with `let` here
7 |     total = 5
  |     ^^^^^^^^^ cannot assign twice
  |
help: declare it with `let mut`
  |
6 |     let mut total = 0
  |         +++

error[E0052]: cannot find function `Countre`
 --> counter.sisu:8:13
  |
8 |     let c = Countre(count: total)
  |             ^^^^^^^
  |
help: a class with a similar name exists: `Counter`
  |
8 -     let c = Countre(count: total)
8 +     let c = Counter(count: total)
  |

sisu: 2 errors
sisu: for more on a code, run `sisu explain <code>`: E0001, E0052
```

`sisu run` compiles a program and runs it in one step. Everything after the
file goes to the program, and `sisu` exits with the program's exit code.
`-e` takes the program as text, and `;` separates two statements on one line:

```console
$ sisu run wc.sisu words.txt
$ sisu run -e 'fn main() { let x = 2; print(x * 21) }'
42
$ sisu run -e 'class P { x: i64; y: i64 } fn main() { print(P(x: 1, y: 2).y) }'
2
```

`sisu explain E0001` prints why the rule exists, with a broken and a fixed
program. `sisu check --format json` writes the same diagnostics as JSON Lines for
editors and agents, and marks each fix as safe or unsafe.
[Diagnostic codes](docs/diagnostics/README.md) lists every code.

## Status

Sisu has no releases and runs on x86-64 and ARM64 Linux only. Today it compiles
functions, `i64`, `f64`, `u8`, `bool` and `char`, classes with methods,
optionals, enums with payloads and methods, `match` with nested,
alternative, guarded and range patterns, arrays, and strings with `+`, `+=`,
interpolation (with width and precision specs) and `chars()`. Programs
read standard input with `read_line()`, write standard error with `eprint`,
take `main(args)` and stop with `panic`. Conversions that can fail return an optional. An expression
statement whose value nothing uses warns (`W0004`); `let _ = e` drops a value
on purpose. The
[roadmap](docs/roadmap.md) proposes generics, closures,
records, modules with a standard library, and `Result` with `?`.

The compiler, `sisu`, is written in Rust and uses LLVM for code generation.

## Start here

| You want to | Read |
| --- | --- |
| Build Sisu and compile a program | [Developing Sisu](docs/development.md) |
| See how the compiler is put together | [Architecture](docs/architecture.md) |
| See what is planned | [Roadmap](docs/roadmap.md) |
| Compare Sisu's speed, memory and binary size with other languages | [Benchmarks](docs/benchmarks.md) |
| Change the code | [Contributing](CONTRIBUTING.md) |
| Look up a term | [Glossary](GLOSSARY.md) |
| Read why a design choice was made | [Decisions](docs/adr) |
