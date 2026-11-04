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

## Range indexes

`xs[a..b]` is the part of `xs` from the index `a` up to, and not including, the index `b`. The bounds are `i64`s ([E0032](../diagnostics/E0032.md)). `xs[a..=b]` includes the index `b`. A range without `a` starts at 0, and a range without `b` runs to the end: `xs[a..]`, `xs[..b]` and `xs[..=b]`. The result is an array of the same type. `xs[..]` is not a range index; write `xs` ([E0023](../diagnostics/E0023.md)).

```sisu
fn main() {
    let xs = [1, 2, 3, 4, 5]
    print(xs[1..3])                  // [2, 3]
    print(xs[1..=3])                 // [2, 3, 4]
    print(xs[2..])                   // [3, 4, 5]
    print(xs[..2])                   // [1, 2]
    print(xs[2..2])                  // []
}
```

A range index copies no element. The new array shares its elements with `xs`, and a write or a `push` to either array does not change the other. Changing a part of an array in place, such as `xs[1..3] = ys`, is an error, because a range index gives a new array ([E0021](../diagnostics/E0021.md)). A range index is not a place. A `push` to one is an error too; keep the part in a `let mut` variable and push to that.

The range must satisfy `0 <= start <= end <= length`, where `a..=b` ends at `b + 1`. A range that does not stops the program with a panic, as a bad index does. The panic names the first check that fails, in this order: a negative start, an end past the length, and a start past the end.

```sisu panics
fn main() {
    let xs = [1, 2, 3, 4, 5]
    print(xs[2..9])
}
```

```stderr
sisu: panic at example.sisu:3:14: range end 9 is past length 5
```

The base is evaluated first, then the start, then the end, and then the checks run. A range index on a value that is not an array or a string, such as an `i64`, is an error ([E0068](../diagnostics/E0068.md)). An optional array has no range index either ([E0062](../diagnostics/E0062.md)).

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
| `xs.first()` | The element type, optional | The first element, or `None` when `xs` is empty |
| `xs.last()` | The element type, optional | The last element, or `None` when `xs` is empty |
| `xs.get(i)` | The element type, optional | The element at `i`, or `None` when `i` is outside the array |
| `xs.push(x)` | none | Adds `x` after the last element, and writes `xs` |

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
    print(xs.first() ?? 0)       // 3
    print(xs.get(3) ?? -1)       // -1
}
```

The rules:

- `len` is a method, so write `xs.len()`. `xs.len` is an error ([E0057](../diagnostics/E0057.md)), and so is `len(xs)` ([E0070](../diagnostics/E0070.md)).
- `sum` takes only an `[i64]` or an `[f64]` ([E0069](../diagnostics/E0069.md)). It adds the elements in order, and the sum of an empty array is zero.
- The prelude writes `sum` in Sisu, in an `extend [i64]` block and an `extend [f64]` block. It writes `contains` in an `extend<T: Eq> [T]` block, and `first`, `last` and `get` in an `extend<T> [T]` block. The compiler writes `len`. A program cannot write an `extend` block for an array: only the prelude can.
- On an `[i64]`, a sum outside the range of `i64` stops the program with a panic ([Overflow](operators.md#overflow)). The panic names the line of the `.sum()` call, not a line of the prelude.
- The argument of `contains` has the element type ([E0032](../diagnostics/E0032.md)). So `None` is valid only when the elements are optional.
- `contains` needs `==` on the element type, so an array of objects has it only when the class conforms to `Eq`, and an array of a type parameter `T` only with the bound `T: Eq` ([E0154](../diagnostics/E0154.md)). The error stands at the call, and no line of the prelude shows in it.
- `contains` uses `==`, so `contains(f64.nan)` is always `false` ([Types](types.md#floating-point-numbers)).
- `first`, `last` and `get` give `None` where `xs[i]` would stop the program. For `get(i)`, that is a negative `i` and an `i` from `xs.len()` on. For `first` and `last`, it is an empty array. They are the safe form of an index. On a slice they read the slice ([Range index](#range-indexes)).
- `get` takes an `i64`, not a range. `xs.get(1..3)` is an error ([E0032](../diagnostics/E0032.md)): write `xs[1..3]`, which stops the program when the range is out of bounds.
- `push` writes the array, as `xs[i] = x` does, so it needs a place that can change: a `let mut` variable, a `mut` field, or an element of one. A `let` variable, a parameter and a field without `mut` are errors ([E0001](../diagnostics/E0001.md)). A value that is not stored, such as `f().push(1)`, is an error too ([E0021](../diagnostics/E0021.md)).
- `push` has no value. The argument has the element type, so `push(1)` on an `[i64?]` adds `Some(1)` ([E0032](../diagnostics/E0032.md)), and `push` takes exactly one argument ([E0053](../diagnostics/E0053.md)).
- A `push` onto an array that another variable holds does not change that variable. Arrays are values ([Arrays are values](#arrays-are-values)).
- `xs?.push(x)` on an optional array adds `x` when `xs` holds an array. When `xs` is `None`, nothing happens and `x` is not computed. The call tests `xs` again after it computes `x`. When `x` made `xs` `None`, the call adds nothing. The place follows the rules above, so `xs` is a `let mut` variable. `bag?.items?.push(x)` goes through an optional object, and `items` is a `mut` field. `gs[i]?.push(x)` on an array `gs` of optional arrays checks `i` before it computes `x`, so a bad index stops the program before `x` runs.
- An assignment can go through `?.` too: `bag?.items = []` writes only when `bag` holds an object ([Optional chaining](operators.md#optional-chaining)).
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

```sisu
fn main() {
    let mut xs = [1, 2]
    xs.push(3)
    print(xs.len())                    // 3
    print(xs)                          // [1, 2, 3]
}
```

On an optional array, `?.push` adds the element only when the array is there.

```sisu
class Bag {
    mut items: [i64]
}

