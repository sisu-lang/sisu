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

**Rules**

- A name that a statement in the block declares is in scope only to the end of the block.
- A block that does not end in an expression gives a `unit` value ([unit](types.md#unit)).
- A block that reaches a [`return`](statements.md#return), [`break` or `continue`](statements.md#break-and-continue) has the type `never` ([Types](types.md#never)).
- A statement cannot start with `{`. [E0081](../diagnostics/E0081.md)

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

**Rules**

- The condition must be a `bool`. [E0032](../diagnostics/E0032.md)
- The branches are the members of a [join](#joins). [E0032](../diagnostics/E0032.md)
- A branch of the type `never` takes the type of the others ([Types](types.md#never)).
- An `if` without an `else` whose branch gives `unit` gives a `unit` value. When its branch has a value of another type, the `if` has no value. It is a statement. Where `unit?` is expected, an `if` without an `else` has no value either. [E0043](../diagnostics/E0043.md)
- An `else if` is an `if` in the `else` branch.

```sisu
fn main() {
    let n = 7
    let kind = if n > 5 { "big" } else { 0 }    // error[E0032]
}
```

## match

A `match` is an expression. Its value is the value of the arm that matches. The values of the arms follow the rules of the branches of an `if`. [Enums and patterns](enums.md#match) tells the patterns, the guards and the coverage rules.

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

## Joins

Some expressions combine values into one type. Each combined value is a *member* of a *join*. These are the members:

- the branches of an `if` or an `if let`, and the arms of a `match`
- the elements of an [array literal](arrays.md#array-literals)
- the `break` values of one [loop](#loop)
- the two sides of `??`
- the two operands of an operator, and the operands of a [comparison chain](operators.md#comparison-chains)
- the arguments of one type parameter of a [generic call](generics.md#inference)

**Rules**

- The type of a join is the type of one of its members. Each other member converts to that type ([ADR 0052](../adr/0052-a-join-takes-the-type-of-one-of-its-members.md)). [E0032](../diagnostics/E0032.md)
- A `T` converts to a `T?`. A value of a type that conforms to an interface `I` converts to an `any I`.
- When a member is optional, the type of another member with `?` added can be the type of the join. With an `e: any Seen`, `[e, Some(Eye(v: 1))]` is a `[(any Seen)?]`.
- A join never makes an `any I` that no member has. With a `Lid` and an `Eye` that conform to `Seen`, `[Eye(v: 1), Lid(v: 2)]` is an error. [E0032](../diagnostics/E0032.md)
- When the members are optional at different depths, the type has each of these `?`. So `[[[1, None], None], None]` is a `[[[i64?]?]?]`.
- A value that is already made, such as a local, converts at its top only. With an `m: i64?` and an `xs: [i64]`, `if c { [m] } else { [1] }` is a `[i64?]`, but `if c { [m] } else { xs }` is an error. [E0032](../diagnostics/E0032.md)
- The order of the members changes neither the type nor the error.
- An expected type is the type of the join, and each member must convert to it. [E0032](../diagnostics/E0032.md)
- An integer literal in a join takes its type as [Literals](literals.md#the-type-of-an-integer-literal) says.
- When no type fits each member, the error is at the join. It gives the type of each group of members. [E0032](../diagnostics/E0032.md)

```sisu
interface Seen {
    fn seen(self) -> i64
}

struct Eye: Seen {
    v: i64
    fn seen(self) -> i64 { self.v }
}

struct Lid: Seen {
    v: i64
    fn seen(self) -> i64 { self.v }
}

fn main() {
    let c = [1].len() == 1
    let m: i64? = Some(5)
    let e: any Seen = Eye(v: 3)
    let a = if c { [m] } else { [1] }
    let b = [e, Some(Lid(v: 4))]
    let d = [[[1, None], None], None]
    print(a.len())                         // 1
    print(b.len())                         // 2
    print(d.len())                         // 2
}
```

```sisu
interface Seen {
    fn seen(self) -> i64
}

struct Eye: Seen {
    v: i64
    fn seen(self) -> i64 { self.v }
}

struct Lid: Seen {
    v: i64
    fn seen(self) -> i64 { self.v }
}

fn main() {
    let c = [1].len() == 1
    let m: i64? = Some(5)
    let xs: [i64] = [1]
    let f = [Eye(v: 1), Lid(v: 2)]         // error[E0032]
    let g = if c { [m] } else { xs }       // error[E0032]
}
```

## loop

A `loop` is an expression. `break` with a value ends the loop, and that value is the value of the loop. [Statements](statements.md#loop) tells how a loop runs, and how a loop label names it.

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

**Types**

- The values of the `break`s of a loop are the members of a [join](#joins). [E0032](../diagnostics/E0032.md)
- The expected type is the type of the loop. With none, the join of the `break` values gives it. So with a `v: u8`, `loop { if c { break 1 }; break v }` is a `u8`, as is `loop { if c { break v }; break 1 }`. `v + loop { break 1 }` and `loop { break 1 } + v` are `u8`s too.
- A `break` value of the type `never` takes the type of the other `break` values, as a branch of an `if` does.
- The loop has the type `never` only when it has no `break`, or when each `break` has a value of the type `never` ([Types](types.md#never)).

```sisu
fn main() {
    let c = [1].len() == 1
    let v: u8 = 250
    let n = loop {
        if c {
            break 1
        }
        break v
    }
    print(n + 5)                 // 6
    print(v + loop { break 1 })  // 251
}
```

**Loops with no value**

- A loop whose `break`s have no value gives a `unit` value. This is also true when the other `break`s have values of the type `never`.
- A `break` with no value gives `unit`. So a loop that has a `break` with no value and a `break` with a value of another type is an error. [E0043](../diagnostics/E0043.md)
- A `unit?` value is a value of another type here. So `loop { if c { break o }; break }` with an `o: unit?` is an error, as a `return` with no value is in a function that returns a `unit?` ([E0036](../diagnostics/E0036.md)).

```sisu
fn main() {
    let c = [1].len() == 1
    let o: unit? = None
    let x = loop {
        if c {
            break o
        }
        break                    // error[E0043]
    }
    print(x)
}
```

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

**Arguments**

- The number of arguments must be the number of parameters. [E0053](../diagnostics/E0053.md)
- Each argument must have the type of its parameter. [E0032](../diagnostics/E0032.md)
- Where the parameter is a `T?`, a `T` becomes a `T?` ([Types](types.md#optionals)).
- The argument for a `mut` parameter is the exception. It has exactly the type of the parameter, and it is never promoted ([`mut` parameters](functions.md#mut-parameters)).

**Other rules**

- A call of a function that does not exist is an error. [E0052](../diagnostics/E0052.md)
- A call of a function that has no return type gives a `unit` value ([unit](types.md#unit)).
- A call written with the name of a type, such as `f64(n)`, is a conversion ([Types](types.md#conversions)).

```sisu
fn area(width: i64, height: i64) -> i64 {
    return width * height
}

fn main() {
    print(area(6))               // error[E0053]
}
```

### Calls with mut arguments

An argument for a `mut` parameter is written `mut place`. The call calculates all the arguments from left to right before it reads any `mut` place. Then it copies in each `mut` place, runs the function, and stores each last value back, in the order of the arguments ([What a call does](functions.md#what-a-call-does)).

So a call inside an argument runs first. It can change the place that a `mut` argument names, and the function then gets the changed value. A plain argument that reads the same place gets the value from before the copy-in.

```sisu
fn show(mut xs: [i64], old: [i64]) {
    xs[0] += 1
    print(old)                   // [1, 2]
    print(xs)                    // [2, 2]
}

fn main() {
    let mut xs = [1, 2]
    show(mut xs, xs)
    print(xs)                    // [2, 2]
}
```

### Method calls

A method call `x.m(a)` runs the method `m` of the value `x`, with `x` as its receiver ([Classes](classes.md#methods)). The built-in types have methods too, such as the methods on numbers ([Types](types.md#methods-on-numbers)).

```sisu
fn main() {
    let x = 2.0
    print(x.sqrt())              // 1.4142135623730951
    print(7.min(3))              // 3
}
```

A name that is not a field or a method of the type is an error ([E0063](../diagnostics/E0063.md)). A field called as a method is an error too ([E0058](../diagnostics/E0058.md)).

A method of a class, a struct or an enum read as a field is an error ([E0057](../diagnostics/E0057.md)), and so is `len` of an array. Another built-in method read as a field, such as `s.len` on a string, is E0063.

## Fields

`x.name` reads the field `name` of the object `x` ([Classes](classes.md#fields)). For an optional object, use `?.` ([Operators](operators.md#optional-chaining)).

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

`xs[i]` reads the element at the index `i` of the array `xs` ([Arrays](arrays.md#indexes)). The first element has the index 0. The index must be an `i64` ([E0032](../diagnostics/E0032.md)).

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

Only an array takes a single index. A `string` takes only a range index ([Substrings](strings.md#substrings)). A single index on a string, or any index on another value, is an error ([E0068](../diagnostics/E0068.md)).

`xs[a..b]` is a range index. It gives the part of the array `xs` from `a` up to, and not including, `b`, and it stops the program with a panic when the range is outside the array ([Range indexes](arrays.md#range-indexes)).

## return, break and continue

`return`, `break` and `continue` are expressions of the type `never` ([Types](types.md#never)). They can stand wherever an expression can. Each one takes everything to its right that an expression can take, so `a ?? return b ?? 7` is `a ?? return (b ?? 7)`.

```sisu
fn pick(a: i64?, b: i64?) -> i64 {
    let v = a ?? return b ?? 7
    v
}

fn main() {
    print(pick(None, None))      // 7
    print(pick(None, Some(2)))   // 2
    print(pick(Some(1), None))   // 1
}
```

**What a jump takes**

- A `return` at the end of a line takes no value. A value on the next line is a new statement.
- Before a `{`, a `return` or a `break` with no value takes the block as its value.
- In the header of an `if`, a `while`, a `for` or a `match`, the jump then takes the body as its value, and the body is missing. [E0011](../diagnostics/E0011.md)
- To keep the block as the body, put the jump in `( )`. This does not help a `break` in a loop header or outside every loop.

```sisu
fn f(o: bool?) {
    if o ?? return {             // error[E0011]
        print(1)
    }
}

fn main() {
    f(None)
}
```

**Where a jump is valid**

- A jump where a value must be used is an error. The places that take a `never` value are the right side of `??`, a branch of an `if` and an arm of a `match`. [E0043](../diagnostics/E0043.md)
- The rules of each jump do not change ([return](statements.md#return), [break and continue](statements.md#break-and-continue)).
- In the header of a loop, `break` and `continue` are an error, also on the right of a `??`. [E0082](../diagnostics/E0082.md)

```sisu
fn show(n: i64) {
    print(n)
}

fn main() {
    show(return)                 // error[E0043]
}
```

## Expressions as statements

An expression can stand as a statement, such as a call of `print`. When its value is not used, the program builds with a warning ([W0004](../diagnostics/W0004.md)). There is no warning for the types `unit`, `unit?` and `never`, or for a call of `dbg`. For an `if`, an `if let` or a `match`, the check applies to the value of each branch.

```sisu
fn main() {
    let n = 2
    n * 21                       // warning[W0004]
    print(n)                     // 2
}
```

An expression that has no value cannot be used as a value: as the value of a `let`, an argument or an operand ([E0043](../diagnostics/E0043.md)). These expressions have no value:

- an `if` without an `else` whose branch has a value of a type other than `unit`
- an expression of the type `never` ([Types](types.md#never))

These expressions give a `unit` value. It fits only where `unit`, `unit?` or a type parameter of the called function is expected ([unit](types.md#unit)).

- a call of a function that has no return type, such as `print(1)`
- a block that does not end in an expression
- a `loop` whose `break`s have no value

```sisu
fn main() {
    let x: i64 = print(1)        // error[E0043]
}
```
