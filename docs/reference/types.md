# Types

A *type* is a set of values and the operations that apply to them. Every value, binding and expression in Sisu has one type, and `sisu` knows it before the program runs.

```sisu
fn main() {
    let n: i64 = 42
    let b: u8 = 255
    let x: f64 = 2.5
    let ok: bool = true
    let c: char = 'ä'
    let s: string = "sauna"
    print("{n} {b} {x} {ok} {c} {s}")    // 42 255 2.5 true ä sauna
}
```

These are the built-in types:

| Type | Values |
| --- | --- |
| `i64` | The integers from -9223372036854775808 to 9223372036854775807 |
| `u8` | The integers from 0 to 255 |
| `f64` | 64-bit floating-point numbers, as IEEE 754 defines them |
| `bool` | `true` and `false` |
| `char` | One Unicode scalar value |
| `string` | Text in UTF-8 |

A program can also use [arrays](arrays.md), such as `[i64]`, optionals, such as `i64?`, and the [classes](classes.md) and enums that it declares.

## Type names

A type name comes after a `:` in a `let`, a parameter or a field, and after `->` for the return type of a function.

```sisu
fn twice(n: i64) -> i64 {
    return n * 2
}

fn main() {
    let names: [string] = ["Aino", "Eero"]
    let found: i64? = None
    print(twice(21))                     // 42
    print(names.len())                   // 2
    print(found ?? 0)                    // 0
}
```

The forms of a type name:

