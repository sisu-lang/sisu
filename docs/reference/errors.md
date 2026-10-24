# Errors and Result

A function that can fail says so in its return type. The type `T | E` holds a *success* of the type `T`, or a *failure* of the *error type* `E`. A value of that type is a *result*.

```sisu pending
fn parse(text: string) -> i64 | string {
    i64(text) ?? fail "not a number: {text}"
}

fn double(text: string) -> i64 | string {
    let n = parse(text)?
    n * 2
}

fn main() {
    print(double("21") ?? 0)           // 42
    print(double("x") ?? 0)            // 0
}
```

A failure passes up one visible step at a time: `fail` makes it, `?` passes it on, and `??`, `if let` and `match` handle it. Sisu has no exceptions. In the example, `i64(text)` is `None` when the text is not a number ([Parsing a string](types.md#parsing-a-string)).

## Results

`T | E` is short for the type `Result<T, E>` ([Option and Result](types.md#option-and-result)). Both spellings are the same type. A message from `sisu` writes the type with `|`.

```sisu pending
enum PortError: Error {
    Empty
    NotANumber(text: string)

    fn to_string(self) -> string {
        match self {
            Empty => "the port is empty"
            NotANumber(text) => "{text} is not a number"
        }
    }
}

fn port(text: string) -> i64 | PortError {
    if text == "" {
        fail PortError.Empty
    }
    i64(text) ?? fail PortError.NotANumber(text: text)
}

fn save(path: string, text: string) -> unit | IoError {
    write_file(path, text)
}

fn main() {
    let ports: [i64 | PortError] = [port("80"), port("")]
    print(ports.len())                 // 2
}
```

The rules:

- The left side is the success type. It can be any type, also `unit` and an error type. The right side is the error type.
- The error type must conform to the interface `Error`. So `-> i64 | bool` is an error. The interface `Error` requires `Display`.
- `string` conforms to `Error`. So `-> i64 | string` is a function that fails with a message.
- A function that returns no value on success returns `unit | E`. There is no form `fn save(...) | IoError`.
- A result has one error type. `T | A | B` is an error. To report several causes, declare one enum with a variant for each cause.
- `|` in a type never means "one of these types". `i64 | string` is an `i64` or a failure message, and `i64 | bool` does not build.
- `i64 | E?` is `i64 | (E?)`, and its right side is not an error type. For an optional result, write `(i64 | E)?`.
- A generic function that returns `T | E` needs the bound `E: Error`. `Result<i64, bool>` is the same error as `i64 | bool`.
- An alias can name a result with a fixed error type: `type IoResult<T> = T | IoError`.
- A program cannot declare its own top-level `Error`.

## A plain value is the success

Where a `T | E` is expected, a value of type `T` becomes the success. A function body that ends with a value returns it as a success. A `unit | E` body that reaches its end succeeds.

```sisu pending
fn size(name: string) -> i64 | string {
    if name == "" {
        fail "no name"
    }
    name.len()
}

fn log(text: string) -> unit | string {
    print(text)
}

fn main() {
    print(size("sisu") ?? 0)           // 4
    print(size("") ?? 0)               // 0
    let _ = log("done")                // done
}
```

The rules:

- A value of the error type `E` does not become a failure. A failure comes from `fail`, or from `Err(e)` where a result is expected.
- `Ok(x)` and `Err(e)` are valid expressions. Code needs them mostly in patterns.

## fail

`fail e` ends the enclosing function with the failure `e`. The function must return a `T | E`, and `e` must have the type `E`. In any other function, `fail` is an error.

```sisu pending
enum ConfigError: Error {
    BadLine(line: i64)
}

fn check(line: i64, ok: bool) -> unit | ConfigError {
    if !ok {
        fail ConfigError.BadLine(line: line)
    }
}

fn main() {
    match check(2, false) {
        Ok(_) => print("ok")
        Err(_) => print("failed")               // failed
    }
}
```

`fail` is a keyword. An expression `fail e` has the type `never`, as a call of `panic` does. So it can be a branch of an `if` or a `match`, an arm, and the right side of `??` ([Types](types.md#never)).

## The `?` operator

`x?` passes a failure up. When `x` is a `T | E` that holds a success, `x?` is the value of type `T`. When `x` holds a failure, the enclosing function returns that failure.

```sisu pending
enum TotalError: Error {
    Io(IoError)
    Empty
}

extend IoError: Convert<TotalError> {
    fn convert(self) -> TotalError {
        TotalError.Io(self)
    }
}

fn length(path: string) -> i64 | TotalError {
    let text = read_file(path)?
    if text.len() == 0 {
        fail TotalError.Empty
    }
    text.len()
}

fn main() -> unit | TotalError {
    print(length("notes.txt")?)
}
```

The rules:

- The return type of the enclosing function must be a result `U | F`. When `F` is `E`, the failure passes up as it is.
- When `F` is not `E`, `E` must conform to `Convert<F>`. Then `?` returns `e.convert()` as the failure. Above, an `IoError` becomes a `TotalError`.
- On an optional `T?`, `x?` is the value when `x` holds one. When `x` is `None`, the enclosing function returns `None`. Its return type must be an optional.
- `?` on a result in a function that returns an optional is an error. So is `?` on an optional in a function that returns a result. Write `x ?? fail ...` to turn an empty optional into a failure.
- `?` works on a result or an optional only. `5?` is an error.
- `?` is a postfix operator and binds as a call does. `x??` is the operator `??`. To apply `?` twice, write `(x?)?`.
- On a result, `x?.name` passes a failure up, then reads the member: `read_file(p)?.len()` is `(read_file(p)?).len()`. On an optional, `?.` is still [optional chaining](operators.md#optional-chaining).
- `x?` calculates `x` once. When it is a failure, nothing after it in the same statement runs.

## Handling a failure

`r ?? d` gives the success of the result `r`. On any failure it gives `d`. The program calculates `d` only on a failure. Its type is `T`, or `never`, as a call of `panic` has.

```sisu pending
enum LoadError: Error {
    Missing
}

fn load(name: string) -> string | LoadError {
    if name == "" {
        fail LoadError.Missing
    }
    name
}

fn main() {
    print(load("") ?? "default")                // default
    print(load("sisu") ?? panic("must load"))   // sisu
    if let Ok(text) = load("sauna") {
        print(text)                             // sauna
    }
    match load("") {
        Ok(text) => print(text)
        Err(_) => print("failed")               // failed
    }
}
```

The rules:

- `if let` and `while let` on a result name `Ok(...)` or `Err(...)`: `if let Ok(config) = load(...)`. The pattern shows that the program lets a failure go. A short pattern, `if let config = load(...)`, is an error.
- On an optional, the short form of `if let` stays.
- A `match` on a result works as on any enum, with the check that it covers every value.
- A `match` with the arms `Ok(Some(x))`, `Ok(None)` and `Err(e)` tells the end of the input from a failure.
- To pass the failure up instead, apply `?` to the operand: `while let line = next()?`.

## An unused result

A result that the program does not use is an error. It replaces the warning for an unused value ([W0004](../diagnostics/W0004.md)). To ignore a result on purpose, write `let _ = e`.

```sisu pending
fn save(path: string, text: string) -> unit | IoError {
    write_file(path, text)
}

fn main() {
    // The result of `save` may be a failure and is not used.
    save("out.txt", "text")
    // This ignores it on purpose.
    let _ = save("out.txt", "text")
}
```

The last expression of a function body is its value, so a `unit | E` body that ends in `save(...)` returns that result. It is used.

## A main that fails

`main` can return a `unit | E`. When it returns a failure, the program writes `error: ` and the text of the error to standard error, and ends with the exit code 1.

```sisu pending
fn parse(text: string) -> i64 | string {
    i64(text) ?? fail "not a number: {text}"
}

fn main() -> unit | string {
    let n = parse("12x")?
    print(n)
}
```

The program above writes `error: not a number: 12x` to standard error. A panic still ends the program with the exit code 101.

`main` with any other result type, or with a type other than `unit`, is an error ([E0029](../diagnostics/E0029.md)).

## Files

`read_file` and `write_file` read and write a whole file. A failure names the path.

```sisu pending
fn main() -> unit | IoError {
    let text = read_file("notes.txt")?
    write_file("copy.txt", text)
}
```

| Function | Result |
| --- | --- |
| `read_file(path)` | The text of the file, a `string \| IoError` |
| `write_file(path, text)` | A `unit \| IoError`. It creates the file or replaces its content |

`IoError` is an enum that conforms to `Error`. It has the variants `NotFound(path)`, `PermissionDenied(path)`, `NotUtf8(path)` and `Other(path, reason)`. A file that is not valid UTF-8 gives `NotUtf8`.

These functions give a program no file handle. A program cannot append to a file, read it line by line or move inside it. The functions `print`, `eprint` and `read_line` keep their rules ([Input and output](io.md)).
