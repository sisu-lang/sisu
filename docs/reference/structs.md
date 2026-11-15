# Structs

A *struct* is a type whose values hold fields. A struct value is a copy: a binding, an argument, a field and an element each hold a copy of the value.

```sisu
struct Point {
    x: i64
    y: i64

    fn dist2(self) -> i64 {
        self.x * self.x + self.y * self.y
    }
}

fn main() {
    let mut p = Point(x: 1, y: 2)
    print(p)                        // Point(x: 1, y: 2)
    p.x = 4
    print(p.dist2())                // 20
    print(p == Point(x: 4, y: 2))   // true
}
```

A [class](classes.md) is a reference to a shared object. A struct has no identity: two structs with the same fields are the same value.

## Declarations

A struct declaration is `struct`, the name, and a body in `{` and `}`. The body holds the fields and the methods of the struct, as the body of a [class](classes.md#declarations) does.

```sisu
struct Pair {
    first: i64
    second: i64

    fn sum(self) -> i64 {
        return self.first + self.second
    }
}

struct Rock {}

fn main() {
    print(Pair(first: 3, second: 4).sum())   // 7
    print(Rock())                             // Rock()
}
```

**Fields**

- A field is `name: Type`. A field of a struct has no `mut`, because the root of a write decides what can change (see below). [E0175](../diagnostics/E0175.md)
- A line break separates two fields. A `;` can separate them too. `struct A { v: i64 }` is valid.
- A `,` between two fields is an error. [E0015](../diagnostics/E0015.md)
- Two fields, two methods, or a field and a method of one struct cannot have the same name. [E0034](../diagnostics/E0034.md)
- A struct can have no fields. `Rock()` builds the struct `Rock`.

```sisu
struct Cell {
    mut n: i64         // error[E0175]
}

fn main() {
    print(Cell(n: 1).n)
}
```

**The struct**

- A struct is declared at the top level. Its name follows the rules of a function name ([Functions](functions.md#declarations)).
- A struct can have type parameters after its name, such as `struct Pair<T>`. It can list the interfaces that it conforms to after a `:`, and a type parameter can have a bound ([Generics](generics.md)). These work as they do for an [enum](enums.md#declarations).
- A struct is not a pattern. `match`, `if let` and `while let` do not take a struct apart. [E0124](../diagnostics/E0124.md) [E0051](../diagnostics/E0051.md)

## Constructors

A constructor is the name of the struct and a list of labeled values in `(` and `)`. It has every label, in the order of the declaration.

```sisu
struct Person {
    name: string
    age: i64
}

fn main() {
    let ilse = Person(name: "Ilse", age: 30)
    print(ilse.name)         // Ilse
    print(ilse.age)          // 30
}
```

- The rules are those of a [class constructor](classes.md#constructors). [E0056](../diagnostics/E0056.md) [E0064](../diagnostics/E0064.md) [E0065](../diagnostics/E0065.md) [E0066](../diagnostics/E0066.md)

## Copies

Binding, passing, storing and returning a struct is a copy. The copy and the original do not change together.

```sisu
struct Team {
    size: i64
    names: [string]
}

fn main() {
    let a = Team(size: 1, names: ["Ilse"])
    let mut b = a
    b.size += 1
    b.names.push("Aino")
    print(a.size)            // 1
    print(b.size)            // 2
    print(a.names.len())     // 1
    print(b.names.len())     // 2
}
```

An array in a field is shared until the first write, as every array is ([Arrays](arrays.md#when-an-array-is-copied)). A write to `b.names` copies the array first, so `a.names` keeps its elements.

A class object in a field stays shared. A copy of the struct holds the same object, and a write to a `mut` field of the object shows in both.

```sisu
class Counter {
    mut hits: i64
}

struct Door {
    counter: Counter
    open: bool
}

fn main() {
    let a = Door(counter: Counter(hits: 0), open: false)
    let b = a
    b.counter.hits += 1
    print(a.counter.hits)    // 1
    print(a.open == b.open)  // true
}
```

## What can change

A write needs a root that can change. The roots that can change are a `let mut`, a `mut` parameter and `mut self`. The write can have any depth: a field, a field of a field, an element of an array in a field.

```sisu
struct Person {
    name: string
}

struct Team {
    leader: Person
    size: i64
}

fn main() {
    let t = Team(leader: Person(name: "Ilse"), size: 1)
    let mut u = t
    u.leader.name = "Aino"
    print(t.leader.name)     // Ilse
    print(u.leader.name)     // Aino
    print(t.size)            // 1
}
```

These are never roots that can change: a `let`, a plain parameter and a plain `self`. A write through one of them is an error. [E0001](../diagnostics/E0001.md)

```sisu
struct Person {
    name: string
}

struct Team {
    leader: Person
}

fn main() {
    let t = Team(leader: Person(name: "Ilse"))
    t.leader.name = "Aino"   // error[E0001]
}
```

- A `mut` argument is a place with a root that can change, as a write is. On any other place, it is an error. [E0169](../diagnostics/E0169.md)
- The receiver of a call of a method that takes `mut self` is a place with a root that can change, as a `mut` argument is. [E0001](../diagnostics/E0001.md)
- A write that stays inside a value that is only a temporary is an error, as `g()[0] = 3` is: nothing could read the change. [E0021](../diagnostics/E0021.md)
- A write whose path crosses a class object keeps the rule of the class: the `mut` field of the object decides.
- A write through a struct goes into the value that the struct has when the write occurs. So when an index replaces the struct, the write goes into the new value. A write through a class goes into the object that the path found first ([Assignment](statements.md#assignment)).

```sisu
struct Cell {
    n: i64
}

fn bump(mut c: Cell) {
    c.n += 1
}

fn main() {
    let mut a = Cell(n: 1)
    bump(mut a)
    print(a.n)               // 2
}
```

## Equality and text

`==` and `!=` work on a struct when each of its fields has them. An `any` field has no `==`. [E0159](../diagnostics/E0159.md) `print`, an interpolation and `to_string` work on a struct too. The text shows each field with its label.

```sisu
struct Point {
    x: i64
    y: i64
}

fn main() {
    let a = Point(x: 1, y: 2)
    print(a)                          // Point(x: 1, y: 2)
    print(a == Point(x: 1, y: 2))     // true
    print(a != Point(x: 1, y: 3))     // true
    print("{a}")                      // Point(x: 1, y: 2)
}
```

A struct with no fields prints as `Rock()`. A written `Eq` or `Display` replaces the generated one ([Interfaces](generics.md#interfaces)).

```sisu
struct Pair {
    a: i64
    b: i64
}

extend Pair: Eq {
    fn eq(self, other: Pair) -> bool {
        self.a == other.a
    }
}

fn main() {
    let p = Pair(a: 1, b: 2)
    print(p == Pair(a: 1, b: 9))   // true
    print(p.b)                     // 2
}
```

## Layout

A struct holds its fields inline, with no count word. An array of structs holds its elements inline.

A struct over 256 bytes stays in memory, and a function takes it by address. A struct, or an enum payload, of 4 GiB or more is an error. [E0160](../diagnostics/E0160.md)

### Links

A field can name the struct itself, through an optional. The field closes a cycle, so it holds a box: a value of the struct is a small value, and the chain of boxes is as long as the program builds it.

```sisu
struct Node {
    value: i64
    next: Node?
}

fn main() {
    let list = Node(value: 1, next: Node(value: 2, next: None))
    print(list)                       // Node(value: 1, next: Node(value: 2, next: None))
}
```

A struct that no value can be built of is an error: `struct S { s: S }`, and two structs that need each other. A field of type `S?` fixes it, and so does an enum with a variant that holds no `S`. [E0027](../diagnostics/E0027.md)

```sisu
struct Bad {           // error[E0027]
    inner: Bad
}

fn main() {}
```

`struct Tree { kids: [Tree] }` needs no box. An array is a counted block already.

## Methods

A method of a struct follows the rules of a [method of a class](classes.md#methods). It has `self` as its first parameter. With plain `self`, the method reads a copy of the value. A write to a part of `self`, or an assignment to `self`, is an error. [E0001](../diagnostics/E0001.md) [E0073](../diagnostics/E0073.md) The help offers `mut self`, or you can return the changed copy.

```sisu
struct Counter {
    n: i64

    fn next(self) -> Counter {
        Counter(n: self.n + 1)
    }
}

fn main() {
    let mut c = Counter(n: 0)
    c = c.next()
    c = c.next()
    print(c.n)               // 2
}
```

`extend` adds methods to a struct. An `extend` block can also conform the struct to an interface, as it does for a class or an enum ([Modules](modules.md), [Generics](generics.md#interfaces)).

### mut self

A method can take `mut self`, in the declaration of the struct and in `extend`. Then `self` is a `mut` parameter. A write to `self` or to a part of it changes the place that the method was called on. So does `self = Point(x: 0, y: 0)`.

The call has no mark: `p.move_by(3, 4)`.

```sisu
struct Point {
    x: i64
    y: i64

    fn move_by(mut self, dx: i64, dy: i64) {
        self.x += dx
        self.y += dy
    }
}

struct Ship {
    at: Point
    crew: [Point]
}

fn main() {
    let mut p = Point(x: 1, y: 2)
    let before = p
    p.move_by(3, 4)
    print(p)                 // Point(x: 4, y: 6)
    print(before)            // Point(x: 1, y: 2)
    let mut ship = Ship(at: p, crew: [p, p])
    ship.at.move_by(1, 1)
    ship.crew[1].move_by(2, 2)
    print(ship.at)           // Point(x: 5, y: 7)
    print(ship.crew)         // [Point(x: 4, y: 6), Point(x: 6, y: 8)]
}
```

**The receiver**

- The receiver is a place with a root that can change. A local, a field, an element, an element of a field and a field of `self` in a `mut self` method are all places. Then the call changes that place.
- A `let`, a plain parameter and a plain `self` have no root that can change. A call on one is an error, with the help that a write has. [E0001](../diagnostics/E0001.md)
- A value that is no place is a temporary, such as the result of a call. A call on a temporary, or on a part of one, is an error, because nothing could read the change. [E0021](../diagnostics/E0021.md)
- A call on a place that crosses a class object keeps the rule of the class: the `mut` field of the object decides.

```sisu
struct Point {
    x: i64
    y: i64

    fn move_by(mut self, dx: i64, dy: i64) {
        self.x += dx
        self.y += dy
    }
}

fn origin() -> Point {
    Point(x: 0, y: 0)
}

fn main() {
    let p = Point(x: 1, y: 2)
    p.move_by(1, 1)          // error[E0001]
    origin().move_by(1, 1)   // error[E0021]
}
```

**The order**

- The receiver's own operands run first, once, from left to right. Then the arguments run from left to right. Then the receiver's place is read and goes to the call, as a `mut` argument does. So an argument that reads the root sees the place as it was.
- The receiver is the first `mut` slot of the call. So `p.change(mut p)` is an error, as a second `mut` argument of one place is. [E0171](../diagnostics/E0171.md)
- A call on a field or an element copies the value in and out, as a `mut` argument on such a place does. The cost grows with the size of the value. A method that pushes onto an array field costs time in proportion to the array, for each call. A later change of the compiler will remove this cost.

**Until later work**

- A call through `?.` on a struct value is an error for now. `o?.move_by(1, 1)` on a `Point?` is not valid. Replace the whole value instead. [E0062](../diagnostics/E0062.md)
- An interface with a requirement that takes `mut self` cannot be `any` for now. Use a type parameter. [E0164](../diagnostics/E0164.md)

An interface can require `mut self`. A method with plain `self` meets it, because a read is the smaller demand. A method with `mut self` does not meet a requirement with plain `self`. [E0152](../diagnostics/E0152.md) The [interfaces](generics.md#interfaces) page has an example.

A `mut self` method that never changes `self` builds with a warning, because its `mut` changes nothing. [W0002](../diagnostics/W0002.md) A call of a `mut self` method counts as a change of its root, as a `mut` argument does.

## Warnings

- A field of a struct that no live code reads builds with a warning. [W0013](../diagnostics/W0013.md) A generated `==` and a generated `print` read each field.
- A `let mut` that copies a place and is then only written to builds with a warning. The writes change the copy, and nothing reads it. [W0016](../diagnostics/W0016.md)