- A built-in type, such as `i64`.
- The name of a class or an enum that the program declares. A generic class or enum also takes type arguments, such as `Stack<i64>` ([Generics](generics.md#type-arguments)).
- `[T]`, an array of elements of the type `T`.
- `T?`, an optional of the type `T`.

A name that is not a type is an error ([E0030](../diagnostics/E0030.md)).

When a `let` has no type name, the binding gets the type of its value. A `None` or a `[]` alone does not tell its type, so it needs a type name ([E0050](../diagnostics/E0050.md)).

```sisu
fn main() {
    let found = None           // error[E0050]
}
```

## Integers

An `i64` is a signed 64-bit integer. A `u8` is an unsigned 8-bit integer, a byte. An integer literal is an `i64`, unless its context makes it a `u8` or an `f64` ([Literals](literals.md#the-type-of-an-integer-literal)).

```sisu
fn main() {
    let n = 1000
    let b: u8 = 200
    print(n * 3)               // 3000
    print(b + 55)              // 255
}
```

The rules:

- When the result of integer arithmetic is outside the range of its type, the program stops with a panic ([Operators](operators.md#overflow)). In a [constant](statements.md#constants), this is an error when the program builds ([E0088](../diagnostics/E0088.md)).
- A `u8` cannot be negative, so a unary `-` on a `u8` is an error ([E0042](../diagnostics/E0042.md)).
- An `i64` and a `u8` do not mix in one operation. Convert one of them first ([Conversions](#conversions)).

```sisu
fn main() {
    let b: u8 = 200
    let n = 1000
    print(n + b)               // error[E0032]
}
```

## Floating-point numbers

An `f64` is a 64-bit floating-point number. Its arithmetic follows IEEE 754 and never panics. A division by zero gives an infinity or NaN ("not a number"), and a result that is too large gives an infinity.

```sisu
fn main() {
    print(7.0 / 2.0)           // 3.5
    print(0.1 + 0.2)           // 0.30000000000000004
    print(1.0 / 0.0)           // inf
    print(-1.0 / 0.0)          // -inf
    print(0.0 / 0.0)           // nan
    print(1e300 * 1e300)       // inf
}
```

The rules:

- NaN is not equal to any value, also not to itself. To test for NaN, use `x.is_nan()`. `x == f64.nan` is always false, and `x != f64.nan` is always true. Both build with a warning ([W0005](../diagnostics/W0005.md)). `<`, `<=`, `>` and `>=` with NaN are always false.
- `-0.0 == 0.0` is true, but `print` shows the sign: `-0.0`.
- An `f64` and an integer do not mix in one operation. Convert one of them first ([Conversions](#conversions)).

```sisu
fn main() {
    let x = f64.nan
    print(x == x)              // false
    print(x.is_nan())          // true
    print(-0.0 == 0.0)         // true
}
```

### Constants

`f64` has these constants. Write them after the type name, such as `f64.pi`.

| Constant | Value |
| --- | --- |
| `f64.pi` | π, 3.141592653589793 |
| `f64.tau` | 2π, 6.283185307179586 |
| `f64.e` | e, 2.718281828459045 |
| `f64.infinity` | Positive infinity. `-f64.infinity` is negative infinity |
| `f64.nan` | NaN |
| `f64.max` | The largest finite `f64`, 1.7976931348623157e308 |
| `f64.min_positive` | The smallest positive normal `f64`, 2.2250738585072014e-308 |
| `f64.epsilon` | The difference between 1.0 and the next larger `f64`, 2.220446049250313e-16 |

`i64` and `u8` have no constants.

## Methods on numbers

`i64` and `f64` have these methods. Each parameter has the type of the receiver. `u8` has no methods.

| Method | Receivers | Result |
| --- | --- | --- |
| `x.min(y)` | `i64`, `f64` | The smaller of `x` and `y` |
| `x.max(y)` | `i64`, `f64` | The larger of `x` and `y` |
| `x.clamp(lo, hi)` | `i64`, `f64` | `x.max(lo).min(hi)`: `x` moved into the range from `lo` to `hi` |
| `x.abs()` | `f64` | The absolute value |
| `x.sqrt()` | `f64` | The square root |
| `x.pow(y)` | `f64` | `x` to the power `y` |
| `x.exp()` | `f64` | e to the power `x` |
| `x.ln()`, `x.log10()`, `x.log2()` | `f64` | The logarithm to the base e, 10 or 2 |
| `x.floor()`, `x.ceil()` | `f64` | The nearest whole number below or above `x` |
| `x.trunc()` | `f64` | `x` without its fraction |
| `x.round()` | `f64` | The nearest whole number. A half goes away from zero |
| `x.sin()`, `x.cos()`, `x.tan()` | `f64` | The sine, cosine or tangent of `x` radians |
| `x.asin()`, `x.acos()`, `x.atan()` | `f64` | The inverse sine, cosine or tangent, in radians |
| `y.atan2(x)` | `f64` | The angle in radians of the point (`x`, `y`) |
| `x.hypot(y)` | `f64` | The square root of `x * x + y * y` |
| `x.is_nan()` | `f64` | `true` when `x` is NaN |
| `x.is_finite()` | `f64` | `true` when `x` is not an infinity and not NaN |
| `x.is_infinite()` | `f64` | `true` when `x` is an infinity |

The `f64` methods return an `f64`, except the three `is_` methods, which return a `bool`. `min`, `max` and `clamp` return the type of the receiver.

```sisu
fn main() {
    print(2.0.sqrt())              // 1.4142135623730951
    print(2.5.round())             // 3.0
    print((-2.5).round())          // -3.0
    print(7.clamp(0, 5))           // 5
    print(1.0.atan2(0.0))          // 1.5707963267948966
}
```

The rules:

- When `lo` is larger than `hi`, `clamp` gives `hi`.
- On `f64`, `min`, `max` and `clamp` order `-0.0` below `0.0`. When one operand of `min` or `max` is NaN, they give the other operand.

```sisu
fn main() {
    print(7.clamp(5, 0))           // 0
    print((-0.0).max(0.0))         // 0.0
    print(f64.nan.min(2.0))        // 2.0
}
```

## bool

A `bool` is `true` or `false`. The condition of an `if` or a `while` must be a `bool`: Sisu does not change a number or another value into a `bool` ([E0032](../diagnostics/E0032.md)).

```sisu
fn main() {
    let n = 3
    if n > 0 {
        print("positive")      // positive
    }
}
```

```sisu
fn main() {
    let n = 3
    if n {                     // error[E0032]
        print("positive")
    }
}
```

## char

A `char` is one Unicode scalar value: a code point from 0 to 10FFFF, but not D800 to DFFF. Two `char`s compare by their code points.

```sisu
fn main() {
    let c = 'ä'
    print(c)                   // ä
    print(i64(c))              // 228
    print('a' < 'ä')           // true
}
```

Arithmetic does not apply to a `char` ([E0094](../diagnostics/E0094.md)). To calculate with a code point, convert the `char` to an `i64` and back.

```sisu
fn main() {
    let c = 'a'
    let next = char(i64(c) + 1) ?? c
    print(next)                // b
}
```

## string

A `string` is text in UTF-8. `+` joins two strings, and a string literal can interpolate values ([Literals](literals.md#interpolation)). Two strings compare byte by byte, which is the order of their code points.

```sisu
fn main() {
    let name = "Aino"
    let greeting = "Hello, " + name
    print(greeting)            // Hello, Aino
    print("Z" < "a")           // true
}
```

A `string` is not an array. An index on a string is an error ([E0068](../diagnostics/E0068.md)). [Strings and formatting](strings.md) tells the methods of a string and its format specs.

## Optionals

An *optional* `T?` holds a value of the type `T`, or `None`. Use `??` to give a value for `None`, and [`if let`](enums.md#if-let-and-while-let) to use the value only when it is there.

```sisu
fn main() {
    let found: i64? = 7
    let missing: i64? = None
    print(found ?? 0)          // 7
    print(missing ?? 0)        // 0
    if let n = found {
        print(n * 2)           // 14
    }
}
```

The rules:

- Where an optional `T?` is expected, a `T` becomes a `T?` that holds it. Some examples are a `let` with a type, an assignment, an argument and a `return` value. Others are a field, an array element, the other side of an `==` or `!=`, and the other branches of an `if` or a `match`.
- `None` needs an expected optional type. Where the context expects a type that is not optional, `None` is an error ([E0032](../diagnostics/E0032.md)).
- An optional is not its value. An arithmetic operator, a comparison, a field or a method of `T` does not apply to a `T?` ([E0032](../diagnostics/E0032.md), [E0062](../diagnostics/E0062.md)). Use `??`, `?.` ([Operators](operators.md#default-values)) or [`if let`](enums.md#if-let-and-while-let).
- `x == None` and `x != None` test whether `x` holds a value. When `x` is not optional, the test is an error ([E0044](../diagnostics/E0044.md)).
- An optional of an optional, such as `i64??`, is not a type ([E0014](../diagnostics/E0014.md)). The section [Option and Result](#option-and-result) is in development, and it changes this rule.
- `print` and interpolation do not take an optional ([E0059](../diagnostics/E0059.md), [E0098](../diagnostics/E0098.md)).

```sisu
fn half(n: i64) -> i64? {
    if n % 2 == 1 {
        return None
    }
    return n / 2
}

fn main() {
    print(half(10) ?? -1)      // 5
    print(half(7) ?? -1)       // -1
    let o = if half(4) == 2 { 1 } else { None }
    print(o ?? 0)              // 1
}
```

```sisu
fn main() {
    let o: i64? = 5
    print(o + 1)               // error[E0032]
}
```

## Option and Result

`T?` is short for `Option<T>`, and `T | E` is short for `Result<T, E>`. `Option` and `Result` are enums that a program uses with no declaration. Both spellings of a type are the same type.

```sisu pending
fn half(n: i64) -> Option<i64> {
    if n % 2 == 1 {
        return None
    }
    Some(n / 2)
}

fn main() {
    let a: i64? = half(8)
    match a {
        Some(n) => print(n)            // 4
        None => print("odd")
    }
}
```

The rules:

- `Option<T>` is the enum with the variants `None` and `Some(T)`. `Result<T, E>` is the enum with the variants `Ok(T)` and `Err(E)`. [Errors and Result](errors.md#results) tells how a result is used.
- `Some`, `None`, `Ok` and `Err` are names in expressions and in patterns, without the name of the enum.
- A pattern whose outermost name is `Some` or `None` matches the optional itself. Any other pattern matches the value inside it. So `if let` takes any pattern for the type of the value inside.
- A `match` on an optional takes the arms `Some(...)` and `None`.
- A `print`, `eprint` or interpolation of an optional builds with a warning, because the text can be `None`. `x.to_string()` gives the same text with no warning, for a program that wants `None` in its output.

### Optionals of optionals

An optional can hold an optional. `i64??` is `Option<Option<i64>>`. So generic code that finds the first element of a `[i64?]` gives an `i64??`. `None` tells that the array is empty. `Some(None)` tells that the first element is `None`.

```sisu pending
fn first<T>(xs: [T]) -> T? {
    if xs.len() > 0 { xs[0] } else { None }
}

fn main() {
    let xs: [i64?] = [None, 5]
    let a: i64?? = first(xs)
    print((a ?? 7) ?? 8)               // 8
    let b: i64?? = Some(None)
    print((b ?? Some(7)) ?? 8)         // 8
    let c: i64?? = None
    print((c ?? Some(7)) ?? 8)         // 7
}
```

The rules:

- A plain value becomes an optional through one layer only. The middle state of an `i64??` is written `Some(None)`.
- `??` and `if let` peel one layer. On an `i64??` named `x`, `x ?? 5` gives an `i64?`, and `(x ?? None) ?? 0` gives an `i64`. `if let y = x` binds `y` with the type `i64?`.
- `?.` removes one layer, as it does now. If `a?.b` has a `b` of the type `i64?`, it gives an `i64?`, not an `i64??`.
- A type argument can be an optional. This lifts the rule in [Type arguments](generics.md#type-arguments).

## never

`never` is the type of an expression that never finishes. These expressions have the type `never`:

- A call of [`panic`](functions.md#panic), and [`todo`](functions.md#todo).
- A block that reaches a `return`, `break` or `continue` statement, or another expression of the type `never`.
- An `if` or a `match` whose branches are all of the type `never`.
- A `loop` with no `break` that ends it, or whose `break`s all have a value of the type `never`.
- A [`while`](statements.md#reachability) whose condition is the literal `true`, with no `break` that ends it.
- A string literal with an interpolation part of the type `never`.

A program cannot write `never` as a type name. A `never` branch of an `if` or a `match` takes the type of the other branches. So does a `never` `break` value of a `loop`. The right side of `??` can be `never`.

```sisu
fn parse(text: string) -> i64 {
    let n = if text == "one" { 1 } else { return -1 }
    return n * 10
}

fn main() {
    let o: i64? = 3
    print(o ?? panic("no value"))    // 3
    print(parse("one"))              // 10
    print(parse("two"))              // -1
}
```

The rules:

- An expression of the type `never` has no value, so a `let` cannot bind it ([E0043](../diagnostics/E0043.md)), and `print` cannot take it ([E0061](../diagnostics/E0061.md)).
- Code after an expression of the type `never` cannot run, and the program builds with a warning ([W0001](../diagnostics/W0001.md)).
- A function body that ends in an expression of the type `never` needs no value at its end ([Statements](statements.md#the-end-of-a-function)).

```sisu
fn first_even(xs: [i64]) -> i64 {
    for x in xs {
        if x % 2 == 0 {
            return x
        }
    }
    panic("no even number")
}

fn main() {
    print(first_even([3, 8, 5]))     // 8
}
```

## unit

`unit` is the type of an expression that gives no value, such as a call of `print`. It is an ordinary type. A program can write it, for example in `-> unit`.

```sisu pending
fn id<T>(x: T) -> T {
    x
}

fn main() {
    let u = print("x")                 // x
    let v = id(u)
    print(u == v)                      // true
    print(u)                           // ()
}
```

The rules:

- `unit` can be a type argument, such as `[unit]`, `unit?` and `Stack<unit>`. This lifts the rule in [Type arguments](generics.md#type-arguments).
- `unit` has no literal. A `unit` value comes from a call, such as `print(...)`.
- A program can bind, pass, store, return and compare a `unit` value. All `unit` values are equal.
- A `unit` value prints as `()`.

## Conversions

A *conversion* is a call written with the name of a type, such as `f64(n)`. Sisu never converts a value to another type without one, except an integer literal ([Literals](literals.md#the-type-of-an-integer-literal)) and a `T` that becomes a `T?`.

```sisu
fn main() {
    let n = 7
    let x = 2.5
    print(f64(n) * x)              // 17.5
    print(u8(n) ?? 0)              // 7
    print(i64(x) ?? 0)             // 0
}
```

| Conversion | From | Result | `None` when |
| --- | --- | --- | --- |
| `f64(n)` | `i64` | `f64` | Never. Above 2^53, it gives the nearest `f64` |
| `i64(x)` | `f64` | `i64?` | `x` is not a whole number, is NaN or an infinity, or is outside the range of `i64` |
| `i64(b)` | `u8` | `i64` | Never |
| `i64(c)` | `char` | `i64` | Never. The result is the code point |
| `u8(n)` | `i64` | `u8?` | `n` is outside 0 to 255 |
| `char(n)` | `i64` | `char?` | `n` is not a Unicode scalar value |
| `string(cs)` | `[char]` | `string` | Never |

A conversion that can fail returns an optional. It never panics. To round an `f64` to an `i64`, round it first, for example `i64(x.round())`.

```sisu
fn main() {
    print(i64(2.7) ?? -1)          // -1
    print(i64(2.7.round()) ?? -1)  // 3
    print(u8(300) ?? 0)            // 0
    print(char(228) ?? '?')        // ä
    print(string(['o', 'k']))      // ok
}
```

The rules:

- A conversion takes only the types in its row of the table. A `u8` converts to an `f64` or a `char` through an `i64`, such as `f64(i64(b))`.
- The argument of `f64`, `i64`, `u8` or `char` has no expected type. So an integer literal in it is an `i64`: `f64(3)` is `3.0`, and `u8(300)` is `None`.
- The argument of `string` expects a `[char]`, so `string([])` is the empty string.
- `i64(-0.0)` is `0`.
- A conversion to the type that the value already has is an error ([E0054](../diagnostics/E0054.md)).
- In a constant, a conversion that can fail is an error ([E0100](../diagnostics/E0100.md)).

```sisu
fn main() {
    let n = 5
    print(i64(n))                  // error[E0054]
}
```

### Parsing a string

`i64(s)` on a `string` gives an `i64?`. It follows the rule for a conversion that can fail: the result is `None`, not a panic.

```sisu pending
fn main() {
    print(i64("42") ?? -1)             // 42
    print(i64("-7") ?? -1)             // -7
    print(i64("4 2") ?? -1)            // -1
    print(i64("+5") ?? -1)             // -1
}
```

The result is `None` unless `s` is an optional `-` followed by one or more ASCII digits, and the value fits in an `i64`. A `+`, a space, an underscore or `0x` gives `None`.
