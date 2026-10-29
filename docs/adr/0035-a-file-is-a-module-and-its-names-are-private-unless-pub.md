# A file is a module, and its names are private unless `pub`

Status: accepted when milestone 11 part 1 merges. It resolves "Names and
namespaces" in the roadmap's "Open decisions" for modules, and it builds on
[ADR 0015](0015-the-prelude-holds-only-what-almost-every-program-uses.md)
(the prelude), [ADR 0019](0019-enums-are-values-named-through-their-enum.md)
(names through `.`) and
[ADR 0023](0023-generics-are-monomorphized-and-checked-at-the-definition.md)
(a generic body is checked where it is defined).

A Sisu program can be more than one file. The file tree is the module tree,
and each name is private to its file unless it is `pub`.

- **Vocabulary.** A *module* is one `.sisu` file. A *package* is a folder of
  modules. A *project* is a root folder with a `sisu.toml`; projects come in
  a later milestone, and until then every program is one project without a
  manifest. Other code depends on one concept: the project.
- **The path names the module.** `report/table.sisu` is the module
  `report.table`. No `mod` declaration is needed, as in Python and Go and
  unlike Rust. A module name is lowercase ASCII (`[a-z_][a-z0-9_]*`) and not a
  keyword, and an import must match the file name byte for byte, so a build
  is the same on Linux and on macOS.
- **A package's front door is `pkg.sisu`.** `cli/pkg.sisu` is the module
  `cli`, and `cli/domain.sisu` is `cli.domain`. A folder without `pkg.sisu`
  is not a module, but its files are. Only the file name `pkg.sisu` is
  reserved.
- **An import binds what it shows.** `import report.table` binds `table`.
  `import report.table.{Table, render}` binds those `pub` items. `as` renames.
  The braces name items only, never a submodule. There is no `import *`, and
  a submodule is reached only through its own import, so a reader sees where
  each name comes from at the top of the file.
- **The root is the entry file's folder.** The program is the entry file and
  each file that its imports reach. A file that nothing imports is not read,
  so a half-written file next to a script does not break the script. An
  import cannot go above the root, and the root `std` always means the
  standard library. Only the entry file's `main` runs.
- **Private by default, `pub` per member.** Each top-level item, each field
  and each method is private to its module unless it is `pub`. A `pub mut`
  field may also be assigned from other modules. Enum variants follow their
  enum, and interface requirements follow their interface. A class or record
  with a private field can be built and taken apart only in its own module,
  which gives opaque types. A `pub` item must not name a private type.
- **A conformance has one home per project.** `extend T: I<A>` is allowed
  only in the project that declares `T`, `I` or a type in `A`, with Rust's
  order rule for type parameters. So two independent projects cannot both
  conform one type to one interface. The rule covers the built-in types:
  when milestone 10 gives operators their interfaces, `extend f64:
  Mul<Vec2>` is allowed in the project that declares `Vec2` (decided
  2026-10-29). A plain `extend T { ... }`, and a
  conformance that replaces a generated one, must be in `T`'s module.
- **One checker sees the whole program.** A loader parses each reached file
  and builds the module graph. Each module has one scope, its own items and
  its imports, and every name lookup goes through it, with the `pub` check
  as part of the lookup. Imports may form cycles. Codegen still makes one
  LLVM module, and each generated name carries its module path.

## Considered options

- **Rust's `mod` declarations.** A parent file must list its children, so
  adding a file takes two edits, and a file that is not listed is silently
  left out. Rejected: the file tree already says it.
- **Go's packages, where a folder is the unit.** Files in one folder see
  each other's names without an import, so a file cannot be read on its own.
  Rejected: a reader must see where each name comes from.
- **A front door next to its folder (`cli.sisu` beside `cli/`), as Rust 2018
  does.** One package is then spread over two places. Rejected for
  `pkg.sisu` inside the folder; `cli.sisu` beside `cli/pkg.sisu` is an error
  when `import cli` reads it.
- **Public by default, as in Python.** Every helper becomes part of the
  module's interface, and an agent's change inside a module can break code
  outside it. Rejected: ADR 0027 asks for code that is safe to accept.
- **Rename the names and merge the files before the checker.** It takes less
  work to build first, but it makes a second resolver that must agree with the
  checker, and the checker loses each item's module. Rejected for module
  scopes in the checker.
- **Check each module against the `pub` signatures of the others.**
  Rejected: imports may form cycles, and inferred ownership
  ([ADR 0029](0029-parameters-are-borrowed-unless-consumed.md)) needs the
  whole program.
- **A conformance anywhere, as Swift allows.** Two libraries can then
  conform one type to one interface, and the program that uses both cannot
  build. Rejected for Rust's orphan rule, at the project level.

## Prior art

Python and Go name a module by its path; Python's `__init__.py` and Rust's
`mod.rs` are the front doors that `pkg.sisu` replaces. Rust's `pub` per item
and field and its orphan rule are the model for visibility and
conformances. rustc's `SourceMap` gives each file a base offset in one
offset space.

## Consequences

- `Span` stays two offsets: each file gets a base offset, and a source map
  gives the file of an offset.
- The checker's flat tables keyed by name become per-module scopes keyed by
  item id. This touches most of the checker and the monomorphizer.
- Diagnostics write a type's short name, with its module path only when two
  types of one name meet in one message. Panic messages write the path from
  the root, so a binary holds no path of the build machine. A class's
  generated `Display` prints its short name, so moving a file never changes
  what a program prints. A record's generated `Display` prints its `pub`
  fields, then one `..` when it has a private field, so `print` shows no
  private field
  (decided 2026-10-29).
- `sisu check` of an entry file with no `main` checks it as a library, with
  no warning for an unused `pub` item (decided 2026-10-29).
- An unused import gets a warning with a safe fix that removes it.
- Re-exports, project-level visibility, separate compilation, `sisu.toml`
  and dependencies come later. Each one can be added without breaking a
  program.
