# Classes

A *class* is a type whose values are references to objects. All objects of a class have the same fields and methods.

```sisu
class Counter {
    name: string
    mut hits: i64

    fn hit(self) {
        self.hits += 1
    }

    fn report(self) -> string {
        return "{self.name}: {self.hits}"
    }
}

fn main() {
    let door = Counter(name: "door", hits: 0)
    door.hit()
    door.hit()
    print(door.report())         // door: 2
}
```

An object is shared. A binding, an argument or a field that holds an object refers to that object, and does not copy it.

## Declarations

A class declaration is `class`, the name, and a body in `{` and `}`. The body holds the fields and the methods of the class, in any order.

```sisu
class Point {
    x: i64
    y: i64

    fn sum(self) -> i64 {
        return self.x + self.y
    }
}

class Empty {}

fn main() {
    print(Point(x: 3, y: 4).sum())   // 7
    let e = Empty()
}
```

The rules:

- A field is `name: Type`, or `mut name: Type` for a field that can change. It has no `let` ([E0016](../diagnostics/E0016.md)).
- Each field is on a line of its own. A `;` can separate two fields on one line, but a `,` cannot ([E0015](../diagnostics/E0015.md)).
- A class is declared at the top level. Its name follows the rules of a function name ([Functions](functions.md#declarations)).
- Two fields, two methods, or a field and a method of one class cannot have the same name ([E0034](../diagnostics/E0034.md)).
- A class cannot extend another class. Sisu has no inheritance.
- A generic class also has type parameters after its name, such as `class Stack<T>`.

A field cannot need an object of its own class, directly or through other classes, because no first object could exist ([E0027](../diagnostics/E0027.md)). An optional field, such as `next: Node?`, or an array field, such as `children: [Node]`, can be empty, so it is valid.

```sisu
class Node {
    value: i64
    next: Node                   // error[E0027]
}

fn main() {
    print(1)
}
```

## Constructors

A *constructor* `Name(field: value, ...)` makes a new object of the class `Name`. It gives a value to each field.

```sisu
class Sauna {
    name: string
    mut heat: i64
}

fn main() {
    let s = Sauna(name: "lake", heat: 80)
    print("{s.name} {s.heat}")   // lake 80
}
```

The rules:

- A constructor names each field once, with its label, in the order of the declaration. A field has no default value.
- A missing field is an error ([E0056](../diagnostics/E0056.md)). So is an argument after the last field ([E0064](../diagnostics/E0064.md)), and an argument without a label ([E0065](../diagnostics/E0065.md)).
- Fields in a different order are an error ([E0066](../diagnostics/E0066.md)). A label that is not a field of the class is an error too ([E0063](../diagnostics/E0063.md)).
- Each value must have the type of its field. For an optional field `T?`, a `T` becomes a `T?` ([Types](types.md#optionals)).
- The program calculates the arguments from left to right, which is the order of the fields ([Order of evaluation](expressions.md#order-of-evaluation)).
- Each call of a constructor makes a new object, also with the same values.

```sisu
class Point {
    x: i64
    y: i64
}

fn main() {
    let p = Point(y: 2, x: 1)    // error[E0066]
}
```

## Fields

`x.name` reads the field `name` of the object `x` ([Fields](expressions.md#fields)). A field without `mut` gets its value from the constructor and never changes. A `mut` field can change.

```sisu
class Player {
    name: string
    mut score: i64
}

fn main() {
    let p = Player(name: "Aino", score: 0)
    p.score += 10
    print(p.score)               // 10
}
```

The rules:

- An assignment to a field without `mut` is an error, also in a method ([E0001](../diagnostics/E0001.md)).
- An assignment to a `mut` field changes the object. The binding that holds the object does not need `mut` ([Assignment](statements.md#assignment)).
- The object can come from any expression, such as a parameter, an element of an array, a field of another object, or `self`.
- An assignment cannot go through `?.` ([E0022](../diagnostics/E0022.md)).
- A `let mut` binding of an object must change by an assignment to the binding itself. A change to a field of its object does not count ([W0002](../diagnostics/W0002.md)).
- To write an element of an array in a field, the field must be `mut` ([E0001](../diagnostics/E0001.md)).
- For an optional object, read a field with `?.` ([Optional chaining](operators.md#optional-chaining)).

```sisu
class Box {
    mut value: i64
}

fn fill(b: Box) {
    b.value = 7
}

fn main() {
    let boxes = [Box(value: 1), Box(value: 2)]
    fill(boxes[0])
    boxes[1].value = 9
    print(boxes[0].value + boxes[1].value)   // 16
}
```

```sisu
class Bag {
    items: [i64]
}

fn main() {
    let bag = Bag(items: [1, 2])
    bag.items[0] = 5             // error[E0001]
}
```

## Methods

A *method* is a function in the body of a class. Its first parameter is `self`, the object that the method is called on. A method call `x.name(...)` runs the method with `x` as `self` ([Method calls](expressions.md#method-calls)).

```sisu
class Account {
    owner: string
    mut balance: i64

    fn deposit(self, amount: i64) {
        self.balance += amount
    }

    fn show(self) -> string {
        return "{self.owner}: {self.balance}"
    }
}

fn main() {
    let a = Account(owner: "Eero", balance: 5)
    a.deposit(10)
    print(a.show())              // Eero: 15
}
```

The rules:

- `self` is the first parameter of each method ([E0017](../diagnostics/E0017.md), [E0020](../diagnostics/E0020.md)). It is `self`, never `mut self` ([E0018](../diagnostics/E0018.md)).
- `self` is valid only in a method ([E0041](../diagnostics/E0041.md)). An assignment to `self` is an error ([E0073](../diagnostics/E0073.md)).
- A method can write the `mut` fields of `self`. The change shows through every reference to the object.
- In a method, use `self` to reach the fields and the other methods: `self.x` and `self.m()`. A bare `x` or `m()` is an error ([E0033](../diagnostics/E0033.md), [E0052](../diagnostics/E0052.md)).
- A method can call a method that is declared below it, and it can call itself.
- A method can take the name of a function, such as `len` or `print`. A call `x.print()` finds the method, and `print(...)` finds the function.
- A method is not a function. A call `name(x)` does not find the method `name` ([E0052](../diagnostics/E0052.md)). A call on the class, such as `Account.show()`, is an error ([E0105](../diagnostics/E0105.md)).
- For an optional object, call a method with `?.` ([Optional chaining](operators.md#optional-chaining)).

A method can return `self`, so that calls can follow one another.

```sisu
class Path {
    mut steps: i64

    fn step(self) -> Path {
        self.steps += 1
        return self
    }
}

fn main() {
    let p = Path(steps: 0)
    print(p.step().step().steps)     // 2
}
```

```sisu
class Counter {
    mut n: i64

    fn reset(self) {
        n = 0                    // error[E0033]
    }
}

fn main() {
    Counter(n: 1).reset()
}
```

## Identity

Each object has an identity. `a is b` is true when `a` and `b` refer to the same object ([`is`](operators.md#is)). A copy of a reference refers to the same object, and a constructor makes a new one.

```sisu
class Sauna {
    mut heat: i64
}

fn main() {
    let a = Sauna(heat: 80)
    let b = a
    let c = Sauna(heat: 80)
    b.heat = 90
    print(a.heat)                // 90
    print(a is b)                // true
    print(a is c)                // false
}
```

An object stays in memory while a reference to it exists.
