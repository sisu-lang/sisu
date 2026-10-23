# Expressions

An *expression* is code that the program calculates: a literal, a name, an operation, a call, or a block, `if`, `match` or `loop`. Most expressions give a value.

```sisu
fn square(n: i64) -> i64 {
    return n * n
}

fn main() {
    let n = 4
    let size = if n > 3 { "large" } else { "small" }
    print(square(n) + 1)         // 17
    print(size)                  // large
}
```

## Order of evaluation

The program calculates the parts of an expression from left to right, then uses them. Precedence decides how the parts group, not when they run. These are the parts:

- the operands of an operator
- the arguments of a call
- the receiver of a method, before its arguments
- the elements of an array literal
- the parts of an interpolation

```sisu
fn show(n: i64) -> i64 {
    print("part {n}")
    return n
}

fn add(a: i64, b: i64) -> i64 {
    return a + b
}

fn main() {
    print(show(1) + show(2) * show(3))
    print(add(show(4), show(5)))
}
```

```output
part 1
part 2
part 3
7
part 4
part 5
9
```

The program calculates each part once. `&&`, `||`, `??`, `?.` and a comparison chain can skip a part ([Logical operators](operators.md#logical-operators), [Default values](operators.md#default-values), [Optional chaining](operators.md#optional-chaining), [Comparison chains](operators.md#comparison-chains)).

## Names and parentheses

A name reads the value of a binding or a constant. A name that is not in scope is an error ([E0033](../diagnostics/E0033.md)).

Parentheses group an expression. They do not change its value or its type.

```sisu
fn main() {
    let n = 6
    print((n + 1) * 6)           // 42
}
```

## Blocks

A *block* is a list of statements between `{` and `}`. When its last statement is an expression, that expression is the value of the block.

```sisu
fn main() {
    let area = {
        let width = 6
        let height = 7
        width * height
    }
    print(area)                  // 42
}
```

The rules:

- A name that a statement in the block declares is in scope only to the end of the block.
- A block that does not end in an expression has no value ([E0043](../diagnostics/E0043.md)).
- A block that reaches a `return`, `break` or `continue` has the type `never` ([Types](types.md#never)).
- A statement cannot start with `{` ([E0081](../diagnostics/E0081.md)).

```sisu
fn main() {
    {                            // error[E0081]
        print(1)
    }
}
```

## if

An `if` with an `else` is an expression. Its value is the value of the branch that runs.

```sisu
fn main() {
    let n = 7
    let kind = if n % 2 == 0 { "even" } else { "odd" }
    print(kind)                  // odd
    let size = if n < 5 { 1 } else if n < 10 { 2 } else { 3 }
    print(size)                  // 2
}
```

The rules:

- The condition must be a `bool` ([E0032](../diagnostics/E0032.md)).
- The values of the branches must have one type ([E0032](../diagnostics/E0032.md)). When one branch is a `T` and another a `T?`, the `if` is a `T?`. A branch of the type `never` takes the type of the others ([Types](types.md#never)).
- An `if` without an `else` has no value ([E0043](../diagnostics/E0043.md)). It is a statement.
- An `else if` is an `if` in the `else` branch.

```sisu
fn main() {
    let n = 7
    let kind = if n > 5 { "big" } else { 0 }    // error[E0032]
}
```

## match

A `match` is an expression. Its value is the value of the arm that matches. The values of the arms follow the rules of the branches of an `if`.

```sisu
fn main() {
    let n = 3
    let name = match n {
        1 => "one"
        2 | 3 => "a few"
        _ => "many"
    }
    print(name)                  // a few
}
```

## loop

A `loop` is an expression. `break` with a value ends the loop, and that value is the value of the loop.

```sisu
fn main() {
    let mut n = 1
    let first = loop {
        n = n * 3
        if n > 20 {
            break n
        }
    }
    print(first)                 // 27
}
```

The rules:

- The values of all the `break`s of a loop must have one type ([E0032](../diagnostics/E0032.md)).
- A `break` value of the type `never` takes the type of the other `break` values, as a branch of an `if` does.
- The loop has the type `never` only when it has no `break`, or when each `break` has a value of the type `never` ([Types](types.md#never)).
- A loop whose `break`s have no value has no value ([E0043](../diagnostics/E0043.md)). This applies also when the other `break`s have values of the type `never`.
- A loop that has a `break` with no value and a `break` with a value is an error ([E0043](../diagnostics/E0043.md)).

```sisu
fn wait(ready: bool) {
    loop {
        if ready {
            break
        }
        break panic("not ready")
    }
    print("finished")
}

fn main() {
    wait(true)                   // finished
}
```

## Calls

A call `f(a, b)` runs the function `f` with the arguments `a` and `b`. Its value is the value that the function returns.

```sisu
fn area(width: i64, height: i64) -> i64 {
    return width * height
}

fn main() {
    print(area(6, 7))            // 42
}
```

The rules:

- The number of arguments must be the number of parameters ([E0053](../diagnostics/E0053.md)).
- Each argument must have the type of its parameter ([E0032](../diagnostics/E0032.md)). Where the parameter is a `T?`, a `T` becomes a `T?` ([Types](types.md#optionals)).
- A call of a function that does not exist is an error ([E0052](../diagnostics/E0052.md)).
- A call of a function that has no return type has no value ([E0043](../diagnostics/E0043.md)).
- A call written with the name of a type, such as `f64(n)`, is a conversion ([Types](types.md#conversions)).

```sisu
fn area(width: i64, height: i64) -> i64 {
    return width * height
}

fn main() {
    print(area(6))               // error[E0053]
}
```

### Method calls

A method call `x.m(a)` runs the method `m` of the value `x`, with `x` as its receiver. The built-in types have methods too, such as the methods on numbers ([Types](types.md#methods-on-numbers)).

```sisu
fn main() {
    let x = 2.0
    print(x.sqrt())              // 1.4142135623730951
    print(7.min(3))              // 3
}
```

A name that is not a field or a method of the type is an error ([E0063](../diagnostics/E0063.md)). A field called as a method is an error too ([E0058](../diagnostics/E0058.md)), and so is a method read as a field ([E0057](../diagnostics/E0057.md)).

## Fields

`x.name` reads the field `name` of the object `x`. For an optional object, use `?.` ([Operators](operators.md#optional-chaining)).

```sisu
class Point {
    x: i64
    y: i64
}

fn main() {
    let p = Point(x: 3, y: 4)
    print(p.x + p.y)             // 7
}
```

## Indexes

`xs[i]` reads the element at the index `i` of the array `xs`. The first element has the index 0. The index must be an `i64` ([E0032](../diagnostics/E0032.md)).

```sisu
fn main() {
    let xs = [10, 20, 30]
    print(xs[0])                 // 10
    print(xs[2])                 // 30
}
```

An index that is negative, or not less than the length of the array, stops the program with a panic.

```sisu panics
fn main() {
    let xs = [10, 20, 30]
    print(xs[3])
}
```

```stderr
sisu: panic at example.sisu:3:14: index 3 out of bounds for length 3
```

Only an array has an index. An index on another value, such as a `string`, is an error ([E0068](../diagnostics/E0068.md)).

## Expressions as statements

An expression can stand as a statement, such as a call of `print`. When its value is not used, and it is not a call that returns no value, the program builds with a warning ([W0004](../diagnostics/W0004.md)).

```sisu
fn main() {
    let n = 2
    n * 21                       // warning[W0004]
    print(n)                     // 2
}
```

An expression that has no value cannot be used as a value: as the value of a `let`, an argument or an operand ([E0043](../diagnostics/E0043.md)). These expressions have no value:

- a call of a function that has no return type, such as `print(1)`
- an `if` without an `else`
- a block that does not end in an expression
- a `loop` whose `break`s have no value
- an expression of the type `never` ([Types](types.md#never))

```sisu
fn main() {
    let x = print(1)             // error[E0043]
}
```
