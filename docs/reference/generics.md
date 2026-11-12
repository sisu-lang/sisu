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

**Rules**

- A type parameter can be the type of a parameter, a field, a payload, a return value or a binding.
- A method of a generic class or enum can use the type parameters of its type.
- A method can add type parameters of its own, such as `C` in `with_second`.
- Two type parameters of one declaration cannot have the same name. A method cannot reuse a name from its type. [E0034](../diagnostics/E0034.md)
- A type parameter cannot take a reserved name, such as `i64` or `Option`. [E0026](../diagnostics/E0026.md)
- `main` has no type parameters. [E0029](../diagnostics/E0029.md)

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

**Rules**

- A type gives one type argument for each type parameter. `Box` alone and `Box<i64, i64>` are errors. [E0137](../diagnostics/E0137.md)
- A class, an enum or a function that is not generic takes no type arguments. [E0137](../diagnostics/E0137.md)
- The type arguments of a variant go on its enum: `Tree<i64>.Node(...)`, not `Tree.Node<i64>(...)`. [E0137](../diagnostics/E0137.md)
- A type argument can be an optional, such as `i64?` in `Box<i64?>`.
- A type argument cannot be `unit` or `never`. `unit` is the type of a call that gives no value, such as `print(1)`. [E0138](../diagnostics/E0138.md)
- The section [unit](types.md#unit) is in development. It removes the rule for `unit`.

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

**Where a type argument comes from**

- From the arguments of the call: `first([1, 2])` gives `i64` for `T`.
- From the expected type: a `let` with a type, a parameter or a return type.
- From the call, when you write it: `first<string>([])` or `Stack<i64>(items: [])`. A method call can write them too.
- A later use of the result gives no type.
- In an array literal, a generic call takes its type from the other elements. In an `if` or a `match`, it takes it from the other branches. With an `x: u8`, `[x, id(None)]` is a `[u8?]`.
- A `None` or a `[]` alone has no type. When nothing else fixes a type parameter, the call is an error. Annotate the result, or write the type arguments. [E0050](../diagnostics/E0050.md)

**Calls inside calls**

- An inner call takes its missing types from the outer call. In `let r: Result<i64, string> = id(Ok(1))`, `r` gives `T` for `id`, and `T` gives `E` for `Ok`.
- An inner call also takes the parts of the parameter type that are already known. For `unwrap<T>(r: Result<T, string>)`, `unwrap(Ok(1))` gets `string` for `E` from the parameter.
- An inner call waits when only a later argument can fix one of its type parameters. Take `fn mk2<X>() -> Two<X, bool>`, `fn mk<E>() -> Result<i64, E>` and `g<T>(c: Two<T, bool>, d: Result<T, string>)`. In `g(mk2(), mk())`, `mk()` fixes `T` as `i64`, and then `mk2()` gives `Two<i64, bool>`.
- A call that waits is checked when its last open type parameter is fixed. If one stays open, it is checked after the other arguments.
- A call of a function, a construction, `Ok`, `Err` and a variant can wait. A method call does not wait.
- A call under `Some(...)`, an array literal or another call does not wait. There, the arguments are checked in order.

**Integer literals**

- An integer literal argument takes the type that the expected type gives. This is also true under `dbg`, a block, an `if`, a `match` or another generic call: `let d: u8 = id({ 250 })` is valid.
- A concrete argument fixes a type parameter before a literal does. In `pick(1, x)` with an `x: u8`, `T` is `u8`.
- A `loop` argument takes the type that an expected type gives, when that type is not optional and not `any`: `let r: Result<u8, i64> = Ok(loop { break 1 })` is valid. An earlier argument, a `mut` place, the receiver or a written type argument can also give it. With a `v: u8`, `pick(v, loop { break 1 })` is valid, and `pick(loop { break 1 }, v)` is an error. With none of these, the first `break` value gives the type: `id(loop { break 1 })` is an `i64`, and `let x: u8? = id(loop { break 1 })` is an error. Under a generic call, the left side of `??`, the other side of `==`, another branch and an earlier `break` value give the `loop` no type: with an `o: u8?`, `o ?? id(loop { break 1 })` is an error.

**Optionals**

The examples use `fn pick<T>(a: T, b: T) -> T`, `fn h<T>(a: T, b: T?) -> T` and `fn opt<T>(x: T) -> T?`.

- An argument that makes an optional fixes `T` before a plain literal. `pick(1, Some(2))` is an `i64?`.
- An argument that holds an optional also comes first. `pick([1], [Some(2)])` is a `[i64?]`.
- So does a call that wraps an optional. For `fn wrap<T>(x: T) -> [T]`, `pick([1], wrap(Some(2)))` is a `[i64?]`. A call wraps an optional when its parameter and its return type name one type parameter.
- An optional at a `T?` place fills that `?`, so the call does not hold an optional. `pick(h(1, Some(2)), Some(3))` is an `i64?`, and `let v: i64?? = h(1, Some(2))` is valid.
- A call that waits never makes the outer call hold an optional. For `fn okv<T>(r: Result<T, string>) -> T`, `pick(1, okv(Ok(Some(2))))` is an error: `1` fixes `T` as `i64` first. Write `pick(Some(1), ...)`, or give the result a type. [E0032](../diagnostics/E0032.md)
- A call that returns `T` can bind `T` to the whole optional context. It does so when an argument at a `T` or `[T]` place makes or holds an optional. For `first<T>(xs: [T]) -> T`, `let v: i64?? = first([None, Some(1)])` binds `T` to `i64??`, so `v` is the outer `None`.
- A call that returns `T?` does the same when the context has no more optionals than the call makes. `let w: i64?? = opt(Some(2))` binds `T` to `i64?`.
- When the context has more optionals, its payload meets `T?`. `let w: i64??? = opt(Some(2))` also binds `T` to `i64?`.
- `Some(Some(2))` at a `T?` place fills the `?` and leaves one optional, so `T` is an optional. `let v: i64? = h(1, Some(Some(2)))` is valid, and a context `i64??` also binds `T` to `i64?`.
- Inference counts only the optionals that the arguments show. A later argument with more optionals does not widen `T`, so the order can change the result. `pick(Some(Some(1)), Some(2))` is an `i64??`, but `pick(Some(2), Some(Some(1)))` is an error. [E0032](../diagnostics/E0032.md)
- A `None` at a `T?` place shows no optional. `let w: i64?? = h(1, None)` is an error: expected `i64??`, found `i64`.
- A call that returns `T?` does the same with more optionals. For `fn opt2<T>(x: T, _y: T) -> T?`, `let w: i64??? = opt2(Some(2), Some(Some(3)))` is an error: expected `i64`, found `i64?`.
- In each of these cases, write the type argument. `pick<i64??>(Some(2), Some(Some(1)))`, `h<i64?>(1, None)` and `opt2<i64??>(Some(2), Some(Some(3)))` are valid.

**Where the error is**

- When an argument does not agree with the expected type, the error is at the argument. `let r: Result<i64, string> = Ok("x")` reports `"x"`: expected `i64`, found `string`. [E0032](../diagnostics/E0032.md)
- With written type arguments, as in `id<i64>(n)`, the error is at the call.
- An argument also meets the parts that an earlier argument fixed. For `f<A, B>(a: A, p: Two<A, B>)`, `f("s", Two(a: 1, b: true))` reports `1`: expected `string`, found `i64`. This is also true inside `Some(...)`, an array literal or another generic call.
- When the argument has the wrong shape, the error is at the whole argument. While the parameter type names a type parameter that the call has not fixed, the error gives the shape in words. For `take<T>(xs: [T])`, `take(5)` reports `5`: expected an array, found `i64`. For a `Box<U>` parameter, it reports expected a `Box`.
- An optional parameter type keeps the type in the message. For `fn topt<T>(xs: [T]?)`, `topt(5)` reports expected `[T]?`, found `i64`.
- A call that waits also keeps the type. `take(Ok(1))` reports `Ok(1)`: expected `[T]`, found `Result<i64, E>`.
- When a call that waited does not agree with the argument that fixed its type, the error is at the call that waited. For `fn mks<E>() -> Result<string, E>`, `fn mkr<X>() -> Two<i64, Two<X, bool>>` and `gn<T>(c: Two<T, Two<T, bool>>, d: Result<T, string>)`, `gn(mkr(), mks())` reports `mkr()`: expected `Two<string, Two<string, bool>>`, found `Two<i64, Two<string, bool>>`.
- A result that cannot fit the expected type gives the error of a call that is not generic, also when a type parameter stays open. `let y: i64 = mk2()` reports expected `i64`, found `Two<X, bool>`. [E0032](../diagnostics/E0032.md)
- `1 == mk2()` reports cannot compare `i64` with `Two<X, bool>`. [E0047](../diagnostics/E0047.md)

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

**What a `T` can always do**

- Pass it, return it and store it, also in an optional or an array.
- Print it. `print(x)`, `"{x}"` and `x.to_string()` need no bound, because every type has a `Display` ([Display, Eq and Ord](#display-eq-and-ord)).
- Compare a `T?` with `None`: `x == None` and `x != None` are valid.

**What needs a bound**

When the compiler can tell which bound fixes the error, the help names it ([Bounds](#bounds)).

- `==` and `!=` need `T: Eq`. Two `T?` values need it too. [E0139](../diagnostics/E0139.md)
- `<`, `<=`, `>` and `>=` need `T: Ord`. [E0139](../diagnostics/E0139.md)
- `contains` on a `[T]` uses `==`, so it needs `T: Eq`. [E0154](../diagnostics/E0154.md)
- `is` needs `T: AnyObject`, because `T` can be a type that is not a class. [E0046](../diagnostics/E0046.md)
- A method other than `to_string()` needs a bound that gives it. When one interface declares the method, the help names it. [E0063](../diagnostics/E0063.md)

**What a `T` cannot do**

- Be an operand of `+` or of any other operator that computes, whatever the bounds are. [E0139](../diagnostics/E0139.md)
- Be an operand of `!`, `&&` or `||`. They take only a `bool`. [E0032](../diagnostics/E0032.md)
- Have a field. [E0063](../diagnostics/E0063.md)
- Fill an array with `[x; n]`, because `T` can be a class ([Filled arrays](arrays.md#filled-arrays)). `[None; n]` is valid. [E0067](../diagnostics/E0067.md)
- Be the subject of a `match`. In a payload of the type `T`, a pattern is a name or `_` ([Variant patterns](enums.md#variants)). [E0124](../diagnostics/E0124.md)
- Meet a value of another type in a comparison (`==`, `!=`, `<`, `<=`, `>` or `>=`), whatever the bounds are. `a == 5`, `a < 5` and `a < b` with a `U` cannot be compared. [E0047](../diagnostics/E0047.md)

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

**What a bound gives**

- In the body, a `T` can use the methods of its bounds and the operators that they map to.
- `Eq` gives `==` and `!=`. `Ord` gives `<`, `<=`, `>` and `>=`. `a.eq(b)` and `a.less(b)` work in the same way.
- An array or an optional of `T` has `==` when `T` has `Eq`. An enum has `==` only when every payload type has it.

**Where the error is**

- A use that the bounds do not give is an error where the body is written. The help suggests the missing bound.
- A type argument that does not meet a bound is an error at the call. The call reports it once, and the arguments after it add no error for that type.

**`AnyObject`**

- Every class conforms to `AnyObject`. The only other type that does is an `any` value of a class-only interface.
- `T: AnyObject` lets the body use `is` on a `T`. It also lets a class declare a [weak field](values.md#weak-fields) of type `T?`.
- An associated type with the bound `AnyObject`, as in `type Item: AnyObject`, allows `is` on a `T.Item`.
- An interface that inherits `AnyObject` is class-only. Its `any` values allow `is`, and a weak field can hold one.

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

**Rules**

- A type that names an interface has each method of it, with the same signature. A missing method is an error.
- A class or an enum lists its interfaces after `:`, with `,` between them: `class Tree: Display, Eq`. A bound joins interfaces with `&`: `T: Shape & Ord`.
- A type conforms to an interface with the same type arguments at most once.
- `Self` in an interface is the type that conforms to it, as in `fn eq(self, other: Self) -> bool`.
- An interface can require other interfaces: `interface Shape: Display`. A type that conforms to `Shape` conforms to `Display` too.
- A method in an interface can have type parameters of its own. Such an interface can be a bound, but not an `any` type.

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

**Rules**

- `extend Type { ... }` adds methods that are not part of an interface.
- A program cannot extend a built-in type, such as `string` or `[T]`.
- One type can conform to a generic interface, such as `Transform<T>`, several times, with different type arguments.
- When a call can use more than one conformance, the expected type picks the one that returns it: `let f: Fahrenheit = c.transform()`. With no expected type, or when two conformances return that type, the call is an error.

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

**Rules**

- `extend<T: Eq> Stack<T>: Eq` makes `Stack<T>` conform to `Eq` only when `T` does.
- Methods with the same name on different instantiations do not overlap.
- Two plain extensions, with no interface, that both cover an instantiation cannot define the same method. `extend<T> Stack<T>` and `extend Stack<i64>` cannot both define `total`.

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

class Money: Ord {
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

**`Display`**

- Every type that can be a type argument has a `Display`.
- A class prints its name and its type arguments, never its fields. A class with a `to_string` method prints the text of that method.
- An enum that writes `to_string` prints its own text.
- `to_string()` is a method of every type except `unit` and `never`. It gives the text that `print` writes, as a `string`.
- A method named `to_string` must be `fn to_string(self) -> string`.
- When the bounds of a written `to_string` do not hold for the receiver, the generated one answers. An example is `extend<T: Eq> Box<T>` and a `Box<Node>`. For any other method, bounds that fail are an error.

**`Eq` and `Ord`**

- `Eq` requires `fn eq(self, other: Self) -> bool`, and `a != b` is `!a.eq(b)`.
- `Ord` requires `fn less(self, other: Self) -> bool`.
- `Ord` requires `Eq`, so a type that names `Ord` conforms to `Eq` too. It needs an `eq` method when no `Eq` is generated for it. [E0152](../diagnostics/E0152.md)
- A class has no `==` and no `<` until it names `Eq` or `Ord`. `a is b` compares two objects by identity.
- A value that holds a class, such as an array of objects, has `==` only when the class conforms to `Eq`.
- An enum has `==` when all its payloads have it. It has `<` only when it names `Ord`.
- `i64`, `u8`, `char` and `string` have `Eq` and `Ord` with no declaration.
- `f64` has `Eq` and `Ord` with the rules of IEEE 754. `nan` is not equal to `nan`, also in generic code.

**Writing a conformance**

- A conformance that a program writes replaces the generated one.
- A type can name a generated interface and write no method for it: `enum Level: Eq` has no `eq`.
- Where nothing is generated, a missing method is an error: `class Node: Eq` needs an `eq`.
- Only `to_string` conforms by name. A type with its own `eq` or `less` conforms to `Eq` or `Ord` only when its declaration names the interface.

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

**Declaring an alias**

- A type alias is a top-level declaration. It can come before or after a declaration that uses it.
- Its name must differ from the name of each function, class, enum, interface and constant. [E0034](../diagnostics/E0034.md)
- Its name cannot be the name of a built-in type. [E0026](../diagnostics/E0026.md)
- An alias can have type parameters, such as `Pairs<T>`. They take no bounds.
- An alias cannot refer to itself, directly or through another alias. A class or an enum can hold itself. [E0161](../diagnostics/E0161.md)
- An alias cannot name an interface. [E0147](../diagnostics/E0147.md)

**Using an alias**

- An alias is the type that it names, also inside type arguments: a `Stack<Grid>` is a `Stack<[[char?]]>`.
- A use gives every type argument: `Pairs` alone is an error. [E0137](../diagnostics/E0137.md)
- An alias of a class or an enum makes values as the type does. For `type B<T> = Box<T>`, `B<i64>(value: 1)` writes the type arguments. Without them, the arguments and the context give them, as for a generic call.
- An alias of a number type converts values and has the constants of the type, as `Meters(4)` and `Meters.pi` show.
- An alias of an optional type has its `Some` and `None`.
- The pattern `Alias.Variant(x)` holds the type arguments that the alias fixes.
- An alias of a type with no constructor, such as an array, cannot be called. [E0105](../diagnostics/E0105.md)
- `extend` on an alias extends the type that the alias names. So `extend Grid` is an error, as `extend [[char?]]` is. [E0155](../diagnostics/E0155.md)
- An alias of a type with a bound, such as `type H<T> = Heap<T>` for a `Heap<T: Ord>`, is checked at each use. A bound that fails is reported once, as an error of that type.

**Limits and messages**

- The type that an alias names is at most 256 levels deep. [E0071](../diagnostics/E0071.md)
- One written type adds at most 100,000 distinct types, and the whole program at most 1,000,000. [E0162](../diagnostics/E0162.md)
- A message about a value that does not fit a declared type shows the alias: "expected `Grid`, found `string`". Other messages show the type that the alias names.

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

**Rules**

- An interface name alone is not a type. Write `T: Shape` for a type parameter, or `any Shape` for a value.
- A value converts to `any Shape` when its type conforms to `Shape` and it is not an optional.
- In a generic body, a value of type `T` converts, whatever type the call gives to `T`.
- A generic function with the bound `T: Shape` accepts an `[any Shape]`.
- An `any` value prints through the `Display` of the value that it holds.
- An `any` value has no `==` and no `<`, also against a value that converts to it. [E0159](../diagnostics/E0159.md)
- An object converts with no allocation. Any other value goes into one box, which allocates once. Copies of the `any` value share the box, and the last copy frees it.

**Optional interface values**

- An optional interface value has the type `(any Shape)?`. The parentheses are necessary: `any Shape?` is an error. [E0165](../diagnostics/E0165.md)
- A `Circle` converts to `(any Shape)?`, and it is wrapped.
- A `Circle?` converts to neither `any Shape` nor `(any Shape)?`. [E0032](../diagnostics/E0032.md)

**Interfaces that cannot be `any`**

- An interface cannot be `any` when it has a generic method, an associated type, or `Self` in a place other than the receiver. So `any Eq` and `any Ord` are errors. [E0164](../diagnostics/E0164.md)
- The same is true when it inherits an interface with `Self` in its arguments, as in `interface Node: Link<Self>`. [E0164](../diagnostics/E0164.md)
- The same is true when it inherits an interface that has one of these requirements. [E0164](../diagnostics/E0164.md)

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