fn main() {
    let mut xs: [i64]? = [1]
    xs?.push(2)
    print(xs ?? [])                    // [1, 2]
    let mut none: [i64]? = None
    none?.push(3)
    print(none ?? [0])                 // [0]
    let bag: Bag? = Bag(items: [1])
    bag?.items?.push(2)
    print(bag?.items ?? [])            // [1, 2]
}
```

```sisu pending
fn main() {
    let xs = [1, 2, 3]
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
- A function cannot change an array that it gets as a plain argument. To change the array of the caller, the parameter is `mut` ([`mut` parameters](functions.md#mut-parameters)). A function can also return a changed array, and the caller writes `xs = change(xs)`.

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

Sisu does not copy the elements when the program binds or passes an array. A small slice, such as `xs[1..3]` of a long array, can go into a class field, an enum payload, an array or an `any I`. That store copies the slice. So the slice does not keep the long array alive ([ADR 0042](../adr/0042-a-slice-shares-its-block.md)). Other stores do not copy the elements. The holders share the elements until a write to one of them. That write copies the elements first, and a loop that writes the array can copy them before it starts. A program cannot see when the copy occurs.

## When an array is copied

Binding, passing, storing or returning an array copies no element. The holders share the elements. A write to an array copies the elements first when another holder shares them. Then the writer has elements of its own, and its next writes do not copy. A program cannot see the copy, but it costs time.

The cases:

- A write to an array that no other holder shares does not copy.
- A write to a copy that the program made with `let mut b = a` copies at the first write, while `a` still holds the elements.
- A plain parameter cannot be written. A function that needs a changed array copies it into a `let mut` binding, and the first write copies the elements.
- A `mut` argument that is a local variable moves the array into the parameter. A write by the function does not copy, unless another holder shares the elements. When the call ends, the array goes back into the variable.
- A `mut` argument that is a field or an element copies the value in: the field keeps its array during the call, because code that reads the field sees the old value. The first write by the function copies the elements once, and the next writes do not.

```sisu
fn fill(mut xs: [i64], n: i64) {
    for i in 0..xs.len() {
        xs[i] = n
    }
}

fn main() {
    let mut xs = [0; 1000]
    for n in 1..=3 {
        fill(mut xs, n)
    }
    print(xs[999])               // 3
}
```

Here, each call moves `xs` into the parameter and back, so no call copies the 1000 elements.
