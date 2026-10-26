# Diagnostic codes

Every diagnostic that `sisu` reports about the source has a stable code:
`E0001`, `E0002`, ... for errors and `W0001`, `W0002`, ... for warnings
([ADR 0013](../adr/0013-diagnostics-have-stable-codes.md)). Each code has an
explanation in this directory, which `sisu explain <code>` prints as it
stands. The code may be in any case. Links in an explanation are relative to
`docs/diagnostics/`.

| Code | Severity | Title |
| --- | --- | --- |
| [E0001](E0001.md) | error | assignment to something that cannot change |
| [E0002](E0002.md) | error | line that starts with a binary `-` |
| [E0003](E0003.md) | error | bracket that is never closed |
| [E0004](E0004.md) | error | `{` on a line of its own |
| [E0005](E0005.md) | error | `[` that starts a line inside brackets |
| [E0006](E0006.md) | error | `else` on a line of its own |
| [E0007](E0007.md) | error | closing bracket that does not match |
| [E0008](E0008.md) | error | `_` outside digits |
| [E0009](E0009.md) | error | integer literal too large for `i64` |
| [E0010](E0010.md) | error | character that Sisu does not use |
| [E0011](E0011.md) | error | token where something else belongs |
| [E0013](E0013.md) | error | `var` instead of `let mut` |
| [E0015](E0015.md) | error | `,` between fields, variants or arms |
| [E0016](E0016.md) | error | `let` in front of a field |
| [E0017](E0017.md) | error | method without `self` |
| [E0018](E0018.md) | error | `mut self` on a method |
| [E0019](E0019.md) | error | `mut` parameter |
| [E0020](E0020.md) | error | `self` that is not the first parameter |
| [E0021](E0021.md) | error | assignment to something that is not a place |
| [E0022](E0022.md) | error | assignment through `?.` |
| [E0023](E0023.md) | error | range outside `for` and patterns |
| [E0024](E0024.md) | error | chained `==`, `!=` or `is` |
| [E0025](E0025.md) | error | comparison chain that turns around |
| [E0026](E0026.md) | error | name taken by a built-in |
| [E0027](E0027.md) | error | class that can never be constructed |
| [E0028](E0028.md) | error | program without `main` |
| [E0029](E0029.md) | error | `main` with a wrong signature |
| [E0030](E0030.md) | error | type that does not exist |
| [E0031](E0031.md) | error | function body without the value it returns |
| [E0032](E0032.md) | error | value of the wrong type |
| [E0033](E0033.md) | error | name that is not in scope |
| [E0034](E0034.md) | error | name defined twice |
| [E0035](E0035.md) | error | `for` over something that is not an array or a range |
| [E0036](E0036.md) | error | `return` without a value |
| [E0037](E0037.md) | error | `return` with a value in a function that returns none |
| [E0038](E0038.md) | error | `break` or `continue` outside a loop |
| [E0039](E0039.md) | error | parameter name used twice |
| [E0040](E0040.md) | error | integer literal too large for `u8` |
| [E0041](E0041.md) | error | `self` outside a method |
| [E0042](E0042.md) | error | `u8` that is negated |
| [E0043](E0043.md) | error | expression without a value |
| [E0044](E0044.md) | error | comparison of a value that is never `None` with `None` |
| [E0045](E0045.md) | error | `is` used to test for `None` |
| [E0046](E0046.md) | error | `is` on something that is not an object |
| [E0047](E0047.md) | error | comparison of values of different types |
| [E0048](E0048.md) | error | `??` on a value that is not optional |
| [E0049](E0049.md) | error | `?.` on a value that is not optional |
| [E0050](E0050.md) | error | a type that cannot be inferred |
| [E0051](E0051.md) | error | `if let` or `while let` on a value that is not an enum or optional |
| [E0052](E0052.md) | error | call of a function that does not exist |
| [E0053](E0053.md) | error | call with the wrong number of arguments |
| [E0054](E0054.md) | error | conversion to the type the value already has |
| [E0055](E0055.md) | error | labeled argument in a call |
| [E0056](E0056.md) | error | constructor without every field |
| [E0057](E0057.md) | error | method read as a field |
| [E0058](E0058.md) | error | field called as a method |
| [E0059](E0059.md) | error | `print` or `eprint` of something it cannot print |
| [E0060](E0060.md) | error | `print` or `eprint` of an array |
| [E0061](E0061.md) | error | `print` or `eprint` of an expression that never finishes |
| [E0062](E0062.md) | error | optional used as if it were present |
| [E0063](E0063.md) | error | field or method that does not exist |
| [E0064](E0064.md) | error | constructor with more arguments than fields |
| [E0065](E0065.md) | error | constructor argument without a label |
| [E0066](E0066.md) | error | constructor that names its fields out of order |
| [E0067](E0067.md) | error | array filled with one object |
| [E0068](E0068.md) | error | index of something that is not an array |
| [E0069](E0069.md) | error | `sum` of an array that is not `[i64]` or `[f64]` |
| [E0070](E0070.md) | error | method called as a function |
| [E0071](E0071.md) | error | nesting too deep |
| [E0072](E0072.md) | error | source that is not valid UTF-8 |
| [E0073](E0073.md) | error | assignment to `self` |
| [E0074](E0074.md) | error | assignment to a function |
| [E0075](E0075.md) | error | base prefix in upper case |
| [E0076](E0076.md) | error | base prefix without digits |
| [E0077](E0077.md) | error | digit outside the literal's base |
| [E0078](E0078.md) | error | `mut` on `_` |
| [E0079](E0079.md) | error | `_` used as a value |
| [E0080](E0080.md) | error | `_` used as a name |
| [E0081](E0081.md) | error | block that stands alone as a statement |
| [E0082](E0082.md) | error | `break` or `continue` in a loop header |
| [E0083](E0083.md) | error | word reserved for a later version |
| [E0084](E0084.md) | error | `const` inside a function |
| [E0085](E0085.md) | error | constant of a type that cannot be folded |
| [E0086](E0086.md) | error | constant value not known at compile time |
| [E0087](E0087.md) | error | constant that uses itself or a constant below it |
| [E0088](E0088.md) | error | constant whose value would panic |
| [E0089](E0089.md) | error | unknown escape |
| [E0090](E0090.md) | error | malformed `\u` escape |
| [E0091](E0091.md) | error | escape that is not a Unicode scalar value |
| [E0092](E0092.md) | error | `char` literal that is not one code point |
| [E0093](E0093.md) | error | literal that is not closed |
| [E0094](E0094.md) | error | arithmetic on `char` |
| [E0095](E0095.md) | error | `}` in a string |
| [E0096](E0096.md) | error | empty interpolation |
| [E0097](E0097.md) | error | comment inside an interpolation |
| [E0098](E0098.md) | error | value that cannot be interpolated |
| [E0099](E0099.md) | error | `todo` written another way |
| [E0100](E0100.md) | error | conversion that can fail, in a constant |
| [E0101](E0101.md) | error | float without digits on both sides of `.` |
| [E0102](E0102.md) | error | exponent without digits |
| [E0103](E0103.md) | error | float literal out of range for `f64` |
| [E0104](E0104.md) | error | integer literal not exactly an `f64` |
| [E0105](E0105.md) | error | a type, an interface or a function used as a value |
| [E0106](E0106.md) | error | math written another way |
| [E0107](E0107.md) | error | `**` |
| [E0108](E0108.md) | error | malformed format spec |
| [E0109](E0109.md) | error | unsupported format spec |
| [E0110](E0110.md) | error | format spec width or precision above its limit |
| [E0111](E0111.md) | error | precision on a value that is not an `f64` |
| [E0112](E0112.md) | error | variant after a method |
| [E0113](E0113.md) | error | variant name not capitalized |
| [E0114](E0114.md) | error | payload labels mixed |
| [E0115](E0115.md) | error | enum without variants |
| [E0116](E0116.md) | error | local named like an enum |
| [E0117](E0117.md) | error | variant without its enum |
| [E0118](E0118.md) | error | unknown variant |
| [E0119](E0119.md) | error | variant built the wrong way |
| [E0120](E0120.md) | error | variant or method used as the other |
| [E0121](E0121.md) | error | field access on an enum |
| [E0122](E0122.md) | error | an idiom of another language in an enum or `match` |
| [E0123](E0123.md) | error | a pattern written another language's way |
| [E0124](E0124.md) | error | `match` on a type it cannot take |
| [E0125](E0125.md) | error | upper-case name where no enum is expected |
| [E0126](E0126.md) | error | constant in a pattern |
| [E0127](E0127.md) | error | only a name here |
| [E0128](E0128.md) | error | wrong number of sub-patterns |
| [E0129](E0129.md) | error | name bound twice in one pattern |
| [E0130](E0130.md) | error | `match` that does not cover every value |
| [E0131](E0131.md) | error | `if let` or `while let` that always matches |
| [E0132](E0132.md) | error | range written another language's way |
| [E0133](E0133.md) | error | arm split across lines |
| [E0134](E0134.md) | error | interpolation in a string pattern |
| [E0135](E0135.md) | error | a range pattern that cannot match |
| [E0136](E0136.md) | error | alternatives bind different names |
| [E0137](E0137.md) | error | type arguments that do not fit |
| [E0138](E0138.md) | error | type that cannot be a type argument |
| [E0139](E0139.md) | error | operator on a value of a type parameter |
| [E0140](E0140.md) | error | instantiations that never end |
| [E0141](E0141.md) | error | `break` with a value in a loop that has none |
| [E0142](E0142.md) | error | loop label that is also a name in scope |
| [E0143](E0143.md) | error | loop label used by an enclosing loop |
| [E0144](E0144.md) | error | `break` or `continue` naming no loop label around it |
| [E0145](E0145.md) | error | loop label on something that is not a loop |
| [E0146](E0146.md) | error | body or field in an interface |
| [E0147](E0147.md) | error | interface used as a type |
| [E0148](E0148.md) | error | bound that is not an interface |
| [E0149](E0149.md) | error | `Self` outside a type or an interface |
| [E0150](E0150.md) | error | interface that inherits from itself |
| [E0151](E0151.md) | error | inherited interface with a built argument |
| [E0152](E0152.md) | error | conformance that a type does not meet |
| [E0153](E0153.md) | error | second conformance to an interface |
| [E0154](E0154.md) | error | type that does not conform to an interface that is needed |
| [E0155](E0155.md) | error | `extend` of a type that is not a class or an enum |
| [E0156](E0156.md) | error | call that more than one conformance answers |
| [E0157](E0157.md) | error | bound that gives too many interfaces |
| [W0001](W0001.md) | warning | unreachable code |
| [W0002](W0002.md) | warning | variable that is never reassigned |
| [W0003](W0003.md) | warning | `todo` left in the program |
| [W0004](W0004.md) | warning | unused value |
| [W0005](W0005.md) | warning | comparison with `f64.nan` |
| [W0006](W0006.md) | warning | unreachable arm |
| [W0007](W0007.md) | warning | unnecessary `;` |
| [W0008](W0008.md) | warning | loop label that is never used |
| [W0009](W0009.md) | warning | `while` with the condition `true` |
| [W0010](W0010.md) | warning | function that calls itself on every path |
| [W0011](W0011.md) | warning | `while` condition that nothing in the loop changes |
| [W0012](W0012.md) | warning | function, method, class, enum, interface or constant that no live code uses |
| [W0013](W0013.md) | warning | field that no live code reads, or variant that no live code builds |
| [W0014](W0014.md) | warning | local or parameter that nothing reads |

