# Every language feature has a generator rule

Status: accepted when the program fuzzer merges.

A language feature is done only when the program fuzzer's generator produces
it. A feature here is an AST variant, an operator, a type, a prelude function
or a built-in method. A unit test enforces the rule: it fails when an enum of
the AST, the TIR or the types gains a variant that no generated program holds.

## Considered options

- **A checklist item in reviews.** It depends on someone remembering, which is
  the failure the rule exists to prevent.
- **Generator work after each milestone.** The generator falls behind the
  language, and bugs in the new feature are found late or never.
- **Enforced by a test.** Chosen: adding a feature without a rule fails
  `make test`.

## Prior art

Csmith and YARPGen generate C that is valid by construction; rustlantis
generates Rust MIR for differential testing.

## Consequences

- Every milestone plan has a generator task. Milestone 8 must add type
  parameters, type arguments and generic calls before `feat/m8-generics`
  merges.
- A variant that no valid program holds is excluded in the test with its
  reason.
