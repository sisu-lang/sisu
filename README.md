<p align="center">
  <img src="docs/images/sisu-logo.png" alt="Sisu logo: a letter S folded from a ribbon that shades from blue to orange, with the word sisu on its lower curve" width="160">
</p>

# Sisu

[![CI](https://github.com/sisu-lang/sisu/actions/workflows/ci.yml/badge.svg)](https://github.com/sisu-lang/sisu/actions/workflows/ci.yml)

Sisu is a small, statically typed programming language that compiles to native
x86-64 and ARM64 Linux code and ARM64 macOS code. Sisu is Finnish for grit.

<!-- GitHub has no Sisu grammar; Rust's is the closest, so the examples use it. -->

```rust
enum Expr {
    Num(i64)
    Var(string)
    Add(Expr, Expr)
    Mul(Expr, Expr)

    fn show(self) -> string {
        match self {
            Num(n) => "{n}"
            Var(name) => name
            Add(a, b) => "({a.show()} + {b.show()})"
            Mul(a, b) => "{a.show()} * {b.show()}"
        }
    }
}

fn simplify(e: Expr) -> Expr {
    match e {
        Add(Num(0), x) | Add(x, Num(0)) => simplify(x)
        Mul(Num(1), x) | Mul(x, Num(1)) => simplify(x)
        Mul(Num(0), _) | Mul(_, Num(0)) => Expr.Num(0)
        other => other
    }
}

fn main() {
    let e = Expr.Add(Expr.Mul(Expr.Var("x"), Expr.Num(1)), Expr.Num(0))
    print("{e.show()} is {simplify(e).show()}")    // (x * 1 + 0) is x
}
```

An enum holds data, and `match` takes the data apart with nested patterns and
alternatives. To keep the example short, `simplify` rewrites only the outer
expression. The checker makes sure that each `match` covers every value.
Remove the `other` arm, and `sisu check` shows a value that no arm takes:

```console
$ sisu check simplify.sisu
error[E0130]: `match` does not cover `Num(_)`
  --> simplify.sisu:18:5
   |
18 |     match e {
   |     ^^^^^
   |
help: add a `_ =>` arm
   |
22 +         _ => todo
   |

sisu: 1 error
sisu: for more on a code, run `sisu explain <code>`: E0130
```

Sisu takes its readable syntax from Python, its strict checks from Rust, and
reference counting and optionals from Swift. Programs read top to
bottom, the compiler catches mistakes before a program runs, and memory is
freed without a garbage collector.

Sisu is for command-line tools, scripts and server-side software that should
be fast, small and reliable, and for code that agents write and people must
verify. [ADR 0027](docs/adr/0027-what-sisu-is-for.md) (proposed) records what
Sisu is for and proposes what 1.0 means.

Types are checked at compile time and inferred for local variables. There is
no null: a value that may be missing has an optional type such as `User?`, and
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

A `User?` is either a user or `None`. `?.` reads through it, and `??` gives a
fallback. Reference counting frees an object when its last reference goes away.

```rust
class User {
    name: string
    manager: User?
}

fn main() {
    let ada = User(name: "Ada", manager: None)
    let bob = User(name: "Bob", manager: ada)
    print(bob.manager?.name ?? "nobody")    // Ada
    print(ada.manager?.name ?? "nobody")    // nobody
    // print(ada.manager.name) gives error[E0062]: `ada.manager` may be `None`
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
    print(primes[0])         // 2
    print(copy[0])           // 11

    let mut lower = 0
    for c in "Hello, World".chars() {
        if 'a' <= c <= 'z' { lower += 1 }
    }
    print(lower)             // 8
}
```

Arithmetic never wraps silently. Overflow and division by zero stop the program
and name the line. An `i64` holds 20!, but not 21!:

```rust
fn main() {
    let mut f = 1
    for n in 1..=25 {
        f *= n
        print("{n}! = {f}")
    }
}
```

```console
$ sisu build factorial.sisu && ./factorial
1! = 1
2! = 2
...
20! = 2432902008176640000
sisu: panic at factorial.sisu:4:9: integer overflow
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
$ sisu run -e 'class P { x: i64; y: i64 } fn main() { let p = P(x: 1, y: 2); print(p.x + p.y) }'
3
```

`sisu explain E0001` prints why the rule exists, with a broken and a fixed
program. `sisu check --format json` writes the same diagnostics as JSON Lines for
editors and agents, and marks each fix as safe or unsafe.
[Diagnostic codes](docs/diagnostics/README.md) lists every code.

## Status

Sisu has no releases and runs on x86-64 and ARM64 Linux and ARM64 macOS only.
Today it compiles:

- functions, and the types `i64`, `f64`, `u8`, `bool` and `char`
- classes with methods, optionals, and enums with payloads and methods
- generic functions, classes and enums, and interfaces: bounds such as
  `T: Shape & Named`, conformance in a declaration or an `extend` block, and
  associated types
- `match` with nested, alternative, guarded and range patterns
- arrays, and strings with `+`, `+=`, `chars()` and interpolation, with width
  and precision specs
- `main(args)`, `read_line()` from standard input, `eprint` to standard error,
  and `panic`

A conversion that can fail returns an optional. An expression statement whose
value nothing uses gives warning `W0004`; `let _ = e` drops a value on purpose.

The [roadmap](docs/roadmap.md) proposes interface values, closures, records,
modules with a standard library, and `Result` with `?`.

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