## Explanations

Each explanation has the same shape: a `# <CODE>: <title>` line, one
paragraph that states the rule, then `## Why`, `## Broken` and `## Fixed`,
and an optional `## Related`. The Broken program reports exactly its code;
the Fixed program reports nothing. E0099 is the one exception: its Fixed
program keeps the `todo` its fix leaves, so it reports W0003. Tests compile
both, and apply every safe fix to the Broken program.

## Numbers

A new code takes the next free number in its series, in the order codes land
on `main`. A code is never reused or renumbered.

E0012 and E0014 are the retired codes. When the compiler stops reporting a code,
its variant leaves `DiagnosticKind` and its row leaves the table above. Its
file stays here and gains a `Status: retired` line after its title, naming
the ADR that retired it, and `sisu explain` serves it from `RETIRED`, the
table of retired codes in `crates/sisuc/src/diagnostic/registry.rs`. The site
leaves it out.

## JSON output

`--format json` writes every line on stderr as one JSON object (JSON
Lines). It works with `check`, `run` and `build` (with or without `--emit`);
stdout is the same as in text, and stays empty when there is an error. A usage
error is clap's text on stderr, with exit code 2, in either format.

Every object has `version`, which is `1` and is bumped on any breaking
change, and `kind`, one of `diagnostic`, `tool_error` and `summary`. Keys
come in the order shown.

