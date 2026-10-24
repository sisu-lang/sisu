# Enums and patterns

An *enum* is a type whose values are each one of a fixed list of *variants*. A `match` takes a value apart by its variant.

```sisu
enum Shape {
    Dot
    Circle(i64)
    Rect(w: i64, h: i64)
}

fn area(s: Shape) -> i64 {
    match s {
        Dot => 0
        Circle(r) => 3 * r * r
        Rect(w, h) => w * h
    }
}

fn main() {
    print(area(Shape.Circle(2)))       // 12
    print(area(Shape.Rect(w: 3, h: 4)))   // 12
    print(area(Shape.Dot))             // 0
}
```

A *pattern* describes the values that an arm of a `match` accepts. An `if let` and a `while let` also use patterns.

## Declarations

An enum declaration is `enum`, the name, and a body in `{` and `}`. The body lists the variants, then the methods.

```sisu
enum Light {
    Red
    Amber
    Green

    fn go(self) -> bool {
        match self {
            Green => true
            _ => false
        }
    }
}

fn main() {
    print(Light.Green.go())      // true
    print(Light.Red.go())        // false
}
```

A variant is a name, and then a payload in `(` and `)` when it holds values. The payload lists the types of the values, with a `,` between them. A payload can give each type a label, such as `w: i64`.

The rules:

- A variant name starts with an upper-case letter ([E0113](../diagnostics/E0113.md)).
- A payload labels all its values or none ([E0114](../diagnostics/E0114.md)). A label helps to build the variant. A pattern does not use it.
- A line break separates two variants. A `;` can separate them too, but a `,` cannot ([E0015](../diagnostics/E0015.md)).
- An enum has at least one variant ([E0115](../diagnostics/E0115.md)).
- The variants come before the methods ([E0112](../diagnostics/E0112.md)).
- An enum is declared at the top level. Its name follows the rules of a function name ([Functions](functions.md#declarations)).
- Two variants, or a variant and a method, of one enum cannot have the same name ([E0034](../diagnostics/E0034.md)). A variant can have the name of a class or a function.
- A variant cannot be named `Some`, `None`, `Ok` or `Err` ([E0026](../diagnostics/E0026.md)).
- A generic enum also has type parameters after its name, such as `enum Maybe<T>` ([Generics](generics.md#type-parameters)).

A method follows the rules of a [method of a class](classes.md#methods): it has `self` as its first parameter. `self` is the value of the enum. An enum has no method without `self` ([E0017](../diagnostics/E0017.md)).

A payload can name the enum itself, as `Cons(i64, List)` does below. Some value of the enum must be possible to build ([E0027](../diagnostics/E0027.md)). In this example, `Nil` does it. An optional or an array in a payload does it too.

```sisu
enum List {
    Nil
    Cons(i64, List)
}

fn sum(l: List) -> i64 {
    match l {
        Nil => 0
        Cons(head, tail) => head + sum(tail)
    }
}

fn main() {
    let l = List.Cons(1, List.Cons(2, List.Nil))
    print(sum(l))                // 3
}
```

```sisu
enum Loop {                      // error[E0027]
    Again(Loop)
}

fn main() {
    print(1)
}
```

## Building a variant

`Enum.Variant` builds a variant that holds nothing. `Enum.Variant(values)` builds a variant with a payload. A variant is always built through its enum.

```sisu
enum Shape {
    Dot
    Circle(i64)
    Rect(w: i64, h: i64)
}

fn main() {
    let a = Shape.Dot
    let b = Shape.Circle(5)
    let c = Shape.Rect(w: 3, h: 4)
    let all = [a, b, c]
    print(all.len())             // 3
}
```

The rules:

- A payload without labels takes one value for each type, in order, with no labels. A wrong number of values is an error ([E0053](../diagnostics/E0053.md)), and so is a label ([E0055](../diagnostics/E0055.md)).
- A payload with labels takes each value with its label, in the order of the declaration. The rules are those of a [class constructor](classes.md#constructors) ([E0056](../diagnostics/E0056.md), [E0064](../diagnostics/E0064.md), [E0065](../diagnostics/E0065.md), [E0066](../diagnostics/E0066.md)).
- Each value must have the type of its place ([E0032](../diagnostics/E0032.md)).
- A variant that holds nothing is written with no parentheses, and a variant with a payload always has them ([E0119](../diagnostics/E0119.md)).
- The enum must have the variant ([E0118](../diagnostics/E0118.md)).
- The variant is not written alone, as `Circle(5)` ([E0117](../diagnostics/E0117.md)). Sisu has only the one access operator, `.`, so `Shape::Circle` is an error too ([E0122](../diagnostics/E0122.md)).
- A variant is not a method. `x.Circle(5)` is an error, and so is `Shape.area(x)` ([E0120](../diagnostics/E0120.md)).
- A local, a parameter or a binding cannot have the name of an enum ([E0116](../diagnostics/E0116.md)).

```sisu
enum Shape {
    Circle(i64)
}

fn main() {
    let c = Circle(5)            // error[E0117]
}
```

## Enum values

An enum value behaves as a copy ([Values and memory](values.md#copied-and-shared)). The value never changes. A `let mut` binding can get a new value, and a `match` takes the value apart.

```sisu
enum Light {
    Red
    Green

    fn next(self) -> Light {
        match self {
            Red => Light.Green
            Green => Light.Red
        }
    }
}

fn main() {
    let mut light = Light.Red
    light = light.next()
    print(light == Light.Green)  // true
    print(light.next() == Light.Red)   // true
}
```

The rules:

- An enum value has no fields to read or write ([E0121](../diagnostics/E0121.md)). A `match` names the values of a payload.
- Two values of an enum are equal when they are the same variant and their payloads are equal ([Operators](operators.md#equality)).
- An enum has no order. `<` and the other comparison operators do not take it ([E0032](../diagnostics/E0032.md)).
- An object in a payload stays shared. Two copies of an enum value refer to the same object ([Values and memory](values.md#copied-and-shared)).
- `is` does not take an enum ([E0046](../diagnostics/E0046.md)).

## match

A `match` is an expression. It calculates its value once, tries the arms from the first to the last, and runs the first arm that accepts the value. The value of that arm is the value of the `match`.

```sisu
enum Coin {
    Cent
    Euro(i64)
}

fn main() {
    let c = Coin.Euro(2)
    let cents = match c {
        Cent => 1
        Euro(n) => n * 100
    }
    print(cents)                 // 200
}
```

An arm is a pattern, an optional [guard](#guards), `=>`, and a body. The body is an expression, a block, an assignment, `return`, `break` or `continue`. The names that the pattern binds are in scope in the guard and the body of the arm, and nowhere else.

```sisu
enum Event {
    Add(i64)
    Reset
    Quit
}

fn main() {
    let mut total = 0
    let events = [Event.Add(2), Event.Add(3), Event.Reset, Event.Add(4), Event.Quit, Event.Add(9)]
    for e in events {
        match e {
            Add(n) => total += n
            Reset => total = 0
            Quit => break
        }
    }
    print(total)                 // 4
}
```

The rules:

- `match` takes an enum, an `i64`, a `u8`, a `bool`, a `char` or a `string`. It does not take a class, an array, an optional or an `f64` ([E0124](../diagnostics/E0124.md)).
- A line break or a `;` ends an arm. A `,` is an error ([E0015](../diagnostics/E0015.md)). A line that ends with `|` or `=>` continues the arm. A line that starts with `|`, `=>` or `if` does not. When the arm above is not complete, that is an error ([E0133](../diagnostics/E0133.md)).
- The values of the arms follow the rules of the branches of an `if` ([Expressions](expressions.md#match)). When every arm has the type `never`, so has the `match` ([Types](types.md#never)).
- An arm with a value and an arm with no value cannot be in one `match` ([E0043](../diagnostics/E0043.md)). An arm of the type `never` goes with both. A `match` statement whose arms have no value is valid.
- A `match` statement whose arms have a value gives a warning ([W0004](../diagnostics/W0004.md)).
- Every value of the type must reach an arm ([Coverage](#coverage)).

```sisu
fn main() {
    let n = 5
    match n {
        0 => print("zero")
        1..10 => print("small")
        _ => print("large")
    }
}
```

```output
small
```

```sisu
class Point {
    x: i64
}

fn main() {
    let p = Point(x: 1)
    let n = match p {            // error[E0124]
        _ => 0
    }
}
```

## Patterns

A pattern accepts a value or rejects it. When it accepts the value, it can bind names to parts of the value.

| Pattern | Accepts | Binds |
| --- | --- | --- |
| `_` | Every value | Nothing |
| A name such as `n` | Every value | The name is the value |
| `42`, `-3`, `'a'`, `"sauna"`, `true` | A value equal to the literal | Nothing |
| `1..5`, `1..=5`, `'a'..='z'` | An integer or a `char` in the range | Nothing |
| `Dot`, `Circle(r)`, `Rect(w, h)` | A value of that variant, if each part of its payload accepts its pattern | The names in the sub-patterns |
| `Shape.Circle(r)` | The same, with the enum written | The names in the sub-patterns |
| `Square(w) \| Rect(w, _)` | A value that one of the alternatives accepts | The names that each alternative binds |

```sisu
enum Shape {
    Dot
    Circle(i64)
    Rect(w: i64, h: i64)
}

fn describe(s: Shape) -> string {
    match s {
        Dot => "dot"
        Circle(0) => "empty circle"
        Circle(1..=9) => "small circle"
        Circle(r) => "circle {r}"
        Rect(w, h) if w == h => "square {w}"
        Rect(w, h) => "{w} by {h}"
    }
}

fn main() {
    print(describe(Shape.Dot))                 // dot
    print(describe(Shape.Circle(0)))           // empty circle
    print(describe(Shape.Circle(4)))           // small circle
    print(describe(Shape.Circle(40)))          // circle 40
    print(describe(Shape.Rect(w: 3, h: 3)))    // square 3
    print(describe(Shape.Rect(w: 3, h: 4)))    // 3 by 4
}
```

### Names

A pattern name that does not start with an upper-case letter binds the value. The new name hides any other binding for the arm, and that binding is the same again after the arm. A name that is a constant is an error ([E0126](../diagnostics/E0126.md)).

```sisu
fn main() {
    let x = 1
    let kind = match 5 {
        x if x > 3 => "big"
        _ => "small"
    }
    print(kind)                  // big
    print(x)                     // 1
}
```

A name that starts with an upper-case letter is a variant. The type of the value decides which enum has the variant, so a pattern needs no enum name. Two enums can have a variant of one name.

The rules:

- A variant that is not in the enum is an error ([E0118](../diagnostics/E0118.md)). An upper-case name for a value that is not an enum is an error ([E0125](../diagnostics/E0125.md)).
- A pattern can write the enum, as in `Shape.Circle(r)`. It must be the enum of the value ([E0032](../diagnostics/E0032.md)).
- A name that is a constant is an error, also as the bound of a range ([E0126](../diagnostics/E0126.md)). Compare with the constant in a guard.
- A name that a pattern binds cannot change ([E0001](../diagnostics/E0001.md)). A pattern has no `mut` ([E0011](../diagnostics/E0011.md)).
- A name appears once in a pattern. `Add(x, x)` does not compare two values ([E0129](../diagnostics/E0129.md)).

### Literals and ranges

A literal pattern accepts the values that are equal to it. The literal has the type of the value that the `match` tests ([Literals](literals.md#the-type-of-an-integer-literal)). A literal that does not fit its type is an error, as `300` for a `u8` ([E0040](../diagnostics/E0040.md)). A negative literal for a `u8` is an error too ([E0042](../diagnostics/E0042.md)). A literal of another type is an error ([E0032](../diagnostics/E0032.md)).

A range `a..b` accepts the values from `a` up to, but not including, `b`. A range `a..=b` includes `b`. Both bounds are literals. The type of a range is an integer or a `char`.

- A range that accepts no value is an error, such as `5..=4` and `5..5` ([E0135](../diagnostics/E0135.md)). So is a range of any type but an integer and a `char`, such as a `string`.
- A range has two bounds. `a..<b` and `a...b` are errors ([E0132](../diagnostics/E0132.md)).
- A string pattern is the text as written. It has no interpolation ([E0134](../diagnostics/E0134.md)).

### Variants

A variant pattern has one sub-pattern for each value of the payload, in order. Use `_` for a value that the arm does not need. A variant that holds nothing has no parentheses.

```sisu
enum Expr {
    Num(i64)
    Add(Expr, Expr)
    Mul(Expr, Expr)
}

fn simplify(e: Expr) -> Expr {
    match e {
        Add(Num(0), x) | Add(x, Num(0)) => x
        Mul(Num(1), x) | Mul(x, Num(1)) => x
        other => other
    }
}

fn eval(e: Expr) -> i64 {
    match e {
        Num(n) => n
        Add(a, b) => eval(a) + eval(b)
        Mul(a, b) => eval(a) * eval(b)
    }
}

fn main() {
    let e = Expr.Mul(Expr.Num(1), Expr.Add(Expr.Num(0), Expr.Num(7)))
    print(eval(simplify(e)))     // 7
}
```

The rules:

- A sub-pattern follows the type of its value. It is a variant pattern for an enum. It is a literal for an integer, a `char`, a `bool` or a `string`, and a range for an integer or a `char`. A name or `_` fits every type.
- The number of sub-patterns must be the number of values in the payload ([E0128](../diagnostics/E0128.md)).
- A pattern takes the payload by position. It has no labels and no braces ([E0123](../diagnostics/E0123.md)).
- A payload of the type `f64`, an optional, an array or a class takes only a name or `_` as its pattern. So does a payload of a type parameter such as `T`, in generic code ([Generics](generics.md#values-of-a-type-parameter)).
- In such a place, a bare upper-case name is an error ([E0125](../diagnostics/E0125.md)). So is a variant pattern, with parentheses or with the enum written, and so is `None` ([E0127](../diagnostics/E0127.md)).

```sisu
enum Shape {
    Rect(w: i64, h: i64)
}

fn area(s: Shape) -> i64 {
    match s {
        Rect(w: x, h: y) => x * y     // error[E0123]
    }
}

fn main() {
    print(area(Shape.Rect(w: 2, h: 3)))
}
```

### Alternatives

`p1 | p2` accepts a value that `p1` accepts or `p2` accepts. The pattern of an arm can be an alternative, and so can a sub-pattern.

```sisu
enum Shape {
    Square(i64)
    Rect(i64, i64)
    Dot
}

fn width(s: Shape) -> i64 {
    match s {
        Square(w) | Rect(w, _) => w
        Dot => 0
    }
}

fn main() {
    print(width(Shape.Square(2)))   // 2
    print(width(Shape.Rect(5, 1)))  // 5
    print(width(Shape.Dot))         // 0
}
```

Each alternative binds the same names ([E0136](../diagnostics/E0136.md)) with the same types ([E0032](../diagnostics/E0032.md)). The alternatives are tried from left to right.

## Guards

A *guard* is `if` and a condition after the pattern. The arm accepts the value only when the pattern accepts it and the condition is `true`. The condition can use the names of the pattern.

```sisu
fn check(n: i64) -> bool {
    print("check {n}")
    n > 5
}

fn main() {
    let n = 3
    let size = match n {
        x if check(x) => "big"
        x if check(x + 10) => "medium"
        _ => "small"
    }
    print(size)
}
```

```output
check 3
check 13
medium
```

The rules:

- The condition is a `bool` ([E0032](../diagnostics/E0032.md)).
- The program calculates the condition only when the pattern accepts the value.
- A guard belongs to the whole arm. When an alternative accepts the value and the guard is `false`, the program tries the next alternative. It calculates the guard again, with the names of that alternative.
- With several sets of alternatives, the set nearest the end of the pattern changes first. A set in the arm pattern itself changes last.
- When no alternative has a `true` guard, the `match` goes on with the next arm.
- A guarded arm covers no values, because its guard can be `false` ([Coverage](#coverage)).

```sisu
enum Pair {
    P(i64, i64)
}

fn big(n: i64) -> bool {
    print("check {n}")
    n > 3
}

fn main() {
    let r = match Pair.P(1, 5) {
        P(a, _) | P(_, a) if big(a) => a * 10
        _ => 0
    }
    print(r)
}
```

```output
check 1
check 5
50
```

```sisu
enum Ints {
    T(i64, i64)
}

enum Duo {
    D(Ints, Ints)
}

fn see(x: i64, y: i64) -> bool {
    print("see {x} {y}")
    false
}

fn main() {
    let r = match Duo.D(Ints.T(1, 2), Ints.T(3, 4)) {
        D(T(x, _) | T(_, x), T(y, _) | T(_, y)) if see(x, y) => 1
        D(_, _) => 0
    }
    print(r)
}
```

```output
see 1 3
see 1 4
see 2 3
see 2 4
0
```

## Coverage

A `match` must have an arm for every value that its type can hold. The error says which value no arm accepts ([E0130](../diagnostics/E0130.md)).

```sisu
enum Expr {
    Num(i64)
    Add(Expr, Expr)
    Mul(Expr, Expr)
}

fn eval(e: Expr) -> i64 {
    match e {                    // error[E0130]
        Num(n) => n
        Add(a, b) => eval(a) + eval(b)
    }
}

fn main() {
    print(eval(Expr.Num(1)))
}
```

The rules:

- An enum is covered when its arms accept every variant, and for each variant, every value of its payload.
- A `bool` is covered by `true` and `false`.
- Literals and ranges cover an integer or a `char` when together they hold every value of the type. For example, `0..=127` and `128..=255` cover a `u8`.
- Only `_` or a name covers a `string`.
- `_` and a name cover every value.
- A guarded arm covers nothing.

An arm is unreachable when the arms above it already accept every value that it accepts. The arm never runs, and `sisu` warns about it ([W0006](../diagnostics/W0006.md)). It warns about an unreachable alternative of a `|` too.

```sisu
enum Light {
    Red
    Green
}

fn go(l: Light) -> bool {
    match l {
        _ => false
        Green => true           // warning[W0006]
    }
}

fn main() {
    print(go(Light.Green))      // false
}
```

## if let and while let

`if let pattern = value { ... }` runs its block when the pattern accepts the value. The block can use the names of the pattern. An `else` block runs when the pattern does not accept the value.

```sisu
enum Command {
    Go(i64)
    Stop
    Wait(i64)
}

fn main() {
    let c = Command.Wait(2)
    if let Go(n) = c {
        print("go {n}")
    } else if let Wait(n) = c {
        print("wait {n}")       // wait 2
    } else {
        print("stop")
    }
}
```

`while let pattern = value { ... }` calculates the value, and runs its block while the pattern accepts the value. The program calculates the value again for each turn.

```sisu
enum Step {
    Next(i64)
    Done
}

fn step(i: i64) -> Step {
    if i < 3 { Step.Next(i) } else { Step.Done }
}

fn main() {
    let mut i = 0
    while let Next(n) = step(i) {
        print(n * 10)
        i += 1
    }
}
```

```output
0
10
20
```

The rules:

- The value is an enum or an optional ([E0051](../diagnostics/E0051.md)).
- For an enum, the pattern is any pattern. When the pattern accepts every value of the enum, the test means nothing and the use is an error ([E0131](../diagnostics/E0131.md)). Use a `match`.
- For an optional `T?`, the pattern is a name or `_`. A name is bound to the value of the optional when it has one ([Types](types.md#optionals)).
- The names of the pattern are in scope in the first block only, and not in the `else` block.
- An `if let` with an `else` is an expression, as an `if` is. Without an `else`, it has no value ([E0043](../diagnostics/E0043.md)).

```sisu
fn main() {
    let found: i64? = 7
    if let n = found {
        print(n + 1)             // 8
    }
}
```

```sisu
enum Pair {
    P(i64, i64)
}

fn main() {
    let p = Pair.P(1, 2)
    if let P(a, b) = p {         // error[E0131]
        print(a + b)
    }
}
```
