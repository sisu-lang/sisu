# Strings are UTF-8 values

Status: accepted, 2026-10-12. It answers the open question of ADR 0009 and
the roadmap's "What a string is". [ADR 0016](0016-word-functions-trust-the-utf-8-invariant.md)
amends its first rule for the word functions.
[ADR 0018](0018-a-conversion-that-can-fail-returns-an-optional.md)
supersedes its statement that `char(n)` panics: `char(n)` returns a `char?`.

A `string` is a sequence of bytes that is always valid UTF-8. Four rules
follow from that.

- **Valid UTF-8 is an invariant.** Every `string` is valid UTF-8, from its
  creation until it is freed. Nothing at run time reports it, because nothing
  can break it; the runtime aborts if a compiler bug breaks it. The lexer
  validates literals, `char(n)` validates its argument and panics when `n` is
  not a scalar value, and every operation (`+`, `+=`, interpolation,
  `string(cs)`) joins valid pieces whole.
  The first operation that turns bytes into a `string`, such as reading a
  line from standard input, must validate them and report failure instead of
  producing the string.
- **A string is a value (ADR 0007).** Binding, passing, returning and storing
  a string behaves as a copy. It is a counted type: it shares one block until
  `+=` writes to it, and the write copies first when another holder exists.
- **`char` is a Unicode scalar value.** One code point, `0` to `10FFFF`
  without the surrogates. It is not what a reader sees as one character: `e`
  plus a combining accent is two `char`s.
- **`len()` counts bytes, and `s[i]` does not exist.** `len()` is constant
  time. To work on text, call `chars()` for the code points or `bytes()` for
  the bytes and index the array.

UTF-8 encoding and decoding, and number formatting, live in the runtime over
raw buffers, taking and returning pointers and integers. The string layout
(count, length, capacity, bytes) is written once, in codegen, as ADR 0009
asks.

## Considered options

- **Bytes only, with no encoding promise (C, Go)**: the simplest to build.
  But every function that handles text must then guard against bytes that are
  not text, and a `char` or a print can show garbage. With the invariant, a
  string is always text, and `bytes()` still gives raw access.
- **Grapheme clusters as the unit (Swift)**: `len()` and indexing match what
  a reader sees. But finding a boundary needs Unicode tables that change with
  each Unicode version, `len()` costs time in proportion to the string, and
  the compiler would carry those tables. Sisu keeps the cheap, stable unit and
  leaves clusters to a later library.
- **UTF-16 code units (Java, Kotlin)**: indexing by unit is constant time.
  But it is wrong for any character outside the Basic Multilingual Plane,
  source files and standard input are UTF-8 already, so every read and write
  would convert, and surrogates leak into the language.

## Consequences

- `"é"` precomposed and `e` plus a combining accent are not equal. There is no
  normalisation. Comparison is by bytes, which for valid UTF-8 is code-point
  order.
- Each `chars()` or `bytes()` call builds a new array.
- A string literal's bytes live in the program image, in a writable block
  whose count is shared by every evaluation of that literal. ADR 0010's tasks
  must change literal handling before concurrency lands, because two threads
  would race on that count. For example, give literals an immortal count that
  retain and release skip, or copy the literal once per task.
- A substring shares the bytes of its parent. [ADR 0042](0042-a-slice-shares-its-block.md)
  records this: `s[a..b]` gives a `string` that shares its parent's block.
  Small-string storage inside the variable's slot is still an open decision in
  the roadmap. It does not change what a program can observe.
