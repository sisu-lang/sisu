# Language reference

The *language reference* states what each construct of Sisu means and the rules that it follows, with examples.

```sisu
fn main() {
    print("Hello")         // Hello
}
```

Each page is about one part of the language. A section gives the everyday form first, then the exact rules and the edge cases. You can stop after its first paragraph.

A page states a rule only when `sisu` follows it today. A test checks every example. When an example runs, the test compares its output with the page.

## How to read an example

Every example is a full program with a [`main` function](functions.md#main), except the example of the error for a program without `main` and the files of a program of several files ([Modules](modules.md)).

### Output comments

A comment after code, such as `// Hello` above, is a line of output. The program prints these lines in the order of the source, and nothing else.

```sisu
fn main() {
    let n = 6
    print(n * 7)           // 42
    print(n == 6)          // true
}
```

An example with no output comments and no output panel prints nothing. A comment on a line of its own is never output.

### Output panels

When the output does not follow the order of the source, for example in a loop, an output panel shows all of it. Then the comments in the code are ordinary comments.

```sisu
fn main() {
    // Each turn of the loop prints one line.
    for i in 1..4 {
        print(i * 10)
    }
}
```

```output
10
20
30
```

### Input panels

An input panel shows the text that the program reads from standard input.

```sisu
fn main() {
    let name = read_line() ?? "nobody"
    print("Hello, {name}")
}
```

```input
Mikko
```

```output
Hello, Mikko
```

### Arguments

When the program gets arguments, the caption above it shows the command, such as `./example sauna sisu`.

```sisu args="sauna sisu"
fn main(args: [string]) {
    for arg in args {
        print(arg)
    }
}
```

```output
sauna
sisu
```

### Errors and warnings

A comment such as `// error[E0008]` marks a line where `sisu` reports that diagnostic. Under the example, a link goes to the page that explains the code. An example with an error does not build, so it has no output.

```sisu
fn main() {
    print(1000_)           // error[E0008]
}
```

An example with only warnings builds and runs. Its other comments are output, as usual.

```sisu
fn main() {
    let mut n = 42         // warning[W0002]
    print(n)               // 42
}
```

### Panics

A stderr panel shows what the program writes to standard error. When the program panics, this is the panic message, and the program stops with exit code 101. The example below panics on an [integer overflow](operators.md#overflow).

```sisu panics
fn main() {
    let big = 9223372036854775807
    print(big + 1)
}
```

```stderr
sisu: panic at example.sisu:3:11: integer overflow
```

Some examples never end, for example a program with an endless `loop`. The test builds them, but does not run them.

### In development

The tag "In development" marks a section about a feature that is not complete. Its examples show the planned form, and `sisu` does not build them yet.

When the feature works, its examples become ordinary examples, and the tag goes away. In the list of pages, a page has the tag when it has examples and all of them are in development.

## The Markdown source

The pages are Markdown files in `docs/reference/`. An example is a fenced block with the info string `sisu`. Its panels are the blocks `output`, `input` and `stderr`, directly after it, at most one of each, in any order.

````text
```sisu args="sauna sisu"
fn main(args: [string]) {
    print(args.len())      // 2
}
```

```input
text for standard input
```
````

After `sisu`, the info string can have these words, in any order:

| Word | Meaning |
| --- | --- |
| `pending` | The example is in development. `sisu check` must report an error. |
| `panics` | The program must panic. A `stderr` block with the panic message is necessary. |
| `no-run` | The program never ends. The test builds it, but does not run it. |
| `args="..."` | The arguments of the program, separated by spaces. |

`pending`, `panics` and `no-run` exclude each other. A diagnostic marker is a trailing comment that is exactly `error[Ennnn]` or `warning[Wnnnn]`. It must be on the line that the diagnostic names.

````text
```sisu panics
fn main() {
    todo                   // warning[W0003]
}
```

```stderr
sisu: panic at example.sisu:2:5: not yet implemented
```
````

A `text` block, as above, and an `sh` block are not checked. The test rejects a block of any other kind. Every block is a fence of backticks at the start of a line, and every link is an inline link.
