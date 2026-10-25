# Modules

A program can be more than one file. Each file is a *module*, and its names are private to it unless it marks them `pub`. Other modules use them through `import`.

The examples on this page have more than one file. Each block is one file, and its first line names the file. `sisu run main.sisu` runs the program.

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

## Modules and packages

A module's path from the [root](#the-root-and-the-program) names it. The file `report/table.sisu` is the module `report.table`. No declaration lists the modules: the file tree is the module tree.

A folder is a *package*. Its *front door* is the file `pkg.sisu` in that folder: `cli/pkg.sisu` is the module `cli`, and `cli/flags.sisu` is the module `cli.flags`.

```sisu pending
// cli/pkg.sisu
pub fn usage() -> string {
    "usage: tally [--verbose] FILE"
}
```

```sisu pending
// cli/flags.sisu
pub fn is_verbose(arg: string) -> bool {
    arg == "--verbose" || arg == "-v"
}
```

```sisu pending
// main.sisu
import cli
import cli.flags

fn main() {
    print(cli.usage())                 // usage: tally [--verbose] FILE
    print(flags.is_verbose("-v"))      // true
}
```

The rules:

- A module name is lowercase ASCII: a letter or `_`, then letters, digits and `_`. It is not a keyword. A file with another name, such as `My-Tool.sisu`, can be an entry file, but never a module.
- An import must match the file name byte for byte, also on a file system that ignores case. `import report` does not find `Report.sisu`. Two names in one folder that differ only in case are an error when an import names either one.
- A folder without `pkg.sisu` is not a module, but its files are: `import cli.flags` works without `cli/pkg.sisu`, and `import cli` is an error that lists the modules in `cli/`.
- `import cli.pkg` is an error, because `cli/pkg.sisu` is the module `cli`. Only the file name `pkg.sisu` is reserved: a folder named `pkg` is an ordinary package, so `cli/pkg/pkg.sisu` is the module `cli.pkg`. When both `cli/pkg.sisu` and `cli/pkg/` exist, `import cli.pkg` is an error that names both.
- `cli.sisu` beside `cli/pkg.sisu` makes `import cli` an error that names both files.
- A `pkg.sisu` in the root has no module name, and no import reaches it.

## The root and the program

The *root* is the folder of the *entry file*, the file that `sisu run`, `sisu build` or `sisu check` gets. Import paths start from the root. `sisu build dir/` builds `dir/main.sisu`, and with `-e` the root is the current folder.

The program is the entry file and each file that its imports reach. `sisu` reads no other file, so a half-written file next to a script does not break it.

- An import cannot go above the root. Import paths have no `..`.
- `sisu` replaces a symbolic link with its real path. A link whose real path is outside the root is an error. When the entry file is a link, the root is the real file's folder.
- One file that two import paths reach is one module.
- The first segment `std` always names the standard library, never a file or folder in the root. Until the standard library comes, `import std.os` is an error: "the standard library has no module `os`".

Only the entry file's `main` runs. A `main` in another module is an ordinary function, so one file can be a script and a module. Another module can import the entry file.

## Imports

An `import` line names a module by its path and binds names in this file.

```sisu pending
// main.sisu
import report.table                          // binds `table`
import report.table.{titled, Table as Grid}  // binds `titled` and `Grid`

fn main() {
    let a: Grid = titled("Saunas")
    let b = table.titled("Lakes")
    print("{a.title} and {b.title}")         // Saunas and Lakes
}
```

