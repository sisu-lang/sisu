# A failure is written `T | E`

Status: accepted, 2026-11-14.

The spec of milestone 12 (error handling) makes these choices. They are
recorded here because later tasks and later specs cite them.

## Decision

- **The `|` spelling is `Result`.** `T | E` is shorthand for the prelude's
  `Result<T, E>`, as `T?` is for `Option<T>`. Both spellings are the same type.
  Error messages print the `|` form. The left side is the success and the
  right side is the failure.
- **`fail` makes a failure.** `fail e` returns the failure `e` from the
  function.
- **`Result` itself needs `Error`.** The prelude declares
  `enum Result<T, E: Error>`, so `Result<i64, bool>` is an error, and so is
  `-> i64 | bool`. `string` conforms to `Error` in the prelude, so a small
  program can write `-> i64 | string` and `fail "no input"`.
- **There are no general unions.** `|` is only the spelling of `Result`.
- **An unused result is an error.** An unused value of type `T | E` is a
  compile error, where W0004 would warn about an unused value. `let _ = e`
  ignores it on purpose.
- **`?` works on `T | E` and on `T?`, and on no other type.** It works on
  both: it passes a failure or a `None` up to the caller.
- **`unit` is an ordinary type.** It can be a type argument anywhere. It
  prints as `()`, and it has `Eq` and `Ord`, under which all `unit` values are
  equal.
- **`main` can fail.** A `main` that returns `unit | E` and fails prints the
  error and exits with code 1.
- **A file that is not UTF-8 fails.** The read returns a failure. It does not
  repair the bytes.
- **`?` and `fail` convert a failure by one set of rules.** The first rule
  that applies is used: the failure type is already the one that the function
  returns; the function returns `AnyError`; or the failure type conforms to
  `Convert<F>`.
- **`AnyError` is the one spelling of the catch-all.** It is a struct that
  holds any error, for a failure that no caller matches. `any Error` on the
  right of `|` is an error whose fix writes `AnyError`.
- **`Error` has `source`, with a default in `lang.sisu`.** `source` gives the
  error that caused this error, or `None`. A conformance that does not write
  it gets the default, which returns `None`. A default body is legal only in
  `lang.sisu`.
- **`@from` writes a `Convert` conformance.** On an enum variant with one
  payload, it lets `?` and `fail` convert the payload's type to the enum. The
  attributes design (ADR 0041) is widened so that a variant can take an
  attribute.
- **A failing `main` can print an error return trace.** Each failure records
  the sites that it passed through, on Zig's model. The trace is printed when
  the environment variable `SISU_BACKTRACE` is set to a value other than `0`
  or the empty string.

E0098 retires under ADR 0013. Its `unit` form goes, because `unit` prints as
`()`, and its form for a missing value goes too, so it cannot fire. The `unit`
forms of E0059 and E0138 go as well. The change to ADR 0036 is in its note.

## Considered options

- **General unions, such as `i64 | bool`**: rejected. The right side must be an
  error type, so `|` is only the spelling of `Result`.

## Consequences

- A user program cannot declare its own top-level `Error`, `Convert` or
  `AnyError` until modules exist (milestone 11). The declaration is E0026.
- `unit` can be a type argument, so a generic helper over `T | E` works with a
  `unit | E` function.
- ADR 0036, ADR 0040, ADR 0018, ADR 0009 and ADR 0015 each have a note that
  says what changes for them.

## Reconsider when

- A function needs a second error type in one signature.
- `?` on a user type is wanted.
- A program needs to downcast an `AnyError`.
- Concurrency comes. The error return trace then needs one trace for each
  thread.
