# Closures and iterators

A *closure* is a function without a name. A program writes it where it needs it, and the closure can use the names around it.

```sisu pending
fn apply_twice(f: (i64) -> i64, x: i64) -> i64 {
    f(f(x))
}

fn main() {
    let step = 3
    print(apply_twice(x => x + step, 1))   // 7
}
```

A function becomes a value that a program can pass, store and return. An *iterator* produces the elements of a sequence one at a time, and `for` loops over it.

## Function types

A *function type* is written `(T1, T2) -> R`. It lists the types of the parameters and the type of the result. `() -> R` takes nothing. A function that returns no value has the result type `unit`.

```sisu pending
fn add(a: i64, b: i64) -> i64 {
    a + b
}

fn adder(n: i64) -> (i64) -> i64 {
    x => x + n
}

fn main() {
    let ops: [(i64, i64) -> i64] = [add, (a, b) => a * b]
    let plus = ops[0]
    let times = ops[1]
    print(plus(2, 3))                  // 5
    print(times(2, 3))                 // 6
    let add5 = adder(5)
    print(add5(1))                     // 6
}
```

The rules:

- A named function is a value of its function type. `add` has the type `(i64, i64) -> i64`.
- A function type goes anywhere a type goes: in a parameter, a return type, a local, a field, a payload, an array, an optional and a type argument.
- `->` groups to the right. `(i64) -> (i64) -> i64` takes an `i64` and returns a function. An optional function type needs parentheses: `(() -> unit)?`.
- A function type has no labels. A named function takes its arguments by position when a call goes through a value.
- `unit` is a type name that a program can write. `-> unit` on a function declaration means the same as no return type.
- Function types match exactly. A `(i64) -> i64` does not convert to a `(i64) -> i64?`. A closure fits, because `sisu` checks its body against the expected type.
- For a named function that does not fit, wrap it in a closure: `x => f(x)`.
- A generic named function used as a value takes its type arguments from the expected type.
- A function value has no `==`. `f == g` is an error.

```sisu pending
class Button {
    mut on_click: (() -> unit)?
}

enum Command {
    Run(() -> unit)
    Quit
}

fn hello() {
    print("hello")
}

fn main() {
    let b = Button(on_click: hello)
    if let f = b.on_click {
        f()                            // hello
    }
    let c = Command.Run(hello)
    if let Command.Run(f) = c {
        f()                            // hello
    }
}
```

## Calling a function value

`f(x)` calls the function value that `f` holds. `f` can be a local, a parameter or a captured name. `b.on_click()` calls a field of a function type.

```sisu pending
fn twice(f: () -> unit) {
    f()
    f()
}

class Button {
    on_click: () -> unit
}

fn main() {
    let say = () => print("hi")
    twice(say)                         // hi
    let b = Button(on_click: say)
    b.on_click()                       // hi
}
```

The rules:

- The arguments of the call must fit the parameter types of the function type, in order and by position.
- A function inside an optional is called after `if let`: `if let f = b.on_click { f() }`.

## Closures

A closure is `parameters => body`. A closure is a value of a function type, so it goes anywhere a function type goes.

| Form | Meaning |
| --- | --- |
| `x => x + 1` | One parameter, with no parentheses |
| `(a, b) => a + b` | Several parameters |
| `() => print("hi")` | No parameters |
| `(x: i64) => x * 2` | A parameter with a type |
| `_ => 0` | A parameter that the closure does not use |
| `x => { ... }` | A block as the body. Its last expression is the value |

```sisu pending
fn apply(f: (i64) -> i64, x: i64) -> i64 {
    f(x)
}

fn main() {
    print(apply(x => x + 1, 1))        // 2
    print(apply(_ => 0, 1))            // 0
    print(apply(x => {
        let y = x * 2
        y + 1
    }, 5))                             // 11
    let square = (x: i64) => x * x
    print(square(4))                   // 16
}
```

The rules:

- The parameter types come from the expected type. A closure with no expected function type must write the type of each parameter. So `let f = x => x + 1` is an error, and `let f = (x: i64) => x + 1` is valid.
- The return type always comes from the body. A closure has no syntax for it. Where an expected type exists, `sisu` checks the body against its return type.
- `_` as a parameter name binds nothing. A parameter list can have it more than once.
- The body is one expression, one assignment or compound assignment, or a block. An assignment has the type `unit`, so `() => c.count += 1` is valid.
- A `return` in a block body returns from the closure. A `break` or `continue` cannot leave the closure for a loop around it.
- A closure whose expected result type is `unit` gives no value. When its body has a value, the program builds with the warning for an unused value ([W0004](../diagnostics/W0004.md)).
- A closure argument of a generic call is checked after the other arguments. Its body can fix a type argument that nothing else fixed. In `fold(0, (acc, x) => acc + x)`, the `0` fixes the type of `acc`.

## Captures

A closure uses the names around it: each local, parameter and `self` that its body names. It captures them by value. The closure gets a copy when the program creates the closure.

```sisu pending
class Counter {
    mut count: i64
}

fn main() {
    let mut n = 1
    let show = () => n
    n = 5
    print(show())                      // 1
    print(n)                           // 5
    let c = Counter(count: 0)
    let bump = () => c.count += 1
    bump()
    bump()
    print(c.count)                     // 2
}
```

