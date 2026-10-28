# The prelude holds only what almost every program uses

Status: accepted, 2026-10-13. It settles which names milestone 5 makes
visible in every file.

The prelude is the set of names visible in every file without an import. A
name belongs in it only if almost every program uses it, or the language
itself depends on it. Everything else goes in a module (milestone 11). Each
prelude name costs a name that user code cannot define, and a reader cannot
tell where a bare name comes from. The roadmap rejects `import *` for the
same reason.

- **The milestone 5 prelude is `print`, `eprint`, `read_line` and `panic`.**
  `todo` is a keyword, not a prelude name. Later milestones add `assert`.
  Milestone 8 adds `Option`, `Result`, `Some`, `None`, `Ok`, `Err`,
  `Display`, `Eq`, `Ord`, `AnyObject` and `dbg`. Most of them are declared in
  Sisu in the prelude file, `prelude.sisu` in `crates/sisuc/src/`, which the
  compiler builds into every program (`dbg` stays built in). The prelude takes
  `AnyObject` rather than `Object`, so that programs keep `Object` for their
  own classes. Reading a line stays in the prelude for good, as Python's
  `input()`, Swift's `readLine()` and Kotlin's `readln()` do.
- **A top-level function, class or constant may not take a prelude name.**
  It is the existing "defined twice" error with its "is a built-in function"
  message, as for `print` today. Milestone 4 already takes `print` from
  constants. The names of the four language interfaces (`Display`, `Eq`,
  `Ord` and `AnyObject`) get the "is a built-in interface" form of E0026
  instead, for an interface and a type parameter too.
- **A local, a parameter, a method or a field may take a prelude name.** A
  method is always called on a receiver, and a local is never called as a
  function, so neither hides the prelude name.
- **Six names are reserved before milestone 8 adds them: `Option`, `Result`,
  `Some`, `None`, `Ok` and `Err`.** No top-level item, variant, local,
  parameter or binding may take one; a method or a field may, except
  `None`, which is already a keyword and so names no method or field. This is
  stricter than the previous bullet for locals, parameters and bindings,
  because milestone 8 makes `Some(x)`, `None`, `Ok(x)` and `Err(e)` bare
  names in expressions and in patterns. A local named `Ok` would then turn
  `Ok(1)` from a constructor into a call or a pattern binding, so no
  program written now may hold a name that milestone 8 takes. A top-level
  item gets the existing "built-in" error; `Some`, `Ok` and `Err` get it with
  the message "is a built-in name".

## Considered options

- **Minimal built-ins (Go)**: Go has about 15 built-ins. Only `print` and
  `println` do I/O, they write to standard error, and they are meant for
  debugging. General-purpose I/O needs a package. The prelude stays small,
  but every program that prints to standard output must import a package
  first. Sisu's first programs read and write text, so the cost
  would fall on nearly every program.
- **The whole library (Swift)**: Swift exposes its whole standard library in
  every file. Nothing needs an import, but a reader must know the whole
  library to read any name in it, and cannot tell which module a name came
  from. Python's 70 built-ins sit
  between the two ends and have the same problem on a smaller scale.
- **A prelude that user code can shadow (Rust)**: Rust's prelude holds types
  and traits but no I/O functions, and user code may define a name that hides
  a prelude one. It leaves users free, but a reader of a call to `print`
  must check whether this file redefined it. Sisu does not take this option
  for top-level names.

## Consequences

- A program that defines a top-level `print`, `eprint`, `read_line` or
  `panic` is rejected, and so is a class or constant with one of those names.
  A program that defined such a name before milestone 5 must rename it.
- Adding a name to the prelude later can break a program that used it for a
  top-level name, so each addition must meet the rule above.
- A function that does not meet the rule goes in a module, such as `os` and
  `io` in milestone 11.
