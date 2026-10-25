# Modules

A program can be more than one file. Each file is a *module*, and its names are private to it unless it marks them `pub`. Other modules use them through `import`.

```sisu pending
// report/table.sisu
pub class Table {
    pub title: string
    mut rows: [string]

    pub fn add(self, row: string) {
        self.rows.push(row)
    }

    pub fn size(self) -> i64 {
        self.rows.len()
    }
}

pub fn titled(title: string) -> Table {
    Table(title: title, rows: [])
}
```

```sisu pending
// main.sisu
import report.table

fn main() {
    let t = table.titled("Saunas")
    t.add("Löyly")
    print(t.title)                     // Saunas
    print(t.size())                    // 1
}
```

The examples on this page have more than one file. Each block is one file, and its first line names the file. `sisu run main.sisu` runs the program.

## Modules and packages

A module's path from the root names it. The file `report/table.sisu` is the module `report.table`. No declaration lists the modules: the file tree is the module tree.

A folder is a *package*. Its *front door* is the file `pkg.sisu` in that folder: `cli/pkg.sisu` is the module `cli`, and `cli/domain.sisu` is the module `cli.domain`.

```sisu pending
// cli/pkg.sisu
pub fn run() {
    print("run")
}
```

```sisu pending
// cli/domain.sisu
pub fn parse() {
    print("parse")
}
```

```sisu pending
// main.sisu
import cli
import cli.domain

fn main() {
    cli.run()                          // run
    domain.parse()                     // parse
}
```

The rules:

- A module name is lowercase ASCII: a letter or `_`, then letters, digits and `_`. It is not a keyword. A file with another name, such as `My-Tool.sisu`, can be an entry file, but never a module.
- An import must match the file name byte for byte, also on a file system that ignores case. `import report` does not find `Report.sisu`. Two names in one folder that differ only in case are an error when an import names either one.
- A folder without `pkg.sisu` is not a module, but its files are: `import cli.domain` works without `cli/pkg.sisu`, and `import cli` is an error that lists the modules in `cli/`.
- `import cli.pkg` is an error, because `cli/pkg.sisu` is the module `cli`. A real folder named `pkg` is an ordinary package: `cli/pkg/pkg.sisu` is the module `cli.pkg`. Only the file name `pkg.sisu` is reserved; `pkg` is an ordinary name.
- `cli.sisu` beside `cli/pkg.sisu` makes `import cli` an error that names both files.

## The root and the program

The *root* is the folder of the *entry file*, the file that `sisu run`, `sisu build` or `sisu check` gets. Import paths start from the root. `sisu build dir/` builds `dir/main.sisu`, and with `-e` the root is the current folder.

The program is the entry file and each file that its imports reach. `sisu` reads no other file, so a half-written file next to a script does not break it.

- An import cannot go above the root. Import paths have no `..`.
- A symbolic link is followed before the check. A link that points out of the root is an error.
- One file that two import paths reach is one module.
- The first segment `std` always names the standard library, never a file or folder in the root.

## Imports

```sisu pending
// main.sisu
import report.table                       // binds `table`
import report.table.{Table, titled}       // binds `Table` and `titled`
import report.chart as graphs             // binds `graphs`
import report.table.{Table as Grid}       // binds `Grid`

fn main() {
    let t: Table = titled("Saunas")
    print(table.titled("Lakes").title)    // Lakes
}
```

