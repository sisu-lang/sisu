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

A function in the body of a class or an enum is a method ([Classes](classes.md#methods)). This page shows functions without type parameters. A generic function also has type parameters after its name ([Generics](generics.md#type-parameters)).

```sisu
fn main() {
    print(double(21))            // 42
}

fn double(n: i64) -> i64 {
    return n * 2
}
```

**Names**

- Functions, classes, enums and constants share one set of names. Each name is declared once. [E0034](../diagnostics/E0034.md)
- A function cannot take the name of a built-in type, such as `i64`, or `Option`, `Result`, `Some`, `Ok` or `Err`. [E0026](../diagnostics/E0026.md)
- A function cannot take the name `print`, `eprint`, `panic`, `read_line`, `dbg` or `exchange`. [E0034](../diagnostics/E0034.md)
- [Input and output](io.md) tells these functions. The section [Files](errors.md#files) is in development, and it changes this rule.
- A function cannot be named `_`. [E0080](../diagnostics/E0080.md)
- A binding or a parameter can take the name of a function or a class. A call still finds the function or the class, because a binding is never called ([Shadowing](statements.md#shadowing)).

**Where a function goes**

- A function can call a function that is declared below it.
- A function cannot be declared in the body of another function.
- A function that no live code calls builds with a warning. [W0012](../diagnostics/W0012.md)
- Live code is `main`, each function, method and constant whose name starts with `_`, and all the code that live code calls or names.

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

Each parameter has a name and a type. In the body, a parameter is a binding that holds the value of its argument ([Values and memory](values.md#calls)).

```sisu
fn describe(name: string, age: i64) -> string {
    return "{name} is {age}"
}

fn main() {
    print(describe("Eero", 7))   // Eero is 7
}
```

**Changing a parameter**

- A parameter cannot change, unless it is `mut` ([`mut` parameters](#mut-parameters)). An assignment to another parameter is an error. [E0001](../diagnostics/E0001.md)
- To change the value for the body only, copy it into a `let mut` binding. A `let` can shadow a parameter ([Shadowing](statements.md#shadowing)).

**Names and arguments**

- Two parameters of one function cannot have the same name. [E0039](../diagnostics/E0039.md)
- As a binding cannot, a parameter cannot take the name `Option`, `Result`, `Some`, `Ok`, `Err` or the name of an enum ([Shadowing](statements.md#shadowing)).
- An argument must have the type of its parameter. Where the parameter is a `T?`, a `T` becomes a `T?` ([Calls](expressions.md#calls)).

**Warning**

- A parameter of live code that the body never reads builds with a warning. [W0014](../diagnostics/W0014.md)
- A parameter named `_`, or with a name that starts with `_`, gets no warning.

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

### Unused parameters

A parameter can be named `_`. It binds nothing, and a parameter list can have `_` more than once. Use it for a parameter that the function does not need.

```sisu pending
fn first(x: i64, _: i64) -> i64 {
    x
}

fn main() {
    print(first(1, 2))                 // 1
}
```

## mut parameters

A `mut` parameter lets the function change a place of its caller: a variable, a field or an element. The call writes `mut` before that place too. When the call ends, the place holds the last value of the parameter.

```sisu
fn set(mut cells: [i64], i: i64) {
    cells[i] = 9
}

fn main() {
    let mut xs = [1, 2]
    set(mut xs, 0)
    print(xs)                    // [9, 2]
}
```

In the body, a `mut` parameter is a variable. The body can read it, assign it, write one of its elements or fields, and pass it on as a `mut` argument or as a plain argument. `return cells`, or `cells` as the last expression, gives the value to the caller, and the place still gets the same value.

The place of a `mut` argument can be any of these:

- a `let mut` variable, or a `mut` parameter
- a `mut` field of an object, such as `b.cells`
- an element of a place, such as `grid[i]` or `b.rows[i][j]`

```sisu
class Board {
    mut cells: [i64]
}

fn set(mut cells: [i64], i: i64) {
    cells[i] = 9
}

fn main() {
    let b = Board(cells: [1, 2])
    set(mut b.cells, 1)
    print(b.cells)               // [1, 9]
    let mut grid = [[1], [2]]
    set(mut grid[1], 0)
    print(grid)                  // [[1], [9]]
}
```

**Where a call writes `mut`**

- A `mut` parameter needs `mut` before its argument. A call that leaves it out is an error. [E0166](../diagnostics/E0166.md)
- `mut` before an argument for a parameter that is not `mut` is an error. [E0167](../diagnostics/E0167.md)
- These two rules hold for each call: functions, methods, built-in functions, constructors and enum variants.
- `mut` before an expression that is not an argument, such as `let y = mut x`, is an error. [E0168](../diagnostics/E0168.md)

**What the place can be**

- The argument must be a place that an assignment can write. A `let` binding, a field without `mut`, a call, a literal and a range index are not places. [E0169](../diagnostics/E0169.md)
- The place must have exactly the type of the parameter, and no conversion applies. An `i64` place is not an `i64?`, because the function could store `None` in it. [E0170](../diagnostics/E0170.md)
- A path through `?.`, such as `b?.cells`, reads as an optional and writes as the type of the field. No parameter type fits both, so it is an error too. [E0170](../diagnostics/E0170.md)

**Two `mut` arguments**

- Two `mut` arguments of one call cannot name the same place, or one place that holds the other, when the compiler can see it. [E0171](../diagnostics/E0171.md)
- The compiler sees the same variable with the same fields, and indexes that are the same literal or constant, or the same `let` binding or plain parameter.
- An index that is a `let mut` variable is never an error, because another argument can change it. `swap(mut xs[i], mut xs[j])` is valid, because `i` and `j` can differ.
- When two places are one at run time, the call is valid, and the last store-back wins. This happens when two references name one object (`mut a.cells, mut b.cells` where `a` and `b` are the same object), and when two indexes are equal.

**Methods, interfaces and warnings**

- A method can have `mut` parameters.
- `self` is never `mut`: a method changes the `mut` fields of its receiver through `self`. [E0018](../diagnostics/E0018.md)
- A generic function can have `mut` parameters, and so can an interface requirement.
- A class that conforms to the interface must write `mut` in the same places. [E0152](../diagnostics/E0152.md)
- A `mut` parameter that the function never changes builds with a warning, because its `mut` changes nothing for the caller. [W0002](../diagnostics/W0002.md)

```sisu
fn bump(mut n: i64) {
    n += 1
}

fn main() {
    let n = 1
    bump(n)                      // error[E0166]
    bump(mut n)                  // error[E0169]
}
```

### What a call does

A call with `mut` arguments runs in this order:

1. It calculates the arguments from left to right. For a `mut` argument, it calculates the root of the place when the root is not a binding, such as `mk()` in `mut mk().cells`. It also calculates each index along the place, once, and keeps the values. It reads no object and no element of the place yet.
2. It copies in each `mut` argument, in order. It follows the place from its root, reads each object along it, keeps each of these objects until the store-back, and reads the value of the place. An index that is out of range stops the program with a panic, as `xs[i]` does.
3. It runs the function. Code that reads the place during the call sees the value from step 2.
4. It stores back the last value of each `mut` parameter, in order. Each store-back writes the value into the place, on the objects that step 2 kept, with the indexes from step 1. It runs on each way out of the function, `return` included. When two places are one, the last store-back wins.

So the other arguments run before the copy-in, and a call in one of them can change the place. The copy-in then reads the objects that these calls left: when one of them replaces an object along the place, the function gets the value from the new object.

```sisu
fn g(mut xs: [i64]) -> i64 {
    xs[0] = 5
    return 0
}

fn f(mut xs: [i64], n: i64) {
    xs[1] = 6 + n
}

fn main() {
    let mut xs = [0, 0]
    f(mut xs, g(mut xs))
    print(xs)                    // [5, 6]
}
```

A class object is shared. A write by other code during the call to the place that a `mut` argument names is lost: the store-back replaces it.

```sisu
class Board {
    mut cells: [i64]
}

fn grow(mut cells: [i64], b: Board) {
    b.cells = [9]
    cells[0] = 1
}

fn main() {
    let b = Board(cells: [0, 0])
    grow(mut b.cells, b)
    print(b.cells)               // [1, 0]
}
```

When the function removes the element that the place names, the store-back has no element to write. It drops the value, and the program goes on.

```sisu
class Board {
    mut cells: [i64]
}

fn set(mut n: i64, b: Board) {
    b.cells = []
    n = 9
}

fn main() {
    let b = Board(cells: [0])
    set(mut b.cells[0], b)
    print(b.cells)               // []
}
```

### exchange

`exchange(mut place, with)` stores `with` in the place, and gives the value that the place held. It is a function of the prelude, so a program cannot declare a top-level item with its name ([E0034](../diagnostics/E0034.md)).

```sisu
class Board {
    mut cells: [i64]
}

fn main() {
    let b = Board(cells: [1])
    let old = exchange(mut b.cells, [])
    print(old)                   // [1]
    print(b.cells)               // []
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

**Rules**

- Each path through the body must end in a value or in a statement that never finishes ([Reachability](statements.md#the-end-of-a-function)). [E0031](../diagnostics/E0031.md)
- The value must have the type `T`. [E0032](../diagnostics/E0032.md)
- When `T` is an optional `U?`, a `U` value becomes a `U?` ([Types](types.md#optionals)).
- A function without a return type gives no value, so a call of it has no value. [E0043](../diagnostics/E0043.md)
- When the body of a function without a return type ends in a value, the program builds with a warning. [W0004](../diagnostics/W0004.md)
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

### A call without a value

A function without a return type has the return type `unit`. A call of it gives no value to print or to compare ([unit](types.md#unit)).

```sisu
fn log(text: string) {
    print(text)
}

fn main() {
    print(log("hi") == 1)             // error[E0043]
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

A function that calls itself on every path can never return. The program builds with a warning when each path reaches such a call before a way out ([W0010](../diagnostics/W0010.md)).

For this check, a loop that can finish a pass is a way out. So a call after such a loop does not warn. To repeat without an end, write a `loop`.

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

A program starts with a call of its function `main`. When that first call returns, the program ends with the exit code 0.

```sisu
fn main() {
    print("start")               // start
}
```

**The signature**

- `main` takes no parameters, or one `[string]`. It has no return type. [E0029](../diagnostics/E0029.md)
- A `main` with a `[string]` parameter gets the arguments of the program, without the name of the program ([Input and output](io.md#program-arguments)).
- A program without `main` is an error. [E0028](../diagnostics/E0028.md)

**The run**

- A `return` in the first call of `main` ends the program.
- A program can call `main` again. That call returns to its caller, as any call does.
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

### A main that fails

`main` can return a `unit | E`. When it returns a failure, the program writes `error: ` and the text of the error to standard error. Then it ends with the exit code 1 ([A main that fails](errors.md#a-main-that-fails)).

```sisu pending
fn main() -> unit | string {
    fail "no input"
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

**The message**

- The message is a `string`. [E0032](../diagnostics/E0032.md)
- Standard error gets `sisu: panic at`, then the file, line and column of `panic`, then the message.
- The program writes all its output from before the panic first.

**Where `panic` can go**

- A call of `panic` has the type `never` ([Types](types.md#never)). So it can be a branch of an `if` or a `match`, or the right side of `??`.
- It can also be the value of a `break`, or the end of a function body.
- In other places, it is an error. Such places are an argument, an operand, a condition, an index, and the value of a `let`, an assignment or a `return`. [E0043](../diagnostics/E0043.md)
- As the argument of `print` or `eprint`, it is an error too. [E0061](../diagnostics/E0061.md)

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

**Rules**

- `todo` is a keyword. It has two forms: `todo`, and `todo("message")`. The message is one `string`.
- With a message, the panic message is `not yet implemented: ` and then the message.
- `todo()` and `todo!()` are errors. [E0099](../diagnostics/E0099.md)
- `todo` has the type `never`, as a call of `panic` has. So it can be in the same places, such as the end of a function body.

```sisu panics
fn main() {
    todo("read the file")        // warning[W0003]
}
```

```stderr
sisu: panic at example.sisu:2:5: not yet implemented: read the file
```

## Functions as values

A function is a value of a function type, such as `(i64) -> i64`. A program can pass it, store it and return it ([Function types](closures.md#function-types)).

```sisu pending
fn double(n: i64) -> i64 {
    n * 2
}

fn apply(f: (i64) -> i64, x: i64) -> i64 {
    f(x)
}

fn main() {
    print(apply(double, 21))           // 42
}
```