A diagnostic about the source:

```json
{"version":1,"kind":"diagnostic","code":"E0001","severity":"error","message":"cannot assign to `n`","primary":{"file":"a.sisu","start":36,"end":41,"line":4,"column":5,"label":"cannot assign twice"},"secondary":[{"file":"a.sisu","start":26,"end":27,"line":3,"column":9,"label":"declared with `let` here"}],"help":null,"fixes":[{"message":"declare it with `let mut`","safe":true,"edits":[{"start":26,"end":26,"line":3,"column":9,"replacement":"mut "}]}]}
```

- `code`: the diagnostic's code, such as `E0001`.
- `severity`: `"error"` or `"warning"`.
- `message`: what is wrong.
- `primary`: the offending code. `secondary`: a list of other spans that
  explain it. Each span has:
  - `file`: the input path exactly as given on the command line, or `<inline>` for a
    program given with `-e`;
  - `start` and `end`: a half-open byte range of the input;
  - `line` and `column`: where `start` is, both 1-based; the column counts
    characters, not bytes;
  - `label`: the text about the span, or `null`.
- `help`: advice that is not an edit, or `null`. A fix's message is in the
  fix, never here.
- `fixes`: a list of edits that resolve the diagnostic, each with:
  - `message`: what the fix does;
  - `safe`: `true` for a safe fix, whose edits always remove the diagnostic
    and leave source that lexes and parses; `false` for an unsafe fix, which is
    likely but not proven, such as a did-you-mean suggestion, or a line join or
    idiom rewrite whose edits would delete a comment;
  - `edits`: a list of `start`, `end`, `line` and `column` as in a span, and
    `replacement`, the text that replaces the range.

