# Statements

A *statement* is one step of a block: a declaration, an assignment, a loop, a jump or an expression. The statements of a block run in order, one after the other.

```sisu
fn main() {
    let names = ["Aino", "Eero", "Ilona"]
    let mut total = 0
    for name in names {
        total += name.len()
    }
    print(total)                 // 13
}
```

A line break ends a statement, and a `;` separates two statements on one line ([Lexical structure](lexical.md#line-breaks)). An expression can also stand as a statement ([Expressions](expressions.md#expressions-as-statements)).

## let

`let` declares a *binding*: a name for a value. The binding cannot change. The name is in scope from the statement after the `let` to the end of its block.

```sisu
fn main() {
    let width = 6
    let height: i64 = 7
    print(width * height)        // 42
}
```

The rules:

- A `let` always has a value. Without a type name, the binding gets the type of its value ([Types](types.md#type-names)).
- With a type name, the value must have that type ([E0032](../diagnostics/E0032.md)). When the type name is a `T?`, a `T` value becomes a `T?` ([Types](types.md#optionals)).
- The value must be an expression that has a value. [Expressions](expressions.md#expressions-as-statements) lists the expressions without one ([E0043](../diagnostics/E0043.md)).
- The value cannot read the name that the `let` declares. That name starts after the `let` ([E0033](../diagnostics/E0033.md)).
- An assignment to a binding of a `let` is an error ([E0001](../diagnostics/E0001.md)). To change it, declare it with `let mut`.

```sisu
fn main() {
    let count = 1
    count = 2                    // error[E0001]
}
```

### let mut

`let mut` declares a binding that an assignment can change.

```sisu
fn main() {
    let mut count = 0
    count = count + 1
    count += 1
    print(count)                 // 2
}
```

A `let mut` binding must change somewhere in its scope. When no assignment changes it or one of its elements, the program builds with a warning ([W0002](../diagnostics/W0002.md)).

### let _

`let _ = value` calculates the value and then drops it. It declares no binding. Use it to drop a value on purpose, with no warning ([W0004](../diagnostics/W0004.md)).

```sisu
fn tell(n: i64) -> i64 {
    print("told {n}")
    return n
}

fn main() {
    let _ = tell(3)              // told 3
}
```

The rules:

- `_` is not a name, so `let mut _` is an error ([E0078](../diagnostics/E0078.md)).
- `_` is not a value. Reading it, or an assignment `_ = value`, is an error ([E0079](../diagnostics/E0079.md)).
- As for any `let`, the value must be an expression that has a value ([E0043](../diagnostics/E0043.md)).

### Shadowing

A `let` can take the name of a binding or a constant that is already in scope. The new binding hides the old one until the end of its block. It can have a different type.

```sisu
fn main() {
    let n = 5
    let n = n * 2
    print(n)                     // 10
    if n > 5 {
        let n = "large"
        print(n)                 // large
    }
    print(n)                     // 10
}
```

A binding can also take the name of a function, such as `print`, or of a class. A call still finds the function or the class, because a binding is never called.

```sisu
const LIMIT = 3

fn main() {
    let LIMIT = 4
    let print = LIMIT * 2
    print(print)                 // 8
}
```

The rules:

- The value of the new `let` reads the old binding, because the new name starts after the `let`.
- A `let` can shadow a parameter.
- A `let mut` must change before a later `let` in the same block shadows it ([W0002](../diagnostics/W0002.md)).
- Each pass of a loop body runs its `let`s again, so each pass gets new bindings.
- No binding or parameter can take the name `Option`, `Result`, `Some`, `Ok` or `Err` ([E0026](../diagnostics/E0026.md)). `None` is a keyword.
- No binding or parameter can take the name of an enum ([E0116](../diagnostics/E0116.md)).

## Assignment

An assignment `place = value` stores a value in a place. These are the places:

- a binding of a `let mut`
- a `mut` field of an object ([Fields](expressions.md#fields))
- an element of an array in one of these places, such as `xs[i]` or `grid[i][j]`

```sisu
class Counter {
    mut hits: i64
}

fn main() {
    let mut xs = [1, 2, 3]
    xs[0] = 10
    let c = Counter(hits: 0)
    c.hits = 5
    print(xs[0] + c.hits)        // 15
}
```

The rules:

- The value must have the type of the place ([E0032](../diagnostics/E0032.md)). Where the place is a `T?`, a `T` becomes a `T?`.
- As for a `let`, the value must be an expression that has a value. So `x = panic("stop")` is an error ([E0043](../diagnostics/E0043.md)).
- A binding without `mut`, such as one of a `let` or a `for`, cannot change. Neither can a parameter, a field without `mut` or a constant ([E0001](../diagnostics/E0001.md)).
- To write a `mut` field, the binding that holds the object does not need `mut`. To write an element, the array must be in a place that can change.
- A place cannot go through `?.` ([E0022](../diagnostics/E0022.md)). An expression that is not a place, such as `a + 1`, is an error ([E0021](../diagnostics/E0021.md)).
- A function is not a place ([E0074](../diagnostics/E0074.md)).
- An assignment is a statement. It has no value, so it cannot be part of an expression, and `a = b = 1` is an error.

The program calculates the place first, then the value. It checks an index against the length of the array after it calculates the value.

```sisu panics
fn tell(n: i64) -> i64 {
    print("value {n}")
    return n
}

fn main() {
    let mut xs = [0, 0]
    xs[5] = tell(3)              // value 3
}
```

```stderr
sisu: panic at example.sisu:8:8: index 5 out of bounds for length 2
```

### Compound assignment

A *compound assignment* `place op= value` is `place = place op value`. The operators are `+=`, `-=`, `*=`, `/=` and `%=`.

```sisu
fn main() {
    let mut n = 10
    n += 5
    n *= 2
    n %= 7
    print(n)                     // 2
    let mut text = "sau"
    text += "na"
    print(text)                  // sauna
}
```

The rules:

- The operator follows the rules of its [arithmetic operator](operators.md#arithmetic). `+=` on a `string` joins two strings.
- The program calculates the place once. So in `xs[next()] += 1`, `next` runs once.
- It reads the place before it calculates the value. So an index that is out of bounds panics before the value is calculated.
- An integer overflow or an integer division by zero stops the program with a panic, as the operator does ([Operators](operators.md#overflow)).
- The place cannot be an optional, such as an `i64?` ([E0032](../diagnostics/E0032.md)).
- A compound assignment to a `char` is an error, because arithmetic does not apply to a `char` ([E0094](../diagnostics/E0094.md)).

```sisu
fn main() {
    let mut letter = 'a'
    letter += 1                  // error[E0094]
}
```

## Constants

A `const` declares a constant: a name for a value that `sisu` calculates when the program builds. A constant is declared at the top level, next to the functions.

```sisu
const WIDTH = 6
const HEIGHT: i64 = 7
const AREA = WIDTH * HEIGHT
const GREETING = "Hello, " + "sauna"

fn main() {
    print(AREA)                  // 42
    print(GREETING)              // Hello, sauna
}
```

The rules:

- A constant is an `i64`, a `u8`, an `f64`, a `bool`, a `char` or a `string` ([E0085](../diagnostics/E0085.md)).
- Its value can use literals, operators, the conversions that cannot fail, the [constants of `f64`](types.md#constants) and the constants above it ([E0086](../diagnostics/E0086.md)).
- A call, a binding, an array, an `if`, a block or an interpolation in the value is an error ([E0086](../diagnostics/E0086.md)). This applies also to a side of `&&` or `||` that the program would skip.
- A constant can use only the constants that are declared above it, and not itself ([E0087](../diagnostics/E0087.md)).
- A value that would panic, such as an integer overflow, is an error when the program builds ([Operators](operators.md#arithmetic-in-a-constant)). So is a conversion that can fail ([E0100](../diagnostics/E0100.md)).
- A function can use a constant that is declared below it.
- Functions, classes, enums and constants share one set of names ([E0034](../diagnostics/E0034.md)).
- A constant cannot take the name of a built-in type, or `Option`, `Result`, `Some`, `Ok` or `Err` ([E0026](../diagnostics/E0026.md)).
- A constant cannot take the name of a built-in function, such as `print` ([E0034](../diagnostics/E0034.md)). It cannot be named `_` ([E0080](../diagnostics/E0080.md)).
- A constant cannot change ([E0001](../diagnostics/E0001.md)), and a `const` in a function is an error ([E0084](../diagnostics/E0084.md)). Use `let` there.

```sisu
const TOTAL = PART + 1           // error[E0087]
const PART = 2

fn main() {
    print(TOTAL)
}
```

## while

A `while` loop runs its body again and again while its condition is `true`. The program tests the condition before each pass, also before the first.

```sisu
fn main() {
    let mut n = 1
    while n < 100 {
        n *= 3
    }
    print(n)                     // 243
}
```

The rules:

- The condition must be a `bool` ([E0032](../diagnostics/E0032.md)).
- A `while` has no value. A `break` that ends a `while` cannot have a value ([E0141](../diagnostics/E0141.md)).
- A `while` can warn when its condition reads locals that the loop does not change ([W0011](../diagnostics/W0011.md)). The diagnostic page tells when it warns.
- When W0011 warns about a `let mut` that never changes, [W0002](../diagnostics/W0002.md) does not warn about it.
- A `while` with the condition `true` builds with a warning ([W0009](../diagnostics/W0009.md)). Write `loop` for an endless loop.

```sisu
fn main() {
    let mut n = 0
    while true {                 // warning[W0009]
        n += 1
        if n == 3 {
            break
        }
    }
    print(n)                     // 3
}
```

```sisu no-run
fn main() {
    let mut i = 0
    let n = 3
    while i < n {                // warning[W0011]
        print(i)
    }
}
```

## for

A `for` loop runs its body once for each element of an array, or for each integer of a range. On each pass, its name holds the next element.

```sisu
fn main() {
    for name in ["Aino", "Eero"] {
        print(name)
    }
    for i in 0..3 {
        print(i)
    }
    for i in 1..=2 {
        print(i * 10)
    }
}
```

```output
Aino
Eero
0
1
2
10
20
```

A *range* `start..end` holds the integers from `start` up to `end`, without `end`. A range `start..=end` holds `end` too.

The rules:

- The source of a `for` is an array or a range ([E0035](../diagnostics/E0035.md)). An optional array must be unwrapped first ([E0062](../diagnostics/E0062.md)). A range is valid only in a `for` and in a pattern ([E0023](../diagnostics/E0023.md)).
- The two ends of a range are `i64` ([E0032](../diagnostics/E0032.md)). When `start` is not less than `end`, `start..end` is empty. When `start` is more than `end`, `start..=end` is empty.
- `start..=end` also works when `end` is the largest `i64`.
- The program calculates the source once, before the first pass. For a range, it calculates `start`, then `end`.
- The loop visits the elements that the array has when the loop starts. A change to the array in the body does not change them.
- The name is a binding without `mut`, so an assignment to it is an error ([E0001](../diagnostics/E0001.md)). Its scope is the body.
- `for _ in` runs the body and binds no name.
- A `for` has no value. A `break` that ends a `for` cannot have a value ([E0141](../diagnostics/E0141.md)).

```sisu
fn main() {
    let mut xs = [1, 2, 3]
    for x in xs {
        xs[0] = 100
        print(x)
    }
    let mut n = 0
    for _ in 0..n + 2 {
        n += 1
    }
    print(xs[0] + n)
}
```

```output
1
2
3
102
```

## loop

A `loop` runs its body again and again, until a `break` ends it. A `loop` is also an expression: a `break` with a value gives the loop that value ([Expressions](expressions.md#loop)).

```sisu
fn main() {
    let mut n = 0
    loop {
        n += 1
        if n == 3 {
            break
        }
    }
    print(n)                     // 3
}
```

A `loop` that no `break` ends runs again and again. Only a `return`, a panic, or a jump to a loop around it can leave it. Only a `break` that ends the `loop` gives it a value.

```sisu no-run
fn main() {
    loop {
        print("again")
    }
}
```

## break and continue

`break` ends the loop around it. `continue` ends the current pass, and the loop goes on with its next pass. In a `while`, the next pass tests the condition first.

```sisu
fn main() {
    for i in 0..10 {
        if i % 2 == 0 {
            continue
        }
        if i > 6 {
            break
        }
        print(i)
    }
}
```

```output
1
3
5
```

The rules:

- Without a loop label, `break` and `continue` act on the innermost loop around them.
- `break` and `continue` outside a loop are an error ([E0038](../diagnostics/E0038.md)). They never leave a function.
- The header of a loop is the condition of a `while` or the source of a `for`. A jump in the header cannot act on that loop or on a loop around it ([E0082](../diagnostics/E0082.md)).
- A loop inside a header has a body of its own, and its jumps can act on it.
- Only a `break` that ends a `loop` can have a value ([E0141](../diagnostics/E0141.md)). `continue` never has a value.

## Loop labels

A *loop label* is a name and a `:` before a loop. `break name` and `continue name` act on the loop with that label, also from an inner loop.

```sisu
fn main() {
    rows: for row in 0..3 {
        for col in 0..3 {
            if col > row {
                continue rows
            }
            if row == 2 {
                break rows
            }
            print("{row} {col}")
        }
    }
}
```

```output
0 0
1 0
1 1
```

The rules:

- A loop label can be before `loop`, `while` or `for`, and on a line of its own. Before an `if`, a `match` or a block, it is an error ([E0145](../diagnostics/E0145.md)). So is the loop label `_`.
- A loop cannot take the loop label of a loop around it ([E0143](../diagnostics/E0143.md)). Two loops one after the other can have the same loop label.
- At a jump that names a loop label, no binding, function, constant, class, enum or built-in type in scope can have that name ([E0142](../diagnostics/E0142.md)).
- A `continue` that names no loop label around it is an error ([E0144](../diagnostics/E0144.md)).
- A loop label that no `break` or `continue` names builds with a warning ([W0008](../diagnostics/W0008.md)).

After `break`, a name is a loop label when a loop around the `break` has that loop label. Then the rest of the statement is the value. Otherwise, the name starts the value.

After a loop label, the value cannot start with a token that only continues an expression, such as `+` or `.`. A `(` or `[` right after the loop label needs a space before it, as in `break outer (x)` ([E0142](../diagnostics/E0142.md)).

```sisu
fn main() {
    let a = outer: loop {
        break outer -1
    }
    let outer = 10
    let b = loop {
        break outer -1
    }
    print(a)                     // -1
    print(b)                     // 9
}
```

## return

`return value` ends the function and gives its value to the caller. `return` alone ends a function that has no return type. A `return` leaves all the loops around it.

```sisu
fn find(xs: [i64], wanted: i64) -> i64 {
    for i in 0..xs.len() {
        if xs[i] == wanted {
            return i
        }
    }
    return -1
}

fn main() {
    print(find([4, 8, 15], 8))   // 1
    print(find([4, 8, 15], 9))   // -1
}
```

The rules:

- In a function with a return type, each `return` has a value of that type ([E0036](../diagnostics/E0036.md), [E0032](../diagnostics/E0032.md)). Where the type is a `T?`, a `T` becomes a `T?`.
- In a function without a return type, a `return` has no value ([E0037](../diagnostics/E0037.md)).
- The value must be an expression that has a value, so `return panic("stop")` is an error ([E0043](../diagnostics/E0043.md)).
- A `return` from the first call of `main` ends the program. Another call of `main` returns to its caller ([Functions](functions.md#main)).

## Reachability

Some statements never finish, so the code after them cannot run. A statement never finishes when its type is `never`, and [Types](types.md#never) lists these expressions. These are the statements of the type `never`:

- `return`, `break` and `continue`
- an expression of the type `never`, such as a call of `panic` or `todo` ([Functions](functions.md#panic)), or a `loop` that no `break` ends
- a `while` whose condition is the literal `true`, with no `break` that ends it

A block with such a statement never finishes either. A statement after it cannot run, and the program builds with a warning at the first one ([W0001](../diagnostics/W0001.md)).

```sisu
fn main() {
    print(1)                     // 1
    return
    print(2)                     // warning[W0001]
}
```

### The end of a function

A function with a return type must not reach the end of its body without a value. Each path through the body must end in a value or in a statement that never finishes ([E0031](../diagnostics/E0031.md)).

```sisu
fn first_even(xs: [i64]) -> i64 {
    for x in xs {
        if x % 2 == 0 {
            return x
        }
    }
}                                // error[E0031]

fn main() {
    print(first_even([3, 8]))
}
```

The rules:

- A `for`, and a `while` with a condition that is not `true`, can always finish. This applies also when its body always returns.
- An `if` without an `else` can always finish.
- The literal `true` is the only condition that makes a `while` never finish, also in parentheses. `while 1 == 1` and a constant that is `true` can finish.
- A `break` in an inner loop ends only that loop, so it does not end the outer `loop` or `while true`. A `break` with the outer loop's label does.
- Each `break` counts, also one that cannot run, such as a `break` after a `return`.
- A function without a return type can reach the end of its body. Then it returns to its caller.

```sisu
fn first_even(xs: [i64]) -> i64 {
    let mut i = 0
    loop {
        if xs[i] % 2 == 0 {
            return xs[i]
        }
        i += 1
    }
}

fn main() {
    print(first_even([3, 8, 5]))     // 8
}
```
