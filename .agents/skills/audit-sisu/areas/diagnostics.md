# Area: diagnostics

Every program the language forbids gets a diagnostic that points at the
code at fault and says what is wrong in the user's terms. The rules for
messages are in `CODING_STANDARDS.md`, "Diagnostics"; every message is built
in `crates/sisuc/src/diagnostic/registry.rs`, and the existing cases are the
fixtures in `crates/sisuc/tests/diagnostics` and the unit tests of the lexer,
parser and checker.

## Probes

- For each kind of error the lexer, parser and checker report, write the error in three new places (a nested expression, a method body, a loop) and check that the span underlines the right code, on the right line and column, including after a multibyte character and on the last line with no trailing newline.
- Untested messages: list the messages the registry builds, one arm per code and its variants. A message that no `tests/diagnostics/*.err` file shows is a `gap`.
- Cascades: an expression that already failed gets the poison type, so it must not produce a second diagnostic further on. Give a function the wrong return type and use its result in five expressions; misspell a field and use the result. A diagnostic caused only by an earlier one is a finding. Separate uses of an unknown name may each get their own.
- Recovery: the parser reports one error per broken item and resumes at the next; the checker reports every error. Two independent errors in separate items, or in one function for the checker, must both appear.
- Wording: a message that uses compiler terms (`tir`, poison, internal type names), suggests a fix that would not compile, or names the wrong type.

Severity is `gap`, unless a forbidden program gets no diagnostic at all
(then `wrong`) or `sisu` fails (then `crash`).
