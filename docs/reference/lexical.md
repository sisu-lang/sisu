# Lexical structure

The *lexical structure* of Sisu is how source text becomes tokens: names, keywords, literals, operators, comments and line breaks.

```sisu
// A comment runs to the end of its line.
fn main() {
    let greeting = "hello"
    print(greeting)
}
```

```output
hello
```

On this page, the output of an example is in an output panel, so that every `//` in the code is an ordinary comment.

## Source text

A source file is text in UTF-8 ([E0072](../diagnostics/E0072.md)). A line ends with a line feed, or with a carriage return and a line feed. The last line does not need a line break.

Outside comments and literals, the source uses only these characters:

- The ASCII letters, the digits and `_`.
- The characters of the [operators](operators.md) and punctuation, such as `+`, `(` and `"`.
- Spaces, tabs and carriage returns. They separate tokens and have no other meaning.

A carriage return without a line feed does not end a line. So two statements with only a carriage return between them are an error ([E0011](../diagnostics/E0011.md)).

Any other character is an error ([E0010](../diagnostics/E0010.md)). This includes a letter that is not ASCII, such as `é`, and a lone `&` outside a bound, where it joins interfaces (`T: Shape & Named`).

A no-break space and a byte order mark are errors too, also at the start of the file. Comments, character literals and string literals can hold any character.

```sisu
fn main() {
    let total = 2 # 3      // error[E0010]
}
```

## Comments

A comment starts with `//` and runs to the end of the line. It can stand on a line of its own, or after code.

```sisu
fn main() {
    // A comment on a line of its own.
    let n = 2              // A comment after code.
    /// Three slashes start an ordinary comment too.
    print(n)
}
```

```output
2
```

Sisu has no block comment: `/*` is the operator `/`, then the operator `*`. A `//` in a string literal is part of the text, not a comment. A comment cannot be in an interpolation ([E0097](../diagnostics/E0097.md)).

```sisu
fn main() {
    print("a // b")        // Only this part is a comment.
}
```

```output
a // b
```

## Names

A name starts with an ASCII letter or `_`. After that come ASCII letters, digits and `_`. Names are case-sensitive, so `total` and `Total` are two names.

```sisu
fn main() {
    let total = 1
    let Total = 2
    let _count = 3
    let x2 = 4
    print(total + Total + _count + x2)
}
```

```output
10
```

The rules:

