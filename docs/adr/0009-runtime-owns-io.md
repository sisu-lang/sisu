# The runtime owns I/O; Sisu builds its own objects

Sisu code reaches the operating system only through `sisu-runtime`. The
runtime owns everything that calls the operating system: standard input,
output and error, files, buffering and error codes. It never builds or reads
a Sisu object. Text crosses the boundary as a pointer and a length, in both
directions: `sisu_panic(msg, len)` already passes text this way. To return
text, such as a line read from standard input, the runtime returns a pointer
into its own buffer and a length. The generated code copies those bytes into
a new Sisu string at once, because the pointer is valid only until the next
call on that stream. Logic on data, such as splitting, trimming, parsing a
number and formatting, is written in Sisu once the standard library exists
(milestone 11).

We chose this so that a Sisu object's layout lives in one place, codegen.
It also works in milestone 5, before Sisu has a C FFI, modules or a standard
library, and it keeps the portability rule that runtime functions take and
return only integers, `bool`, floats and pointers.

## Considered options

- **The runtime builds Sisu objects**: the runtime reads a line and returns a
  finished Sisu string. That saves a copy, but the string layout (count,
  length, bytes) is then written down twice, in codegen and in Rust, and a
  change to one that misses the other corrupts memory.
- **Sisu calls the operating system through a C FFI**: buffering and line
  splitting are written in Sisu, so the standard library is all Sisu. But
  milestone 5 would need the FFI, `CPtr` and access to `errno`, which are a
  later goal, and Sisu cannot write a library before generics and modules.

## Consequences

- Each read costs one copy of the bytes it returns.
- All writes to standard output and standard error go through the runtime,
  so each stream has one buffer and output keeps its order. Code that writes
  to a stream outside the runtime, such as a C library called through a
  future FFI, can interleave out of order unless the runtime flushes first.
- Basic I/O does not need the C FFI. The FFI stays a later goal, for calling
  C libraries.
- Until `Result` exists (milestone 12), an I/O function reports failure as
  `None` or a panic. The runtime signals it with a null pointer or a
  negative count.
- Where invalid UTF-8 is caught is settled by
  [ADR 0014](0014-strings-are-utf-8-values.md): the first operation that
  turns bytes into a string, such as a read from standard input, validates
  them.

Note (2026-11-14, ADR 0051): File functions will return results, while the standard streams still panic.
The `i64(s)` parse is runtime code until milestone 11.
