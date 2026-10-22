# Architecture

`sisu` compiles one Sisu source file into one native executable. It is a
pipeline: each stage takes what the previous stage produced and hands on
something closer to machine code. The stages run in one process, with LLVM
linked in as a library. Only the final link runs a separate program, the
system C compiler `cc`.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="architecture/sisu-pipeline-dark.svg">
  <img alt="The Sisu compiler pipeline: source text flows through the lexer, parser, checker, mir and codegen to LLVM, and the link step combines the object file with sisu-runtime into an executable." src="architecture/sisu-pipeline-light.svg">
</picture>

This page stays at the level of stages and the data between them. The code
and its comments hold the detail, and [GLOSSARY.md](../GLOSSARY.md) defines
the terms.

## The stages

**Driver** (`crates/sisuc/src/lib.rs`). Reads the command line, runs the
stages in order and prints diagnostics. `args.rs` parses the command line with
clap into a `Mode`, one per subcommand plus `--version` and the usage, and `run.rs`
links a program into a private temporary directory, runs it and removes the
directory. The `sisu` binary (`main.rs`) only
runs it on a thread with a 64 MiB stack. It reads at most 1 GiB of source,
so every span offset fits in a `u32`; a larger file is a tool error. The
fuzzer calls the library too:
`fuzz.rs` runs the `check` path on raw bytes and asserts that every
diagnostic span and fix edit is well formed, and that every safe fix, applied
alone, deletes no comment and removes its diagnostic and no other. For an
input that checks clean, it also prints the typed tree, builds its mir and
runs codegen, and LLVM must verify the module. `sisu check` and `sisu build --emit`
stop the pipeline early, as
[Developing Sisu](development.md#look-inside-the-compiler) lists.
`--format json` switches the diagnostics to JSON Lines, and `sisu explain <code>`
prints the explanation of a code and runs no stage.

**Lexer** (`lexer/`). Turns source text into tokens. A line break that can
end a statement becomes a `Newline` token, because statements end at
newlines ([ADR 0001](adr/0001-newline-terminated-statements.md)). A `;`
between two statements on one line becomes a `Newline` too, and an
unnecessary one, such as a `;` at the end of a line, is dropped with W0007
([ADR 0028](adr/0028-a-semicolon-stands-in-for-a-line-break.md)). It
decodes a `char` literal and its escapes into one scalar value, and a string
literal into its text, so later stages never see the escape text. An integer
literal's token holds only its value; a diagnostic that quotes the literal as
written slices the source with its span. A string with `{expr}` parts
becomes a `StrHead` token, the expression's tokens, and `StrMiddle` or
`StrTail` tokens for the text between and after them. A `:` that ends an
interpolation's expression starts a `FormatSpec` token, which holds the spec's
text. The lexer interns each
name into a `Symbol` (`symbol.rs`), so the parser and checker copy, compare and
hash names as integers; `tir` and diagnostics hold the text as a `String`. After most errors
the lexer skips the bad text and goes on, so it records every one of them,
and the parser keeps the earliest per item. A bracket that is unclosed or does
not match is the exception: the run then reports one diagnostic and the parser
and checker do not run.

**Parser** (`parser.rs`, `ast.rs`). Turns tokens into a syntax tree, the AST.
It reports one error per broken item, then resumes at the next item. The
intact items go on to the checker, and a broken item keeps its name, so a use
of it is not reported as missing. It parses a format spec's text into its alignment, width and precision, and
reports a malformed or unsupported one without breaking its item. An `enum` item parses into its variants, each with
an optional payload of labeled or unlabeled fields, and its methods. A `match`
parses into its scrutinee and its arms, one per line, each a pattern, an
optional `if` guard, `=>` and a body; a pattern is one or more alternatives
joined by `|`. `if let` and `while let` take a pattern too. It rejects an item that nests deeper than
256 levels, because every later stage walks the tree recursively.

**Front end** (`front_end.rs`). Runs the lexer, parser and checker in order and
returns the program and every diagnostic, in position order: the checker's are
merged in by position. The checker runs only when
every item has a name.

**Checker** (`check/`, `tir.rs`). Resolves every name, types every expression
and lowers sugar: `while`, `while let` and `for` become loops, `?.` and `??`
become `if let`, and `x += 1` becomes `x = x + 1`. A `+=` on a string field
or element, or an assignment that adds to its own field or element place,
becomes an `Append`. A local keeps `Assign` with `Concat` and appends in place
through `concat`'s count-1 path. It folds each top-level `const` to a literal
and replaces every use of its name with that literal. A call to a prelude
function such as `print` or `panic` has its own rule, and `panic(msg)` and
`todo` become a `Panic` node of type `never`. A format spec on a part becomes a padding and a precision, and a precision on a
value that is not an `f64` is an error. A method on a number, such as
`x.sqrt()` or `n.max(m)`, becomes a `Math` node, and a constant such as
`f64.pi` becomes a literal. `check/enums.rs` declares each enum: it rejects an
enum that no value can be built of, and finds the payload fields that close a
cycle of enums, the ones that go behind a box, and the enums that hold a
counted part. Building a variant, `Expr.Num(2)`, becomes a `Variant` node
([ADR 0019](adr/0019-enums-are-values-named-through-their-enum.md)).
`check/pattern.rs` checks each pattern against the type at its position and
declares the names it binds. `if let` on an enum becomes a `match` whose `_`
arm is the `else`, and `while let` on one a loop around a `match` whose `_` arm
breaks. `check/exhaust.rs` runs Maranget's usefulness algorithm over each
`match`'s arms: a `match` that misses a value is an error that names one it
misses as a pattern, such as `Mul(_, _)`, and an arm that no value reaches is
a warning. The same check rejects an `if let` or `while let` whose pattern
always matches. `check/operator.rs` checks operators (arithmetic, comparison,
equality, `is` and `??`), `check/branch.rs` `if` and `if let` and the join of
their branch types, `check/call.rs` calls, method calls and constructors, and
`check/member.rs` fields and methods found through `.` and `?.`. The checker
reports every error in one run: an expression that already failed gets the
poison type, so one mistake produces one diagnostic. A program with no errors comes out as the
typed tree, `tir`, which `mir` is built from. A type in `tir` is a
4-byte `Type` handle into one table (`types.rs`), as a name is a `Symbol`;
its `kind()` gives the structure, and equal structures get the same handle.

**mir** (`mir/`). The mid-level IR between `tir` and LLVM IR
([ADR 0025](adr/0025-mir-holds-every-count.md)), built on the build path only,
so `sisu check` does not pay for it. `mir::build` lowers each function of `tir` to
typed locals and basic blocks in which every copy, move and release is its own
statement (`lower.rs`). Parameter inference (`params.rs`,
[ADR 0029](adr/0029-parameters-are-borrowed-unless-consumed.md)) then searches
the whole program for the counted parameters that their functions only read,
marks them borrowed, and rewrites each call to pass such an argument in place;
a function that tests a parameter that came out owned with `match` or `if let`
is lowered again. The call graph's strongly connected components come from
`graph.rs`, which the checker's search for recursive enums shares. The reuse
pass (`reuse.rs`, [ADR 0030](adr/0030-match-takes-its-scrutinee-apart.md)) runs
next, before the ownership pass, and builds each box of a construction into the
block that a `match` arm took apart when it held that box alone. The
ownership pass (`ownership.rs`) then decides each read of a counted local by liveness (`liveness.rs`): a read that is the local's
last use moves its reference instead of sharing it, so a later write to the
array it holds can skip the copy
([ADR 0007](adr/0007-arrays-are-values.md)). The hoist (`hoist.rs`) moves the
`make_unique` of an array a loop writes to the loop's pre-header
([ADR 0021](adr/0021-a-loop-makes-its-written-arrays-unique-before-it-starts.md)).
Elaboration (`elaborate.rs`) then removes each scope release that only empty
paths reach and makes one that only owned paths reach a plain release, so
codegen stores the zero value on a move only for a local that a conditional
release still tests (for every local under `--rc-checks`).
Pair removal (`pairs.rs`) then turns a copy that is only borrowed within a block
into a borrow, and its release into an `end`.
Debug builds of the compiler end with the certifier (`certify.rs`), which
checks on every path that each owned reference is moved or released once and
that nothing writes or changes what a borrow reads.
`tir` carries no moves. `--emit mir` prints the result.

**Codegen** (`codegen/`). Walks `mir` and builds an LLVM module through the
[inkwell](https://github.com/TheDan64/inkwell) bindings. Each Sisu function
becomes an LLVM function named `sisu.<name>`, and each method of a class or an
enum one named `sisu.<Type>.<method>`. Codegen also emits a `main` with the C
signature,
which the C startup code calls and which runs `sisu.main`, passing it `args`,
a `[string]` built from `argc` and `argv`, when `sisu.main` takes that
parameter. Integer overflow
and division by zero call `sisu_panic` instead of wrapping
([ADR 0003](adr/0003-arithmetic-panics.md)). A `Panic` node, which `panic`
and `todo` lower to, calls `sisu_panic_message` with its location and
message, then ends the block with `unreachable`. A `Math` node becomes an LLVM
intrinsic, except `hypot`, which calls `hypot` from `libm`.

`codegen/abi.rs` declares every runtime function from one table, `RUNTIME`,
of names, types and attributes. A unit test reads the runtime's source and
fails when an export has no row, or a row's types differ from its signature.
`sisu_alloc` and `sisu_free` carry LLVM's allocator attributes as a pair. At
`-O2` LLVM may delete an allocation and its free, including when it can compute
every value the program reads without the heap block, such as an array's length
or an element of a fill. An allocation larger than the size checks allow
(about 2^63 bytes) aborts with exit code 134 when its allocator call remains;
at `-O0` every such allocation aborts. On Linux, a smaller request that the
machine cannot back succeeds in a default build, because mimalloc maps memory
without reserving it, and the kernel's out-of-memory killer ends the program
with 137 once it touches the memory
([ADR 0031](adr/0031-compiled-programs-allocate-with-mimalloc.md)).

`codegen/layout.rs` holds how a value sits in memory: the LLVM type of each
Sisu type, the layouts of an object, an array block and a string block, the
count word (`load_count`, `store_count`, `init_count`), the optional accessors
(`empty`, `has_value`, `optional`, `optional_value`), the size of every heap
block (`heap_size`), and the TBAA tags with the tagged loads and stores that
use them.

Objects live on the heap, with a reference count in front of their fields
([ADR 0005](adr/0005-reference-counting.md)). Codegen lays the fields out
largest alignment first, and fields of equal alignment in declared order, so
less padding sits between them; `tir` and every diagnostic keep declared
order. Codegen emits the helpers that
manage them: `sisu_rc.retain`, and for each class `sisu_rc.release.<Class>`
and `sisu_rc.drop.<Class>`. A class that `==` compares also gets
`sisu_rc.eq.<Class>`, made the first time the compiler needs it. A field
read and a field assignment tag their load and store with a TBAA tag of that
field's own, `<Class>.<field>`, so LLVM knows that writing one field changes
no other field, and no count, length or element.

An enum value has no heap block of its own. It is a tag and its variant's
payload, held inline in `%sisu.<Enum>`, with a struct `%sisu.<Enum>.<Variant>`
per variant laid over the payload storage. A payload field that closes a cycle
holds a box instead: a counted heap block, `%sisu.box.<Enum>`, that holds a
value of the enum. Each enum gets `sisu_rc.eq.<Enum>`, and an enum with a
counted part also gets `sisu_rc.retain.<Enum>` and `sisu_rc.release.<Enum>`,
which switch on the tag. A box whose count reaches zero is not freed by
recursion. It joins a pending list, linked through its dead count word, and
`sisu_rc.drain.<Enum>` frees the boxes on the list one by one, so the stack
stays the same size for a value of any depth.

`mir/lower/pattern.rs` lowers a `match`. It evaluates the scrutinee once into
a temporary that the `match` owns, then tests the arms' patterns against it in
order, reading a boxed field through its box. An arm with alternatives tries
them in order, and an arm with a guard that fails falls through to the next
alternative, and then the next arm. The first arm that matches adds
1 to each value it binds, releases the temporary and runs its body. Each arm
assigns its value to the same local. `codegen/matching.rs` emits the switches
on a tag or a value that the tests become, and after the last arm a call to
`llvm.trap`: the checker has proved that some arm matches, and a checker bug
must not become undefined behaviour.

An array is one counted heap block too: a header of two `i64`s, the count and
the length, then the elements. Each array type gets the same helpers, such as
`sisu_rc.release.[i64]`, plus `sisu_rc.unique.[i64]`, which copies the block
before a write when another holder shares it. A loop makes the arrays it
writes unique once, before it starts, when nothing in the loop shares them,
so the writes inside it skip the count check (ADR 0021). An index out of
bounds calls `sisu_panic_bounds`. Codegen tags the loads and stores of an
array's count and length, and every count update and check, with one TBAA
tag, and those of an array element with another, so LLVM knows that writing
an element changes none of them.

A string is one counted block with a header of three `i64`s: the count, the
length and the capacity, then the UTF-8 bytes. A literal is a private global
that holds a reference of its own, so it is never freed, and each use of it
adds one more. `+` appends in place when the left operand's count is 1,
growing the block through `sisu_realloc`, and copies otherwise, so a literal
is never reused in place: its count never drops to 1 at `+`. An `Append` reads its place, evaluates the text, then checks
the count last, so it appends in place only when nothing else holds the string
or an array on its path. `chars()` and `bytes()` build a new array from the
string, and `string(cs)` encodes a `[char]` into a new block. An interpolation
allocates one block sized for the most text each part can give, plus the width
of each padded part, then writes the parts into it, padding each part in place. With `--rc-checks`, these helpers check each count before they
change it, and `main` calls `sisu_rc_check_exit` before it returns.

**LLVM 23**. Runs the `mem2reg` pass, which turns stack slots into registers,
or with `-O2` or `-O3` the `default<O2>` or `default<O3>` pipeline. Then it
emits object code for the machine `sisu` runs on, x86-64 or ARM64.

**Link step** (`link.rs`). Before it compiles, the driver checks that
the runtime archive and the output's directory exist, so a missing one fails
at once. `--allocator` picks the archive: `libsisu_runtime_mimalloc.a` by
default, `libsisu_runtime_system.a` for `--allocator system`. The link step
writes the object code to a fresh temporary file and runs `cc` on it together
with that archive. `cc` adds the C startup code and libc and writes the
executable, through its default linker or the one `--linker` names. The
temporary file is removed whether or not the link succeeds.

**Runtime** (`crates/runtime`). A Rust library with a C ABI. The two static
archives `sisu-runtime-mimalloc` and `sisu-runtime-system` link it, and every
program links one of them. Generated code calls it to print values, to read
lines, to panic and to allocate and free objects, arrays and strings:
`sisu_print_int`, `sisu_print_f64`, `sisu_print_bool`, `sisu_eprint_int`,
`sisu_eprint_f64`, `sisu_eprint_bool`,
`sisu_flush_stdout`, `sisu_panic`, `sisu_panic_message`, `sisu_panic_bounds`,
`sisu_alloc`, `sisu_realloc` and `sisu_free`. Text lives in `text.rs`:
`sisu_print_char`, `sisu_print_str`, `sisu_eprint_char`, `sisu_eprint_str`,
`sisu_str_compare`, and `sisu_write_int`, `sisu_write_bool` and
`sisu_write_char`, which interpolation calls (`float.rs` adds `sisu_write_f64`
for an `f64`, and `sisu_write_f64_fixed` for one with a precision), and
`sisu_pad`, which pads a part with a width in place, and
`sisu_utf8_count`, `sisu_utf8_decode`, `sisu_utf8_len` and `sisu_utf8_encode`,
which `chars()` and `string(cs)` call, and `sisu_words_count` and
`sisu_word_next`, which `words()` calls; `s.words().len()`, written as one
expression, calls only `sisu_words_count`. `stdin.rs` holds `sisu_read_line`,
which `read_line()` calls. `args.rs` holds `sisu_args_len` and `sisu_arg`,
which the generated `main` calls to build `args` when `sisu.main` takes it.
`print(x)` and `"{x}"` share one formatter per
type. The `sisu_print_*` functions write into a buffer that the runtime owns,
and `main` calls `sisu_flush_stdout` before it returns. The `sisu_eprint_*`
functions write each value at once, in one write to standard error. The
runtime counts the blocks that are allocated and not yet freed. A program
built with `--rc-checks` calls `sisu_rc_fault` when a count check fails, and
`sisu_rc_check_exit` at a normal exit, which fails when that number is not
zero. Both exit with code 102.

**Source positions** (`source.rs`). `Span` is a byte range of the source, kept
as two `u32` offsets; `MAX_SOURCE`, 1 GiB, is the longest source that keeps
them in range. `LineIndex` turns a byte offset into a line and column for
people with a binary search. Every stage imports these: the lexer and parser
make spans, the checker carries them, codegen turns them into debug-info
lines, and the diagnostics and the report show them.

**Diagnostics** (`diagnostic/`, `report.rs`). The lexer, parser and checker
report problems as diagnostics. `diagnostic/registry.rs` holds the code table
and every message, label, help and fix: a stage builds a `DiagnosticKind`, and
the registry turns it into a `Diagnostic` with a stable code
([ADR 0013](adr/0013-diagnostics-have-stable-codes.md)). `diagnostic/render.rs`
writes it in the style of `rustc`: the message, the `file:line:col`, and the
source line with the code underlined. `diagnostic/json.rs` writes the same
diagnostic as one JSON line, and `diagnostic/suggest.rs` finds the closest
name for a did-you-mean fix. `report.rs` writes diagnostics and tool errors
as text or JSON, counts them and ends with the summary. Before it writes one,
`front_end::sound_fixes` drops any fix of a lexer or parser diagnostic whose
result would lex or parse worse than the input, and makes unsafe any safe fix
whose edits cover a comment or another lexer error, text no rule sees. Each
judgement parses the source again, so it judges only the first 100 syntax
fixes and drops the rest. It does not judge a checker fix: that would cost a
parse for each one, and the checker runs only on items that parse. The fuzz
target checks that no fix shown makes the syntax worse. The explanations in
[`docs/diagnostics`](diagnostics/README.md) are what `sisu explain` prints.

## Updating the diagram

The diagram's source is
[`architecture/sisu-pipeline.json`](architecture/sisu-pipeline.json), written
for the [archify](https://github.com/tt-a1i/archify) diagram tool. After a
change to the pipeline:

1. Edit the JSON.
2. From `docs/architecture`, render it with archify's command-line tool,
   where `<archify>` is your archify checkout, or the directory of the
   `archify` agent skill when it is installed (`~/.agents/skills/archify`):
   `node <archify>/bin/archify.mjs finalize architecture sisu-pipeline.json
   sisu-pipeline.html --quality showcase`. The HTML lands next to the JSON.
   With `ARCHIFY_CHROME` set to a Chrome or Chromium binary, `finalize`
   also runs archify's browser check.
3. Export both SVGs over `sisu-pipeline-light.svg` and
   `sisu-pipeline-dark.svg`: run `ARCHIFY_CHROME=<chrome> node
   export-svgs.mjs`, which clicks the viewer's **Export → SVG · Light** and
   **Export → SVG · Dark** in headless Chrome. Without Chrome, open the HTML
   and use those two menu items by hand.

The SVGs show the nodes and edges only; the JSON's `cards` appear in the HTML
viewer. A change to cards alone needs step 1 and nothing more.

The rendered HTML is not committed. It is over the 500 KB limit of the
`check-added-large-files` hook, and the JSON regenerates it.