- A name cannot be a keyword or a reserved word.
- `_` alone is not a name. It is a keyword that `let`, `for` and [`match`](enums.md#patterns) use to bind nothing.
- The built-in functions and [types](types.md), such as `print`, `i64` and `string`, are names, not keywords.

## Keywords

These words are keywords. A keyword cannot be a name.

| Keywords | |
| --- | --- |
| Declarations | `fn`, `class`, `enum`, `interface`, `extend`, `const`, `let`, `mut` |
| Control flow | `if`, `else`, `while`, `for`, `in`, `loop`, `match`, `break`, `continue`, `return` |
| Values | `true`, `false`, `None`, `self`, `Self` |
| Other | `is`, `todo`, `_` |

`var` is a keyword that Sisu does not use. Write `let mut` ([E0013](../diagnostics/E0013.md)).

These words are reserved for a later version of Sisu. A reserved word cannot be a name or start an expression ([E0083](../diagnostics/E0083.md)).

`as`, `echo`, `extern`, `import`, `pub`, `record`, `test`, `type`, `weak`

```sisu
fn main() {
    let record = 1         // error[E0083]
}
```

## Line breaks

A line break ends a statement. No `;` is necessary.

```sisu
fn main() {
    let a = 20
    let b = 22
    print(a + b)
}
```

```output
42
```

A line break ends a statement only after a token that can end one. These tokens can end a statement:

- A name, a literal, `true`, `false`, `None`, `self` or `_`.
- `return`, `break`, `continue` or `todo`.
- `?`, `)`, `]` or `}`.

After any other token, such as an operator or `,`, the statement continues on the next line. So a long expression breaks after an operator.

```sisu
fn main() {
    let total = 1 +
        2 +
        3
    print(total)
}
```

```output
6
```

Inside `(` and `[`, a line break never ends a statement. A `{` inside them opens a block, and in that block, line breaks end statements again.

```sisu
fn main() {
    let xs = [
        10,
        20,
    ]
    print(xs.len())
}
```

```output
2
```

A line break inside the type-parameter list of a `fn`, `class` or `enum` header does not end a statement either. This applies when the header starts its statement. A header that comes after a `}` on the same line must keep its list on one line.

```sisu
class Pair<
    A,
    B,
> {
    first: A
    second: B
}

fn main() {
    let p = Pair(first: 1, second: "two")
    print(p.first + p.second.len())
}
```

```output
4
```

### Layout rules

Because a line break can end a statement, some forms must stay on one line.

Outside `(` and `[`, a binary operator ends a line, and cannot start the next line. A `-` at the start of a line, followed by a space or a tab, is an error ([E0002](../diagnostics/E0002.md)). The `-` could be a subtraction or a negation.

```sisu
fn main() {
    let n = 10
    - 2                    // error[E0002]
}
```

This rule applies only when the line break before the `-` ends a statement. After an operator, the statement continues, so a `-` on the next line is a negation.

```sisu
fn main() {
    let n = 10 +
        - 2
    print(n)
}
```

```output
8
```

When the line break before it ends a statement, a `-` with no space or tab after it is a negation. The negation starts a new statement. So in this example, `n` stays 10, and `-2` is an unused value ([W0004](../diagnostics/W0004.md)).

```sisu
fn main() {
    let n = 10
    -2                     // warning[W0004]
    print(n)
}
```

```output
10
```

The other layout rules:

- The `{` of a `fn`, `class`, `enum`, `if`, `else`, `match`, `while`, `for` or `loop` is on the line of its header ([E0004](../diagnostics/E0004.md)).
- An `else` is on the line of the `}` before it: `} else {` ([E0006](../diagnostics/E0006.md)).
- Inside `(` or `[`, a `[` cannot start a line after an expression ([E0005](../diagnostics/E0005.md)). Outside brackets, a `[` at the start of a line starts a new statement. It never indexes the line above.

```sisu
fn main()
{                          // error[E0004]
    print(1)
}
```

## Semicolons

A `;` separates two statements on one line, as a line break does. It also separates two fields, two methods, two variants or two `match` arms on one line.

```sisu
fn main() {
    let x = 2; print(x * 21)
}
```

```output
42
```

A `;` that separates nothing is unnecessary, and the program builds with a warning ([W0007](../diagnostics/W0007.md)). These are the unnecessary places:

- the end or the start of a line
- after `{`, or before `}` or `else`
- each extra `;` in `;;`
- after a top-level `fn`, `class` or `enum`

```sisu
fn main() {
    print(1);              // warning[W0007]
}
```

```output
1
```

The rules:

- After a top-level `const`, a `;` separates it from an item after it on the same line: `const X = 1; fn main() {}`.
- Inside `(` and `[`, a `;` is an error ([E0011](../diagnostics/E0011.md)). The `;` in `[value; length]` is part of the [array syntax](arrays.md#filled-arrays).
- When removing a `;` would join two lines into a different statement, the `;` is an error, not a warning ([E0011](../diagnostics/E0011.md)).

## Line continuation

A line that starts with `.` or `?.` continues the line above it. So a chain of method calls can put each call on its own line. Blank lines and comment lines can come between.

```sisu
fn main() {
    let n = "one two three"
        .words()
        // A comment line does not stop the chain.
        .len()
    print(n)
}
```

```output
3
```

A line that starts with `..` does not continue the line above it, because `..` starts a range. A `;` at the end of the line above also stops the continuation.
