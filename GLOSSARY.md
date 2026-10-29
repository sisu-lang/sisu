# Sisu

Sisu is a small, statically typed language compiled to native code. These are
the terms its spec, compiler and docs use.

## Language

**Binding**:
A name introduced by `let`, `let mut` or a function parameter.
_Avoid_: variable (for a `let` or a parameter)

**Variable**:
A binding declared with `let mut`, the only kind that can be reassigned.
_Avoid_: mutable binding, `var` (the keyword before milestone 4)

**Place**:
An expression that names storage and can stand on the left of `=`: a
variable, a `mut` field, or an element of an array held in a place, such as
`grid[i][j]`. In mir, a place can also name a payload field under a known
variant, which only `match` reads.
_Avoid_: lvalue

**Block value**:
The value of a block's last statement when that statement is an expression;
otherwise the block has no value.

**Separator**:
What ends one statement, field, variant or arm and starts the next: a
line break, or a `;` between two of them on one line. Any other `;` is
redundant: the compiler warns about it, and its fix removes it.
_Avoid_: terminator (no Sisu line needs a `;` at its end)

**Unused value**:
The value of an expression statement that nothing uses. The compiler warns
about one; `let _ = e` discards a value on purpose.
_Avoid_: dropped value (dropping is what a reference count does)

**Unconditional recursion**:
A function that calls itself on every path, so it can never return; the
compiler warns about it.

**Live code**:
Code that `main`, or a function, method or constant whose name starts with `_`,
can reach through calls and constant names. Only a use in live code counts as a
use for the unused-name warnings.

**Loop label**:
The name written before a loop (`outer: for ...`) that a `break` or
`continue` names to leave or restart that loop instead of the innermost one.
_Avoid_: label (alone), tag

**Comparison chain**:
Two or more comparisons in one direction written together, such as
`a < b <= c`, meaning each adjacent pair holds.

**`unit`**:
The type of an expression that produces no value.
_Avoid_: void

**`never`**:
The type of an expression that does not finish, such as `return`, or a
`loop` or `while true` with no `break`; it matches any type.

**Conversion**:
A call written with a type's name, `T(x)`, that turns a value into an
adjacent type, such as `u8(n)`, `i64(c)` or `f64(n)`. A conversion that can fail
returns `T?`.
_Avoid_: cast, fallible conversion, `as`

**Panic**:
The program stopping at runtime with `sisu: panic at <file>:<line>:<col>:
<message>` on stderr and exit code 101. A failed write to standard output or
read from standard input has no location: `sisu: panic: <message>`.
_Avoid_: trap, crash, abort

**Prelude**:
The names visible in every file without an import. Most are declared in Sisu,
in the prelude file the compiler builds into every program.
The prelude file, `crates/sisuc/src/prelude.sisu`, declares `Option` and `Result`.

**RC check**:
A check that a program built with `--rc-checks` makes on its reference
counts: no retain or release of a count below 1, no read of a string, class
or array local whose reference was moved out, and no object still allocated
at a normal exit. A failed check prints `sisu: rc check: <what>` on stderr and
exits with code 102.

**Language reference**:
The pages that state what each construct of Sisu means and its rules, with
checked examples.
_Avoid_: spec, manual

## Arrays

**Array**:
A growable sequence of values of one type, written `[T]`, stored side by side
in one block. An array is a value: binding it, passing it or storing it
behaves as a copy.
Growth methods arrive in milestone 10.
_Avoid_: list, vector

**Copy-on-write**:
How arrays and strings behave as values without copying eagerly: holders
share one block, and a write (to an array element, or `+=` on a string)
copies the block first when another holder exists. A loop that writes an array
it never shares checks the count once, before it starts, and copies there if
needed.
_Avoid_: lazy copy

