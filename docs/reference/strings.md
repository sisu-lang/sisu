# Strings and formatting

A `string` is text in UTF-8. A program joins strings with `+`, looks at their bytes and characters with methods, and builds text with interpolation.

```sisu
fn main() {
    let name = "häst"
    print(name.len())                        // 5
    print(name.chars().len())                // 4
    print("{name} has {name.chars().len()} letters")   // häst has 4 letters
}
```

This page tells how a string works, the methods of a string, how a value becomes text, and the format specs of an interpolation. [Literals](literals.md#string-literals) tells how to write a string literal and its escapes.

## Text and bytes

A `string` always holds valid UTF-8. A *character* of a string is one Unicode scalar value, a `char` ([Types](types.md#char)). Each character takes 1 to 4 bytes. `e` and a combining accent are two characters.

```sisu
fn main() {
    let s = "häst 😀"
    print(s.len())                           // 10
    print(s.chars().len())                   // 6
    print(s.bytes()[1])                      // 195
}
```

The rules:

- A string is a value. Binding, passing and storing it behaves as a copy ([Values and memory](values.md#copied-and-shared)).
- `+` joins two strings ([Arithmetic](operators.md#arithmetic)). `+=` joins a string to a place that can change ([Compound assignment](statements.md#compound-assignment)). Another type beside a string is an error. Use interpolation.
- `==`, `!=`, `<` and the other comparison operators compare byte by byte, which is the order of the code points ([Comparison](operators.md#comparison)).
- Sisu does not normalize text. `"é"` as one character and `"e"` with a combining accent are not equal.
- A string has no index ([E0068](../diagnostics/E0068.md)), no range index ([E0023](../diagnostics/E0023.md)) and no fields. Use `chars()` or `bytes()` to get an array.
- A string can hold the null character `\0`. It counts as one byte.

```sisu
fn main() {
    print("é" == "e\u{301}")                 // false
    print("ant" < "bee")                     // true
    print("ab" < "abc")                      // true
    let mut text = "sau"
    text += "na"
    print(text + "!")                        // sauna!
}
```

```sisu
fn main() {
    let s = "sauna"
    print(s[0])                              // error[E0068]
}
```

## Methods

A string has these methods. None of them takes an argument.

| Method | Result | Meaning |
| --- | --- | --- |
| `s.len()` | `i64` | The number of bytes in `s`, not the number of characters |
| `s.chars()` | `[char]` | The characters of `s`, in order |
| `s.bytes()` | `[u8]` | The UTF-8 bytes of `s`, in order |
| `s.words()` | `[string]` | The words of `s`, in order |

`string(cs)` is the reverse of `chars()`. It builds a string from a `[char]` ([Types](types.md#conversions)).

```sisu
fn main() {
    let s = "ab"
    let mut cs = s.chars()
    cs[0] = 'z'
    print(string(cs))                        // zb
    print(s)                                 // ab
    print(string(cs).len())                  // 2
}
```

The rules:

- `chars()`, `bytes()` and `words()` make a new array each time. A change to the array does not change the string.
- A *word* is a run of characters that are not white space. White space is any character with the Unicode property `White_Space`, such as a space, a tab, a line break and a no-break space.
- A string that is empty, or only white space, has no words. A word is never empty.
- A string has no other methods ([E0063](../diagnostics/E0063.md)). A `char` has no methods either. Convert it with `i64(c)` ([Types](types.md#conversions)).
- A method of an optional string is called with `?.` ([Optional chaining](operators.md#optional-chaining)).

```sisu
fn main() {
    for w in "  one two\tthree\u{A0}four  ".words() {
        print("[{w}]")
    }
    print("".words().len())                  // 0
    print(" \t ".words().len())              // 0
}
```

```output
[one]
[two]
[three]
[four]
0
0
```

## Text of a value

Interpolation and `print` write a value as text in the same way. They take an `i64`, a `u8`, an `f64`, a `bool`, a `char` or a `string` ([E0098](../diagnostics/E0098.md), [E0059](../diagnostics/E0059.md)).

| Type | Text |
| --- | --- |
| `i64`, `u8` | The decimal digits, with a `-` for a negative number |
| `f64` | The shortest decimal text that reads back as the same number |
| `bool` | `true` or `false` |
| `char` | The character |
| `string` | The string, as it is |

The text of an `f64` has at least one digit after the point, such as `100.0`. A number with an absolute value of 1e16 or more has an exponent instead, such as `1e21`. So has a number whose absolute value is below 1e-4 but not 0, such as `1.5e-7`. The exponent has no `+` sign and no leading zero. The text of the special values is `inf`, `-inf` and `nan`, and `-0.0` keeps its sign.

```sisu
fn main() {
    print(100.0)                             // 100.0
    print(0.1 + 0.2)                         // 0.30000000000000004
    print(0.0001)                            // 0.0001
    print(0.00001)                           // 1e-5
    print(9999999999999998.0)                // 9999999999999998.0
    print(1e16)                              // 1e16
    print(1.5e-7)                            // 1.5e-7
    print(-1.0 / 0.0)                        // -inf
    print(-0.0)                              // -0.0
}
```

### Text of other types

`print` and interpolation also take a value of any type that can be a type argument. An array prints as `[1, 2]`. A class prints its name. A class can write its own text with a method `to_string` ([Display, Eq and Ord](generics.md#display-eq-and-ord)).

```sisu pending
class Sauna {
    heat: i64
}

fn main() {
    print([1, 2])                      // [1, 2]
    print(Sauna(heat: 80))             // Sauna
}
```

## Interpolation

`{expression}` in a string literal writes the value of the expression as text ([Literals](literals.md#interpolation)). A `:` after the expression adds a format spec.

```sisu
fn main() {
    let n = 7
    let b: u8 = 200
    let x = 2.5
    let ok = n > 3
    let c = 'ä'
    let s = "sauna"
    print("{n} {b} {x} {ok} {c} {s}")        // 7 200 2.5 true ä sauna
    print("{n + 1} {s + "!"} {n * 2 > 10}")  // 8 sauna! true
}
```

Interpolation does not convert a value: it writes the text of the type that the value has. To write an optional, use `??` or `if let`. To write an array, use a `for` loop.

The program calculates each expression once, from left to right, before it builds the text. This is so also when the parts have format specs.

```sisu
fn part(n: i64) -> i64 {
    print("part {n}")
    n
}

fn main() {
    print("{part(1)}:{part(2):>3}:{part(3)}")
}
```

```output
part 1
part 2
part 3
1:  2:3
```

## Format specs

A *format spec* sets the width of the text of a part, how it aligns, and how many decimals an `f64` has. It follows the expression and a `:`, as in `{x:>8.2}`.

```sisu
fn main() {
    let x = 2.675
    let name = "Väinö"
    print("[{x:>8.2}]")                      // [    2.67]
    print("[{name:<8}]")                     // [Väinö   ]
    print("[{name:^9}]")                     // [  Väinö  ]
    print("[{42:6}]")                        // [    42]
}
```

A spec is `[align][width][.precision]`. A spec needs a width or a precision. The align is optional.

| Part | Form | Meaning |
| --- | --- | --- |
| align | `<`, `>` or `^` | Left, right or centre |
| width | 1 to 1000 | The least width of the text, in characters |
| precision | `.` and 0 to 30 | The number of decimals of an `f64` |

The rules:

- A part that is shorter than its width gets spaces. A part that is longer is not cut.
- Without an align, a number (`i64`, `u8` or `f64`) is on the right, with the spaces on its left. Every other type is on the left, with the spaces on its right.
- The width counts characters, not bytes. `ä` and `😀` are each one column.
- `^` puts half of the spaces on each side. When the number of spaces is odd, the extra space is on the right.
- A precision rounds half to even from the exact value of the `f64`, and never uses an exponent. A precision of 0 writes no point. `inf`, `-inf` and `nan` have no decimals.
- A precision on a value that is not an `f64` is an error ([E0111](../diagnostics/E0111.md)).
- A width above 1000, or a precision above 30, is an error ([E0110](../diagnostics/E0110.md)).
- A spec with no width and no precision, or a `.` with no digits, is an error ([E0108](../diagnostics/E0108.md)).
- Other parts, such as zero padding, a sign, a fill character, a type letter, or a width from a variable, are errors ([E0109](../diagnostics/E0109.md)).
- A `:` inside a string or character literal in the expression does not start a spec.

```sisu
fn main() {
    let t = 3.14159
    let big = 1e21
    print("[{t:.2}][{t:.0}][{t:8.3}]")       // [3.14][3][   3.142]
    print("[{2.5:.0}][{0.5:.0}][{1.5:.0}]")   // [2][0][2]
    print("[{big:.1}]")                      // [1000000000000000000000.0]
    print("[{"a:b"}]")                       // [a:b]
}
```

```sisu
fn main() {
    let n = 7
    print("{n:.2}")                          // error[E0111]
}
```

### The `#` flag

`{x:#}` lays a value out on several lines when it does not fit on one line. A value that fits stays on one line. A part that does not fit breaks into one element per line.

```sisu pending
fn main() {
    let xs = [1, 2, 3]
    print("{xs:#}")                    // [1, 2, 3]
    print("{7:#}")                     // 7
}
```

The rules:

- A hand-written `to_string` gives one piece of text that the layout does not break.
- `#` does not combine with a width, an alignment or a precision. `{x:#>8}` is an error.
- `#` on a number or a string changes nothing.
- There is no function for it: `print("{x:#}")` is the one way.
