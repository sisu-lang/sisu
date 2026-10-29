# Input and output

A program writes text with `print` and `eprint`, and reads text with `read_line` and the arguments of `main`.

```sisu
fn main() {
    let name = read_line() ?? "nobody"
    print("Hello, {name}")
}
```

```input
Eero
```

```output
Hello, Eero
```

## print and eprint

`print(value)` writes the text of `value` and a line break to standard output. `eprint(value)` does the same to standard error.

```sisu
fn main() {
    print(42)                                // 42
    print(2.5)                               // 2.5
    print(true)                              // true
    print('ä')                               // ä
    print("sauna")                           // sauna
    eprint("a warning")
}
```

```stderr
a warning
```

The rules:

- The argument is one value of any type except `unit` and `never` ([Text of other types](strings.md#text-of-other-types)). A call that returns nothing, no argument, and more than one are errors ([E0059](../diagnostics/E0059.md)). An optional builds with the warning [W0015](../diagnostics/W0015.md), because the text can be `None`: unwrap it with `if let`, give a default with `??`, or call `to_string()`.
- An argument of the type `never`, such as a call of `panic`, is an error ([E0061](../diagnostics/E0061.md)).
- The text of a value is the same as in an interpolation ([Strings and formatting](strings.md#text-of-a-value)). `print("{x}")` and `print(x)` write the same text for a value `x` that an interpolation takes: any type except `unit` and `never` ([E0098](../diagnostics/E0098.md)).
- The line break is one `\n` byte. A `\r` is written only when the text holds it.
- `print` and `eprint` give no value ([E0043](../diagnostics/E0043.md)).
- When standard output is a terminal, `print` writes each line at once. Otherwise `print` collects its text and writes it later, and `eprint` still writes at once. So the order of the two streams in one file or pipe is not fixed.
- A panic writes the text that `print` collected before it writes the panic message ([panic](functions.md#panic)).
- A write to standard output that fails, such as on a full disk, is a panic. The next rule gives the exceptions.
- A `print` or `eprint` write on a pipe with no reader ends the program with the signal SIGPIPE, with no panic. A panic still ends with the exit code 101. A closed standard output drops the text. Any other failed write to standard error is ignored.

## dbg

`dbg(e)` writes a line about `e` to standard error, and gives the value of `e` unchanged.

```sisu
fn double(n: i64) -> i64 {
    n * 2
}

fn main() {
    let y = dbg(double(21)) + 1
    print(y)                           // 43
}
```

```stderr
example.sisu:6:17: double(21) = 42
```

The rules:

- The line holds the file, the line and the column of the argument, the source text of the argument as written, and the value. The value is laid out as `{x:#}` lays it out ([The `#` flag](strings.md#the--flag)).
- For an object, `dbg` shows the fields one level deep: `dbg(node)` shows `Node(value: 3, next: Node)`. A field that is a class shows only its text, which is its name or its own `to_string`. So `dbg` follows no reference.
- A class with a method `to_string` shows that text instead of its fields. The same holds for a class inside an array, a payload or an optional.
- An argument of the type `unit` is an error ([E0059](../diagnostics/E0059.md)), and so is an argument of the type `never` ([E0061](../diagnostics/E0061.md)), as for `print`.
- `dbg` runs its argument once and gives its value, so `dbg(f())` calls `f` once. It gives no warning for an optional. A statement `dbg(x)` that drops the value gives no warning for an unused value ([W0004](../diagnostics/W0004.md)) either.
- A local can have the name `dbg` ([E0034](../diagnostics/E0034.md) for a top-level item).

## read_line

`read_line()` reads the next line of standard input. It gives a `string?`: the line, or `None` when no line is left.

```sisu
fn main() {
    let mut n = 0
    while let line = read_line() {
        n += 1
        print("{n}: [{line}]")
    }
    print("lines: {n}")
}
```

```input
first

third
```

```output
1: [first]
2: []
3: [third]
lines: 3
```

The rules:

- A line ends at a `\n`. The line does not include it. A `\r` directly before the `\n` is not in the line either. A `\r` anywhere else stays in the line.
- The last line needs no `\n`. An input that ends with a `\n` has no empty line after it.
- An empty line gives the empty string, not `None`.
- At the end of the input, `read_line` gives `None`. Every later call gives `None` too. An empty input has no line.
- A byte that is not valid UTF-8 becomes the character U+FFFD. Each maximal ill-formed part of the bytes gives one U+FFFD, so a `string` is always valid UTF-8 ([Strings and formatting](strings.md#text-and-bytes)). A zero byte stays in the line.
- `read_line` takes no argument ([E0053](../diagnostics/E0053.md)). A function cannot take its name ([E0034](../diagnostics/E0034.md)).
- A failed read of standard input is a panic. A closed standard input is the end of input.
- A program that needs a line can use `??` with a default or with `panic` ([Operators](operators.md#default-values)).

## Program arguments

A `main` with a `[string]` parameter gets the arguments of the program, in order ([main](functions.md#main)). The name of the program is not one of them. A program with no arguments gets an empty array.

```sisu args="sauna -x --flag"
fn main(args: [string]) {
    print(args.len())                        // 3
    for arg in args {
        print("[{arg}]")
    }
}
```

```output
3
[sauna]
[-x]
[--flag]
```

The rules:

- Sisu does not read any argument itself. A word that looks like a flag, such as `-x`, is an ordinary string.
- An argument can be empty, and it can hold any text. A byte that is not valid UTF-8 becomes U+FFFD, as in `read_line`.
- `sisu run file.sisu a b` gives the program the arguments `a` and `b`.

## Exit code

A program ends with the exit code 0 when `main` returns and the text of `print` is written ([main](functions.md#main)). When that last write fails with an error, the program panics. A panic ends the program with the exit code 101 ([panic](functions.md#panic)).

### A failure from main

When `main` returns a failure, the program writes `error: ` and the text of the error to standard error. Then it ends with the exit code 1 ([A main that fails](errors.md#a-main-that-fails)).

```sisu pending
fn main() -> unit | string {
    fail "missing file"
}
```