A tool error, such as an unreadable input, an output that would overwrite the
input, or a failed link. It has no code; codes are for the source.

```json
{"version":1,"kind":"tool_error","message":"cannot read x.sisu: No such file or directory (os error 2)"}
```

- `message`: what went wrong, without the `sisu:` prefix that text output
  puts before it. A failed link adds the C compiler's own output after a
  newline.

The summary, always the last line, also when there is no diagnostic, with one
exception. `sisu run` writes the summary before the program starts, so the
program's own stderr follows it, and so do the three tool errors that can only
happen once the program is built: it cannot start, its temporary directory
cannot be removed, or `sisu` cannot wait for it.

```json
{"version":1,"kind":"summary","errors":1,"warnings":0,"explanations":{"E0001":"# E0001: ..."}}
```

- `errors` and `warnings`: how many diagnostics of each severity; tool errors
  are not counted. A tool error and the exit code tell that the command
  failed.
- `explanations`: each code seen in this run, sorted, mapped to its
  explanation, the text `sisu explain` prints.

Text output ends the same way, after at least one diagnostic: a line with
the counts, then a line naming the codes seen.

## Applying fixes

- `start` and `end` are a half-open byte range in the original input, never
  in a partly edited one.
- A fix's edits apply together, and they do not overlap.
- The same edit can appear in several diagnostics; for example, two
  assignments to one `let` each offer `let mut`. Apply it once.
- A diagnostic's fixes are alternatives: apply at most one of them.
- A fix that would leave the input with more syntax errors than it has is
  never offered, so applying any one fix keeps the program as parseable as it
  was.