**Last use**:
A read of a local after which the local is not read again before it is
reassigned or goes out of scope. A last use hands over its reference instead
of sharing it, so a write that follows can skip the copy.
_Avoid_: move (see **Move**: the compiler's name for how mir hands it over, not the language rule)

**Range**:
`a..b`, covering `a` up to but not including `b`, or `a..=b`, which includes
`b`. It appears in a `for` header and as a pattern, and is not a value.

## Text

**`string`**:
A sequence of bytes that is always valid UTF-8. A string is a value, like an
array. Two strings are equal when their bytes are equal; there is no Unicode
normalisation.
_Avoid_: str, text

**`char`**:
One Unicode scalar value: a code point that is not a surrogate. It is written
between single quotes, as in `'a'` or `'\u{1F600}'`. It compares by code point
and has no arithmetic: `i64(c)` and `char(n)` convert.
_Avoid_: character (readers take it to mean what they see, which can be
several scalar values)

**Interpolation**:
A `{expr}` inside a string literal. The expression is evaluated and turned
into text, and the whole literal becomes one new `string`.
_Avoid_: formatting, templating

**Format spec**:
The `[align][width][.precision]` after a `:` that ends an interpolation, as
in `{x:>8.2}`.
_Avoid_: format string, format specifier

**Shadowing**:
Declaring a `let` or `let mut` with the name of a binding that is already
visible, in the same scope or an outer one. The new binding replaces the old
one from its declaration onwards, and its right-hand side still sees the old
one.
_Avoid_: redeclaration, rebinding

**Constant**:
A named value declared at the top level with `const` and folded at compile
time. It is typed, and a local binding with its name shadows it. It is
neither a `let` binding nor a literal: each use becomes the literal its value
folds to.
_Avoid_: global, static

**Reserved keyword**:
A word that the lexer already treats as a keyword for a feature that comes in
a later milestone, such as `record` or `import`. Using one as a name is an
error, so a program written today does not break when the feature lands.
_Avoid_: future keyword

## Classes and objects

**Class**:
A named type whose values are references to objects with the same fields and
methods. Sisu classes have no inheritance.
_Avoid_: struct, record (a value type, milestone 10)

**Record**:
A named type with fields and methods whose values are not shared: binding,
passing or storing one behaves as a copy, as an array does. Records come in milestone 10.
_Avoid_: struct, data class

**Object**:
One heap-allocated value of a class, shared by every reference to it.
_Avoid_: instance

**Reference**:
A value of class type: it refers to an object, and copying it copies the
reference, not the object.
_Avoid_: pointer, handle

**Field**:
A named, typed slot in every object of a class or value of a record, declared
on its own line in the type's body. A class field marked `mut` can be
reassigned; a bare one is set by the constructor and never changes.
_Avoid_: property, member variable, attribute

**Method**:
A function declared inside a class, enum or record, or in an `extend` block,
whose first parameter is `self`, called as `receiver.name(args)`.
_Avoid_: member function

**Mutating method**:
A method of a record or an array that takes `mut self`, such as `push`, and
changes the value in the place it was called on. That place must be able to
change.
_Avoid_: `inout`, `mutating`

**Receiver**:
The expression before `.` or `?.` in a field read or a method call: the `a`
in `a.f` and `a.m()`.

**Constructor**:
The call `Class(field: value, ...)` that creates an object, naming every field
in declaration order.
_Avoid_: initializer, init

**Drop**:
Freeing an object when the last reference to it goes away, after releasing
the references in its fields.
_Avoid_: deinit, destructor, finalizer

**Structural equality**:
`==` on a value comparing it part by part: array elements, variant payloads,
an optional's contents. Classes have none: they compare by **Identity**, and
have `==` only when they conform to `Eq` by hand.
Before milestone 8 (ADR 0022), classes compared field by field.

**Identity**:
Whether two references refer to the same object, tested with `is`.
_Avoid_: reference equality, `===`

## Enums and patterns

**Enum**:
A type whose every value is one of a fixed list of variants, declared with
`enum`. An enum value never changes after it is built.
_Avoid_: tagged union, sum type, discriminated union

**Variant**:
One case of an enum, such as `Expr.Num`. It may carry a payload.
_Avoid_: case, constructor, member

**Payload**:
The fields a variant carries, all labeled or none: `Num(i64)`,
`Rect(w: i64, h: i64)`.
_Avoid_: associated value, data

**Scrutinee**:
The value a `match`, `if let` or `while let` tests against a pattern: the `e`
in `match e { ... }`. A `match` evaluates it once, before it tries any arm.
_Avoid_: subject, matched value

**Arm**:
One case of a `match`, on its own line: a pattern, an optional guard, `=>`, and
a body, which is an expression, an assignment, `return`, `break`, `continue`
or a block. The arms are tried top to bottom, and the first whose pattern
matches and whose guard holds runs.
_Avoid_: case, branch (the two sides of an `if`)

**Pattern**:
The shape a `match` arm, `if let` or `while let` tests a value against. A
pattern can bind names to the parts it matches.
_Avoid_: matcher, case

**Alternative**:
One of the patterns joined by `|`, at the top of a pattern or inside one:
`Red | Amber => ...`. The pattern matches when any alternative does, and every
alternative binds the same names.

**Guard**:
An `if` condition after an arm's pattern. The arm runs only when the pattern
matches and the guard holds.
_Avoid_: filter, where clause

**Exhaustive**:
Covering every value of the scrutinee's type. Every `match` must be
exhaustive, or the program does not compile.
_Avoid_: total, complete

## Optionals

**Optional**:
A value of type `T?`, short for `Option<T>`, holding either a `T` or `None`.
Optionals nest: `i64??` is `Option<Option<i64>>`.
_Avoid_: nullable, maybe

**`None`**:
The optional that holds no value.
_Avoid_: nil, null

**Unwrap**:
Getting the value out of one layer of an optional when it holds one, with
`if let`, `while let`, `?.` or `??`. Unwrapping an `i64??` gives an `i64?`.

**Niche**:
A bit pattern that a type's values never use, in which an `Option`-shaped
enum stores its empty variant with no separate flag: null for a reference,
`0x110000` for a `char`.

## Generics and interfaces

Milestone 8 adds these.

**Type parameter**:
A name for a type that a generic function, class, enum, interface or alias
takes in angle brackets, such as `T` in `fn largest<T: Ord>(xs: [T])`.
_Avoid_: generic parameter

**Type argument**:
The type given for a type parameter at a use, written out (`Stack<i64>`) or
inferred (`largest([3, 9, 2])`).

**Bound**:
An interface a type parameter must conform to, written after it: `T: Ord`,
`T: Shape & Ord`. Inside a generic body, a value of type `T` supports only
what its bounds give it.
_Avoid_: constraint

**Instantiation**:
One compiled copy of a generic function or type for one list of type
arguments, such as `largest<i64>`.
_Avoid_: specialization

**Monomorphization**:
Compiling a generic definition once for each instantiation the program
reaches, so that generic code runs as fast as a copy written by hand for
each type.

**Interface**:
A named set of method signatures and associated types, declared with
`interface`. An interface is used as a bound, or after `any` as the type of
an interface value; on its own it is not a type.
_Avoid_: trait, protocol

**Conformance**:
A type's declaration that it meets an interface, with the methods the
interface requires: `class Circle: Shape { ... }` or
`extend Celsius: Convert<Kelvin> { ... }`. A type conforms to an interface
with the same arguments at most once. From milestone 11, a conformance lives
in the project that declares its type, its interface or an argument of its
interface.
_Avoid_: impl, implementation

**Generated conformance**:
A conformance the compiler writes, such as `Display` for every type, or `Eq`
for an enum whose payloads are all `Eq`. A conformance the program writes
replaces it.
_Avoid_: derived, synthesized, auto-implemented

**Language interface**:
One of the four prelude interfaces the language itself uses: `Display` for
`print` and interpolation, `Eq` for `==` and `!=`, `Ord` for `<`, `<=`, `>`
and `>=`, and `AnyObject` for `is` and `weak` in generic code.

**Requirement**:
A method signature or associated type an interface declares, which each
conforming type meets: a method with the same signature, or a choice such as
`type Item = i64`. The word appears in diagnostics as the missing parts of a
conformance (E0152).
_Avoid_: obligation

**Associated type**:
A type an interface names and each conforming type chooses once, such as
`type Item` in `interface Source`. Outside the interface it is written
through the type: `S.Item`. Inside a conformance, the choice is written
`Item`.

**`extend`**:
A block that adds methods to a type declared elsewhere (`extend T { ... }`),
or one conformance (`extend T: I { ... }`). Only the prelude may write a
plain `extend` of a built-in type. From milestone 11, a block without an
interface lives in its type's module. A program may then conform a built-in
type to an interface when it declares the interface or one of its
arguments.

**Interface value**:
A value of type `any I`: a value of any type that conforms to `I`, whose
methods are called through a table.
_Avoid_: trait object, existential

**`AnyObject`**:
The interface every class conforms to and nothing else can. A bound
`T: AnyObject` lets generic code use `is` and `weak` on a `T`. Not to be
confused with **Object**, one heap value of a class.

**Class-only interface**:
An interface that inherits `AnyObject`, so that only classes conform to it
and its interface values support `is` and `weak`.

## Functions and closures

Milestone 9 adds these.

**Function type**:
The type of a function value, written `(i64, i64) -> bool`: the parameter
types in order, then the return type. It has no labels.
_Avoid_: signature (a declaration's), delegate

**Function value**:
A value of a function type: a named function used without a call, or a
closure. It can be stored, passed, returned and called.
_Avoid_: function pointer, callable

**Closure**:
A function written as an expression, such as `x => x + n`, that can use the
locals around it. It has no name.
_Avoid_: lambda, anonymous function, block

**Capture**:
An outer local, parameter or `self` that a closure's body names. A plain
capture is a copy made when the closure is created, and the closure cannot
change it. A **mutable capture** shares the local instead.
_Avoid_: upvalue, closed-over variable

**Mutable capture**:
A capture written `[mut name]` that shares a `let mut` local with the
closure, in place of a copy. A change made by the closure or by the
function is seen by both. The local lives in a counted cell on the heap.
A closure can reach it, so the rules treat it as a field of a class
object.

**Weak capture**:
A capture that does not keep its object alive. Inside the closure the name is
an optional, which reads `None` once the object has been dropped.

## Iterators

Milestone 9 adds these.

**Iterator**:
A value that conforms to `Iterator` and gives its elements one at a time
from `next`, until `next` returns `None`.
_Avoid_: generator, enumerator, cursor

**Iterable**:
A value that conforms to `Iterable`, so `for` can walk it: `iter()` gives
an iterator over it. Arrays, ranges and every iterator are iterable.
_Avoid_: sequence, collection

**Adapter**:
An iterator that wraps another and changes what it gives, such as the
result of `map` or `filter`.
_Avoid_: combinator

## Results

Milestone 12 adds these.

**Result**:
A value of type `T | E`, short for `Result<T, E>`: a success holding a `T` or
a failure holding an `E`.
_Avoid_: union, either

**Success**:
The `Ok` side of a result. A plain value converts to a success where a
result is expected.

**Failure**:
The `Err` side of a result, made by `fail`. A failure passes up with `?`.
_Avoid_: exception, error (the error is the value a failure holds)

**Error type**:
The type on the right of `|`: a type that conforms to `Error`, such as
`IoError` or `string`.
_Avoid_: failure type

**Unused result**:
An unused value whose type is a result. Unlike other unused values it is
an error, not a warning; `let _ = e` ignores it on purpose.

## Modules

Milestone 11 adds these.

**Module**:
One `.sisu` file. Its path from the root names it: `report/table.sisu` is
`report.table`.
_Avoid_: "module" alone for the compiler's unit of LLVM code; say "LLVM
module"

**Package**:
A folder of modules, whose front door is its `pkg.sisu`.
_Avoid_: crate, for a folder

**Front door**:
The file `pkg.sisu` of a package: `import cli` opens `cli/pkg.sisu`.

**Project**:
A root folder with a `sisu.toml`: the unit that other code depends on.
Until projects land, every program is one project without a manifest.
_Avoid_: crate, package, for a root with `sisu.toml`

**Root**:
The folder that import paths start from: the entry file's folder, or the
folder of the project's `sisu.toml`.

**Entry file**:
The file that `sisu run`, `sisu build` or `sisu check` gets. Only its `main`
runs.

**Import**:
A line that binds a module, or `pub` items of a module, in a file:
`import report.table` binds `table`.

**`pub`**:
The mark that makes an item, a field or a method visible to other modules.
Without it, a name is private to its module.
_Avoid_: export, public

## Compiler

**Typed tree (`tir`)**:
The program as the checker understood it: names resolved, every expression
typed, sugar such as `while` and `?.` lowered. mir is lowered from this.
_Avoid_: IR (that is LLVM's), HIR

**mir**:
The mid-level IR between the typed tree and LLVM IR: a control-flow graph of
**Basic blocks** whose statements hold every count (copy, release, move).
Codegen lowers it as written. `--emit mir` prints it.
_Avoid_: MIR (write it in lower case, like `tir`), IR (that is LLVM's)

**Basic block**:
A run of mir statements with one entry and one terminator (a jump, a branch,
a return) that says where control goes next.
_Avoid_: node, label

**Certifier**:
The check, run on every function in debug builds of `sisu`, that mir's
counts are sound: each owned reference is consumed exactly once, no empty
local is read, no borrow outlives its owner, and nothing writes or changes what
a borrow reads.
_Avoid_: verifier, linter

**Scope release**:
A release of a local at the end of its scope, made only if the local still
owns its reference.
_Avoid_: drop (that is freeing an object)

**Move**:
How mir hands over a **Last use**: a consuming read that leaves the local
empty.
_Avoid_: transfer

**Reuse token**:
The block of a box that a `match` took apart while it held the box's only
reference. The arm builds a new box of the same enum into it, or frees it.
_Avoid_: reset, hole

**Borrowed read**:
A read of a local that uses its value without a reference of its own,
because nothing can release or replace the local before the use ends: the
length of an array, a field of an object, an element or a field while
nothing calls a function, writes a counted value or makes an array unique. It
costs no retain and no release.
_Avoid_: borrow (Rust's checked borrow, which Sisu does not have)

**Borrowed parameter**:
A counted parameter that the function only reads, never keeps: the caller
holds its reference for the whole call, and neither side counts it. The
compiler decides this for each parameter; one that the function stores,
returns or hands on to a function that keeps it is owned instead.
_Avoid_: reference parameter, `&` parameter

**Symbol**:
A name as the parser and checker hold it: a small integer that stands for the
name's text. The lexer interns each name, so equal names get the same symbol.
_Avoid_: atom, interned string

**Poison type**:
The type the checker gives an expression that already failed a rule, so that
one mistake produces one diagnostic.
_Avoid_: error type (ambiguous with diagnostics)

**Diagnostic**:
An error or a warning the compiler reports about the source. An error stops
the build; a warning does not.
_Avoid_: report, message

**Diagnostic code**:
The stable name of one kind of diagnostic, such as `E0007` or `W0002`. A code
is never reused, and the same code always reads the same way.
_Avoid_: error code (warnings have codes too)

**Help**:
Advice in a diagnostic that cannot be written as an edit, such as "optionals
do not nest".

**Fix**:
An edit to the source, with a message, that resolves a diagnostic. A fix is
either safe or unsafe: a safe fix always removes the diagnostic and leaves
source that lexes and parses, and a tool may apply it without asking. An
unsafe fix is applied only when the user picks it.
_Avoid_: suggestion, quick fix, guess, certain fix

**Explanation**:
The long-form page for one diagnostic code: the rule, why Sisu has it, and a
broken and a fixed example.
_Avoid_: error index entry

**Broken item**:
A function, class or method that contains a syntax error. Its body is not
checked, and a use of it is never reported as unknown or mistyped.

**Cascade**:
A diagnostic that exists only because of another one. Sisu never reports
one.
_Avoid_: follow-on error, knock-on error

**Foreign idiom**:
A habit from another language, such as `var` or `len(xs)`, that has its own
diagnostic code naming the Sisu way.

**Generator rule**:
The part of the program fuzzer that writes one kind of syntax, type or built-in into generated programs. A language feature is done only when it has one.
_Avoid_: fuzz rule, grammar rule

**Reference count**:
The number of references to an object; the object is dropped when it reaches
0.
_Avoid_: refcount, retain count

**Weak reference**:
A reference, held in a `weak` field, that does not keep its object alive and
reads as `None` once the object has been dropped.
_Avoid_: unowned

**Counted type**:
A type whose values hold counted parts: classes, arrays, strings and boxes,
an enum with a variant that holds one, and optionals of these. Copying such a
value retains each counted part. `i64`, `f64`, `u8`, `bool`, `char` and enums
whose variants hold none are copied whole and are not counted.
_Avoid_: reference type, heap type

**Box**:
The counted heap block the compiler puts behind a payload field that closes
a recursion cycle, such as each `Expr` in `Add(Expr, Expr)`. A program never
names one.
_Avoid_: indirection, `Box<T>`, pointer

**Owned reference**:
A reference that its holder must release exactly once. Every expression of a
counted type yields one; for an enum value, that is one reference to each of
its counted parts.

**Inline program**:
A whole program given on the command line with `-e` in place of a file.
Its diagnostics and panics name the source `<inline>`.
_Avoid_: snippet, one-liner, script
