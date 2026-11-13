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

A write needs a root that can change. The roots that can change are a `let mut`, and a `mut` parameter. The write can have any depth: a field, a field of a field, an element of an array in a field.

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

A method of a struct follows the rules of a [method of a class](classes.md#methods). It has `self` as its first parameter, and `self` is a copy of the value. A method cannot write a field of `self`. A struct method that changes its value returns the changed copy.

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

## Warnings

- A field of a struct that no live code reads builds with a warning. [W0013](../diagnostics/W0013.md) A generated `==` and a generated `print` read each field.
- A `let mut` that copies a place and is then only written to builds with a warning. The writes change the copy, and nothing reads it. [W0016](../diagnostics/W0016.md)
