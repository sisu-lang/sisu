# Functions

A *function* is a named block of code with parameters and an optional return type. A call of the function runs its body.

```sisu
fn area(width: i64, height: i64) -> i64 {
    return width * height
}

fn greet(name: string) {
    print("Hello, {name}")
}

fn main() {
    greet("Aino")                // Hello, Aino
    print(area(6, 7))            // 42
}
```

A call gives the arguments to the parameters in order ([Calls](expressions.md#calls)).

## Declarations

A function declaration is `fn`, the name, the parameters in parentheses, an optional return type after `->`, and the body. A function is declared at the top level.

```sisu
fn main() {
    print(double(21))            // 42
}

fn double(n: i64) -> i64 {
    return n * 2
}
```

The rules:

- A function can call a function that is declared below it.
- Functions, classes, enums and constants share one set of names. Each name is declared once ([E0034](../diagnostics/E0034.md)).
- A function cannot take the name of a built-in type, such as `i64` ([E0026](../diagnostics/E0026.md)). It cannot take the name `print`, `eprint`, `panic` or `read_line` ([E0034](../diagnostics/E0034.md)).
- A function cannot be named `_` ([E0080](../diagnostics/E0080.md)).
- A function cannot be declared in the body of another function.

```sisu
fn twice() {
    print(1)
}

fn twice() {                     // error[E0034]
    print(2)
}

fn main() {
    twice()
}
```

## Parameters

Each parameter has a name and a type. In the body, a parameter is a binding that holds the value of its argument.

```sisu
fn describe(name: string, age: i64) -> string {
    return "{name} is {age}"
}

fn main() {
    print(describe("Eero", 7))   // Eero is 7
}
```

The rules:

- A parameter cannot change. An assignment to it is an error ([E0001](../diagnostics/E0001.md)), and a parameter cannot be `mut` ([E0019](../diagnostics/E0019.md)).
- To change the value, copy it into a `let mut` binding. A `let` can shadow a parameter ([Shadowing](statements.md#shadowing)).
- Two parameters of one function cannot have the same name ([E0039](../diagnostics/E0039.md)).
- An argument must have the type of its parameter. Where the parameter is a `T?`, a `T` becomes a `T?` ([Calls](expressions.md#calls)).

```sisu
fn countdown(n: i64) {
    let mut left = n
    while left > 0 {
        print(left)
        left -= 1
    }
}

fn main() {
    countdown(3)
}
```

```output
3
2
1
```

```sisu
fn countdown(n: i64) {
    while n > 0 {
        print(n)
        n -= 1                   // error[E0001]
    }
}

fn main() {
    countdown(3)
}
```

## Return types

A function with a return type `-> T` gives a value of the type `T`. The value is the value of the last expression of its body, or the value of a `return`.

```sisu
fn sign(n: i64) -> i64 {
    if n < 0 {
        return -1
    }
    if n == 0 { 0 } else { 1 }
}

fn main() {
    print(sign(-5))              // -1
    print(sign(8))               // 1
}
```

The rules:

- Each path through the body must end in a value or in a statement that never finishes ([E0031](../diagnostics/E0031.md), [Reachability](statements.md#the-end-of-a-function)).
- The value must have the type `T` ([E0032](../diagnostics/E0032.md)). When `T` is an optional `U?`, a `U` value becomes a `U?` ([Types](types.md#optionals)).
- A function without a return type gives no value, so a call of it has no value ([E0043](../diagnostics/E0043.md)).
- When the body of a function without a return type ends in a value, the program builds with a warning ([W0004](../diagnostics/W0004.md)).
- For `return`, see [return](statements.md#return).

```sisu
fn half(n: i64) -> i64? {
    if n % 2 == 1 {
        return None
    }
    n / 2
}

fn main() {
    print(half(8) ?? -1)         // 4
    print(half(7) ?? -1)         // -1
}
```

## Recursion

A function can call itself, and two functions can call each other.

```sisu
fn factorial(n: i64) -> i64 {
    if n <= 1 {
        return 1
    }
    n * factorial(n - 1)
}

fn is_even(n: i64) -> bool {
    if n == 0 { true } else { is_odd(n - 1) }
}

fn is_odd(n: i64) -> bool {
    if n == 0 { false } else { is_even(n - 1) }
}

fn main() {
    print(factorial(5))          // 120
    print(is_even(10))           // true
}
```

A call can use space on the stack until it returns. When a recursion goes too deep, the stack is full and the program stops. This is not a panic, and output that the program did not write yet can be lost.

A function that calls itself on every path can never return, and the program builds with a warning ([W0010](../diagnostics/W0010.md)). To repeat without an end, write a `loop`.

```sisu no-run
fn count(n: i64) {               // warning[W0010]
    print(n)
    count(n + 1)
}

fn main() {
    count(0)
}
```

## main

A program starts at its function `main`. When `main` returns, the program ends with the exit code 0.

```sisu
fn main() {
    print("start")               // start
}
```

The rules:

- `main` takes no parameters, or one `[string]`. It has no return type ([E0029](../diagnostics/E0029.md)).
- A `main` with a `[string]` parameter gets the arguments of the program, without the name of the program.
- A program without `main` is an error ([E0028](../diagnostics/E0028.md)).
- A `return` in `main` ends the program.
- A panic ends the program with the exit code 101 ([panic](#panic)).

```sisu args="sauna sisu"
fn main(args: [string]) {
    print(args.len())            // 2
    print(args[0])               // sauna
}
```

```sisu
fn start() {                     // error[E0028]
    print("start")
}
```

## panic

`panic(message)` stops the program. It writes the message to standard error and the program ends with the exit code 101.

```sisu panics
fn check(n: i64) -> i64 {
    if n < 0 {
        panic("negative: {n}")
    }
    return n
}

fn main() {
    print(check(2))              // 2
    print(check(-1))
}
```

```stderr
sisu: panic at example.sisu:3:9: negative: -1
```

The rules:

- The message is a `string` ([E0032](../diagnostics/E0032.md)).
- Standard error gets `sisu: panic at`, then the file, line and column of `panic`, then the message.
- The program writes all its output from before the panic first.
- A call of `panic` has the type `never`. So it can be where a value is expected, such as on the right side of `??` ([Types](types.md#never)).

```sisu panics
fn main() {
    let found: i64? = None
    print(found ?? panic("no value"))
}
```

```stderr
sisu: panic at example.sisu:3:20: no value
```

## todo

`todo` marks code that you did not write yet. When it runs, it panics with the message `not yet implemented`. The program builds with a warning at each `todo` ([W0003](../diagnostics/W0003.md)).

```sisu panics
fn area(width: i64) -> i64 {
    todo                         // warning[W0003]
}

fn main() {
    print(area(3))
}
```

```stderr
sisu: panic at example.sisu:2:5: not yet implemented
```

The rules:

- `todo` is a keyword. It has two forms: `todo`, and `todo("message")` with one message.
- With a message, the panic message is `not yet implemented: ` and then the message.
- Other forms, such as `todo()` or `todo!()`, are errors ([E0099](../diagnostics/E0099.md)).
- `todo` has the type `never`, as a call of `panic` has. So it can stand in for the value of a function.

```sisu panics
fn main() {
    todo("read the file")        // warning[W0003]
}
```

```stderr
sisu: panic at example.sisu:2:5: not yet implemented: read the file
```
