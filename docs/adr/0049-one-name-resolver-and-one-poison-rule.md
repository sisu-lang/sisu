# One name resolver and one poison rule

Status: accepted, 2026-11-08. Parts 3 to 6 of the diagnostics foundation build
it. It amends [ADR 0013](0013-diagnostics-have-stable-codes.md).

The compiler had 158 issues with the `diagnostic` label in 15 days. Most came
from shared causes. Seven places decide what a name means, each one alone. A
flag on `Poisoned` passes down by hand. An error has no "why", because the check
that finds a mismatch does not know where the expected type came from.

The decision has four parts:

- **One name resolver.** One function in the checker, in its `names` module,
  decides what a name means at each position. It owns the maps from a name to a
  meaning. One function chooses the diagnostic for a name of the wrong kind.
- **The poisoned-item rules.** An alias whose target fails for any reason is
  poisoned. A top-level name defined twice is poisoned. A language interface's
  name stays the language's at a bound, a conformance, an inheritance list and
  `any`.
- **The poison rule.** A poisoned value is typed when the source states its type
  apart from the poisoned part, and untyped otherwise, with nothing in between.
  A second mistake in one expression can wait for the next run.
- **`Reported`.** It is a zero-size token. Only the function that adds an error
  and the function `proof` make one. A poisoned value and a poisoned declaration
  hold one, so a rejection has a message. `proof` is a promise that an error
  exists by the end of the check, as rustc's `delayed_bug` is. A debug build
  checks it. The token does not prevent a second report. Its known
  limit: a rollback can drop the report that made a token.

As built, `Reported` and `Diagnostics` are in `check/reports.rs`. A `Poisoned`
holds a `Reported`, and so does `Declared::Poisoned`, the state of an item that
is poisoned. `Reported` is `Copy`, so a copy proves the same report. Each call of
`proof` has a tag that names why no token was in scope: `quiet reader`,
`declaration`, `binding`, `unknown state`, `parser mark` or `held misfit`. The
flag on `Poisoned` is gone: a poisoned value is typed or untyped by its form.

`Poisoned::new`, `typed` and `untyped` each take a token, so no poisoned value
exists without one. `Diagnostics` has no `DerefMut`: code reads the list, and
`rewrite` and `strip_fixes_since` change a report in place. A rollback (`mark`,
`drop_since`, `take_since` with `Held`, `restore` and `insert_at`) cuts and puts
back the promises with the reports. `promises_kept(broken)` is true when no
promise exists, the list holds an error, or the parser marked one. `check`
asserts it with `debug_assert!` before `finish`.

## Considered options

- **A separate resolve stage, before the checker**: rejected. A name's meaning
  will depend on a type. `docs/register.md` says a bare `V` means `E.V` when the
  expected type is `E`. The modules spec (option B) puts one resolver in the
  checker.
- **A poison type in `types.rs`, as rustc's `Ty::new_error`**: rejected.
  `Err(Poisoned)` keeps a poisoned value out of `tir` by its type. A poison type
  would be a value that every reader of a type must handle.
- **No token, only the `debug_assert!` at the end of the check**: rejected. It
  sees only the whole program. It cannot tell which check rejected with no
  message.
- **A rule that drops a later error by its text, as Go does
  (`go/types/errors.go`)**: rejected. It hides a true error when its text holds
  "invalid operand" or "invalid type".

## Consequences

- The register row on name maps goes.
- Milestone 11 adds modules to the resolver.
- Structs part 1 registers `struct_ids` through the resolver.
- A true error can wait for the next run, and a fixture pins each such case.
