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

`first` works for an array whose elements have a type that a type argument can be ([Type arguments](#type-arguments)). A function, a class and an enum can be generic. This page also tells what a generic body can do with a value of a type parameter, and which features are in development.

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

- A type parameter is a type in the whole declaration. It can be the type of a parameter, a field or a payload, a return type, and a binding in the body.
- A method of a generic class or enum uses the type parameters of its type. A method can also add type parameters of its own, such as `C` above.
- The type parameters of one declaration have different names. A method cannot reuse a name from its type ([E0034](../diagnostics/E0034.md)).
- A type parameter cannot take a name that [E0026](../diagnostics/E0026.md) lists, such as `i64` or `Option`.
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

- A type that names a generic class or enum gives one type argument for each type parameter ([E0137](../diagnostics/E0137.md)). As a type, `Box` alone and `Box<i64, i64>` are errors.
- A class, an enum or a function that is not generic takes no type arguments ([E0137](../diagnostics/E0137.md)).
- The type arguments of a variant go on its enum: `Tree<i64>.Node(...)`, not `Tree.Node<i64>(...)` ([E0137](../diagnostics/E0137.md)).
- A type argument cannot be `unit` or `never` ([E0138](../diagnostics/E0138.md)). The type of a call that gives no value, such as `print(1)`, is `unit`. A type argument can be an optional, such as `i64?` in `Box<i64?>`. The section [unit](types.md#unit) is in development, and it lifts the rule for `unit`.

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
- An argument such as `Ok(1)`, whose own arguments give only some of its types, takes the others from the outer call. In `let r: Result<i64, string> = id(Ok(1))`, the type of `r` gives `T` for `id`, and `T` gives `E` for `Ok`.
- Such an argument also takes the parts of the parameter type that do not wait for a type argument. For `unwrap<T>(r: Result<T, string>)`, `unwrap(Ok(1))` gives `string` for `E` from the parameter, and `Ok(1)` then gives `i64` for `T`.
- An argument that is a call waits for a later argument when only a later argument can fix one of its own type parameters. Take `fn mk2<X>() -> Two<X, bool>`, `fn mk<E>() -> Result<i64, E>` and `g<T>(c: Two<T, bool>, d: Result<T, string>)`. In `g(mk2(), mk())`, the call `mk2()` waits. The call `mk()` fixes `T` as `i64`, and `mk2()` then gives `Two<i64, bool>`. A call waits for the type parameters of the outer call that its parameter type names at the places of its own open type parameters. It is checked when the last of them is fixed, or, if one stays open, after the other arguments. This holds for a call of a function, a construction, `Ok`, `Err` and a variant. A method call does not wait. A call under `Some(...)`, an array literal or another call does not wait either. There, the arguments are checked in order, so a part that only a later argument fixes does not help an earlier one. When a woken call disagrees with the argument that woke it, the woken call is blamed. Take `fn mks<E>() -> Result<string, E>`, `fn mkr<X>() -> Two<i64, Two<X, bool>>` and `gn<T>(c: Two<T, Two<T, bool>>, d: Result<T, string>)`. The call `gn(mkr(), mks())` reports `mkr()`: expected `Two<string, Two<string, bool>>`, found `Two<i64, Two<string, bool>>`.
- When an argument does not agree with the expected type, the error is at the argument. `let r: Result<i64, string> = Ok("x")` reports `"x"`: expected `i64`, found `string` ([E0032](../diagnostics/E0032.md)). With written type arguments, as in `id<i64>(n)`, the error stays at the call.
- An argument also takes the parts that an earlier argument fixed, with the other parts left open. For `f<A, B>(a: A, p: Two<A, B>)`, `f("s", Two(a: 1, b: true))` reports `1`: expected `string`, found `i64`, because `"s"` gives `string` for `A`. This also applies inside `Some(...)`, an array literal or another generic call.
- When the argument has the wrong shape as a whole, the error is at the whole argument, and a type parameter that the call has not fixed yet keeps its name. For `take<T>(xs: [T])`, `take(Ok(1))` reports `Ok(1)`: expected `[T]`, found `Result<i64, E>`, because no `E` makes a `Result` an array.
- An integer literal argument takes the type that the expected type gives its type parameter. This is also true under `dbg`, a block, an `if`, a `match` or another generic call: `let d: u8 = id({ 250 })` is valid.
- A concrete argument fixes a type parameter before a literal does. In `pick(1, x)` with an `x: u8`, `T` is `u8`. An argument that makes an optional, such as `Some(2)` or `if c { 2 } else { None }`, fixes it before a plain literal. So `pick(1, Some(2))` is an `i64?`. An argument that holds an optional also comes first, such as `[Some(2)]`. So does a call that wraps one. A call wraps the optional of an argument when the type of the argument's place and the return type name one type parameter. So `pick([1], [Some(2)])` is a `[i64?]`, and so is `pick([1], wrap(Some(2)))` for `fn wrap<T>(x: T) -> [T]`. An optional at a `T?` place does not make the call hold one, because it fills the `?` of that place. So `pick(h(1, Some(2)), Some(3))` is an `i64?`, with `h` as below.
- A call that waits for one of its own types does not make a call around it hold an optional, whatever its arguments hold. This is true of `Ok(..)`, `Err(..)`, a construction, a variant and a call of a function. Take `fn pick<T>(a: T, b: T) -> T` and `fn okv<T>(r: Result<T, string>) -> T`. In `pick(1, okv(Ok(Some(2))))`, the literal `1` fixes `T` as `i64` first, and `Some(2)` is then an error: expected `i64`, found `i64?` ([E0032](../diagnostics/E0032.md)). Write `pick(Some(1), okv(Ok(Some(2))))`, or give the result a type: `let x: i64? = pick(1, okv(Ok(Some(2))))`.
- An optional context meets the return type whole when an argument at a `T` or `[T]` place makes or holds an optional, with `T` the returned parameter. For `first<T>(xs: [T]) -> T`, `let v: i64?? = first([None, Some(1)])` binds `T` to `i64??`, and `v` is the outer `None`. The same is true for a call that returns `T?`, when the context has no more optionals than the call makes. For `fn opt<T>(x: T) -> T?`, `let w: i64?? = opt(Some(2))` binds `T` to `i64?`, and `let w: i64??? = opt(Some(2))` binds it to `i64?`: the context has more optionals than the call makes, so its payload `i64??` meets `T?`. An optional at a `T?` place fills that `?`. For `fn h<T>(a: T, b: T?) -> T`, the line `let v: i64?? = h(1, Some(2))` is valid. A value that makes an optional inside its optional, such as `Some(Some(2))`, makes the call an optional at a `T?` place. So `let v: i64? = h(1, Some(Some(2)))` is valid too. A context `i64??` binds `T` to `i64?` in that call, as `Some(Some(2))` fits `i64??`.
- In an array literal, a generic call that waits for its type takes it from the other elements. In an `if` or a `match`, it takes it from the other branches. With an `x: u8`, `[x, id(None)]` is a `[u8?]`.
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

- An operator that computes with a `T` or compares it, such as `+`, `<` or `==`, is an error ([E0139](../diagnostics/E0139.md)), unless a bound gives it: `T: Eq` gives `==` and `!=`, and `T: Ord` gives `<`, `<=`, `>` and `>=` ([Bounds](#bounds)). The help names the bound to add. `contains` on a `[T]` uses `==`, so it needs `T: Eq` ([E0154](../diagnostics/E0154.md)). `x != None` and `x == None` on a `T?` are valid. `x == y` on two `T?` needs `T: Eq`. When the other operand has another type, the error is the type mismatch, whatever the bounds are: `a == b` with a `T` and a `U` is [E0047](../diagnostics/E0047.md), and `a < 5` is [E0032](../diagnostics/E0032.md).
- `!`, `&&` and `||` take only a `bool`, so a `T` there is a type mismatch ([E0032](../diagnostics/E0032.md)).
- A `T` has the methods that its bounds give, and `to_string()`, which needs no bound. It has no other method and no field ([E0063](../diagnostics/E0063.md)).
- `is` does not apply to a `T`, because `T` can stand for a type that is not a class ([E0046](../diagnostics/E0046.md)). The bound `T: AnyObject` allows it ([Bounds](#bounds)). The help names the bound to add.
- `print` takes a `T` with no bound, because every type has a `Display` ([Display, Eq and Ord](#display-eq-and-ord)). Interpolation and `to_string()` take a `T` as well.
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

```sisu
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

- In the body, a value of type `T` can do what its bounds give it. These are the methods of the interfaces and the operators that they map to.
- `Eq` gives `==` and `!=`. `Ord` gives `<`, `<=`, `>` and `>=`. An array, an optional or an enum of `T` has `==` when `T` has `Eq`. A call of `a.eq(b)` or `a.less(b)` on a `T` works the same way.
- Anything else is an error where the body is written. The help suggests the bound that is missing.
- A type argument that does not conform to a bound is an error at the call. The call reports it once. The arguments after it are not checked against the type that failed, so they add no error for it.
- Every type parameter can print, with no bound. `print(x)`, `"{x}"` and `x.to_string()` work on a `T`.
- Every class conforms to `AnyObject`. So does an `any` value of a class-only interface (next rule). Nothing else does. `T: AnyObject` lets the body use `is` on values of type `T`, and lets a class declare a [weak field](values.md#weak-fields) of type `T?`. An associated type with the bound `AnyObject` (`type Item: AnyObject`) allows `is` on values of type `T.Item` in the same way.
- An interface that inherits `AnyObject` is class-only. Its `any` values support `is` too, and a class can hold one in a weak field.

```sisu
class Node {}

fn same<T: AnyObject>(a: T, b: T) -> bool {
    a is b
}

fn main() {
    let a = Node()
    print(same(a, a))                  // true
    print(same(a, Node()))             // false
}
```

## Interfaces

An *interface* lists the methods that a type must have. A class or an enum names its interfaces after its name. A method in an interface has `self` and no body.

```sisu
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
    print(total_area([Circle(r: 1.0), Circle(r: 2.0)]))   // 15.707963267948966
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

```sisu
class Celsius {
    degrees: f64
}

class Fahrenheit {
    degrees: f64
}

interface Transform<T> {
    fn transform(self) -> T
}

extend Celsius: Transform<Fahrenheit> {
    fn transform(self) -> Fahrenheit {
        Fahrenheit(degrees: self.degrees * 1.8 + 32.0)
    }
}

fn main() {
    let f: Fahrenheit = Celsius(degrees: 100.0).transform()
    print(f.degrees)                   // 212.0
}
```

The rules:

- `extend Type { ... }` adds methods that follow no interface.
- A program cannot extend a built-in type, such as `string` or `[T]`.
- A generic interface, such as `Transform<T>`, lets one type conform to it several times, with different type arguments.
- When a call can use more than one conformance, the expected type picks one: `let f: Fahrenheit = c.transform()`. Without an expected type, the call is an error.

### Generic extensions

An `extend` can have type parameters. `extend<T> Stack<T>` adds methods to every `Stack`. `extend Stack<i64>` adds methods to `Stack<i64>` only.

```sisu
class Stack<T> {
    mut items: [T]
}

extend<T> Stack<T> {
    fn size(self) -> i64 {
        self.items.len()
    }
}

extend<T: Eq> Stack<T>: Eq {
    fn eq(self, other: Stack<T>) -> bool {
        self.items == other.items
    }
}

extend Stack<i64> {
    fn total(self) -> i64 {
        self.items.sum()
    }
}

fn main() {
    let a = Stack(items: [1, 2])
    let b = Stack(items: [1, 2])
    print(a.size())                    // 2
    print(a.total())                   // 3
    print(a == b)                      // true
}
```

The rules:

- `extend<T: Eq> Stack<T>: Eq` makes `Stack<T>` conform to `Eq` only when `T` does.
- Methods with the same name on different instantiations never overlap.
- One method name in two extensions that both cover an instantiation is an error. An example is `extend<T> Stack<T>` and `extend Stack<i64>`, which both define `total`.

### Associated types

An *associated type* is a type that each conforming type chooses once. The interface declares it with `type`. A conforming type sets it with `type Name = Type`.

```sisu
interface Source {
    type Item: Display
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

fn first_item<S: Source>(s: S) -> S.Item? {
    s.next()
}

fn show_next<S: Source>(s: S) {
    if let x = s.next() {
        print(x)
    }
}

fn main() {
    let c = Countdown(left: 2)
    print(first_item(c) ?? 0)          // 2
    show_next(c)                       // 1
}
```

Inside the interface, the associated type is written `Item`. Elsewhere it is written through the type: `S.Item` for a type parameter `S: Source`.

An associated type can carry a bound, such as `type Item: Display`. Generic code can then print an `S.Item`, as `show_next` does.

Use an associated type when each type has one answer, such as the item of a `Source`. Use a type parameter, as in `Transform<T>`, when one type can conform in several ways. An interface can have both.

### Display, Eq and Ord

`Display`, `Eq` and `Ord` are interfaces of the language. `print` and interpolation use `Display`. `==` and `!=` use `Eq`. `<`, `<=`, `>` and `>=` use `Ord`.

```sisu
class Point {
    x: i64
    y: i64

    fn to_string(self) -> string {
        "({self.x}, {self.y})"
    }
}

class Money: Eq, Ord {
    cents: i64

    fn eq(self, other: Money) -> bool {
        self.cents == other.cents
    }

    fn less(self, other: Money) -> bool {
        self.cents < other.cents
    }
}

enum Level: Eq {
    Low
    High
}

fn main() {
    print(Point(x: 1, y: 2))                           // (1, 2)
    print(Money(cents: 5) == Money(cents: 5))          // true
    print(Money(cents: 3) < Money(cents: 5))           // true
    print(Level.Low == Level.High)                     // false
}
```

The rules:

- Every type that can be a type argument has a `Display`.
- A class prints its name, with its type arguments, and never its fields. A class with a method `to_string` prints the text of that method.
- A class has no `==` and no `<`. A class that conforms to `Eq` has `==`, and one that conforms to `Ord` has `<`. `a is b` compares two objects by identity.
- A value that holds a class, such as an array of objects, has `==` only when the class conforms to `Eq`.
- `f64` conforms to `Eq` and `Ord` with the rules of IEEE 754. In generic code, `nan` is not equal to `nan`, as in other code.
- An enum has `==` when all its payloads have it, and it has `<` only when it conforms to `Ord`.
- The methods that the interfaces require are `fn to_string(self) -> string` for `Display`, `fn eq(self, other: Self) -> bool` for `Eq`, and `fn less(self, other: Self) -> bool` for `Ord`. A method named `to_string` with another signature is an error.
- `Ord` requires `Eq`. A type that conforms to `Ord` but not to `Eq` is an error.
- For `Eq`, `a != b` is `!a.eq(b)`.
- A conformance that a program writes replaces the generated one. An enum that writes `to_string` prints its own text.
- `to_string()` is a method of every type except `unit` and `never`. It gives the text that `print` writes, as a `string`. When the bounds of a written `to_string` do not hold for the receiver, as in `extend<T: Eq> Box<T>` for a `Box<Node>`, the generated one answers. A call of another method whose bounds fail is an error.
- A type can name a generated interface and write no method for it: `enum Level: Eq` has no `eq`. Where nothing is generated, such as `class Node: Eq` with no `eq`, a missing method is an error.
- Only `to_string` conforms by name. A type with its own `eq` or `less` method conforms to `Eq` or `Ord` only when its declaration names the interface.
- `i64`, `u8`, `char` and `string` have `Eq` and `Ord` with no declaration. An enum has `Eq` with no declaration when all its payloads have it.

## Type aliases

A *type alias* gives a new name to a type. It is the same type as the type that it names, so no conversion is necessary.

```sisu
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

- A type alias is a top-level declaration. Its name must differ from the name of any function, class, enum, interface or constant ([E0034](../diagnostics/E0034.md)), and from a built-in type's name ([E0026](../diagnostics/E0026.md)). An alias can stand before the declaration that uses it, and after it.
- An alias can have type parameters, such as `Pairs<T>`. They take no bounds. A type writes every argument: `Pairs` alone is an error ([E0137](../diagnostics/E0137.md)).
- An alias is the type that it names, also inside type arguments: a `Stack<Grid>` is a `Stack<[[char?]]>`. An `extend` block of an alias extends the type that the alias names, so `extend Grid` is an error, as `extend [[char?]]` is ([E0155](../diagnostics/E0155.md)).
- An alias of a class or an enum builds values as the type does, and an alias of a number type converts and has its constants. An alias of an optional type has its `Some` and `None`. A use can write the type arguments, as in `Pairs<i64>(...)`, or leave them to its arguments and its context, as a generic call does. The pattern `Alias.Variant(x)` holds the arguments that the alias fixes. An alias of a type with no constructor, such as an array, cannot be called ([E0105](../diagnostics/E0105.md)).
- An alias of a type with a bound, such as `type H<T> = Heap<T>` for a `Heap<T: Ord>`, is checked at each use, on the type that the use names. An alias of an interface is an error ([E0147](../diagnostics/E0147.md)). A use of an alias that fails such a bound is that type's error, so it is reported once.
- An alias that refers to itself, directly or through another alias, is an error ([E0161](../diagnostics/E0161.md)). A class or an enum can hold itself.
- The type that an alias names is no deeper than 256 levels ([E0071](../diagnostics/E0071.md)), and one written type adds no more than 100,000 distinct types, and the whole program no more than 1,000,000 ([E0162](../diagnostics/E0162.md)).
- A message about a value that does not fit a declared type shows the alias as the declaration writes it: "expected `Grid`, found `string`". Any other message shows the type that the alias names.

```sisu
class Point {
    x: f64
    y: f64
}

enum Shape {
    Dot
    Line(f64)
}

type P = Point
type S = Shape
type Meters = f64
type Maybe = i64?

fn describe(shape: Shape) -> string {
    match shape {
        S.Dot => "dot"
        S.Line(length) => "line {length}"
    }
}

fn main() {
    let p: Point = P(x: 1.0, y: 2.0)
    let m: f64 = Meters(4) + Meters.pi
    let n: i64? = Maybe.Some(5)
    print(p.x + p.y)                   // 3.0
    print(m > 7.0)                     // true
    print(n ?? 0)                      // 5
    print(describe(S.Line(3.0)))       // line 3.0
    print(describe(S.Dot))             // dot
}
```

## Interface values

`any Shape` is a type for a value of any type that conforms to `Shape`. A value converts to it where an `any` type is expected. An object converts as itself. Any other value, such as an enum, a number or a `string`, goes into a box that holds it. A call of a method on it uses the method of the type that the value has. An interface that inherits `AnyObject` is class-only, as [Bounds](#bounds) says.

```sisu
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
    print(shapes[0].area())            // 3.141592653589793
    print(shapes[1].area())            // 6.0
    print(shapes[1])                   // Rect
}
```

The rules:

- An interface name alone is not a type. Write `T: Shape` for a type parameter, or `any Shape` for a value.
- An interface whose methods are generic, that uses `Self` besides the receiver, that has an associated type, or that has an inherited interface with `Self` in its arguments (`interface Node: Link<Self>`), cannot be `any` ([E0164](../diagnostics/E0164.md)). So `any Eq` and `any Ord` are errors. The same holds for an interface that inherits one with such a requirement.
- A value converts to `any Shape` when its type conforms to `Shape` and it is not an optional. An optional interface value has the type `(any Shape)?`, and the parentheses are required: `any Shape?` is an error ([E0165](../diagnostics/E0165.md)). A `Circle` converts to `(any Shape)?` and is wrapped. A `Circle?` converts to neither `any Shape` nor `(any Shape)?` ([E0032](../diagnostics/E0032.md)). In a generic body, a value of a type parameter `T` converts, whatever type the call gives to `T`.
- An `any` value prints through the `Display` of the type that it holds. It has no `==` and no `<`, also against a value that converts to it ([E0159](../diagnostics/E0159.md)).
- A generic function with the bound `T: Shape` accepts an `[any Shape]`.
- An object converts with no allocation. Any other value goes into one box, which allocates once. Copies of an `any` value share its box, and the box is freed with its last copy.

A value that is no object converts in the same way. A number, a `string` and a `char` conform to `Display`, and an enum can list an interface as a class does:

```sisu
interface Shape {
    fn area(self) -> f64
}

enum Slab: Shape {
    Plain
    Labeled(string)

    fn area(self) -> f64 {
        match self {
            Slab.Plain => 2.0
            Slab.Labeled(text) => 1.0 + f64(text.len())
        }
    }
}

fn main() {
    let items: [any Display] = ["hi", 'c', 5, [1, 2]]
    print(items)                       // ["hi", 'c', 5, [1, 2]]
    let slab: any Shape = Slab.Labeled("abc")
    let copy = slab
    print(slab.area())                 // 4.0
    print(copy.area())                 // 4.0
    print(Slab.Plain.area())           // 2.0
}
```