- `import a.b` binds the last segment, `b`. Code then writes `b.name`.
- `import a.b.{X, Y}` binds the `pub` items `X` and `Y` of `a.b` without a prefix. The braces name items only, never a module: `import report.{table}` is an error when `report/pkg.sisu` has no item named `table`, and the fix writes `import report.table`.
- `as` renames a module or an item.
- A submodule needs its own import. After `import cli`, the name `cli.domain.parse` is an error; write `import cli.domain`, then `domain.parse()`.
- Imports come before the first item of a file. A module cannot import itself, but modules can import each other in a cycle.
- One name is bound once. Two imports that bind one name, an import that binds a name that the module declares, and an import that binds a prelude name such as `print` are errors ([E0034](../diagnostics/E0034.md)). A local, parameter or loop variable cannot take the name of an imported module. `as` gives an import another name.
- An import that nothing uses gets a warning, and its fix removes it. An import counts as used when the module it reaches declares a conformance that the program uses.
- The prelude is visible in every file without an import ([Option and Result](types.md#option-and-result)).

## Visibility

Each top-level item is private to its module unless it is `pub`: a function, class, enum, constant, interface, type alias or record. In its own module, every name is visible.

Each field and each method needs its own `pub`. A `pub` field can be read in other modules. A `pub mut` field can also be assigned there.

```sisu pending
// counter.sisu
pub class Counter {
    pub mut label: string
    pub start: i64
    mut count: i64

    pub fn bump(self) {
        self.count += 1
    }

    pub fn value(self) -> i64 {
        self.count
    }
}

pub fn counter(label: string) -> Counter {
    Counter(label: label, start: 0, count: 0)
}
```

```sisu pending
// main.sisu
import counter

fn main() {
    let c = counter.counter("visits")
    c.bump()
    c.label = "guests"
    print(c.label)                     // guests
    print(c.value())                   // 1
}
```

In `main.sisu`, `c.count = 5` and `print(c.count)` are errors, because `count` is private. `c.start = 1` is an error too: `start` is `pub`, not `pub mut`. `counter.Counter(label: "x", start: 0, count: 0)` is also an error, because `Counter` has a private field. Other modules make a `Counter` through `counter.counter`.

For a field that other modules read but only its module changes, write a private `mut` field and a `pub` method that returns it, as `value` does above.

The rules:

- Enum variants and their payloads are as visible as their enum. Interface requirements are as visible as their interface. `pub` on one of them is an error.
- A conformance has no `pub`. Its methods are as visible as the interface.
- A class or record with a private field cannot be built in another module, also when the field has a default value. Another module also cannot change a copy of it with a record update, or take it apart in a pattern. A pattern that binds the whole value works. So the module decides which values exist: an *opaque type*.
- A `pub` item cannot name a private type in its signature: a parameter, a result, a `pub` field, a payload of a `pub` enum, a type alias, a bound or an associated type. A private field can have a private type.
- A method can have the name of a private field when the field's type is not a function type.
- A `pub` item that nothing in the program uses gets the unused-name warning, as a private one does.

## Conformances and `extend`

A conformance `extend T: I<A>` is allowed in the project that declares `T`, `I` or a type in `A`. So two libraries can never both conform one type to one interface ([Generics](generics.md#interfaces)).

```sisu pending
// json/pkg.sisu
pub interface ToJson {
    fn to_json(self) -> string
}
```

```sisu pending
// model.sisu
pub class User {
    pub name: string
}
```

```sisu pending
// output/user_json.sisu
import json.{ToJson}
import model.{User}

extend User: ToJson {
    fn to_json(self) -> string {
        "name: {self.name}"
    }
}
```

- In a program without a `sisu.toml`, every file is in one project. So the rule rejects only a conformance where the type, the interface and every argument come from the prelude or the standard library: `extend IoError: Display` is an error.
- A conformance can be in any module of its project. A call finds its methods through the value's type when the conformance's file is part of the program, with no import of the interface.
- A plain `extend T { ... }`, with methods and no interface, must be in `T`'s module, so a type's own methods are always found with the type.
- A conformance that replaces a generated one, such as a hand-written `Display` or `Eq`, must be in its type's module. Otherwise `a == b` could give another answer when the file is not part of the program.

## `main` in a module

Only the entry file's `main` runs. A `main` in another module is an ordinary function, so one file can be a script and a module. Another module can import the entry file.

## Names in messages

A message from `sisu` writes a type's short name, such as `Counter`. When two types of one name meet in one message, both get their module path: `a.Counter` and `b.Counter`. A class's generated text is its short name, so moving a file never changes what a program prints. A panic message writes the file's path from the root: `report/table.sisu:3:5`.
