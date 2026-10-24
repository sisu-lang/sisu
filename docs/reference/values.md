# Values and memory

An object of a class is *shared*: each reference to it refers to the same object. Every other value *behaves as a copy*, so no other holder can change it.

```sisu
class Box {
    mut value: i64
}

fn main() {
    let a = [1, 2]
    let mut b = a
    b[0] = 9
    print(a[0])                  // 1
    let x = Box(value: 1)
    let y = x
    y.value = 9
    print(x.value)               // 9
}
```

This page tells what is copied and what is shared, and when Sisu frees memory. A program does not free memory itself.

## Copied and shared

When the program binds a value, passes it, stores it or returns it, the value is copied or shared as this table tells:

| Value | Binding, passing, storing or returning it |
| --- | --- |
| `i64`, `u8`, `f64`, `bool`, `char` | Copies it |
| `string` | Behaves as a copy |
| An array | Behaves as a copy. The objects in it stay shared |
| An [enum](enums.md#enum-values) value | Behaves as a copy. The objects in its payload stay shared |
| An optional `T?` | Copies or shares it as for `T` |
| An object of a class | Shares it: the new holder refers to the same object |

```sisu
class Box {
    mut value: i64
}

fn main() {
    let mut text = "sau"
    let saved = text
    text += "na"
    print(saved)                 // sau
    let boxes = [Box(value: 1)]
    let mut copy = boxes
    copy[0].value = 5
    print(boxes[0].value)        // 5
    copy[0] = Box(value: 7)
    print(boxes[0].value)        // 5
}
```

The rules:

- After `let mut b = a`, a change to `b` does not change `a`, and a change to `a` does not change `b`. The table tells which values this applies to.
- A reference is a value too. A copy of a reference refers to the same object, so a write to a `mut` field shows through each reference.
- For a value that behaves as a copy, the binding decides whether it can change: `let` or `let mut`. For an object, the `mut` of each field decides ([Classes](classes.md#fields)).
- A `for` loop over an array of objects gets each object, not a copy of it.

## Calls

A call does not copy its arguments. A parameter cannot change ([Parameters](functions.md#parameters)), so a function cannot change a value that it gets as an argument and that behaves as a copy.

A function can write the `mut` fields of an object that it gets, because the object is shared.

```sisu
class Box {
    mut value: i64
}

fn change(b: Box, xs: [i64]) {
    b.value = 10
    let mut mine = xs
    mine[0] = 10
}

fn main() {
    let b = Box(value: 1)
    let xs = [1]
    change(b, xs)
    print(b.value)               // 10
    print(xs[0])                 // 1
}
```

## Copy-on-write

An array or a string is not copied when the program binds it, passes it or stores it. Its holders share its memory until a write to one of them, and that write copies it first ([Arrays](arrays.md#arrays-are-values)). A program cannot see when the copy occurs.

## Identity and equality

`a is b` tells whether two references refer to the same object. `a == b` tells whether two values are equal ([Equality](operators.md#equality)).

```sisu
class Sauna {
    heat: i64
}

fn main() {
    let a = Sauna(heat: 80)
    let b = Sauna(heat: 80)
    let c = a
    print(a is c)                // true
    print(a is b)                // false
    print([1, 2] == [1, 2])      // true
}
```

The rules:

- `is` applies only to objects and to optionals of objects. Two optionals that are both `None` are the same ([`is`](operators.md#is)).
- `is` on a value that behaves as a copy, such as an array, is an error ([E0046](../diagnostics/E0046.md)). Use `==`.
- To test an optional for `None`, use `== None`, not `is` ([E0045](../diagnostics/E0045.md)).

## Object lifetime

Sisu counts the references to each object. An object is freed at once when its last reference goes away.

A reference held by a local can go away at its last use, before the end of its block. When Sisu frees an object, it releases the references in the fields of that object. So an object whose last reference was in such a field is freed too.

```sisu
class Node {
    value: i64
    next: Node?
}

fn main() {
    let mut head: Node? = None
    for i in 1..=3 {
        head = Node(value: i, next: head)
    }
    print(head?.value ?? 0)      // 3
    head = head?.next
    print(head?.value ?? 0)      // 2
}
```

Here, `head = head?.next` replaces the reference to the node with the value 3. Nothing else refers to that node, so Sisu frees it. The node with the value 2 stays, because `head` refers to it.

The rules:

- Sisu frees the memory of an array or a string in the same way, when its last holder goes away.
- No code of the program runs when Sisu frees an object. A program cannot see when it happens. The time changes only how much memory the program uses. The section [Weak fields](#weak-fields) is in development, and it changes this rule.

## Cycles

A *cycle* is a chain of references that comes back to the object where it starts. Objects in a cycle are never freed. This applies also to one object that refers to itself.

```sisu
class Node {
    value: i64
    mut next: Node?
}

fn main() {
    let a = Node(value: 1, next: None)
    let b = Node(value: 2, next: a)
    a.next = b
    print(a.next?.next?.value ?? 0)      // 1
    a.next = None
}
```

To let Sisu free the objects, break the cycle while a reference to one of them remains. Above, `a.next = None` breaks it. So both nodes are freed when their last references go away. The section [Weak fields](#weak-fields) is in development, and it adds a second way to break a cycle.

## Weak fields

A *weak field* refers to an object without keeping it alive. When the last other reference to the object goes away, Sisu frees the object, and the weak field reads `None`.

```sisu pending
class Node {
    value: i64
    weak mut parent: Node?
}

fn attach(child: Node) {
    let parent = Node(value: 1, parent: None)
    child.parent = parent
    print(child.parent?.value ?? 0)      // 1
    print(parent.value)                  // 1
}

fn main() {
    let child = Node(value: 2, parent: None)
    attach(child)
    print(child.parent?.value ?? 0)      // 0
}
```

Here, the local `parent` is the only strong reference to the first node. When `attach` returns, Sisu frees that node, and `child.parent` reads `None`.

The rules:

- `weak` comes before `mut`. A weak field must be a `mut` field. Its type must be an optional of a type that conforms to `AnyObject`. Such a type is a class, so `Node?` is valid.
- A `T?` with `T: AnyObject` is valid too, and so is `(any I)?` for an interface `I` that only classes conform to ([Bounds](generics.md#bounds), [Interface values](generics.md#interface-values)).
- A weak field reads like any optional, with `?.`, `??` and `if let`. A read never fails, and it never gives a freed object.
- A weak field makes the time of the free visible. A reference held by a local can go away at its last use. So a weak field can read `None` before the end of that block.
- A weak field breaks a cycle. When a parent holds its child and the child holds its parent in a weak field, Sisu can free both.
