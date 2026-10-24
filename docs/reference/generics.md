# Generics

A *generic* declaration has *type parameters*, so that one definition works for many types. A use of it gives a *type argument* for each parameter.

```sisu
class Box<T> {
    value: T
}

fn first<T>(xs: [T]) -> T? {
    if xs.len() > 0 { xs[0] } else { None }
}

fn main() {
    let b = Box(value: "sauna")
    print(b.value)                     // sauna
    print(first([10, 20]) ?? 0)        // 10
    print(first(["Aino"]) ?? "none")   // Aino
}
```

`first` works for an array with elements of any type. A function, a class and an enum can be generic. This page also tells what a generic body can do with a value of a type parameter, and which features are in development.

## Type parameters

The type parameters of a declaration are names between `<` and `>` after the name of the declaration. A function, a class or an enum can have them.

```sisu
class Pair<A, B> {
    first: A
    second: B

    fn swap(self) -> Pair<B, A> {
        Pair(first: self.second, second: self.first)
    }

    fn with_second<C>(self, second: C) -> Pair<A, C> {
        Pair(first: self.first, second: second)
    }
}

enum Tree<T> {
    Leaf
    Node(Tree<T>, T, Tree<T>)
}

fn size<T>(t: Tree<T>) -> i64 {
    match t {
        Leaf => 0
        Node(l, _, r) => size(l) + 1 + size(r)
    }
}

fn main() {
    let p = Pair(first: 1, second: "one")
    print(p.swap().first)              // one
    print(p.with_second(2.5).second)   // 2.5
    let t = Tree.Node(Tree.Leaf, 5, Tree.Node(Tree.Leaf, 6, Tree.Leaf))
    print(size(t))                     // 2
}
```

The rules:

- A type parameter is a type in the whole declaration: in its parameters, its return type, its fields, its payloads and the types of the bindings in its body.
- A method of a generic class or enum uses the type parameters of its type. A method can also add type parameters of its own, such as `C` above.
- The type parameters of one declaration have different names. A method cannot reuse a name from its type ([E0034](../diagnostics/E0034.md)).
- A type parameter cannot take the name of a built-in type ([E0026](../diagnostics/E0026.md)).
- `main` has no type parameters ([E0029](../diagnostics/E0029.md)).

```sisu
class Box<T> {
    value: T

    fn map<T>(self, x: T) -> T {         // error[E0034]
        x
    }
}

fn main() {}
```

## Type arguments

A type argument is a type between `<` and `>` after the name of a generic class or enum. It can be any type that a value can have, such as `i64`, `[string]` or another generic type.

```sisu
class Box<T> {
    value: T
}

fn main() {
    let a: Box<i64> = Box(value: 1)
    let b: Box<Box<string>> = Box(value: Box(value: "sauna"))
    let c: [Box<bool>] = [Box(value: true)]
    let d: Box<f64>? = None
    print(a.value)                     // 1
    print(b.value.value)               // sauna
    print(c.len())                     // 1
    print(d == None)                   // true
}
```

The rules:

- A use of a generic class or enum gives one type argument for each type parameter ([E0137](../diagnostics/E0137.md)). `Box` alone, and `Box<i64, i64>`, are errors.
- A class, an enum or a function that is not generic takes no type arguments ([E0137](../diagnostics/E0137.md)).
- The type arguments of a variant go on its enum: `Tree<i64>.Leaf`, not `Tree.Leaf<i64>` ([E0137](../diagnostics/E0137.md)).
- A type argument cannot be `unit`, `never` or an optional ([E0138](../diagnostics/E0138.md)). The type of a call that gives no value, such as `print(1)`, is `unit`. An array of optionals, such as `[i64?]`, is valid. The sections [Optionals of optionals](types.md#optionals-of-optionals) and [unit](types.md#unit) are in development, and they lift the rule for optionals and for `unit`.

```sisu
class Box<T> {
    value: T
}

fn main() {
    let c: Box = Box(value: 1)         // error[E0137]
}
```

## Inference

A call usually needs no type arguments. `sisu` finds each one from the arguments of the call, or from the type that the context expects. The context can be a `let` with a type, a parameter or a return type.

```sisu
class Stack<T> {
    mut items: [T]
}

fn empty<T>() -> Stack<T> {
    Stack(items: [])
}

fn first<T>(xs: [T]) -> T? {
    if xs.len() > 0 { xs[0] } else { None }
}

fn main() {
    // T is i64, from the argument.
    let a = Stack(items: [1, 2])
    // T is string, from the type of b.
    let b: Stack<string> = empty()
    // T is written.
    let c = empty<bool>()
    print(a.items.len())                       // 2
    print(b.items.len())                       // 0
    print(c.items.len())                       // 0
    print(first<string>([]) ?? "empty")        // empty
}
```

The rules:

- A type argument can be written in a call: `first<string>([])`, `Stack<i64>(items: [])`. A method call can write them too.
- Only the arguments and the expected type give a type. A later use of the result does not.
- A `None` or a `[]` alone has no type. When nothing else fixes a type parameter, the call is an error ([E0050](../diagnostics/E0050.md)). Annotate the result, or write the type arguments.

```sisu
fn first<T>(xs: [T]) -> T? {
    if xs.len() > 0 { xs[0] } else { None }
}

fn main() {
    let x = first([])                  // error[E0050]
}
```

### The `<` rule

After a name in an expression, `<` can start a list of type arguments. It does so only when a well-formed list follows and its closing `>` is followed by `(` or `.`. Otherwise `<` is a comparison.

```sisu
fn both(x: bool, y: bool) -> bool {
    x && y
}

fn id<T>(x: T) -> T {
    x
}

fn main() {
    let a = 1
    let b = 2
    let c = 3
    let d = 4
    print(both(a < b, c > d))          // false
    print(id<i64>(a) < id<i64>(b))     // true
}
```

So `both(a < b, c > d)` has two comparisons. In `both(a < b, c > (d))`, the `<` and the `>` read as a list of type arguments for `a`, and `a` is a value. This is an error ([E0137](../diagnostics/E0137.md)). Put the first comparison in parentheses.

```sisu
fn both(x: bool, y: bool) -> bool {
    x && y
}

fn main() {
    let a = 1
    let b = 2
    let c = 3
    let d = 4
    print(both(a < b, c > (d)))        // error[E0137]
}
```

## Values of a type parameter

`sisu` checks a generic body once, where it is written, and not for each type argument. So it reports an error in a generic body also when no call uses the body.

In the body, the type parameter `T` can stand for any type. So the body can pass, return and store a value of type `T`, and it can use it in an optional or an array.

```sisu
fn pick<T>(a: T, b: T, first: bool) -> T {
    if first { a } else { b }
}

fn or_else<T>(x: T?, fallback: T) -> T {
    x ?? fallback
}

fn present<T>(x: T?) -> bool {
    x != None
}

fn main() {
    print(pick("a", "b", false))       // b
    print(or_else(None, 7))            // 7
    print(or_else(3, 7))               // 3
    print(present("x"))                // true
}
```

The rules:

- An operator does not apply to a `T`: not `+`, `<` or `==` ([E0139](../diagnostics/E0139.md)). `contains` on a `[T]` uses `==`, so it is an error too. `x != None` and `x == None` on a `T?` are valid. `x == y` on two `T?` is an error.
- A `T` has no method and no field ([E0063](../diagnostics/E0063.md)).
- `is` does not apply to a `T`, because `T` can stand for a type that is not a class ([E0046](../diagnostics/E0046.md)).
- `print` and interpolation do not take a `T` ([E0059](../diagnostics/E0059.md), [E0098](../diagnostics/E0098.md)).
- `[x; n]` with `x: T` is an error ([E0067](../diagnostics/E0067.md)), because `T` can be a class ([Filled arrays](arrays.md#filled-arrays)). `[None; n]` is valid.
- `match` does not take a `T` ([E0124](../diagnostics/E0124.md)). In a payload of the type `T`, a pattern is a name or `_` ([Variant patterns](enums.md#variants)).

```sisu
fn smaller<T>(a: T, b: T) -> bool {
    a < b                              // error[E0139]
}

fn main() {}
```

The sections [Bounds](#bounds) and [Interfaces](#interfaces) tell how a generic body can ask for more.

## Growing type arguments

A generic function, class or enum cannot use itself with a type argument that grows, directly or through other declarations. `nest([x], n - 1)` in `nest<T>` calls `nest` with `[T]` for `T`, and then with `[[T]]`, without end. This is an error ([E0140](../diagnostics/E0140.md)).

```sisu
fn nest<T>(x: T, n: i64) {
    if n > 0 {
        nest([x], n - 1)               // error[E0140]
    }
}

fn main() {
    nest(1, 3)
}
```

A use with the same type arguments, such as `nest(x, n - 1)`, is valid.

## Bounds

A *bound* says which interface a type argument must conform to. It follows the type parameter after a `:`. `&` joins two bounds, as in `T: Shape & Ord`.

```sisu pending
fn largest<T: Ord>(xs: [T]) -> T? {
    let mut best: T? = None
    for x in xs {
        if let b = best {
            if b < x {
                best = x
            }
        } else {
            best = x
        }
    }
    best
}

fn main() {
    print(largest([3, 9, 2]) ?? 0)     // 9
    print(largest(["b", "a"]) ?? "")   // b
}
```

The rules:

- In the body, a value of type `T` can do what its bounds give it: the methods of the interfaces, and the operators that they map to. `Eq` gives `==` and `!=`. `Ord` gives `<`, `<=`, `>` and `>=`.
- Anything else is an error where the body is written. The help suggests the bound that is missing.
- A type argument that does not conform to a bound is an error at the call.
- Every type parameter can print, with no bound. `print(x)`, `"{x}"` and `x.to_string()` work on a `T`.
- `T: AnyObject` lets the body use `is` on values of type `T`.

## Interfaces

An *interface* lists the methods that a type must have. A class or an enum names its interfaces after its name. A method in an interface has `self` and no body.

```sisu pending
interface Shape {
    fn area(self) -> f64
}

class Circle: Shape {
    r: f64

    fn area(self) -> f64 {
        f64.pi * self.r * self.r
    }
}

fn total_area<T: Shape>(shapes: [T]) -> f64 {
    let mut sum = 0.0
    for s in shapes {
        sum += s.area()
    }
    sum
}

fn main() {
    print(total_area([Circle(r: 1.0), Circle(r: 2.0)]))
}
```

The rules:

- A type that names an interface must have each method of it, with the same signature. A missing method is an error.
- A type conforms to an interface with the same type arguments at most once.
- `Self` in an interface is the type that conforms to it, as in `fn eq(self, other: Self) -> bool`.
- An interface can require other interfaces: `interface Shape: Display`. A type that conforms to `Shape` conforms to `Display` too.
- Several interfaces follow a name, separated by `,`: `class Tree: Display, Eq`.
- A method in an interface can have type parameters of its own. Such an interface can bound a type parameter, but `any` cannot take it.

### Conformance after the declaration

`extend` adds methods to a type that is declared elsewhere. `extend Type: Interface` makes the type conform to the interface, with the methods of that interface in its body. One block names one interface.

```sisu pending
class Celsius {
    degrees: f64
}

class Fahrenheit {
    degrees: f64
}

interface Convert<T> {
    fn convert(self) -> T
}

extend Celsius: Convert<Fahrenheit> {
    fn convert(self) -> Fahrenheit {
        Fahrenheit(degrees: self.degrees * 1.8 + 32.0)
    }
}

fn main() {
    let f: Fahrenheit = Celsius(degrees: 100.0).convert()
    print(f.degrees)                   // 212.0
}
```

The rules:

- `extend Type { ... }` adds methods that follow no interface.
- A program cannot extend a built-in type, such as `string` or `[T]`.
- A generic interface, such as `Convert<T>`, lets one type conform to it several times, with different type arguments.
- When a call can use more than one conformance, the expected type picks one: `let f: Fahrenheit = c.convert()`. Without an expected type, the call is an error.

### Associated types

An *associated type* is a type that each conforming type chooses once. The interface declares it with `type`. A conforming type sets it with `type Name = Type`.

```sisu pending
interface Source {
    type Item
    fn next(self) -> Item?
}

class Countdown: Source {
    mut left: i64

    type Item = i64

    fn next(self) -> i64? {
        if self.left == 0 {
            return None
        }
        self.left -= 1
        self.left + 1
    }
}

fn main() {
    let c = Countdown(left: 2)
    print(c.next() ?? 0)               // 2
}
```

Use an associated type when each type has one answer, such as the item of a `Source`. Use a type parameter, as in `Convert<T>`, when one type can conform in several ways. An interface can have both.

### Display, Eq and Ord

`Display`, `Eq` and `Ord` are interfaces of the language. `print` and interpolation use `Display`. `==` and `!=` use `Eq`. `<`, `<=`, `>` and `>=` use `Ord`.

```sisu pending
class Point {
    x: i64
    y: i64

    fn to_string(self) -> string {
        "({self.x}, {self.y})"
    }
}

class Money: Eq {
    cents: i64

    fn eq(self, other: Money) -> bool {
        self.cents == other.cents
    }
}

fn main() {
    print(Point(x: 1, y: 2))                           // (1, 2)
    print(Money(cents: 5) == Money(cents: 5))          // true
}
```

The rules:

- A class, an enum, an array and a built-in type each have a `Display`. A class prints its name, with its type arguments, and never its fields. A class with a method `to_string` prints the text of that method.
- A class has no `==` and no `<`. A class that conforms to `Eq` has `==`, and one that conforms to `Ord` has `<`. `a is b` compares two objects by identity.
- A value that holds a class, such as an array of objects, has `==` only when the class conforms to `Eq`.
- `f64` conforms to `Eq` and `Ord` with the rules of IEEE 754. In generic code, `nan` is not equal to `nan`, as in other code.
- An enum has `==` when all its payloads have it, and it has `<` only when it conforms to `Ord`.
- Only `to_string` conforms by name. A type conforms to `Eq` or `Ord` only when its declaration names it.

## Type aliases

A *type alias* gives a new name to a type. It is the same type as the type that it names, so no conversion is necessary.

```sisu pending
type Grid = [[char?]]
type Pairs<T> = [[T]]

fn main() {
    let g: Grid = [['a', None], [None, 'b']]
    let p: Pairs<i64> = [[1, 2], [3, 4]]
    print(g.len())                     // 2
    print(p[1][0])                     // 3
}
```

The rules:

- A type alias is a top-level declaration. It shares the names of functions, classes, enums and constants.
- An alias can have type parameters, such as `Pairs<T>`.
- An alias that refers to itself, directly or through another alias, is an error.

## Interface values

`any Shape` is a type for a value of any type that conforms to `Shape`. A value converts to it where an `any` type is expected. A call of a method on it uses the method of the type that the value has.

```sisu pending
interface Shape {
    fn area(self) -> f64
}

class Circle: Shape {
    r: f64

    fn area(self) -> f64 {
        f64.pi * self.r * self.r
    }
}

class Rect: Shape {
    w: f64
    h: f64

    fn area(self) -> f64 {
        self.w * self.h
    }
}

fn main() {
    let shapes: [any Shape] = [Circle(r: 1.0), Rect(w: 2.0, h: 3.0)]
    for s in shapes {
        print(s.area())
    }
}
```

The rules:

- An interface name alone is not a type. Write `T: Shape` for a type parameter, or `any Shape` for a value.
- An interface whose methods are generic, or that uses `Self` besides the receiver, cannot be `any`. So `any Eq` and `any Ord` are errors.
- An `any` value prints through the `Display` of its type. It has no `==` and no `<`.
- A generic function with the bound `T: Shape` accepts an `[any Shape]`.
