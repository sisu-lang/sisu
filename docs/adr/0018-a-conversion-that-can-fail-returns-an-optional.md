# A conversion that can fail returns an optional

Status: accepted, 2026-10-15. It supersedes the statement in
[ADR 0014](0014-strings-are-utf-8-values.md) that `char(n)` panics when `n` is
not a scalar value, and the same behaviour of `u8(n)` from milestone 3.

A conversion is a call written with a type's name. One that can fail returns
an optional. One that cannot fail returns the plain type. No conversion
panics.

| Conversion | Argument | Result | `None` when |
| --- | --- | --- | --- |
| `f64(n)` | `i64` | `f64` | never; rounds to nearest beyond 2^53 |
| `i64(x)` | `f64` | `i64?` | `x` is not a whole number, is `nan` or infinite, or is outside `i64` |
| `i64(b)` | `u8` | `i64` | never |
| `i64(c)` | `char` | `i64` | never |
| `u8(n)` | `i64` | `u8?` | `n` is outside 0 to 255 |
| `char(n)` | `i64` | `char?` | `n` is not a Unicode scalar value |

- **Into an integer, a conversion is exact or `None`. Into `f64`, it rounds.**
  "Can fail" means there is no meaningful nearest value, not that a bit is
  lost. `f64(n)` is not `f64?` because every language surveyed rounds (Swift's
  `Double(n)`, Go's `float64(n)`, Python's `float(n)`, Rust's `n as f64`), and
  an optional on every `f64(n)` would fill numeric code with `??`. A later
  `f32(x)` follows the same rule.
- **`i64(2.7)` is `None`, not `2`.** The caller states the rounding:
  `i64(x.floor())`, `i64(x.round())` or `i64(x.trunc())`. `-0.0` converts to
  `0`.
- **`u8(...)` and `char(...)` change.** Milestone 3's `u8(x)` and milestone
  4's `char(n)` panicked. They now return `u8?` and `char?`.
- **The optional is permanent.** It is not a stand-in for milestone 12's
  `Result`. A conversion fails for one reason only, and the caller already
  has it in hand. The caller decides what failure means, in plain sight:
  `u8(n) ?? 255`, or `u8(c - 48) ?? panic("not a digit")`.

## Considered options

- **Swift's panicking `Int(x)` plus `Int(exactly:)`**: the short form is
  easy to write, and it panics on input the caller did not check. Sisu wants
  panics to be deliberate.
- **Rust's `as` and `TryFrom`**: `as` wraps or saturates without a word, and
  `TryFrom` returns a `Result` that needs milestone 12's machinery.
- **Kotlin's `toIntOrNull`**: the right result, but a method per target type
  and a different spelling from the conversions that cannot fail.
- **`u8?(x)`**: rejected because it reads too much like milestone 12's
  `u8(x)?`.
- **`u8.exact(x)`**: rejected for the same reason as the method form, and
  because it makes two spellings for one idea.
- **`x.to_u8_or_none()`**: rejected as long, and it names the failure
  instead of the type.

## Consequences

- A conversion that can fail is an error in a constant. A constant holds no
  optional, so `const B = u8(200)` is rejected, and the fix is to write the
  literal with its type: `const B: u8 = 200`.
- `let b = u8(65)` is a `u8?`. The plain form is `let b: u8 = 65`. A literal
  argument gets no special case outside constants.
- Every conversion is still between exactly two types. `u8(x)` with
  `x: f64`, and `f64(b)` with `b: u8`, go through `i64`.
- The run-time panics "value does not fit in `u8`" and "value is not a valid
  `char`" go away. A diagnostic code that no program can reach any longer is
  marked retired under ADR 0013.

Note (2026-11-14, ADR 0051): The table gains the `string` to `i64` row. Parsing a string fails for
several reasons (empty, a bad character, overflow) and still returns an
optional, as Swift's `Int("x")` does.