The rules:

- A closure made before a `let mut` binding changes keeps the old value.
- A captured array or string is copied as any binding of it is ([Values and memory](values.md#copied-and-shared)).
- A closure that captures an object shares the object. It can write the `mut` fields of the object.
- An assignment to a captured name is an error, also when the name is a `let mut`. The closure cannot change it.

```sisu pending
fn main() {
    let mut total = 0
    // The closure cannot change `total`.
    let add = (x: i64) => total += x
    add(2)
}
```

To keep a value that changes, return the new value from the closure, or keep the value in a `mut` field of an object.

## Weak captures

A closure that captures an object keeps the object alive. When the object also holds the closure, as in a field, the two never get freed ([Cycles](values.md#cycles)). A *weak capture* does not keep the object alive.

```sisu pending
class Button {
    mut on_click: (() -> unit)?
}

class Counter {
    mut count: i64
    button: Button

    fn wire(self) {
        self.button.on_click = [weak self] () => self?.increment()
    }

    fn increment(self) {
        self.count += 1
    }
}

fn main() {
    let c = Counter(count: 0, button: Button(on_click: None))
    c.wire()
    if let f = c.button.on_click {
        f()
    }
    print(c.count)                     // 1
}
```

The rules:

- `[weak name]` before the parameters captures `name` weakly. A list of several is `[weak a, weak b]`.
- Only a class object, or an optional class object, can be captured weakly. Any other capture is an error.
- Inside the closure, a name that is captured weakly is an optional. Read it with `?.`, `??` and `if let`, as a [weak field](values.md#weak-fields).
- When the object is gone, the closure still runs. Here, `self?.increment()` then does nothing.
- A closure can capture an object `x` strongly, and the program can store the closure in a place that starts at `x`, such as `x.button.on_click`. Then the object keeps itself alive, and the program builds with a warning. `[weak x]` removes the cycle.

## Iterators

An *iterator* is a value with a method `next`. Each call of `next` gives the next element as an optional, and `None` when no element is left. The interface `Iterator` states this.

```sisu pending
class Countdown: Iterator {
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
    for n in Countdown(left: 3) {
        print(n)                       // 3, then 2, then 1
    }
}
```

An *iterable* is a value that can make an iterator. The interface `Iterable` has the associated types `Item` and `Iter`, and a method `iter`. A `for` loop takes an iterable.

The rules:

- `for x in e` with an array or a range works as it does now. For any other `e`, the type of `e` must conform to `Iterable`. The loop calls `e.iter()` once, then `next()` until the result is `None`.
- An array conforms to `Iterable`. `xs.iter()` gives the iterator of an array. It holds the array, so a change to the array during the loop copies the array, and the loop sees the old elements.
- Every iterator is an iterable. Its `iter()` gives a copy of the iterator itself.
- A class conforms to `Iterator` with a plain `self` method, because a class method can write the `mut` fields of its object.
- A range is a value. `0..n` and `0..=n` can be stored, passed and returned, and they are iterators of `i64`.
- A `for` loop copies its iterator. After a `break`, the iterator of the program is where it was. A second `for` over it starts again. To continue where a loop stopped, call `next` in a `while let`. An iterator that is an object is shared, so the loop advances it.
- A bound can fix the associated type: `fn total<S: Iterable<Item = i64>>(xs: S) -> i64` takes an array, a range or a chain of iterator methods.

## Iterator methods

Every iterable has these methods. An *adapter* returns a new iterator and does nothing until the program asks it for an element. The other methods use the iterator up.

| Method | Result | Kind |
| --- | --- | --- |
| `map(f)` | An iterator of the results of `f` | Adapter |
| `filter(keep)` | An iterator of the elements for which `keep` is true | Adapter |
| `take(n)` | An iterator of the first `n` elements | Adapter |
| `take_while(keep)` | An iterator of the elements before the first one for which `keep` is false | Adapter |
| `filter_some()` | For an iterator of `T?`, an iterator of the `T` values | Adapter |
| `count()` | The number of elements, an `i64` | Uses up |
| `any(f)`, `all(f)` | A `bool` | Uses up, stops early |
| `find(f)` | The first element for which `f` is true, as an optional | Uses up, stops early |
| `fold(init, f)` | `f` applied to `init` and each element in turn | Uses up |
| `to_array()` | An array of the elements | Uses up |
| `sum()` | The sum, for elements of type `i64` or `f64` | Uses up |

```sisu pending
fn main() {
    let words = ["sauna", "sisu", "loyly", "kiuas"]
    print(words.filter(w => w.len() > 4).count())     // 3
    let total = (0..10).filter(x => x % 2 == 0).map(x => x * x).sum()
    print(total)                                      // 120
    print(words.map(w => w.len()).fold(0, (acc, n) => acc + n))   // 19
}
```

The rules:

- A chain such as `.filter(...).map(...).sum()` builds no array between its steps.
- A statement that only calls an adapter, such as `xs.map(f)`, builds with the warning for an unused value. A note tells that `map` does nothing until a method uses up the iterator.
- `take(n)` returns at most `n` elements.
- `any`, `all` and `find` stop at the first element that decides the result.
- `sum` of `i64` adds with the checked `+`, so it panics on overflow ([Overflow](operators.md#overflow)).