- `import a.b` binds the last segment, `b`. Code then writes `b.X` for an item `X` of `a.b`.
- `import a.b.{X, Y}` binds the `pub` items `X` and `Y` of `a.b` without a prefix. The braces name items only, never a module: `import report.{table}` is an error when `report/pkg.sisu` has no item named `table`, and the fix writes `import report.table`.
- `as` renames a module or an item: `import report.table as tables` binds `tables`.
- A submodule needs its own import. After `import cli`, the name `cli.flags.is_verbose` is an error; write `import cli.flags`, then `flags.is_verbose(arg)`.
- Imports come before the first item of a file. A module cannot import itself, but modules can import each other in a cycle.
- At the top level of a module, a name binds one thing. Two imports that bind one name, an import that binds a name that the module declares, and an import that binds a prelude name such as `print` are errors ([E0034](../diagnostics/E0034.md)). `as` gives an import another name.
- A local, parameter or loop variable cannot take the name of an imported module. It can take the name of an imported item, as it can take the name of a function ([Shadowing](statements.md#shadowing)).
- An import that nothing uses gets a warning, and its fix removes it. An import counts as used when the code names it, or when the module it imports reaches, through its own imports, a module with a conformance that the program uses.
- The prelude is visible in every file without an import ([Option and Result](types.md#option-and-result)).
- `pub import`, `pub(...)` and a `---` fence at the top of a file are reserved. Each is an error that says the feature is not in Sisu yet.

## Visibility

Each top-level item is private to its module unless it is `pub`: a function, class, enum, constant, interface, type alias or record. In its own module, every name is visible.

Each field and each method needs its own `pub`. A `pub` field can be read in other modules. A `pub mut` field can also be assigned there.

```sisu pending
// bank.sisu
pub class Account {
    pub owner: string
    pub mut note: string
    mut balance: i64

    pub fn deposit(self, amount: i64) {
        self.balance += amount
    }

    pub fn balance(self) -> i64 {
        self.balance
    }
}

pub fn open(owner: string) -> Account {
    Account(owner: owner, note: "", balance: 0)
}
```

```sisu pending
// main.sisu
import bank

fn main() {
    let a = bank.open("Eero")
    a.deposit(10)
    a.note = "savings"
    print("{a.owner}: {a.note}")       // Eero: savings
    print(a.balance())                 // 10
}
```

In `main.sisu`, `a.balance = 5` is an error, because the field `balance` is private. `a.owner = "Aino"` is an error too: `owner` is `pub`, not `pub mut`. `bank.Account(owner: "Aino", note: "", balance: 0)` is also an error, because `Account` has a private field. Other modules make an `Account` through `bank.open`.

For a field that other modules read but only its module changes, write a private `mut` field and a `pub` method that returns it, as `balance` does above.

The rules:

- Enum variants and their payloads are as visible as their enum. Interface requirements are as visible as their interface. `pub` on one of them is an error.
- A conformance has no `pub`. Its methods are as visible as the interface.
- A class or record with a private field cannot be built in another module. Another module also cannot change a copy of it with a record update, or take it apart in a pattern. A pattern that binds the whole value works. So the module decides which values exist: an *opaque type*.
- A `pub` item cannot name a private type in its signature: a parameter, a result, a `pub` field, a payload of a `pub` enum, a type alias, a bound or an associated type. A private field can have a private type. The arguments of a conformance of a `pub` type to a `pub` interface follow the same rule.
- A method can have the name of a private field when the field's type is not a function type, as `balance` does above.
- A `pub` item that nothing in the program uses gets the unused-name warning ([W0012](../diagnostics/W0012.md)), as a private one does.

## Names reached through a type

A value's fields and methods are found through its type, with no import of the type's module.

```sisu pending
// teller.sisu
import bank

pub fn open_savings(owner: string) -> bank.Account {
    let a = bank.open(owner)
    a.note = "savings"
    a
}
```

```sisu pending
// main.sisu
import teller

fn main() {
    let a = teller.open_savings("Eero")
    a.deposit(25)
    print(a.balance())                 // 25
}
```

`main.sisu` does not import `bank`, but it calls the `pub` methods of `bank.Account`.

- A generic's body keeps the privacy of its own module, in every module that uses it ([Generics](generics.md)).
- A `pub const` can use a `pub const` of another module. In one module, a constant still uses only the constants above it. Constants of several modules that use each other are an error ([E0087](../diagnostics/E0087.md)).

## Conformances and `extend`

A conformance `extend T: I<A>` is allowed in the project that declares `T`, `I` or a type in `A`. So two projects can never both conform one type to one interface with the same arguments ([Generics](generics.md#interfaces)).

```sisu pending
// csv/pkg.sisu
pub interface CsvRow {
    fn csv_row(self) -> string
}
```

```sisu pending
// model.sisu
pub class User {
    pub name: string
    pub email: string
}
```

```sisu pending
// output/user_csv.sisu
import csv.{CsvRow}
import model.{User}

extend User: CsvRow {
    fn csv_row(self) -> string {
        "{self.name},{self.email}"
    }
}
```

```sisu pending
// main.sisu
import model.{User}
import output.user_csv                 // used: it adds User: CsvRow

fn main() {
    let u = User(name: "Aino", email: "aino@example.com")
    print(u.csv_row())                 // Aino,aino@example.com
}
```

- The rule looks only at the outermost type of `T`, of `I` and of each argument in `A`. `[User]` and `User?` do not count as types of the program, and neither do the type arguments of `T`: `extend Result<User, MyErr>: Display` is an error.
- In a program without a `sisu.toml`, every file is in one project. So the rule rejects only a conformance where the type, the interface and every argument come from the prelude or the standard library: `extend IoError: Display` is an error, and `extend IoError: Convert<TotalsError>` is allowed.
- A conformance can be in any module of its project. A call finds its methods through the value's type when the conformance's file is part of the program, with no import of the interface. Above, `main.sisu` imports `output.user_csv` only to make that file part of the program.
- A plain `extend T { ... }`, with methods and no interface, must be in `T`'s module, so a type's own methods are always found with the type.
- A conformance that replaces a generated one, such as a hand-written `Display` or `Eq`, must be in its type's module. Otherwise `a == b` could give another answer when the file is not part of the program.

## Names in messages

A message from `sisu` writes a type's short name, such as `Account`. When two types of one name meet in one message, both get their module path: `a.Account` and `b.Account`. A panic message writes the file's path from the root: `report/table.sisu:3:5`.

A class's generated `Display` prints only its short name, so moving a file never changes what a program prints. A record's generated `Display` prints every field, private fields too. To hide a field, write the record's own `Display`.
