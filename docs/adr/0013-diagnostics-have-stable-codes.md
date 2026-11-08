# Diagnostics have stable codes

Status: accepted, 2026-10-12. The diagnostics foundation lands before the rest
of milestone 4.

Every diagnostic the compiler reports follows four rules. They make the output
something a person, a script and an agent can all rely on.

- **Every diagnostic has a stable code.** Errors are `E0001`, `E0002`, and so
  on. Warnings are `W0001`, `W0002`, and so on. A number is given when its
  code lands on `main`, in landing order. A code is never reused or
  renumbered. A code the compiler no longer produces keeps its explanation,
  marked retired.
- **The message, labels, help and fixes live in one registry.** A call site
  names what happened and where, and passes the facts. It writes no text. The
  same code always has the same wording.
- **Every code has an explanation, and a test compiles it.** `sisuc --explain
  E0001` prints what the code means, an example that triggers it, and the
  fix. A test compiles each example and checks that it reports the code. A
  fixture per code pins the text and the `line:col`.
- **Diagnostics never cascade.** A broken item is reported once. Nothing that
  depends on it adds a second report: its uses, its calls and the checks that
  need it are skipped. The first diagnostic is the one to fix.

Programs read the same diagnostics as JSON, one object per line, with the
code, severity, spans and fixes. A fix is safe when applying
it always removes the diagnostic and leaves source that still lexes and
parses, and unsafe otherwise. A tool may apply a safe fix without asking, and
an unsafe fix only when the user picks it. A fix may reveal diagnostics it was
hiding.

## Considered options

- **Code ranges per compiler phase (`E01xx` lexer, `E02xx` parser, and so
  on)**: a code would say which phase found the problem. But a rule moves
  between phases, and a code must not change when it does. Two series in
  landing order never collide, even when two milestones are drafted at once.
- **Wording at the call site**: a call site can say exactly what it means.
  But two sites that report one rule drift apart, and nothing checks that
  every code has an explanation. In one registry, an exhaustive `match` ties
  each variant to its code, message and explanation, so a code missing any of
  them does not build.
- **Statement-level recovery inside a broken item**: it would report more
  than one syntax error in a function body. But every later error depends on
  how the parser guessed, and so can be false. We report one error per item
  instead. Recovery inside a body can come later without changing a code or
  the output format.

## Consequences

- All existing diagnostics move to codes, and `Diagnostic::error` and
  `Diagnostic::warning` are removed.
- A second syntax error in a broken item shows up only after the first is
  fixed.
- A milestone is not done until its new codes have explanations and tests.
  A milestone spec lists its diagnostics by name and never picks a number.
- `docs/diagnostics/<CODE>.md` holds one explanation per code. A README in
  that directory has the code list and the JSON shape.

## Amendment, 2026-10-13: keep diagnostics cheap

- **A fix is optional.** Offer one only when the edit is obvious and simple.
  Otherwise give help text. A spec lists diagnostics by rule and names a fix
  only where it earns its place.
- **The compiler drops a broken fix.** Before a fix is reported, it is applied
  and the result is lexed and parsed. A fix that fails is dropped, so call
  sites need no special cases for parentheses, comments or line breaks.
- **A new code only when its explanation would differ.** Otherwise the rule
  becomes a new form of an existing code, with its own facts and message.

## Amendment, 2026-11-08: the fourth rule, a cascade rests on a guess

One mistake gives only true errors. An error is true when it follows from what
the source states. A cascade is an error that follows from a guess about code
that failed. Shared code, not review by hand, keeps this so: one name resolver
and one poison rule ([ADR 0049](0049-one-name-resolver-and-one-poison-rule.md)).
A change that adds a cascade blocks its review. An old cascade on a rare form is
an ordinary issue.
