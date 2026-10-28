# Literals

A *literal* is a value that the source writes directly: a number, a character, a string or a boolean.

```sisu
fn main() {
    print(42)          // 42
    print(2.5)         // 2.5
    print('ä')         // ä
    print("sauna")     // sauna
    print(true)        // true
}
```

| Literal | Examples | Type |
| --- | --- | --- |
| Integer | `42`, `1_000_000`, `0xff`, `0b1010`, `0o17` | `i64`, or `u8` or `f64` when the context expects one |
| Float | `2.5`, `1e-3`, `6.674e-11` | `f64` |
| Character | `'a'`, `'ä'`, `'\n'`, `'\u{1F600}'` | `char` |
| String | `"sauna"`, `"n = {n}"` | `string` |
| Boolean | `true`, `false` | `bool` |

## Integer literals

An integer literal is a sequence of decimal digits, such as `42`. Zeros at the start do not change the base: `007` is 7.

```sisu
fn main() {
    print(1_000_000 + 1)    // 1000001
    print(007)              // 7
}
```

A `-` before a literal is the [unary minus operator](operators.md#unary-operators). It is not part of the literal.

### Bases

A prefix gives a different base: `0x` for hexadecimal, `0b` for binary and `0o` for octal. A hexadecimal digit can be upper case or lower case, but the prefix must be lower case.

```sisu
fn main() {
    print(0xff)        // 255
    print(0xFF)        // 255
    print(0b1010)      // 10
    print(0o17)        // 15
}
```

These are errors:

- An upper case prefix, such as `0XFF` ([E0075](../diagnostics/E0075.md)).
- A prefix with no digits after it, such as `0x` ([E0076](../diagnostics/E0076.md)).
- A digit that is not of the base, such as the `2` in `0b102` ([E0077](../diagnostics/E0077.md)).

```sisu
fn main() {
    print(0b102)       // error[E0077]
}
```

A literal with a prefix is an integer. It has no fraction and no exponent.

### Digit separators

A `_` between two digits makes a long number easier to read. It does not change the value.

```sisu
fn main() {
    print(1_000)           // 1000
    print(0xff_ff)         // 65535
    print(0b1111_0000)     // 240
}
```

A `_` must have a digit on each side. So `1__000`, `1000_` and `0x_ff` are errors ([E0008](../diagnostics/E0008.md)). `_1000` is not a literal: it is a name.

```sisu
fn main() {
    print(1000_)       // error[E0008]
}
```

### Range

The value of an integer literal must fit in an `i64`. The largest literal is `9223372036854775807`. A larger one is an error ([E0009](../diagnostics/E0009.md)).

```sisu
fn main() {
    print(9223372036854775808)    // error[E0009]
}
```

The one exception is the smallest `i64`. Its magnitude, `9223372036854775808`, is a literal only directly after a unary `-`. Spaces, tabs, line breaks and comments can come between the `-` and the digits. No other token can come between them, so parentheses cannot.

```sisu
fn main() {
    print(-9223372036854775808)    // -9223372036854775808
}
```

```sisu
fn main() {
    print(-(9223372036854775808))  // error[E0009]
}
```

The same rule applies in other bases, such as `-0x8000000000000000`.

A `.`, `?.` or `[` after the literal applies before the `-` does ([Precedence](operators.md#precedence)). So `-9223372036854775808.max(0)` is an error.

### The type of an integer literal

An integer literal is an `i64`, unless its context makes it a `u8` or an `f64`.

```sisu
fn main() {
    let b: u8 = 250
    let x = 2.5
    print(b + 5)       // 255
    print(x * 2)       // 5.0
    print(7 / 2)       // 3
}
```

The rules, in order:

- When the context expects a `u8` or a `u8?`, the literal is a `u8`. When the context expects an `f64` or an `f64?`, the literal is an `f64`.
- In a group of operands, the operands that are not integer literals decide the type of its integer literals. If one of them is a `u8` or a `u8?`, the literals in the group are `u8`s. If one is an `f64`, an `f64?` or a float literal, they are `f64`s. These are the groups:
  - the two operands of an [arithmetic operator](operators.md#arithmetic) or a comparison, such as `+`, `<` or `==`
  - the operands of a [comparison chain](operators.md#comparison-chains)
  - the elements of an [array literal](arrays.md#array-literals)
  - the branches of an `if` or an `if let`, or the arms of a `match`
- When a literal has no expected type and is not in a group, it is an `i64`.

These places give an expected type:

- the declared type of a `let` or a `const`
- the place of an [assignment](statements.md#assignment), also with `+=` and the other [compound operators](statements.md#compound-assignment)
- a parameter of a function or a method, also a built-in method such as `max`
- a field of a [class constructor](classes.md#constructors), or a value of an enum variant
- the return type, for a `return` value and for the last expression of a function body
- the element type, for each element of an array literal
- the type of the value in the left operand of `??`, for the right operand
- the expected type of an arithmetic operation or a unary `-`, for its operands
- the type of the value that a `match` tests, for a literal in a pattern
- the expected type of a loop, for each `break` value
- the expected type of a block, for its last expression
- the expected type of an `if` or a `match`, for each branch or arm

```sisu
fn main() {
    let o: f64? = None
    let c = 2
    let x = 2.5
    print(o ?? 3)                        // 3.0
    print(match c { 1 => x; _ => 4 })    // 4.0
    print(x.max(7))                      // 7.0
    let y: f64 = loop { break 1 }
    print(y)                             // 1.0
    let z: f64 = -3 + 1
    print(z)                             // -2.0
}
```

When a loop has no expected type, its `break` values give the loop its type. Their types must agree ([E0032](../diagnostics/E0032.md)).

An operand that has only integer literals, `+`, `-`, `*`, `/`, `%`, unary `-` and parentheses counts as a literal in a group. So `(2 + 3) * b` is a `u8` product when `b` is a `u8`.

```sisu
fn main() {
    let b: u8 = 50
    print((2 + 3) * b)    // 250
}
```

A `u8` literal must be 255 or less ([E0040](../diagnostics/E0040.md)). An integer literal that becomes an `f64` must have a value that an `f64` holds exactly ([E0104](../diagnostics/E0104.md)). Every integer up to 2^53 is such a value.

```sisu
fn main() {
    let b: u8 = 256    // error[E0040]
    let c: u8 = 1
    print(c < 300)     // error[E0040]
}
```

A float literal never becomes an integer.

## Float literals

A float literal has a fraction, an exponent or both. Its type is always [`f64`](types.md#floating-point-numbers).

```sisu
fn main() {
    print(2.5)          // 2.5
    print(3.0e8)        // 300000000.0
    print(1e-3)         // 0.001
    print(1_000.000_1)  // 1000.0001
}
```

The rules:

- A fraction is a `.` with at least one digit on each side. So `1.` and `.5` are errors ([E0101](../diagnostics/E0101.md)).
- An exponent is `e` or `E`, an optional `+` or `-`, and at least one digit ([E0102](../diagnostics/E0102.md)).
- A digit separator can go between two digits of the integer part, the fraction or the exponent.
- The value is the `f64` nearest to the literal. A literal so large that it rounds to infinity is an error ([E0103](../diagnostics/E0103.md)). A literal too small for an `f64`, such as `1e-400`, becomes `0.0`.

```sisu
fn main() {
    print(1.)          // error[E0101]
}
```

A `.` after an integer starts a fraction only when a digit follows it. So `0..n` is a range, `1.max(2)` calls a method on `1`, and `1.e5` is the integer `1`, a `.` and the name `e5`.

```sisu
fn main() {
    print(1.max(2))    // 2
}
```

## Character literals

A character literal is one Unicode scalar value between single quotes, such as `'a'` or `'ä'`. Its type is [`char`](types.md#char). An escape can write the value.

```sisu
fn main() {
    print('a')         // a
    print('\u{E4}')    // ä
    print('{')         // {
}
```

A character literal holds exactly one code point. So `''` and `'ab'` are errors ([E0092](../diagnostics/E0092.md)). An `e` followed by a combining accent is two code points, so it is an error too.

```sisu
fn main() {
    print('ab')        // error[E0092]
}
```

A `'` in a character literal must be the escape `\'`. A `"` can stand as it is. A character literal must close on its line ([E0093](../diagnostics/E0093.md)).

## String literals

A string literal is text between double quotes, such as `"sauna"`. Its type is [`string`](types.md#string). [Strings and formatting](strings.md) tells how a string works. The text can hold any character, and the escapes below.

```sisu
fn main() {
    print("sauna")           // sauna
    print("it's \"hot\"")    // it's "hot"
}
```

A string literal must close on its line ([E0093](../diagnostics/E0093.md)). For a line break in the text, write `\n`.

### Interpolation

A `{` in a string literal starts an interpolation. The program calculates the expression between `{` and `}`, and puts its text into the string.

```sisu
fn main() {
    let n = 3
    print("{n} + {n} = {n + n}")    // 3 + 3 = 6
}
```

The rules:

- The value of the expression can have any type except `unit`, `never` and an optional ([E0098](../diagnostics/E0098.md)). The text of an array, an object or an enum is in [Text of other types](strings.md#text-of-other-types). The section [Option and Result](types.md#option-and-result) is in development, and it changes the rule for an optional.
- The expression can include a string literal of its own: `"{ "a" + "b" }"`.
- The expression must be on the line of the string literal.
- A `}` closes the interpolation only when every bracket opened in the interpolation is closed. A `{` or `}` in a string or character literal in the expression does not count, so `"{ c == '}' }"` is valid.
- A `:` after the expression starts a format spec, such as `"{x:.2}"` ([Format specs](strings.md#format-specs)).
- An empty interpolation, `"{}"` or `"{ }"`, is an error ([E0096](../diagnostics/E0096.md)).
- A comment cannot be in an interpolation ([E0097](../diagnostics/E0097.md)).
- A `}` in the text, outside an interpolation, is an error ([E0095](../diagnostics/E0095.md)).

```sisu
fn main() {
    let name: string? = "Sisu"
    print("name = {name}")   // error[E0098]
}
```

For a literal brace in the text, write `\{` or `\}`.

```sisu
fn main() {
    print("\{n\}")     // {n}
}
```

## Escapes

Character and string literals take the same escapes.

| Escape | Value |
| --- | --- |
| `\n` | line feed |
| `\t` | tab |
| `\r` | carriage return |
| `\0` | null, the code point 0 |
| `\\` | `\` |
| `\"` | `"` |
| `\'` | `'` |
| `\{` | `{`, not an interpolation |
| `\}` | `}` |
| `\u{...}` | the Unicode scalar value with that hexadecimal code, such as `\u{E4}` for `ä` |

```sisu
fn main() {
    print("a\tb")              // a	b
    print("C:\\temp")          // C:\temp
    print("\u{1F600}")         // 😀
}
```

The rules:

- Any other character after a `\` is an error ([E0089](../diagnostics/E0089.md)). There is no `\x` byte escape.
- A `\u` escape has 1 to 6 hexadecimal digits between `{` and `}` ([E0090](../diagnostics/E0090.md)).
- Its value must be a Unicode scalar value: 0 to 10FFFF, but not D800 to DFFF ([E0091](../diagnostics/E0091.md)).

```sisu
fn main() {
    print("\q")        // error[E0089]
}
```

## Boolean literals

`true` and `false` are the two values of [`bool`](types.md#bool).

```sisu
fn main() {
    print(true)        // true
    print(!false)      // true
}
```
