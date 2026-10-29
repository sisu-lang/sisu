# Operators

An *operator* is a symbol or a keyword that makes a new value from one or two operands, such as `+` in `a + b`.

```sisu
fn main() {
    let a = 20
    let b = 22
    print(a + b)                 // 42
    print(a < b && b < 30)       // true
    let o: i64? = None
    print(o ?? a)                // 20
}
```

## Precedence

When an expression has more than one operator, the operator with the higher precedence applies first. So `1 + 2 * 3` is `1 + (2 * 3)`. Parentheses change the order.

```sisu
fn main() {
    print(1 + 2 * 3)             // 7
    print((1 + 2) * 3)           // 9
    print(10 - 4 - 3)            // 3
}
```

This table lists the operators from the highest precedence to the lowest:

| Operators | Kind | Grouping |
| --- | --- | --- |
| `.`, `?.`, `[]`, a call `f(...)` | Postfix | From left to right |
| `-`, `!` | Unary prefix | From right to left |
| `*`, `/`, `%` | Multiplication | From left to right |
| `+`, `-` | Addition | From left to right |
| `??` | Default value | From right to left |
| `<`, `<=`, `>`, `>=` | Comparison | A chain |
| `==`, `!=`, `is` | Equality | Does not chain |
| `&&` | And | From left to right |
| `\|\|` | Or | From left to right |

The rules:

- A postfix operator applies before a unary operator. So `-2.5.floor()` is `-(2.5.floor())`.
- A unary operator applies before a binary operator. So `-2 * 3` is `(-2) * 3`.
- `??` applies after `+` and before a comparison. So `o ?? 1 + 2` is `o ?? (1 + 2)`, and `o ?? 0 < 5` is `(o ?? 0) < 5`.
- `a ?? b ?? c` is `a ?? (b ?? c)`.
- A comparison and an equality do not group from left to right: see [Comparison chains](#comparison-chains) and [Equality](#equality).

```sisu
fn main() {
    print(-2.5.floor())          // -2.0
    print((-2.5).floor())        // -3.0
    let o: i64? = None
    print(o ?? 1 + 2)            // 3
    print(o ?? 0 < 5)            // true
}
```

The program calculates the operands of a binary operator from left to right, then applies the operator. `&&`, `||` and `??` can skip their right operand, as their sections tell.

A `..` or `..=` range is not an operator. It is part of a [`for` loop](statements.md#for) or a [pattern](enums.md#literals-and-ranges), and is an error in other places ([E0023](../diagnostics/E0023.md)). The section [Loops over iterables](statements.md#loops-over-iterables) is in development, and it makes a range a value.

## Arithmetic

`+`, `-`, `*`, `/` and `%` take two numbers of one type: two `i64`s, two `u8`s or two `f64`s. The result has that type. `+` also joins two `string`s.

```sisu
fn main() {
    let b: u8 = 200
    print(7 + 2 * 3)             // 13
    print(b + 55)                // 255
    print(1.5 * 4.0)             // 6.0
    print("sau" + "na")          // sauna
}
```

The rules:

- The two operands must have one type. Sisu does not convert an operand to the type of the other ([E0032](../diagnostics/E0032.md)). Integer literals are the exception ([Literals](literals.md#the-type-of-an-integer-literal)).
- Arithmetic does not apply to a `char` ([E0094](../diagnostics/E0094.md)), a `bool`, an optional or an array.
- A `+` with a `string` and another type is an error. Use interpolation, such as `"n = {n}"`.
- Sisu has no `**` operator ([E0107](../diagnostics/E0107.md)). For a power, use the `f64` method `pow` ([Types](types.md#methods-on-numbers)).

```sisu
fn main() {
    let n = 2
    let x = 1.5
    print(n * x)                 // error[E0032]
}
```

### Division and remainder

On integers, `/` gives the quotient without its fraction: it rounds toward zero. `%` gives the remainder, which has the sign of the left operand. So `a == a / b * b + a % b` is always true.

```sisu
fn main() {
    print(7 / 2)                 // 3
    print(-7 / 2)                // -3
    print(7 % 3)                 // 1
    print(-7 % 3)                // -1
    print(7 % -3)                // 1
}
```

On `f64`, `/` is the IEEE 754 division. `%` gives the remainder of a division that rounds toward zero, with the sign of the left operand.

```sisu
fn main() {
    print(7.0 / 2.0)             // 3.5
    print(7.5 % 2.0)             // 1.5
    print(-7.5 % 2.0)            // -1.5
}
```

### Overflow

At run time, an integer `+`, `-` or `*` whose result is outside the range of its type stops the program with a panic. Sisu never wraps a result. These operations also panic with an overflow:

- a unary `-` on the smallest `i64`
- `/` and `%` when the left operand is the smallest `i64` and the right operand is `-1`

```sisu panics
fn main() {
    let big = 9223372036854775807
    print(big * 2)
}
```

```stderr
sisu: panic at example.sisu:3:11: integer overflow
```

A `u8` result must be from 0 to 255, so a `u8` subtraction below zero panics too.

```sisu panics
fn main() {
    let a: u8 = 1
    let b: u8 = 2
    print(a - b)
}
```

```stderr
sisu: panic at example.sisu:4:11: integer overflow
```

`f64` arithmetic never panics. A result that is too large is an infinity ([Types](types.md#floating-point-numbers)).

### Division by zero

At run time, `/` or `%` on integers with a right operand of zero stops the program with a panic.

```sisu panics
fn main() {
    let zero = 0
    print(7 / zero)
}
```

```stderr
sisu: panic at example.sisu:3:11: division by zero
```

On `f64`, a division by zero does not panic. It gives an infinity or NaN.

```sisu
fn main() {
    print(7.0 / 0.0)             // inf
    print(7.0 % 0.0)             // nan
}
```

### Arithmetic in a constant

The program calculates a [constant](statements.md#constants) when it builds. So an integer overflow or an integer division by zero in a constant is an error, not a panic ([E0088](../diagnostics/E0088.md)).

```sisu
const LIMIT = 9223372036854775807 + 1    // error[E0088]

fn main() {
    print(LIMIT)
}
```

Floating-point arithmetic in a constant follows the rules that it has at run time. A division by zero gives an infinity or NaN, and is not an error.

```sisu
const INFINITY = 1.0 / 0.0
const NAN = 0.0 / 0.0

fn main() {
    print(INFINITY)              // inf
    print(NAN)                   // nan
}
```

## Unary operators

A unary `-` changes the sign of an `i64` or an `f64`. `!` gives the opposite of a `bool`.

```sisu
fn main() {
    let n = 5
    let x = 2.5
    print(-n)                    // -5
    print(-x)                    // -2.5
    print(!true)                 // false
}
```

The rules:

- A unary `-` on a `u8` is an error, because a `u8` cannot be negative ([E0042](../diagnostics/E0042.md)).
- `!` takes only a `bool` ([E0032](../diagnostics/E0032.md)).
- A `-` before an integer literal is a unary `-`, not part of the literal ([Literals](literals.md#integer-literals)).

## Comparison

`<`, `<=`, `>` and `>=` compare two values of one type and give a `bool`. The types are `i64`, `u8`, `f64`, `char` and `string`.

```sisu
fn main() {
    print(2 < 3)                 // true
    print(2.5 >= 3.0)            // false
    print('a' < 'b')             // true
    print("apple" < "banana")    // true
}
```

The rules:

- A `char` compares by its code point. A `string` compares byte by byte, which is the order of the code points. So the ASCII upper case letters come before the ASCII lower case letters, and a prefix comes before the longer string.
- An `f64` comparison follows IEEE 754: a comparison with NaN is false.
- A `bool`, an array, a class or an enum has no `<` ([E0159](../diagnostics/E0159.md)), unless a class or an enum conforms to `Ord` ([Display, Eq and Ord](generics.md#display-eq-and-ord)). An optional cannot be compared with `<` ([E0032](../diagnostics/E0032.md)).
- The two operands must have one type ([E0032](../diagnostics/E0032.md)).

### Comparison chains

A *comparison chain* such as `a < b < c` is true when each comparison in it is true. Here, these are `a < b` and `b < c`. The program calculates each operand at most once, and stops at the first comparison that is false.

```sisu
fn main() {
    let n = 5
    print(0 <= n < 10)           // true
    print(1 < 2 < 2)             // false
    print("a" < "b" <= "b")      // true
}
```

A chain goes in one direction: all its operators are `<` and `<=`, or all are `>` and `>=` ([E0025](../diagnostics/E0025.md)).

```sisu
fn main() {
    print(1 < 2 > 0)             // error[E0025]
}
```

## Equality

`==` is true when two values of one type are equal. `!=` is its opposite.

```sisu
fn main() {
    print(2 + 2 == 4)            // true
    print("sauna" != "sisu")     // true
    print([1, 2] == [1, 2])      // true
    let o: i64? = 5
    print(o == 5)                // true
    print(o == None)             // false
}
```

The rules:

- Two [arrays](arrays.md#equality) are equal when they have the same length and their elements are equal, in order. Two [enum](enums.md#enum-values) values are equal when they are the same variant and their payloads are equal.
- A `T` beside a `T?` becomes a `T?`. Two optionals are equal when both are `None`, or both hold equal values.
- `x == None` and `x != None` test whether the optional `x` holds a value. When `x` is not optional, the test is an error ([E0044](../diagnostics/E0044.md)).
- Values of two types cannot be compared ([E0047](../diagnostics/E0047.md), [E0032](../diagnostics/E0032.md)).
- A class has no `==`, because objects compare by identity. Use `a is b` ([`is`](#is)), or make the class conform to `Eq` and write its `eq` method. Without it, `a == b` is an error ([E0159](../diagnostics/E0159.md)), and so is `==` on an array or an optional of that class ([Display, Eq and Ord](generics.md#display-eq-and-ord)).
- An `f64` follows IEEE 754: NaN is not equal to itself, and `-0.0 == 0.0` is true. So an array that holds NaN is not equal to itself.
- `==`, `!=` and `is` do not chain: `a == b == c` is an error ([E0024](../diagnostics/E0024.md)). Join two tests with `&&`.

```sisu
fn main() {
    let a = 1
    let b = 1
    print(a == b == true)        // error[E0024]
}
```

### `is`

`a is b` is true when `a` and `b` refer to the same object. It applies to [classes](classes.md#identity), and to optionals of classes. When two optionals are both `None`, `a is b` is true. An object and `None` are not the same.

```sisu
class Sauna {
    heat: i64
}

fn main() {
    let a = Sauna(heat: 80)
    let b = Sauna(heat: 80)
    let c = a
    print(a is c)                // true
    print(a is b)                // false
    print(a.heat == b.heat)      // true
    let x: Sauna? = None
    let y: Sauna? = None
    print(x is y)                // true
    print(x is a)                // false
}
```

`is` on a value that is not an object, such as an `i64`, a `string`, an array or an enum value, is an error ([E0046](../diagnostics/E0046.md)). To test an optional for `None`, use `== None` ([E0045](../diagnostics/E0045.md)). [Values and memory](values.md#identity-and-equality) compares `is` with `==`.

## Logical operators

`&&` is true when both operands are true. `||` is true when one or both operands are true. The operands must be `bool`s.

```sisu
fn main() {
    let n = 7
    print(n > 0 && n < 10)       // true
    print(n < 0 || n == 7)       // true
}
```

`&&` and `||` calculate the right operand only when it can change the result. When the left operand of `&&` is false, or the left operand of `||` is true, the right operand does not run.

```sisu
fn check(b: bool) -> bool {
    print("checked")
    return b
}

fn main() {
    print(false && check(true))
    print(true || check(false))
    print(true && check(true))
}
```

```output
false
true
checked
true
```

## Default values

`a ?? b` gives the value in the optional `a`. When `a` is `None`, it gives `b`. The program calculates `b` only when `a` is `None`.

```sisu
fn main() {
    let found: i64? = 7
    let missing: i64? = None
    print(found ?? 0)            // 7
    print(missing ?? 0)          // 0
}
```

The rules:

- The left operand must be an optional ([E0048](../diagnostics/E0048.md)).
- When `a` is a `T?` and `b` is a `T`, the result is a `T`.
- When `b` is a `T?` too, the result is a `T?`. So `a ?? b ?? 0` tries `a`, then `b`, then gives `0`.
- `b` can be an expression that never finishes, such as `panic("...")` ([Types](types.md#never)). Then the result is a `T`.

```sisu
fn main() {
    let a: i64? = None
    let b: i64? = 3
    print(a ?? b ?? 0)           // 3
    print(b ?? panic("no b"))    // 3
}
```

## Optional chaining

`a?.name` reads the field `name` of the optional `a`, and `a?.name(...)` calls its method. When `a` is `None`, the result is `None`. Then the method and its arguments do not run.

```sisu
class Room {
    name: string
    next: Room?
}

fn main() {
    let hall = Room(name: "hall", next: Room(name: "sauna", next: None))
    print(hall.next?.name ?? "none")           // sauna
    print(hall.next?.next?.name ?? "none")     // none
}
```

The rules:

- The result is an optional of the type of the field or the method's result. When that type is already an optional, the result is that optional, not an optional of an optional. So a chain such as `a?.b?.c` gives one optional.
- `a?.b.c` is an error when `b` is not optional, because `a?.b` is an optional ([E0062](../diagnostics/E0062.md)). Write `a?.b?.c`.
- A method that returns no value has no value through `?.` too ([E0043](../diagnostics/E0043.md)). When `a` is `None`, the method and its arguments do not run.
- `?.` on a value that is not optional is an error ([E0049](../diagnostics/E0049.md)). Use `.`.
- An assignment cannot go through `?.` ([E0022](../diagnostics/E0022.md)).
- A line that starts with `?.` continues the line above it ([Lexical structure](lexical.md#line-continuation)).

```sisu
class Sauna {
    heat: i64

    fn show(self, label: string) {
        print("{label}: {self.heat}")
    }
}

fn label() -> string {
    print("label")
    return "heat"
}

fn main() {
    let on: Sauna? = Sauna(heat: 80)
    let off: Sauna? = None
    on?.show(label())
    off?.show(label())
}
```

```output
label
heat: 80
```

## Passing a failure up

`x?` on a result gives the success, or ends the enclosing function with the failure. On an optional, `x?` gives the value, or ends the function with `None`. `r ?? d` on a result gives the success, or `d` on any failure ([The `?` operator](errors.md#the--operator)).

```sisu pending
fn parse(text: string) -> i64 | string {
    i64(text) ?? fail "not a number: {text}"
}

fn twice(text: string) -> i64 | string {
    parse(text)? * 2
}

fn main() {
    print(twice("21") ?? 0)            // 42
}
```

`?` is a postfix operator and binds as a call does.
