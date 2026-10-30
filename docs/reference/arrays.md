# Arrays

An *array* `[T]` holds values of the type `T` in order. Each value is an *element*, and its index tells its position, from 0.

```sisu
fn main() {
    let mut scores = [70, 85, 90]
    scores[0] = 75
    print(scores[0])             // 75
    print(scores.len())          // 3
    print(scores.sum())          // 250
}
```

An array is a value. When the program binds, passes or stores an array, it behaves as a copy ([Arrays are values](#arrays-are-values)).

## Array literals

An array literal `[a, b, c]` makes an array of its elements, in order. The program calculates the elements from left to right.

```sisu
fn main() {
    let names = ["Aino", "Eero"]
    let rows = [[1, 2], [3]]
    let found = [4, None, 6]
    let empty: [i64] = []
    print(names.len() + rows.len() + found.len() + empty.len())   // 7
}
```

The rules:

- All the elements have one type ([E0032](../diagnostics/E0032.md)). The other elements decide the type of an integer literal, so `[1, 2.5]` is an `[f64]` ([Literals](literals.md#the-type-of-an-integer-literal)).
- A `None` element makes the type of the other elements optional. So `[4, None, 6]` is an `[i64?]`.
- `[]` alone does not tell its type ([E0050](../diagnostics/E0050.md)). Where the context expects an array, such as a `let` with a type, a parameter or a field, `[]` gets that type.
- An array literal can continue over many lines. A `,` after the last element is valid.

## Filled arrays

`[value; length]` makes an array of `length` elements. Each element is a copy of `value`.

```sisu
fn main() {
    let zeros = [0; 4]
    let mut grid = [[0; 3]; 2]
    grid[0][1] = 5
    print(zeros.len())           // 4
    print(grid[0][1])            // 5
    print(grid[1][1])            // 0
}
```

The rules:

- The program calculates `value` once, then `length`.
- `length` is an `i64` ([E0032](../diagnostics/E0032.md)). A length of 0 gives an empty array.
- At run time, a negative length stops the program with a panic. The program also panics when the size of the array in bytes is more than an `i64` can hold.
- When the type of `value` is a class or an optional class, every element would be the same object. This is an error ([E0067](../diagnostics/E0067.md)), even when a binding of an optional class type holds `None`. A value whose type is a type parameter, such as `T`, is also an error, because `T` can be a class ([Generics](generics.md#values-of-a-type-parameter)). The literal `None` is the one exception: `[None; length]` is valid where the context gives its type, such as `let slots: [Box?] = [None; 3]`. Without that context it is an error ([E0050](../diagnostics/E0050.md)).
- When `value` holds objects, such as an array of objects, every element holds the same objects.

```sisu panics
fn main() {
    let n = -1
    let xs = [0; n]
    print(xs.len())
}
```

```stderr
sisu: panic at example.sisu:3:18: array length is negative
```

## Indexes

`xs[i]` is the element at the index `i` ([Indexes](expressions.md#indexes)). The index is an `i64`. An index that is negative, or not less than the length, stops the program with a panic.

An assignment `xs[i] = value` replaces an element. The array must be in a place that can change, such as a `let mut` binding or a `mut` field ([Assignment](statements.md#assignment)).

```sisu
fn main() {
    let mut grid = [[1, 2], [3, 4]]
    grid[1][0] = 30
    grid[0] = [10, 20, 25]
    grid[0][2] += 5
    print(grid[0][2] + grid[1][0])   // 60
}
```

The rules:

- A binding without `mut` cannot change its array, also not an element of an inner array ([E0001](../diagnostics/E0001.md)).
- An object in an array stays shared, so its `mut` fields can change also when the array cannot ([Classes](classes.md#fields)).
- An optional array has no index. Unwrap it first, for example with [`if let`](enums.md#if-let-and-while-let) ([E0062](../diagnostics/E0062.md)).

```sisu
fn main() {
    let grid = [[1, 2], [3, 4]]
    grid[0][0] = 9               // error[E0001]
}
```

## for

A [`for` loop](statements.md#for) runs its body once for each element of an array, in order.

```sisu
fn main() {
    let names = ["Aino", "Eero"]
    for name in names {
        print("Hello, {name}")
    }
}
```

```output
Hello, Aino
Hello, Eero
```

`print` takes an array, and writes it as `[1, 2]` ([Text of other types](strings.md#text-of-other-types)). An interpolation takes an array too: `"{xs}"` writes the same text, and `xs.to_string()` gives it as a `string`. A `string` or a `char` in an array is quoted: `["a", "b"]`.

## Methods

An array has these methods:

| Method | Result | Meaning |
| --- | --- | --- |
| `xs.len()` | `i64` | The number of elements |
| `xs.sum()` | The element type | The sum of the elements of an `[i64]` or an `[f64]` |
| `xs.contains(x)` | `bool` | `true` when an element is equal to `x`, as `==` tells |

```sisu
fn main() {
    let xs = [3, 1, 4]
    let weights = [0.5, 0.25]
    let maybe = [1, None]
    print(xs.len())              // 3
    print(xs.sum())              // 8
    print(weights.sum())         // 0.75
    print(xs.contains(4))        // true
    print(maybe.contains(None))  // true
}
```

The rules:

- `len` is a method, so write `xs.len()`. `xs.len` is an error ([E0057](../diagnostics/E0057.md)), and so is `len(xs)` ([E0070](../diagnostics/E0070.md)).
- `sum` takes only an `[i64]` or an `[f64]` ([E0069](../diagnostics/E0069.md)). It adds the elements in order, and the sum of an empty array is zero.
- The prelude writes `sum` in Sisu, in an `extend [i64]` block and an `extend [f64]` block, and `contains` in an `extend<T: Eq> [T]` block. The compiler writes `len`. A program cannot write an `extend` block for an array: only the prelude can.
- On an `[i64]`, a sum outside the range of `i64` stops the program with a panic ([Overflow](operators.md#overflow)). The panic names the line of the `.sum()` call, not a line of the prelude.
- The argument of `contains` has the element type ([E0032](../diagnostics/E0032.md)). So `None` is valid only when the elements are optional.
- `contains` needs `==` on the element type, so an array of objects has it only when the class conforms to `Eq`, and an array of a type parameter `T` only with the bound `T: Eq` ([E0154](../diagnostics/E0154.md)). The error stands at the call, and no line of the prelude shows in it.
- `contains` uses `==`, so `contains(f64.nan)` is always `false` ([Types](types.md#floating-point-numbers)).
- A name that is not one of these methods is an error ([E0063](../diagnostics/E0063.md)).
- On an optional array, call a method with `?.` ([Optional chaining](operators.md#optional-chaining)).

```sisu
fn main() {
    let flags = [true, false]
    print(flags.sum())           // error[E0069]
}
```

### More methods

`xs.push(x)` adds `x` after the last element of `xs`. It writes the array, as `xs[i] = x` does. An array also has the [iterator methods](closures.md#iterator-methods).

```sisu pending
fn main() {
    let mut xs = [1, 2]
    xs.push(3)
    print(xs.len())                    // 3
    print(xs.map(x => x * 2).sum())    // 12
}
```

## Equality

Two arrays are equal when they have the same length and their elements are equal, in order ([Equality](operators.md#equality)). `<` and the other comparisons do not apply to arrays ([E0159](../diagnostics/E0159.md)). An array of objects has `==` only when the class conforms to `Eq` ([E0159](../diagnostics/E0159.md)). Nor does `is`, because an array is not an object ([E0046](../diagnostics/E0046.md)).

```sisu
fn main() {
    let a = [1, 2]
    print(a == [1, 2])           // true
    print(a == [2, 1])           // false
    print(a != [])               // true
}
```

## Arrays are values

When the program binds an array, passes it or stores it, the array behaves as a copy. A change to one holder does not change the other holders ([Values and memory](values.md)).

```sisu
fn main() {
    let a = [1, 2, 3]
    let mut b = a
    b[0] = 10
    print(a[0])                  // 1
    print(b[0])                  // 10
}
```

The rules:

- An array that the program reads out of another array, or out of a field, is a value too. A write to it does not change the original.
- A copy of an array holds the same objects as the original. A change to a `mut` field of such an object shows in both arrays.
- A function cannot change an array that it gets as an argument. A function that makes a changed array returns it, and the caller writes `xs = change(xs)`.

```sisu
fn doubled(xs: [i64]) -> [i64] {
    let mut out = xs
    for i in 0..out.len() {
        out[i] *= 2
    }
    return out
}

fn main() {
    let mut xs = [1, 2, 3]
    let before = xs
    xs = doubled(xs)
    print(xs[2])                 // 6
    print(before[2])             // 3
}
```

Sisu does not copy the elements when the program binds, passes or stores an array. The holders share the elements until a write to one of them. That write copies the elements first, and a loop that writes the array can copy them before it starts. A program cannot see when the copy occurs.
